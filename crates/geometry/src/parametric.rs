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

/// Original continuous-source image and one-sided first derivatives.
/// This carries neither Hessians nor a C1/regularity certificate.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SurfaceFirstEnclosure {
    pub position: [Interval; 3],
    pub du: [Interval; 3],
    pub dv: [Interval; 3],
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
pub(crate) struct SurfaceFunctional {
    pub direction: [Interval; 3],
    pub origin: [Interval; 3],
    pub offset: Interval,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SurfaceFunctionalEnclosure {
    pub position: Interval,
    pub du: Interval,
    pub dv: Interval,
}

pub(crate) const fn surface_functional_scratch_bytes<const N: usize>() -> usize {
    std::mem::size_of::<[SurfaceFunctional; N]>()
        + std::mem::size_of::<[monstertruck_modeling::ValidatedSurfaceFunctional; N]>()
        + std::mem::size_of::<[monstertruck_modeling::ValidatedSurfaceFunctionalJet; N]>()
        + std::mem::size_of::<[SurfaceFunctionalEnclosure; N]>()
}

pub(crate) const SURFACE_FIRST_ENCLOSURE_SCRATCH_BYTES: usize =
    surface_functional_scratch_bytes::<3>()
        + std::mem::size_of::<SurfaceFirstEnclosure>();
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

/// Fold interval covectors, origins and offsets into original coefficients.
/// The batch shares native source basis/denominator evaluation. Continuous
/// piecewise first jets support mean-value bounds, not C1 authority.
pub(crate) fn surface_functional_enclosures<const N: usize>(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    functionals: &[SurfaceFunctional; N],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[SurfaceFunctionalEnclosure; N], GeometryError> {
    check_cancel(cancel)?;
    let native_interval = |value: Interval| monstertruck_modeling::ValidatedInterval {
        lower: value.lo,
        upper: value.hi,
    };
    let mut native_work = monstertruck_modeling::ValidatedGeometryBudget::new(
        usize::try_from(work.remaining()).unwrap_or(usize::MAX),
    );
    let result = monstertruck_modeling::validated_surface_functional_jets::<N>(
        surface,
        uv,
        std::array::from_fn(|i| monstertruck_modeling::ValidatedSurfaceFunctional {
            direction: functionals[i].direction.map(native_interval),
            origin: functionals[i].origin.map(native_interval),
            offset: native_interval(functionals[i].offset),
        }),
        &mut native_work,
        cancel,
    );
    debit_native_work(work, native_work.consumed(), cancel)?;
    Ok(result.map_err(native_error)?.map(|jet| SurfaceFunctionalEnclosure {
        position: adapt(jet.position),
        du: adapt(jet.du),
        dv: adapt(jet.dv),
    }))
}

/// Enclose the genuine original first jet with three unit-coordinate rows.
/// Native basis/denominator work is shared and all continuous C0 sides remain
/// enclosed. No second derivatives or smoothness are invented for consumers.
pub(crate) fn surface_piecewise_first_enclosure(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SurfaceFirstEnclosure, GeometryError> {
    let functionals: [SurfaceFunctional; 3] = std::array::from_fn(|axis| SurfaceFunctional {
        direction: std::array::from_fn(|i| Interval::point(if i == axis { 1.0 } else { 0.0 })),
        origin: [Interval::point(0.0); 3],
        offset: Interval::point(0.0),
    });
    let jets = surface_functional_enclosures(surface, uv, &functionals, work, cancel)?;
    Ok(SurfaceFirstEnclosure {
        position: std::array::from_fn(|i| jets[i].position),
        du: std::array::from_fn(|i| jets[i].du),
        dv: std::array::from_fn(|i| jets[i].dv),
    })
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
        if let Some(value) = self.pending.take()
            && value > self.low
        {
            high = value.min(self.high);
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

/// C1 qualification of the literal UV source, followed by the native
/// continuous-piecewise evaluator. A knot at a CLOSED endpoint is included:
/// the evaluator retains both incident sides there, not a chosen smooth side.
/// None means C1 was not proved; it never grants a normal-coordinate ribbon.
pub(crate) fn curve2d_c1_jet(
    curve: &monstertruck_modeling::Curve2D,
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<ChartCurveEnclosure>, GeometryError> {
    if !curve2d_source_is_c1(curve, range, work, cancel)? {
        return Ok(None);
    }
    curve2d_piecewise_jet(curve, range, work, cancel).map(Some)
}

/// Continuous C0 positions and bounded one-sided derivatives, not C1 authority.
pub(crate) fn curve2d_piecewise_jet(
    curve: &monstertruck_modeling::Curve2D,
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<ChartCurveEnclosure, GeometryError> {
    let mut native = monstertruck_modeling::ValidatedGeometryBudget::new(
        usize::try_from(work.remaining()).unwrap_or(usize::MAX),
    );
    let result = monstertruck_modeling::validated_curve2d_piecewise_jet(
        curve, range, &mut native, cancel,
    );
    debit_native_work(work, native.consumed(), cancel)?;
    let jet = result.map_err(native_error)?;
    let adapt = |v: monstertruck_modeling::ValidatedInterval| Interval {
        lo: v.lower,
        hi: v.upper,
    };
    Ok(ChartCurveEnclosure {
        position: jet.position.map(adapt),
        derivative: jet.first.map(adapt),
        second_derivative: jet.second.map(adapt),
    })
}

fn curve2d_source_is_c1(
    mut curve: &monstertruck_modeling::Curve2D,
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    use monstertruck_modeling::Curve2D;
    let mut parameter = Interval::new(range[0], range[1])?;
    for depth in 0..=32 {
        work.charge(1, cancel)?;
        match curve {
            Curve2D::TrimmedCurve(wrapper) => {
                if depth == 32 {
                    break;
                }
                if !wrapper.orientation() {
                    let (a, b) = wrapper.try_range_tuple().ok_or_else(|| {
                        GeometryError::new(
                            GeometryErrorCode::ConditioningFailure,
                            "unbounded original UV trim",
                        )
                    })?;
                    parameter = Interval::point(a).add(Interval::point(b)).sub(parameter);
                }
                // Native evaluation below checks the unchanged matrix and its
                // denominator. A smooth admitted projective image preserves C1.
                curve = wrapper.entity().curve().as_ref();
            }
            Curve2D::BsplineCurve(c) => {
                return spline_curve2d_is_c1(
                    c.knot_vector().as_ref(), c.degree(), c.control_points().len(),
                    |i| {
                        let p = c.control_points()[i];
                        [p.x, p.y, 1.0]
                    },
                    parameter, work, cancel,
                );
            }
            Curve2D::NurbsCurve(c) => {
                return spline_curve2d_is_c1(
                    c.knot_vector().as_ref(), c.degree(), c.control_points().len(),
                    |i| {
                        let p = c.control_points()[i];
                        [p.x, p.y, p.z]
                    },
                    parameter, work, cancel,
                );
            }
            Curve2D::Polyline(c) => {
                let points = c.as_slice();
                for i in 1..points.len().saturating_sub(1) {
                    work.charge(1, cancel)?;
                    if parameter.lo <= i as f64 && i as f64 <= parameter.hi {
                        let control = |j: usize| [points[j].x, points[j].y, 1.0];
                        if !exact_curve2d_c1_join(
                            control(i - 1), control(i), control(i + 1),
                            [i as f64 - 1.0, i as f64, i as f64 + 1.0],
                            work, cancel,
                        )? {
                            return Ok(false);
                        }
                    }
                }
                return Ok(true);
            }
            Curve2D::Line(_) | Curve2D::Conic(_) => return Ok(true),
        }
    }
    Err(GeometryError::new(
        GeometryErrorCode::ResourceLimit,
        "original UV C1 wrapper nesting exceeds cap",
    ))
}

fn spline_curve2d_is_c1(
    knots: &[f64],
    degree: usize,
    count: usize,
    control: impl Fn(usize) -> [f64; 3],
    range: Interval,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    if degree == 0 || count <= degree || knots.len() != count + degree + 1 {
        return Ok(false);
    }
    let mut start = knots.partition_point(|k| *k < range.lo);
    while start < knots.len() && knots[start] <= range.hi {
        work.charge(1, cancel)?;
        let value = knots[start];
        let mut end = start + 1;
        while end < knots.len() && knots[end] == value {
            work.charge(1, cancel)?;
            end += 1;
        }
        if knots[degree] < value && value < knots[count] && end - start >= degree {
            // At multiplicity p the two homogeneous endpoint values are the
            // SAME control coefficient. p+1 is a jump, not C0 authority.
            if end - start != degree || start == 0 || end >= knots.len() {
                return Ok(false);
            }
            let Some(shared) = end.checked_sub(degree + 1) else {
                return Ok(false);
            };
            if shared == 0 || shared + 1 >= count
                || !exact_curve2d_c1_join(
                    control(shared - 1), control(shared), control(shared + 1),
                    [knots[start - 1], value, knots[end]], work, cancel,
                )?
            {
                return Ok(false);
            }
        }
        start = end;
    }
    Ok(true)
}

/// For H=(X,Y,W), d(X/W)=(dX W-X dW)/W². At a multiplicity-p
/// join the degree and shared W² cancel. Cross-multiply the DISTINCT exact
/// source knot lengths; neither normalized tangents nor a positive G1 scale
/// proves this identity. No derivative division or rounded knot subtraction.
fn exact_curve2d_c1_join(
    left: [f64; 3],
    center: [f64; 3],
    right: [f64; 3],
    knots: [f64; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    use crate::exact_coefficient::ExactCoefficient;
    if left.into_iter().chain(center).chain(right).chain(knots)
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
    if left_span.sign() <= 0 || right_span.sign() <= 0 || center[2] == 0.0 {
        return Ok(false);
    }
    for axis in 0..2 {
        let determinant = |a: f64, b: f64, c: f64, d: f64,
                           work: &mut GeometryWorkBudget|
         -> Result<Option<ExactCoefficient>, GeometryError> {
            let Some(mut first) = ExactCoefficient::point(a)
                .product(&ExactCoefficient::point(b), work, cancel)? else {
                return Ok(None);
            };
            let Some(second) = ExactCoefficient::point(c)
                .product(&ExactCoefficient::point(d), work, cancel)? else {
                return Ok(None);
            };
            if first.subtract(&second).is_none() {
                return Ok(None);
            }
            Ok(Some(first))
        };
        let Some(l) = determinant(center[axis], left[2], left[axis], center[2], work)? else {
            return Ok(false);
        };
        let Some(r) = determinant(right[axis], center[2], center[axis], right[2], work)? else {
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
        // Native spline extrapolation continues the terminal piece across an
        // outer knot. Only a knot with source pieces on both sides is a join.
        let terminal = matches!(knots.source, KnotSource::Borrowed(_))
            && (value == knots.raw(0) || value == knots.raw(knots.len() - 1));
        if !terminal && knot.lo < range[1] && range[0] < knot.hi && end - low >= knots.degree {
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
    if let Some((curve, axis)) = profile
        && domain[axis][0] != domain[axis][1]
    {
        return curve_span_smooth_at(curve, domain[axis], depth + 1, work, cancel);
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
#[expect(
    clippy::large_enum_variant,
    reason = "Own a temporary native proof inline without a per-curve allocation; retained proofs remain borrowed."
)]
enum PreparedNative<'a> {
    Owned(monstertruck_modeling::PreparedValidatedCurve<'a>),
    Borrowed(&'a monstertruck_modeling::PreparedValidatedCurve<'static>),
}

pub(crate) struct GnomonicFrame {
    pub(crate) center_mm: [f64; 3],
    pub(crate) x_axis: [f64; 3],
    pub(crate) y_axis: [f64; 3],
    pub(crate) normal: [f64; 3],
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

/// Positive native C1 qualification and whole-image bounds on one closed range.
/// A subset can inherit the bounds, never extend their range or binding.
#[derive(Clone, Copy)]
pub(crate) struct CurveSamplingSmoothRange<'binding, 'source> {
    binding: &'binding PreparedCurveEnclosure<'source>,
    range: (f64, f64),
    jet: CurveEnclosure,
}

/// Chord and Green queries share one certified whole-query curvature bound.
pub(crate) struct CurveSamplingInterval<'a, 'source> {
    binding: &'a PreparedCurveEnclosure<'source>,
    range: (f64, f64),
    enclosure: CurveIntervalEnclosure,
    smooth_range: Option<CurveSamplingSmoothRange<'a, 'source>>,
}
impl<'binding, 'source> CurveSamplingInterval<'binding, 'source> {
    pub(crate) fn enclosure(&self) -> &CurveIntervalEnclosure {
        &self.enclosure
    }

    pub(crate) fn smooth_range(&self) -> Option<CurveSamplingSmoothRange<'binding, 'source>> {
        self.smooth_range
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
        if let Some(receipt) = &self.smooth_range {
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
                &receipt.jet.second_derivative,
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
    /// Retained native query pieces. An explicit binding may be ONE piece
    /// across several original knots. These seed adaptive queries, not a C0
    /// incidence inventory or Taylor qualification: use the separate knot
    /// visitor and `is_smooth_on` for those obligations.
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
    /// Stream every internal literal knot of an original explicit spline in
    /// root-parameter order. A complete result qualifies each BETWEEN-knot span
    /// analytically C1 with bounded one-sided second derivatives: the retained
    /// Native whole-source piecewise jet already admitted all coefficients,
    /// affine operators, finite support and denominators, and no knot is omitted.
    /// This NEVER claims C1 across a joint. Unknown implicit/carrier-induced
    /// knots and nonrepresentable mapped bands return false. No copied buffers.
    pub(crate) fn visit_explicit_knots(
        &self,
        range: (f64, f64),
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
        visit: &mut impl FnMut(f64, &mut GeometryWorkBudget, &AtomicBool) -> Result<(), GeometryError>,
    ) -> Result<bool, GeometryError> {
        check_cancel(cancel)?;
        self.check_range(range)?;
        let Some(original) = self.native.native_curve() else {
            return Ok(false);
        };
        let mut basis = original;
        for depth in 0..=32 {
            work.charge(1, cancel)?;
            match basis {
                Curve::TrimmedCurve(wrapper) if depth < 32 => {
                    basis = wrapper.entity().curve().as_ref();
                }
                Curve::TrimmedCurve(_) => {
                    return Err(GeometryError::new(
                        GeometryErrorCode::ResourceLimit,
                        "original coedge knot-wrapper nesting exceeds cap",
                    ));
                }
                _ => break,
            }
        }
        if !matches!(basis, Curve::BsplineCurve(_) | Curve::NurbsCurve(_)) {
            return Ok(false);
        }
        // Reuse the original knot-axis walker, including every independent
        // retained reflection anchor and its outward/exact TwoSum arithmetic.
        let knots = profile_knots(original, work, cancel)?;
        let mut previous = range.0;
        for index in 0..knots.len() {
            work.charge(1, cancel)?;
            let knot = knots.value(index);
            Interval::new(knot.lo, knot.hi)?;
            if knot.hi <= range.0 || knot.lo >= range.1 {
                continue;
            }
            if knot.lo != knot.hi || knot.lo < previous {
                return Ok(false);
            }
            if knot.lo > previous {
                visit(knot.lo, work, cancel)?;
                previous = knot.lo;
            }
        }
        Ok(true)
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

    /// Piecewise first derivatives of this exact retained owner, not position,
    /// Hessian or C1 authority. Crossing a prepared projection joint declines.
    /// Mapped owners retain the ordinary fresh query's reflected-range checks.
    pub(crate) fn retained_first_enclosure(
        &self,
        range: (f64, f64),
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<[Interval; 3]>, GeometryError> {
        check_cancel(cancel)?;
        self.check_range(range)?;
        work.charge(1, cancel)?;
        if self.native.mapped_basis().is_some() {
            return Ok(None);
        }
        let mut native_work = monstertruck_modeling::ValidatedGeometryBudget::new(
            usize::try_from(work.remaining()).unwrap_or(usize::MAX),
        );
        let result = self.retained_with_native_budget(range, &mut native_work, cancel, true);
        debit_native_work(work, native_work.consumed(), cancel)?;
        result.map(|jet| jet.map(|jet| jet.derivative))
    }

    /// Borrow a finite whole-piece certificate on a contained smooth query.
    /// The caller separately qualifies C1; a retained Hessian is not that proof.
    fn retained_with_native_budget(
        &self,
        range: (f64, f64),
        native_work: &mut monstertruck_modeling::ValidatedGeometryBudget,
        cancel: &AtomicBool,
        single_piece: bool,
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
            if single_piece && result.is_some() {
                return Ok(None);
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
        frame: &GnomonicFrame,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<ChartCurveEnclosure, GeometryError> {
        let &GnomonicFrame {
            center_mm,
            x_axis,
            y_axis,
            normal,
        } = frame;
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
        inherited_smooth_range: Option<CurveSamplingSmoothRange<'_, 'source>>,
        frame: &PlaneFrameMm,
        maximum_plane_error_mm: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<CurveSamplingInterval<'_, 'source>, GeometryError> {
        frame.validate()?;
        if !maximum_plane_error_mm.is_finite()
            || maximum_plane_error_mm <= 0.0
            || maximum_plane_error_mm > SECTION_PLANE_TOLERANCE_MM
        {
            return Err(GeometryError::new(
                GeometryErrorCode::InvalidGeometry,
                "native sampling plane error must narrow the frozen section allowance",
            ));
        }
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
        if let Some(receipt) = inherited_smooth_range {
            work.charge(1, cancel)?;
            // Identity also fixes the active restriction, native parameter gauge
            // and unit view: all are immutable for this binding's borrowed life.
            if !std::ptr::eq(receipt.binding, self) {
                return Err(GeometryError::new(
                    GeometryErrorCode::InvalidGeometry,
                    "native sampler smooth range belongs to another prepared source binding",
                ));
            }
            self.check_range(receipt.range)?;
            if range.0 < receipt.range.0 || range.1 > receipt.range.1 {
                return Err(GeometryError::new(
                    GeometryErrorCode::InvalidGeometry,
                    "native sampler interval leaves its qualified closed smooth range",
                ));
            }
        }
        let mut native_work = monstertruck_modeling::ValidatedGeometryBudget::new(
            usize::try_from(work.remaining()).unwrap_or(usize::MAX),
        );
        let result = (|| -> Result<CurveSamplingInterval<'_, 'source>, GeometryError> {
            let smooth = match inherited_smooth_range {
                Some(_) => true,
                None => self
                    .native
                    .is_smooth_on([range.0, range.1], &mut native_work, cancel)
                    .map_err(native_error)?,
            };
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
                let inherited = match inherited_smooth_range {
                    Some(receipt)
                        if chord_error(&receipt.jet)? <= SECTION_SAMPLING_TOLERANCE_MM =>
                    {
                        Some(receipt.jet)
                    }
                    _ => None,
                };
                let jet = match inherited {
                    Some(jet) => jet,
                    None => {
                        // Only query a tighter native bound when the parent's
                        // qualified enclosure cannot meet this child's policy.
                        let coarse =
                            self.retained_with_native_budget(range, &mut native_work, cancel, false)?;
                        match coarse {
                            Some(jet)
                                if chord_error(&jet)? <= SECTION_SAMPLING_TOLERANCE_MM => jet,
                            _ => self.refined_with_native_budget(range, &mut native_work, cancel)?,
                        }
                    }
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
            if smooth && plane_error_mm > maximum_plane_error_mm {
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
                smooth_range: retained_jet.map(|jet| CurveSamplingSmoothRange {
                    binding: self,
                    range,
                    jet,
                }),
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
    EndpointChartProposals, PreparedSectionOriginals, PreparedSectionPlaneRoots,
    SectionPlaneCoverage, SectionPlaneEndpointReceipt, certify_section_plane_coverage,
    prepare_section_originals, prepare_section_plane_roots,
};

fn cross(a: [Interval; 3], b: [Interval; 3]) -> [Interval; 3] {
    std::array::from_fn(|axis| {
        let i = (axis + 1) % 3;
        let j = (axis + 2) % 3;
        a[i].mul(b[j]).sub(a[j].mul(b[i]))
    })
}
fn regular_jet(jet: &SurfaceEnclosure) -> bool {
    regular_derivatives(jet.du, jet.dv)
}
fn regular_derivatives(du: [Interval; 3], dv: [Interval; 3]) -> bool {
    cross(du, dv).into_iter().any(|component| {
        component.lo.is_finite() && component.hi.is_finite() && !component.contains_zero()
    })
}

/// Recenter the SAME original Cartesian coordinate functions before rational
/// division. Any finite constant origin is valid: its derivative is zero.
/// The native coefficient-first first jets retain every one-sided source span,
/// denominator and interval error; this is not C1 or Hessian authority.
fn correlated_surface_regular(
    surface: &Surface,
    domain: [[f64; 2]; 2],
    root: &SurfaceEnclosure,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let origin = root.position.map(|value| Interval::point(value.mid()));
    let functionals = std::array::from_fn(|axis| SurfaceFunctional {
        direction: std::array::from_fn(|coordinate| Interval::point(
            if coordinate == axis { 1.0 } else { 0.0 },
        )),
        origin,
        offset: Interval::point(0.0),
    });
    let jets = surface_functional_enclosures::<3>(surface, domain, &functionals, work, cancel)?;
    Ok(regular_derivatives(jets.map(|jet| jet.du), jets.map(|jet| jet.dv)))
}
fn certify_surface_regular(
    surface: &Surface,
    domain: [[f64; 2]; 2],
    root: &SurfaceEnclosure,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    if regular_jet(root) || correlated_surface_regular(surface, domain, root, work, cancel)? {
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
        let jet = surface_piecewise_enclosure(surface, cell, work, cancel)?;
        if regular_jet(&jet) || correlated_surface_regular(surface, cell, &jet, work, cancel)? {
            continue;
        }
        if depth == 24 {
            return Err(GeometryError::new(
                GeometryErrorCode::AnalysisIndeterminate,
                format!(
                    "native display patch Jacobian cannot be separated from zero: original_box={domain:?} terminal_box={cell:?} du={:?} dv={:?} normal={:?}",
                    jet.du, jet.dv, cross(jet.du, jet.dv),
                ),
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
#[cfg(test)]
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

/// Face-borrowed audit ownership. Original UV indices retain point enclosures,
/// and exact inventoried mesh boxes retain completed proofs without eviction.
/// Auxiliary comparison boxes/points use bounded keyed scratch. Triangle
/// residuals are always recomputed against the current physical mesh vertices.
pub(crate) struct DisplayTriangleProof<'a> {
    surface: &'a Surface,
    boxes: [Option<DisplayAuxBox>; 64],
    vertices: [Option<DisplayVertexProof>; 256],
    mesh_uvs: &'a [monstertruck_modeling::Vector2],
    mesh_vertices: Vec<Option<[Interval; 3]>>,
    triangle_boxes: Vec<u32>,
    mesh_boxes: Vec<DisplayMeshBox>,
}
struct DisplayBoxProof {
    du: [Interval; 3],
    dv: [Interval; 3],
    full: Option<DisplayFullBoxProof>,
}
struct DisplayAuxBox {
    key: [u64; 4],
    proof: DisplayBoxProof,
}
pub(crate) struct DisplayMeshBox {
    key: [u64; 4],
    proof: Option<DisplayBoxProof>,
    conditioned: Option<DisplayConditionedHessian>,
}
impl DisplayMeshBox {
    pub(crate) fn new(key: [u64; 4]) -> Self {
        Self { key, proof: None, conditioned: None }
    }
}
/// Copied only from a completed same-source auxiliary rank/smooth-span proof.
/// The enclosing mesh slot binds the source owner and original box; this key
/// additionally binds the exact comparison box. No triangle output is retained.
struct DisplayConditionedHessian {
    key: [u64; 4],
    hessian: [[Interval; 3]; 3],
}
#[derive(Clone, Copy, Debug)]
enum DisplayBoxIndex {
    Mesh(usize),
    Auxiliary(usize),
}
struct DisplayFullBoxProof {
    bound: SurfaceEnclosure,
    smooth: bool,
    partitioned: bool,
}
struct DisplayVertexProof {
    key: [u64; 2],
    position: [Interval; 3],
}
/// Fixed proof-only workspace; no mesh vertices, UVs or source data are changed.
struct DisplayHessianScratch {
    boxes: [[[f64; 2]; 2]; 4],
    hull: Option<[[Interval; 3]; 3]>,
    child: Option<SurfaceEnclosure>,
}
/// One proof-only subdivision of the unchanged affine triangle. Midpoint
/// intervals enclose exact convex averages; no rounded child mesh is created.
struct DisplaySubdivisionScratch {
    uv: [[Interval; 2]; 6],
    points: [[Interval; 3]; 6],
    positions: [[Interval; 3]; 6],
    domain: [[f64; 2]; 2],
    remainder: [Interval; 3],
    residual: [Interval; 3],
}
impl<'a> DisplayTriangleProof<'a> {
    pub(crate) fn base_retained_bytes_for_vertices(count: usize) -> Result<usize, GeometryError> {
        count.checked_mul(std::mem::size_of::<Option<[Interval; 3]>>())
            .and_then(|bytes| bytes.checked_add(
                std::mem::size_of::<Self>() + std::mem::size_of::<DisplayHessianScratch>()
                    + std::mem::size_of::<DisplaySubdivisionScratch>(),
            ))
            .ok_or_else(|| GeometryError::new(
                GeometryErrorCode::ResourceLimit, "display UV-point proof storage overflow",
            ))
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        (std::mem::size_of::<Self>() + std::mem::size_of::<DisplayHessianScratch>()
            + std::mem::size_of::<DisplaySubdivisionScratch>()).saturating_add(
                self.mesh_vertices.capacity().saturating_mul(std::mem::size_of::<Option<[Interval; 3]>>()),
            )
            .saturating_add(self.triangle_boxes.capacity().saturating_mul(std::mem::size_of::<u32>()))
            .saturating_add(self.mesh_boxes.capacity().saturating_mul(std::mem::size_of::<DisplayMeshBox>()))
    }
    #[cfg(test)]
    pub(crate) fn new(
        surface: &'a Surface,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        Self::with_mesh_inventory(surface, &[], Vec::new(), Vec::new(), work, cancel)
    }
    pub(crate) fn with_mesh_inventory(
        surface: &'a Surface,
        mesh_uvs: &'a [monstertruck_modeling::Vector2],
        triangle_boxes: Vec<u32>,
        mesh_boxes: Vec<DisplayMeshBox>,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        let uv_count = mesh_uvs.len();
        work.charge((64 + 256 + 1_u64).saturating_add(uv_count as u64), cancel)?;
        let mut mesh_vertices = Vec::new();
        mesh_vertices.try_reserve_exact(uv_count).map_err(|_| GeometryError::new(
            GeometryErrorCode::ResourceLimit, "display UV-point proof allocation failed",
        ))?;
        for index in 0..uv_count {
            if index % 1024 == 0 {
                check_cancel(cancel)?;
            }
            mesh_vertices.push(None);
        }
        Ok(Self {
            surface,
            boxes: [const { None }; 64],
            vertices: [const { None }; 256],
            mesh_uvs,
            mesh_vertices,
            triangle_boxes,
            mesh_boxes,
        })
    }
    #[cfg(test)]
    pub(crate) fn triangle_error_bound(
        &mut self,
        uv: &[[f64; 2]; 3],
        points: &[[f64; 3]; 3],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<f64, GeometryError> {
        self.triangle_error_bound_at(uv, points, None, true, work, cancel)
    }
    pub(crate) fn indexed_triangle_error_bound(
        &mut self,
        uv: &[[f64; 2]; 3],
        points: &[[f64; 3]; 3],
        triangle_ordinal: usize,
        uv_indices: &[usize; 3],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<f64, GeometryError> {
        self.triangle_error_bound_at(uv, points, Some((triangle_ordinal, uv_indices)), true, work, cancel)
    }
    fn triangle_error_bound_at(
        &mut self,
        uv: &[[f64; 2]; 3],
        points: &[[f64; 3]; 3],
        uv_indices: Option<(usize, &[usize; 3])>,
        allow_auxiliary: bool,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<f64, GeometryError> {
        work.charge(1, cancel)?;
        if uv.iter().flatten().any(|value| !value.is_finite()) {
            return Err(GeometryError::new(
                GeometryErrorCode::InvalidGeometry,
                "display triangle proof requires finite native UV coordinates",
            ));
        }
        let domain = std::array::from_fn(|axis| [
            uv.iter().map(|p| p[axis]).fold(f64::INFINITY, f64::min),
            uv.iter().map(|p| p[axis]).fold(f64::NEG_INFINITY, f64::max),
        ]);
        let key = [
            domain[0][0].to_bits(), domain[0][1].to_bits(),
            domain[1][0].to_bits(), domain[1][1].to_bits(),
        ];
        let box_index = if let Some((ordinal, _)) = uv_indices {
            let index = *self.triangle_boxes.get(ordinal).ok_or_else(|| GeometryError::new(
                GeometryErrorCode::InvalidGeometry, "display triangle is outside its captured UV-box inventory",
            ))? as usize;
            let entry = self.mesh_boxes.get(index).ok_or_else(|| GeometryError::new(
                GeometryErrorCode::InvalidGeometry, "display triangle references an absent UV-box slot",
            ))?;
            if entry.key != key {
                return Err(GeometryError::new(
                    GeometryErrorCode::InvalidGeometry, "display triangle changed its exact captured UV box",
                ));
            }
            DisplayBoxIndex::Mesh(index)
        } else {
            let slot = display_proof_slot(&key, self.boxes.len());
            if self.boxes[slot].as_ref().is_some_and(|entry| entry.key != key) {
                self.boxes[slot] = None;
            }
            DisplayBoxIndex::Auxiliary(slot)
        };
        if self.box_proof(box_index).is_none() {
            let anchor = std::array::from_fn(|axis| {
                let low = points.iter().map(|point| point[axis]).fold(f64::INFINITY, f64::min);
                let high = points.iter().map(|point| point[axis]).fold(f64::NEG_INFINITY, f64::max);
                low * 0.5 + high * 0.5
            });
            let first = surface_coordinate_first(self.surface, domain, anchor, work, cancel)
                .map_err(|cause| display_proof_failure(cause, "box first jets", box_index, domain))?;
            let du = first.map(|jet| jet.du);
            let dv = first.map(|jet| jet.dv);
            let full = if regular_derivatives(du, dv) {
                None
            } else {
                let bound = surface_piecewise_enclosure(self.surface, domain, work, cancel)
                    .map_err(|cause| display_proof_failure(cause, "box rank Jet2", box_index, domain))?;
                certify_surface_regular(self.surface, domain, &bound, work, cancel).map_err(|cause| {
                    GeometryError::new(cause.code, format!(
                        "obligation=box rank cache={box_index:?} triangle_uv={uv:?} triangle_points={points:?}: {}", cause.message,
                    ))
                })?;
                let smooth = surface_parameter_span_smooth(self.surface, domain, work, cancel)
                    .map_err(|cause| display_proof_failure(cause, "box rank smooth span", box_index, domain))?;
                Some(DisplayFullBoxProof { bound, smooth, partitioned: false })
            };
            let proof = DisplayBoxProof { du, dv, full };
            match box_index {
                DisplayBoxIndex::Mesh(index) => self.mesh_boxes[index].proof = Some(proof),
                DisplayBoxIndex::Auxiliary(slot) => self.boxes[slot] = Some(DisplayAuxBox { key, proof }),
            }
        }
        let positions = self.triangle_positions(uv, uv_indices, box_index, work, cancel)?;
        let proof = self.box_proof(box_index).expect("completed same-box rank proof");
        let first_error = first_jet_triangle_error(points, domain, proof.du, proof.dv, &positions)?;
        if first_error <= spiling_contracts::geometry::MESH_CARRIER_TOLERANCE_MM {
            return Ok(first_error);
        }
        // A first-order bound/rank decline is not an error. Only then request
        // the existing Hessian/smooth-span proof. Native failures stay terminal.
        if self.box_proof(box_index).expect("same box").full.is_none() {
            let bound = surface_piecewise_enclosure(self.surface, domain, work, cancel)
                .map_err(|cause| display_proof_failure(cause, "box Hessian jets", box_index, domain))?;
            // This exact box already has a completed first-jet rank proof.
            let smooth = surface_parameter_span_smooth(self.surface, domain, work, cancel)
                .map_err(|cause| display_proof_failure(cause, "box Hessian smooth span", box_index, domain))?;
            self.box_proof_mut(box_index).expect("same box").full =
                Some(DisplayFullBoxProof { bound, smooth, partitioned: false });
        }
        let proof = self.box_proof(box_index).expect("same box").full.as_ref()
            .expect("completed Hessian/smooth-span proof");
        let mut error = first_error.min(triangle_interpolation_error_prepared(
            uv, points, domain, &proof.bound, proof.smooth, &positions, (&mut *work, cancel),
        )?);
        if !allow_auxiliary && proof.smooth && !proof.partitioned
            && error > spiling_contracts::geometry::MESH_CARRIER_TOLERANCE_MM
        {
            // Tighten only the single auxiliary comparison's Hessian, once,
            // over four closed subboxes covering this SAME qualified box.
            let hessian = partition_display_hessian(self.surface, domain, proof, work, cancel)
                .map_err(|cause| display_proof_failure(cause, "four-child Hessian", box_index, domain))?;
            let proof = self.box_proof_mut(box_index).expect("same box").full.as_mut()
                .expect("same smooth-span proof");
            [proof.bound.duu, proof.bound.duv, proof.bound.dvv] = hessian;
            proof.partitioned = true;
            error = first_error.min(triangle_interpolation_error_prepared(
                uv, points, domain, &proof.bound, proof.smooth, &positions, (&mut *work, cancel),
            )?);
        }
        if !allow_auxiliary || error <= spiling_contracts::geometry::MESH_CARRIER_TOLERANCE_MM {
            return Ok(error);
        }
        let Some(auxiliary) = auxiliary_smooth_triangle(self.surface, uv, domain, work, cancel)
            .map_err(|cause| display_proof_failure(cause, "auxiliary span qualification", box_index, domain))?
        else {
            work.charge(1, cancel)?;
            let proof = self.box_proof(box_index).expect("same box").full.as_ref()
                .expect("completed same-box derivative proof");
            if proof.smooth
                && let Some(refined) = subdivide_display_triangle(
                    self.surface, uv, points, domain,
                    [&proof.bound.duu, &proof.bound.duv, &proof.bound.dvv], &positions,
                    (work, cancel),
                ).map_err(|cause| display_proof_failure(
                    cause, "filled-triangle subdivision", box_index, domain,
                ))?
            {
                error = error.min(refined);
            }
            return Ok(error);
        };
        let proof = self.box_proof(box_index).expect("same box").full.as_ref()
            .expect("completed same-box derivative proof");
        // The same barycentric weights pair every original UV point with an
        // auxiliary UV point. Convexity bounds the entire displacement by the
        // largest vertex displacement on each axis. Both triangles remain in
        // the ORIGINAL closed box, whose continuous piecewise first jets bound
        // the complete transport; no C1 claim is made at its crossed knots.
        let delta: [f64; 2] = std::array::from_fn(|axis| {
            (0..3).map(|vertex| Interval::point(uv[vertex][axis])
                .sub(Interval::point(auxiliary[vertex][axis])).abs_upper())
                .fold(0.0_f64, f64::max)
        });
        let transport = norm_upper(std::array::from_fn(|axis| {
            Interval::point(proof.bound.du[axis].abs_upper()).mul(Interval::point(delta[0]))
                .add(Interval::point(proof.bound.dv[axis].abs_upper()).mul(Interval::point(delta[1])))
        }))?;
        let auxiliary_domain: [[f64; 2]; 2] = std::array::from_fn(|axis| [
            auxiliary.iter().map(|point| point[axis]).fold(f64::INFINITY, f64::min),
            auxiliary.iter().map(|point| point[axis]).fold(f64::NEG_INFINITY, f64::max),
        ]);
        let auxiliary_key = [
            auxiliary_domain[0][0].to_bits(), auxiliary_domain[0][1].to_bits(),
            auxiliary_domain[1][0].to_bits(), auxiliary_domain[1][1].to_bits(),
        ];
        work.charge(1, cancel)?;
        let parent_index = match box_index {
            DisplayBoxIndex::Mesh(index) => Some(index),
            DisplayBoxIndex::Auxiliary(_) => None,
        };
        let mut retained_error = None;
        if let Some(index) = parent_index
            && self.mesh_boxes[index].conditioned.as_ref()
                .is_some_and(|receipt| receipt.key == auxiliary_key)
        {
            let positions = self.triangle_positions(&auxiliary, None, box_index, work, cancel)?;
            let receipt = self.mesh_boxes[index].conditioned.as_ref()
                .expect("matching completed auxiliary smooth-span proof");
            let bound = smooth_triangle_interpolation_error(
                &auxiliary, points, auxiliary_domain,
                [&receipt.hessian[0], &receipt.hessian[1], &receipt.hessian[2]], &positions,
                work, cancel,
            )?;
            let complete = up(bound + transport);
            if complete <= spiling_contracts::geometry::MESH_CARRIER_TOLERANCE_MM {
                return Ok(error.min(complete));
            }
            // Retaining a valid Hessian must not discard another existing
            // certificate when this triangle's fresh residual still declines.
            retained_error = Some(bound);
        }
        // Exactly one independently qualified comparison. Even if its bound
        // remains too large, there is no precision or conditioning ladder.
        let auxiliary_error = self.triangle_error_bound_at(
            &auxiliary, points, None, false, work, cancel,
        )?;
        if let Some(index) = parent_index {
            let slot = display_proof_slot(&auxiliary_key, self.boxes.len());
            let receipt = self.boxes[slot].as_ref().filter(|entry| entry.key == auxiliary_key)
                .and_then(|entry| entry.proof.full.as_ref().filter(|full| full.smooth))
                .map(|full| DisplayConditionedHessian {
                    key: auxiliary_key,
                    hessian: [full.bound.duu, full.bound.duv, full.bound.dvv],
                });
            if let Some(receipt) = receipt {
                self.mesh_boxes[index].conditioned = Some(receipt);
            }
        }
        let auxiliary_error = retained_error.map_or(auxiliary_error, |old: f64| old.min(auxiliary_error));
        let mut complete = error.min(up(auxiliary_error + transport));
        if complete <= spiling_contracts::geometry::MESH_CARRIER_TOLERANCE_MM {
            return Ok(complete);
        }
        // All existing scalar/conditioning paths have declined. At most ONE
        // exact filled-triangle proof subdivision may now use an already
        // completed smooth Hessian, preferably the actual auxiliary box.
        work.charge(4, cancel)?;
        let slot = display_proof_slot(&auxiliary_key, self.boxes.len());
        let has_retained = parent_index.is_some_and(|index| {
            self.mesh_boxes[index].conditioned.as_ref()
                .is_some_and(|receipt| receipt.key == auxiliary_key)
        });
        let has_auxiliary = self.boxes[slot].as_ref().filter(|entry| entry.key == auxiliary_key)
            .and_then(|entry| entry.proof.full.as_ref()).is_some_and(|full| full.smooth);
        if has_retained || has_auxiliary {
            let positions = self.triangle_positions(&auxiliary, None, box_index, work, cancel)?;
            let hessian = parent_index.and_then(|index| self.mesh_boxes[index].conditioned.as_ref())
                .filter(|receipt| receipt.key == auxiliary_key)
                .map(|receipt| [&receipt.hessian[0], &receipt.hessian[1], &receipt.hessian[2]])
                .or_else(|| {
                    self.boxes[slot].as_ref().filter(|entry| entry.key == auxiliary_key)
                        .and_then(|entry| entry.proof.full.as_ref().filter(|full| full.smooth))
                        .map(|full| [&full.bound.duu, &full.bound.duv, &full.bound.dvv])
                }).expect("completed same-source auxiliary smooth Hessian");
            if let Some(refined) = subdivide_display_triangle(
                self.surface, &auxiliary, points, auxiliary_domain, hessian, &positions,
                (work, cancel),
            ).map_err(|cause| display_proof_failure(
                cause, "auxiliary filled-triangle subdivision", box_index, auxiliary_domain,
            ))? {
                // This is still the original affine physical triangle. Pair
                // every original UV point with its auxiliary point only once.
                complete = complete.min(up(refined + transport));
            }
        } else if let Some(proof) = self.box_proof(box_index)
            .and_then(|proof| proof.full.as_ref()).filter(|full| full.smooth)
            && let Some(refined) = subdivide_display_triangle(
                self.surface, uv, points, domain,
                [&proof.bound.duu, &proof.bound.duv, &proof.bound.dvv], &positions,
                (work, cancel),
            ).map_err(|cause| display_proof_failure(
                cause, "filled-triangle subdivision", box_index, domain,
            ))?
        {
            complete = complete.min(refined);
        }
        Ok(complete)
    }
    fn triangle_positions(
        &mut self,
        uv: &[[f64; 2]; 3],
        uv_indices: Option<(usize, &[usize; 3])>,
        box_index: DisplayBoxIndex,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[[Interval; 3]; 3], GeometryError> {
        let mut positions = [[Interval::point(0.0); 3]; 3];
        for vertex in 0..3 {
            work.charge(1, cancel)?;
            let key = uv[vertex].map(f64::to_bits);
            let index = if let Some((_, indices)) = uv_indices {
                let index = indices[vertex];
                let captured = self.mesh_uvs.get(index).ok_or_else(|| GeometryError::new(
                    GeometryErrorCode::InvalidGeometry, "display UV proof index is outside its captured mesh",
                ))?;
                if [captured.x.to_bits(), captured.y.to_bits()] != key {
                    return Err(GeometryError::new(
                        GeometryErrorCode::InvalidGeometry, "display UV index changed its exact parameter bits",
                    ));
                }
                Some(index)
            } else {
                None
            };
            let auxiliary_slot = if index.is_none() {
                display_proof_slot(&key, self.vertices.len())
            } else {
                0
            };
            let cached = match index {
                Some(index) => self.mesh_vertices[index],
                None => self.vertices[auxiliary_slot].as_ref()
                    .filter(|proof| proof.key == key).map(|proof| proof.position),
            };
            let position = if let Some(position) = cached {
                position
            } else {
                let position = surface_position_enclosure(
                    self.surface, [[uv[vertex][0]; 2], [uv[vertex][1]; 2]], work, cancel,
                ).map_err(|cause| GeometryError::new(cause.code, format!(
                    "obligation=point position cache={box_index:?} uv={:?}: {}", uv[vertex], cause.message,
                )))?;
                match index {
                    Some(index) => self.mesh_vertices[index] = Some(position),
                    None => self.vertices[auxiliary_slot] = Some(DisplayVertexProof { key, position }),
                }
                position
            };
            positions[vertex] = position;
        }
        Ok(positions)
    }
    fn box_proof(&self, index: DisplayBoxIndex) -> Option<&DisplayBoxProof> {
        match index {
            DisplayBoxIndex::Mesh(index) => self.mesh_boxes[index].proof.as_ref(),
            DisplayBoxIndex::Auxiliary(index) => self.boxes[index].as_ref().map(|entry| &entry.proof),
        }
    }
    fn box_proof_mut(&mut self, index: DisplayBoxIndex) -> Option<&mut DisplayBoxProof> {
        match index {
            DisplayBoxIndex::Mesh(index) => self.mesh_boxes[index].proof.as_mut(),
            DisplayBoxIndex::Auxiliary(index) => self.boxes[index].as_mut().map(|entry| &mut entry.proof),
        }
    }
    /// Failure-only inspection of the captured original triangle/box binding.
    pub(crate) fn smooth_span_for_triangle(&self, ordinal: usize, uv: &[[f64; 2]; 3]) -> Option<bool> {
        let domain: [[f64; 2]; 2] = std::array::from_fn(|axis| [
            uv.iter().map(|p| p[axis]).fold(f64::INFINITY, f64::min),
            uv.iter().map(|p| p[axis]).fold(f64::NEG_INFINITY, f64::max),
        ]);
        let key = [
            domain[0][0].to_bits(), domain[0][1].to_bits(),
            domain[1][0].to_bits(), domain[1][1].to_bits(),
        ];
        let index = *self.triangle_boxes.get(ordinal)? as usize;
        self.mesh_boxes.get(index).filter(|entry| entry.key == key)?
            .proof.as_ref()?.full.as_ref().map(|full| full.smooth)
    }
}

fn display_proof_failure(
    cause: GeometryError,
    obligation: &str,
    index: DisplayBoxIndex,
    domain: [[f64; 2]; 2],
) -> GeometryError {
    GeometryError::new(cause.code, format!(
        "obligation={obligation} cache={index:?} uv_box={domain:?}: {}", cause.message,
    ))
}

fn partition_display_hessian(
    surface: &Surface,
    domain: [[f64; 2]; 2],
    parent: &DisplayFullBoxProof,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[[Interval; 3]; 3], GeometryError> {
    if !parent.smooth {
        return Err(GeometryError::new(
            GeometryErrorCode::SingularGeometry,
            "display Hessian partition requires a qualified original smooth span",
        ));
    }
    let middle = domain.map(|range| range[0] * 0.5 + range[1] * 0.5);
    let mut scratch = DisplayHessianScratch {
        boxes: std::array::from_fn(|index| {
            let u = index / 2;
            let v = index % 2;
            [
                [if u == 0 { domain[0][0] } else { middle[0] },
                 if u == 0 { middle[0] } else { domain[0][1] }],
                [if v == 0 { domain[1][0] } else { middle[1] },
                 if v == 0 { middle[1] } else { domain[1][1] }],
            ]
        }),
        hull: None,
        child: None,
    };
    for index in 0..4 {
        work.charge(1, cancel)?;
        scratch.child = None;
        scratch.child = Some(surface_piecewise_enclosure(surface, scratch.boxes[index], work, cancel)
            .map_err(|cause| GeometryError::new(cause.code, format!(
                "Hessian child={index} uv_box={:?}: {}", scratch.boxes[index], cause.message,
            )))?);
        let child = scratch.child.as_ref().expect("completed closed-subbox enclosure");
        if let Some(hull) = &mut scratch.hull {
            for derivative in 0..3 {
                let current = match derivative {
                    0 => &child.duu,
                    1 => &child.duv,
                    _ => &child.dvv,
                };
                for axis in 0..3 {
                    hull[derivative][axis] = hull[derivative][axis].hull(current[axis]);
                }
            }
        } else {
            scratch.hull = Some([child.duu, child.duv, child.dvv]);
        }
    }
    let mut hull = scratch.hull.expect("four closed subboxes cover the parent");
    for derivative in 0..3 {
        let original = match derivative {
            0 => &parent.bound.duu,
            1 => &parent.bound.duv,
            _ => &parent.bound.dvv,
        };
        for axis in 0..3 {
            hull[derivative][axis] = Interval::new(
                hull[derivative][axis].lo.max(original[axis].lo),
                hull[derivative][axis].hi.min(original[axis].hi),
            )?;
        }
    }
    Ok(hull)
}

/// Subtract the mathematical midpoint of each derivative interval. Its constant
/// linear term cancels in barycentric interpolation. The weighted mean absolute
/// deviation of scalar vertex coordinates is at most half their span, so each
/// first-jet interval width contributes span*width/4. Continuous piecewise first
/// jets suffice; this grants neither C1 nor Hessian authority.
fn first_jet_triangle_error(
    points: &[[f64; 3]; 3],
    domain: [[f64; 2]; 2],
    du: [Interval; 3],
    dv: [Interval; 3],
    positions: &[[Interval; 3]; 3],
) -> Result<f64, GeometryError> {
    let span = domain.map(|range| Interval::point(range[1])
        .sub(Interval::point(range[0])).abs_upper());
    norm_upper(std::array::from_fn(|axis| {
        let vertex_error = (0..3).map(|vertex| positions[vertex][axis]
            .sub(Interval::point(points[vertex][axis])).abs_upper()).fold(0.0_f64, f64::max);
        Interval::point(vertex_error)
            .add(Interval::point(du[axis].width()).mul(Interval::point(span[0]))
                .mul(Interval::point(0.25)))
            .add(Interval::point(dv[axis].width()).mul(Interval::point(span[1]))
                .mul(Interval::point(0.25)))
    }))
}

/// Evaluate only the native source position; derivatives and regularity remain
/// separate obligations. Debit completed work even if evaluation fails.
pub(crate) fn surface_position_enclosure(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[Interval; 3], GeometryError> {
    check_cancel(cancel)?;
    let mut native_work = monstertruck_modeling::ValidatedGeometryBudget::new(
        usize::try_from(work.remaining()).unwrap_or(usize::MAX),
    );
    let result = monstertruck_modeling::validated_surface_position_bounds(
        surface, uv, &mut native_work, cancel,
    );
    debit_native_work(work, native_work.consumed(), cancel)?;
    Ok(result.map_err(native_error)?.map(adapt))
}
fn surface_coordinate_first(
    surface: &Surface,
    domain: [[f64; 2]; 2],
    anchor: [f64; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[SurfaceFunctionalEnclosure; 3], GeometryError> {
    let origin = anchor.map(Interval::point);
    let functionals = std::array::from_fn(|axis| SurfaceFunctional {
        direction: std::array::from_fn(|coordinate| Interval::point(
            if coordinate == axis { 1.0 } else { 0.0 },
        )),
        origin,
        offset: Interval::point(0.0),
    });
    surface_functional_enclosures::<3>(surface, domain, &functionals, work, cancel)
}

/// Propose one actual knot-span interior, not an epsilon-nearness waiver.
/// Even a large projection is permitted only as comparison geometry: its full
/// first-jet transport is subsequently charged to the unchanged physical error.
fn auxiliary_smooth_triangle(
    surface: &Surface,
    uv: &[[f64; 2]; 3],
    domain: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[[f64; 2]; 3]>, GeometryError> {
    let axes = surface_knots(surface, work, cancel)?;
    let mut interior = [[0.0; 2]; 2];
    for axis in 0..2 {
        work.charge(axes[axis].len() as u64 + 1, cancel)?;
        let mut selected: Option<[f64; 2]> = None;
        for span in AxisSpans::new(axes[axis], domain[axis]) {
            work.charge(1, cancel)?;
            if selected.is_none_or(|old| span[1] - span[0] > old[1] - old[0]) {
                selected = Some(span);
            }
        }
        let Some(span) = selected else { return Ok(None) };
        let low = span[0].next_up();
        let high = span[1].next_down();
        if !low.is_finite() || !high.is_finite() || low >= high {
            return Ok(None);
        }
        interior[axis] = [low, high];
    }
    work.charge(6, cancel)?;
    let auxiliary = uv.map(|point| std::array::from_fn(|axis| {
        point[axis].clamp(interior[axis][0], interior[axis][1])
    }));
    let auxiliary_domain = std::array::from_fn(|axis| [
        auxiliary.iter().map(|point| point[axis]).fold(f64::INFINITY, f64::min),
        auxiliary.iter().map(|point| point[axis]).fold(f64::NEG_INFINITY, f64::max),
    ]);
    if !surface_parameter_span_smooth(surface, auxiliary_domain, work, cancel)? {
        return Ok(None);
    }
    Ok(Some(auxiliary))
}
fn display_proof_slot(key: &[u64], slots: usize) -> usize {
    let hash = key.iter().fold(0_u64, |hash, bits| {
        (hash ^ *bits).wrapping_mul(0x9e37_79b9_7f4a_7c15).rotate_left(27)
    });
    ((hash ^ (hash >> 32)) as usize) & (slots - 1)
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
    let smooth = surface_parameter_span_smooth(surface, domain, work, cancel)?;
    let mut positions = [[Interval::point(0.0); 3]; 3];
    for vertex in 0..if smooth { 3 } else { 1 } {
        positions[vertex] = surface_enclosure(
            surface, [[uv[vertex][0]; 2], [uv[vertex][1]; 2]], work, cancel,
        )?.position;
    }
    triangle_interpolation_error_prepared(
        uv, points, domain, bound, smooth, &positions, (work, cancel),
    )
}

fn triangle_interpolation_error_prepared(
    uv: &[[f64; 2]; 3],
    points: &[[f64; 3]; 3],
    domain: [[f64; 2]; 2],
    bound: &SurfaceEnclosure,
    smooth: bool,
    positions: &[[Interval; 3]; 3],
    (work, cancel): (&mut GeometryWorkBudget, &AtomicBool),
) -> Result<f64, GeometryError> {
    if !smooth {
        let mut error = norm_upper(std::array::from_fn(|axis| {
            bound.position[axis].sub(Interval::point(points[0][axis]))
        }))?;
        for point in points {
            error = error.max(norm_upper(std::array::from_fn(|axis| {
                positions[0][axis].sub(Interval::point(point[axis]))
            }))?);
        }
        return Ok(error);
    }
    smooth_triangle_interpolation_error(
        uv, points, domain, [&bound.duu, &bound.duv, &bound.dvv], positions, work, cancel,
    )
}

/// The caller supplies completed same-box smoothness and source Hessian proof.
/// This is the common interpolation theorem, not a synthetic first/position jet.
fn smooth_triangle_interpolation_error(
    uv: &[[f64; 2]; 3],
    points: &[[f64; 3]; 3],
    domain: [[f64; 2]; 2],
    hessian: [&[Interval; 3]; 3],
    positions: &[[Interval; 3]; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<f64, GeometryError> {
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
    let remainder = rectangle_triangle_remainder([du, dv], hessian);
    let mut vertex_error = 0.0_f64;
    let mut component_vertex_error = [0.0_f64; 3];
    for vertex in 0..3 {
        let residual = std::array::from_fn(|axis| {
            positions[vertex][axis].sub(Interval::point(points[vertex][axis]))
        });
        for axis in 0..3 {
            component_vertex_error[axis] =
                component_vertex_error[axis].max(residual[axis].abs_upper());
        }
        vertex_error = vertex_error.max(norm_upper(residual)?);
    }
    let rectangle_error = up(norm_upper(remainder)? + vertex_error);
    if rectangle_error <= spiling_contracts::geometry::MESH_CARRIER_TOLERANCE_MM {
        return Ok(rectangle_error);
    }
    let Some(metric_remainder) =
        triangle_metric_remainder(uv, [du, dv], hessian, work, cancel)?
    else {
        return Ok(rectangle_error);
    };
    let component_error = std::array::from_fn(|axis| {
        metric_remainder[axis].add(Interval::point(component_vertex_error[axis]))
    });
    // This optional local arithmetic has no native calls. Overflow declines
    // only this proposal; every native proof failure remains terminal above.
    Ok(match norm_upper(component_error) {
        Ok(error) => rectangle_error.min(error),
        Err(_) => rectangle_error,
    })
}

fn rectangle_triangle_remainder(
    [du, dv]: [f64; 2],
    hessian: [&[Interval; 3]; 3],
) -> [Interval; 3] {
    std::array::from_fn(|axis| {
        Interval::point(hessian[0][axis].abs_upper())
            .mul(Interval::point(du).square())
            .add(
                Interval::point(hessian[1][axis].abs_upper())
                    .mul(Interval::point(du))
                    .mul(Interval::point(dv))
                    .mul(Interval::point(2.0)),
            )
            .add(Interval::point(hessian[2][axis].abs_upper()).mul(Interval::point(dv).square()))
            .mul(Interval::point(0.125))
    })
}

/// Exactly one proof subdivision, not a rounded replacement mesh. The caller
/// supplies a completed C1/Hessian proof on the whole parent box. Exact symbolic
/// midpoint children cover the same UV triangle and the SAME affine physical
/// plane. Interval vertices only enclose those exact objects.
fn subdivide_display_triangle(
    surface: &Surface,
    uv: &[[f64; 2]; 3],
    points: &[[f64; 3]; 3],
    domain: [[f64; 2]; 2],
    hessian: [&[Interval; 3]; 3],
    positions: &[[Interval; 3]; 3],
    (work, cancel): (&mut GeometryWorkBudget, &AtomicBool),
) -> Result<Option<f64>, GeometryError> {
    work.charge(54, cancel)?;
    let zero = Interval::point(0.0);
    let half = Interval::point(0.5);
    let mut scratch = DisplaySubdivisionScratch {
        uv: [[zero; 2]; 6],
        points: [[zero; 3]; 6],
        positions: [[zero; 3]; 6],
        domain: [[0.0; 2]; 2],
        remainder: [zero; 3],
        residual: [zero; 3],
    };
    for vertex in 0..3 {
        work.charge(1, cancel)?;
        for axis in 0..2 {
            if !uv[vertex][axis].is_finite()
                || !domain[axis][0].is_finite() || !domain[axis][1].is_finite()
                || uv[vertex][axis] < domain[axis][0] || uv[vertex][axis] > domain[axis][1]
            {
                return Ok(None);
            }
            scratch.uv[vertex][axis] = Interval::point(uv[vertex][axis]);
        }
        scratch.points[vertex] = points[vertex].map(Interval::point);
        scratch.positions[vertex] = positions[vertex];
    }
    let edges = [[0, 1], [1, 2], [2, 0]];
    for (edge, [a, b]) in edges.into_iter().enumerate() {
        work.charge(1, cancel)?;
        let midpoint = 3 + edge;
        for axis in 0..2 {
            let average = scratch.uv[a][axis].mul(half).add(scratch.uv[b][axis].mul(half));
            // The exact average lies in BOTH the endpoint hull and the parent
            // box. This algebraic intersection is not source/chart authority.
            let lo = average.lo.max(uv[a][axis].min(uv[b][axis])).max(domain[axis][0]);
            let hi = average.hi.min(uv[a][axis].max(uv[b][axis])).min(domain[axis][1]);
            let Ok(average) = Interval::new(lo, hi) else {
                return Ok(None);
            };
            scratch.uv[midpoint][axis] = average;
        }
        for axis in 0..3 {
            // Never substitute F(midpoint): these are the exact averages on
            // the original affine physical triangle, enclosed outward.
            let average = scratch.points[a][axis].mul(half).add(scratch.points[b][axis].mul(half));
            if !average.lo.is_finite() || !average.hi.is_finite() {
                return Ok(None);
            }
            scratch.points[midpoint][axis] = average;
        }
    }
    for midpoint in 3..6 {
        work.charge(1, cancel)?;
        scratch.domain = scratch.uv[midpoint].map(|range| [range.lo, range.hi]);
        scratch.positions[midpoint] = surface_position_enclosure(
            surface, scratch.domain, work, cancel,
        ).map_err(|cause| GeometryError::new(cause.code, format!(
            "triangle proof midpoint={} uv_box={:?}: {}",
            midpoint - 3, scratch.domain, cause.message,
        )))?;
    }
    let mut complete = 0.0_f64;
    // 3=01, 4=12, 5=20. These EXACT conceptual children partition the parent;
    // their interval bounding boxes merely enclose them.
    for child in [[0, 3, 5], [3, 1, 4], [5, 4, 2], [3, 4, 5]] {
        work.charge(1, cancel)?;
        scratch.domain = std::array::from_fn(|axis| [
            child.iter().map(|&vertex| scratch.uv[vertex][axis].lo)
                .fold(f64::INFINITY, f64::min).max(domain[axis][0]),
            child.iter().map(|&vertex| scratch.uv[vertex][axis].hi)
                .fold(f64::NEG_INFINITY, f64::max).min(domain[axis][1]),
        ]);
        let spans = scratch.domain.map(|range| {
            Interval::point(range[1]).sub(Interval::point(range[0])).abs_upper()
        });
        scratch.remainder = rectangle_triangle_remainder(spans, hessian);
        let mut vertex_error = 0.0_f64;
        for vertex in child {
            work.charge(1, cancel)?;
            scratch.residual = std::array::from_fn(|axis| {
                scratch.positions[vertex][axis].sub(scratch.points[vertex][axis])
            });
            let Ok(error) = norm_upper(scratch.residual) else {
                return Ok(None); // Local interval arithmetic only.
            };
            vertex_error = vertex_error.max(error);
        }
        let Ok(remainder) = norm_upper(scratch.remainder) else {
            return Ok(None); // No native errors are caught here.
        };
        let bound = up(remainder + vertex_error);
        if !bound.is_finite() {
            return Ok(None);
        }
        complete = complete.max(bound);
    }
    Ok(Some(complete))
}

/// One actual-triangle variance bound using the same completed box Hessian.
/// Young's inequality supplies a constant diagonal majorant A per component.
/// For every barycentric U and ANY center c:
/// sum(lambda_i * |Ui-U|_A^2) <= max_i |Ui-c|_A^2.
/// Centers are proposals only; no native evaluation or new smoothness claim.
fn triangle_metric_remainder(
    uv: &[[f64; 2]; 3],
    spans: [f64; 2],
    hessian: [&[Interval; 3]; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[Interval; 3]>, GeometryError> {
    work.charge(1, cancel)?;
    let r = spans[1] / spans[0];
    if !r.is_finite() || r <= 0.0 || spans.into_iter().any(|span| !span.is_finite() || span <= 0.0) {
        return Ok(None);
    }
    // The chosen floating-point r is an exact positive constant; its computed
    // reciprocal, unlike its selection, must be enclosed outward.
    let Ok(inverse_r) = Interval::point(1.0).div(Interval::point(r)) else {
        return Ok(None);
    };
    if !inverse_r.lo.is_finite() || !inverse_r.hi.is_finite() {
        return Ok(None);
    }
    let mut remainder = [Interval::point(0.0); 3];
    for axis in 0..3 {
        work.charge(1, cancel)?;
        let mixed = Interval::point(hessian[1][axis].abs_upper());
        let diagonal = [
            Interval::point(hessian[0][axis].abs_upper()).add(mixed.mul(Interval::point(r))),
            Interval::point(hessian[2][axis].abs_upper()).add(mixed.mul(inverse_r)),
        ];
        if diagonal.iter().any(|value| !value.hi.is_finite() || value.hi < 0.0) {
            return Ok(None);
        }
        let diagonal = diagonal.map(|value| Interval::point(value.hi));
        let distance = |a: [f64; 2], b: [f64; 2]| {
            Interval::point(a[0]).sub(Interval::point(b[0])).square().mul(diagonal[0])
                .add(Interval::point(a[1]).sub(Interval::point(b[1])).square().mul(diagonal[1]))
        };
        let edges = [[0, 1], [1, 2], [2, 0]];
        let mut selected = edges[0];
        let mut longest = -1.0_f64;
        for edge in edges {
            work.charge(1, cancel)?;
            let length = distance(uv[edge[0]], uv[edge[1]]).hi;
            if !length.is_finite() {
                return Ok(None);
            }
            if length > longest {
                longest = length;
                selected = edge;
            }
        }
        // The rounded midpoint is itself an exact finite center. The variance
        // identity does not require that it be the exact geometric midpoint.
        let center = std::array::from_fn(|coordinate| {
            uv[selected[0]][coordinate] * 0.5 + uv[selected[1]][coordinate] * 0.5
        });
        if center.into_iter().any(|value| !value.is_finite()) {
            return Ok(None);
        }
        let mut radius = 0.0_f64;
        for vertex in uv {
            work.charge(1, cancel)?;
            let squared_distance = distance(*vertex, center).hi;
            if !squared_distance.is_finite() {
                return Ok(None);
            }
            radius = radius.max(squared_distance);
        }
        remainder[axis] = Interval::point(radius).mul(Interval::point(0.5));
    }
    Ok(Some(remainder))
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

struct BoundaryPatchRequest {
    domain: [[f64; 2]; 2],
    fixed_axis: usize,
    fixed: f64,
    supported: [[f64; 2]; 2],
    vertex: [f64; 3],
}

fn certify_boundary_patch(
    surface: &Surface,
    request: &BoundaryPatchRequest,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    let &BoundaryPatchRequest {
        domain,
        fixed_axis,
        fixed,
        supported,
        vertex,
    } = request;
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
        &BoundaryPatchRequest {
            domain,
            fixed_axis: *fixed_axis,
            fixed: *fixed_parameter,
            supported: *supported_parameter_box,
            vertex: original_vertex_mm,
        },
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
        assert!((0.25..=0.25000001).contains(&bound));
        for i in 0..=20 {
            for j in 0..=20 - i {
                let u = f64::from(i) / 20.0;
                let v = f64::from(j) / 20.0;
                assert!(u * v <= bound);
            }
        }
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let mut memo = DisplayTriangleProof::new(&surface, &mut work, &cancel).unwrap();
        for _ in 0..2 {
            let cached = memo.triangle_error_bound(&uv, &triangle, &mut work, &cancel).unwrap();
            assert!((0.25..=0.25000001).contains(&cached));
        }
        // Cached native point enclosures must not cache a previous mesh
        // triangle's residual or authorize a different interpolation.
        let mut displaced = triangle;
        displaced[0][2] = 1.0;
        let displaced_bound = memo.triangle_error_bound(&uv, &displaced, &mut work, &cancel).unwrap();
        // The unit physical vertex residual remains mandatory even when the
        // filled-triangle proof tightens the former 1.25 Taylor estimate.
        assert!((1.0..=1.25000001).contains(&displaced_bound));
        let narrowed_uv = [[0.0, 0.0], [0.5, 0.0], [0.0, 0.5]];
        let narrowed_triangle = [[0.0, 0.0, 0.0], [0.5, 0.0, 0.0], [0.0, 0.5, 0.0]];
        let narrowed_bound = memo.triangle_error_bound(
            &narrowed_uv, &narrowed_triangle, &mut work, &cancel,
        ).unwrap();
        assert!((0.0625..=0.06250001).contains(&narrowed_bound));
        assert!(memo.triangle_error_bound(
            &uv, &triangle, &mut work, &AtomicBool::new(true),
        ).is_err());
        let mesh_uvs = uv.map(|point| monstertruck_modeling::Vector2::new(point[0], point[1]));
        let mesh_boxes = vec![DisplayMeshBox::new([
            0.0_f64.to_bits(), 1.0_f64.to_bits(), 0.0_f64.to_bits(), 1.0_f64.to_bits(),
        ])];
        let mut indexed = DisplayTriangleProof::with_mesh_inventory(
            &surface, &mesh_uvs, vec![0], mesh_boxes, &mut work, &cancel,
        ).unwrap();
        let indexed_bound = indexed.indexed_triangle_error_bound(
            &uv, &triangle, 0, &[0, 1, 2], &mut work, &cancel,
        ).unwrap();
        assert!((0.25..=0.25000001).contains(&indexed_bound));
        let displaced_indexed = indexed.indexed_triangle_error_bound(
            &uv, &displaced, 0, &[0, 1, 2], &mut work, &cancel,
        ).unwrap();
        // A retained conditioned Hessian is a source proof, not the former
        // triangle's error: this actual displaced vertex has unit residual.
        assert!((1.0..=1.25000001).contains(&displaced_indexed));
        // A captured mesh index cannot lend its native point proof to changed
        // parameter bits, even on the same carrier and within the same box.
        let changed_uv = [[0.0, 0.0], [1.0, 0.25], [0.0, 1.0]];
        let changed_triangle = [[0.0, 0.0, 0.0], [1.0, 0.25, 0.25], [0.0, 1.0, 0.0]];
        assert_eq!(indexed.indexed_triangle_error_bound(
            &changed_uv, &changed_triangle, 0, &[0, 1, 2], &mut work, &cancel,
        ).unwrap_err().code, GeometryErrorCode::InvalidGeometry);
        assert_eq!(indexed.indexed_triangle_error_bound(
            &narrowed_uv, &narrowed_triangle, 0, &[0, 1, 2], &mut work, &cancel,
        ).unwrap_err().code, GeometryErrorCode::InvalidGeometry);
    }

    #[test]
    fn display_triangle_variance_encloses_actual_quadratic_boundary_fan() {
        use monstertruck_modeling::{BsplineSurface, KnotVector, Point3};
        let u = [0.0, 0.5, 1.0];
        let v = [-1.0, 0.0, 1.0];
        let u_square = [0.0, 0.0, 1.0];
        let v_square = [1.0, -1.0, 1.0];
        let surface = Surface::BsplineSurface(BsplineSurface::new(
            (
                KnotVector::bezier_knot(2),
                KnotVector::from(vec![-1.0, -1.0, -1.0, 1.0, 1.0, 1.0]),
            ),
            (0..3).map(|i| {
                (0..3).map(|j| Point3::new(u[i], v[j], u_square[i] + v_square[j])).collect()
            }).collect(),
        ));
        // This original polynomial is F(u,v)=(u,v,u²+v²). The actual affine
        // triangle has z=2u, with error 2u-u²-v² attaining one at (1,0).
        let uv = [[0.0, 0.0], [1.0, -1.0], [1.0, 1.0]];
        let triangle = [[0.0, 0.0, 0.0], [1.0, -1.0, 2.0], [1.0, 1.0, 2.0]];
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let mut proof = DisplayTriangleProof::new(&surface, &mut work, &cancel).unwrap();
        let bound = proof.triangle_error_bound(&uv, &triangle, &mut work, &cancel).unwrap();
        // The rectangle variance estimate is 1.25; this actual fan permits
        // one, without discarding its boundary midpoint or any interior point.
        // A proof subdivision must retain that actual midpoint residual:
        // replacing its physical midpoint by F(midpoint) would underbound one.
        assert!((1.0..=1.00000001).contains(&bound));
        for i in 0..=20 {
            for j in 0..=20 {
                let u = f64::from(i) / 20.0;
                let v = u * (f64::from(j) / 10.0 - 1.0);
                assert!((2.0 * u - u * u - v * v).abs() <= bound);
            }
        }
    }

    #[test]
    fn display_triangle_memo_preserves_nonsmooth_whole_image_proof() {
        use monstertruck_modeling::{BsplineSurface, KnotVector, Point3};
        let surface = Surface::BsplineSurface(BsplineSurface::new(
            (
                KnotVector::from(vec![0.0, 0.0, 1.0, 2.0, 2.0]),
                KnotVector::bezier_knot(1),
            ),
            vec![
                vec![Point3::new(0.0, 0.0, 0.0), Point3::new(0.0, 1.0, 0.0)],
                vec![Point3::new(1.0, 0.0, 0.0), Point3::new(1.0, 1.0, 0.0)],
                vec![Point3::new(2.0, 0.0, 1.0), Point3::new(2.0, 1.0, 1.0)],
            ],
        ));
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let domain = [[0.0, 2.0], [0.0, 1.0]];
        assert!(!surface_parameter_span_smooth(&surface, domain, &mut work, &cancel).unwrap());
        let uv = [[0.0, 0.0], [2.0, 0.0], [0.0, 1.0]];
        let points = [[0.0, 0.0, 0.0], [2.0, 0.0, 1.0], [0.0, 1.0, 0.0]];
        let mut memo = DisplayTriangleProof::new(&surface, &mut work, &cancel).unwrap();
        // Warm a smooth planar span of the same immutable face first.
        let smooth_uv = [[0.0, 0.0], [0.5, 0.0], [0.0, 0.5]];
        let smooth_points = [[0.0, 0.0, 0.0], [0.5, 0.0, 0.0], [0.0, 0.5, 0.0]];
        assert!(memo.triangle_error_bound(
            &smooth_uv, &smooth_points, &mut work, &cancel,
        ).unwrap() < 1.0e-9);
        for _ in 0..2 {
            let bound = memo.triangle_error_bound(&uv, &points, &mut work, &cancel).unwrap();
            // At u=1 the actual kink and affine triangle differ by 1/2.
            assert!((0.5..=3.0).contains(&bound));
        }
        // A roundoff-sized straddle is still NOT a smooth source span. The
        // independently planar right-side comparison plus charged transport
        // can nevertheless prove the original unchanged triangle accurately.
        let sliver_uv = [
            [1.0_f64.next_down(), 0.0],
            [1.03125, 0.0],
            [1.0, 0.03125],
        ];
        let sliver_points = sliver_uv.map(|point| [
            point[0], point[1], (point[0] - 1.0).max(0.0),
        ]);
        assert!(!surface_parameter_span_smooth(
            &surface, [[sliver_uv[0][0], sliver_uv[1][0]], [0.0, 0.03125]],
            &mut work, &cancel,
        ).unwrap());
        assert!(memo.triangle_error_bound(
            &sliver_uv, &sliver_points, &mut work, &cancel,
        ).unwrap() <= spiling_contracts::geometry::MESH_CARRIER_TOLERANCE_MM);
    }

    #[test]
    fn display_hessian_partition_encloses_complete_cubic_patch() {
        use monstertruck_modeling::{BsplineSurface, KnotVector, Point3};
        // z=u^3: its exact u-Hessian ranges over the COMPLETE [0,6].
        let surface = Surface::BsplineSurface(BsplineSurface::new(
            (KnotVector::bezier_knot(3), KnotVector::bezier_knot(1)),
            [0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0].into_iter().enumerate().map(|(i, x)| {
                let z = if i == 3 { 1.0 } else { 0.0 };
                vec![Point3::new(x, 0.0, z), Point3::new(x, 1.0, z)]
            }).collect(),
        ));
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let domain = [[0.0, 1.0]; 2];
        let smooth = surface_parameter_span_smooth(&surface, domain, &mut work, &cancel).unwrap();
        assert!(smooth);
        let parent = DisplayFullBoxProof {
            bound: surface_piecewise_enclosure(&surface, domain, &mut work, &cancel).unwrap(),
            smooth,
            partitioned: false,
        };
        let hessian = partition_display_hessian(&surface, domain, &parent, &mut work, &cancel).unwrap();
        assert!(hessian[0][2].lo <= 0.0 && hessian[0][2].hi >= 6.0);
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
    fn rational_surface_extension_does_not_inherit_strict_curve_domain() {
        use monstertruck_modeling::{BsplineSurface, KnotVector, NurbsSurface, Vector4};
        let surface = Surface::NurbsSurface(NurbsSurface::new(BsplineSurface::new(
            (
                KnotVector::from(vec![0.0, 1.0, 2.0, 3.0]),
                KnotVector::bezier_knot(1),
            ),
            vec![
                vec![
                    Vector4::new(0.0, 0.0, 7.0, 1.0),
                    Vector4::new(0.0, 1.0, 7.0, 1.0),
                ],
                vec![
                    Vector4::new(1.0, 0.0, 7.0, 1.0),
                    Vector4::new(1.0, 1.0, 7.0, 1.0),
                ],
            ],
        )));
        // Outside the regular U interval, only the first tensor row contributes.
        // Its nonzero basis mass cancels in the quotient: (0, v, 7).
        // This is a coefficient-extension enclosure, not source-domain admission.
        let jet = surface_enclosure(
            &surface,
            [[0.5; 2]; 2],
            &mut GeometryWorkBudget::new(1_000_000),
            &AtomicBool::new(false),
        )
        .unwrap();
        for (actual, expected) in [
            (jet.position, [0.0, 0.5, 7.0]),
            (jet.du, [0.0; 3]),
            (jet.dv, [0.0, 1.0, 0.0]),
        ] {
            for (actual, expected) in actual.into_iter().zip(expected) {
                assert!(
                    actual.lo <= expected && expected <= actual.hi && actual.hi - actual.lo < 1e-9,
                    "{actual:?} does not tightly enclose {expected}"
                );
            }
        }
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
            .gnomonic_jet(
                (0.0, 1.0),
                &crate::parametric::GnomonicFrame {
                    center_mm: [0.0; 3],
                    x_axis: x,
                    y_axis: y,
                    normal,
                },
                &mut work,
                &cancel,
            )
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
                    &GnomonicFrame {
                        center_mm: [0.0, 0.0, 1.5],
                        x_axis: x,
                        y_axis: y,
                        normal,
                    },
                    &mut work,
                    &cancel
                )
                .unwrap_err()
                .code,
            GeometryErrorCode::AnalysisIndeterminate
        );
        assert_eq!(
            prepared
                .gnomonic_jet(
                    (0.0, 1.0),
                    &GnomonicFrame {
                        center_mm: [0.0; 3],
                        x_axis: x,
                        y_axis: x,
                        normal
                    },
                    &mut work,
                    &cancel,
                )
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
        // Piecewise first derivatives can enclose both one-sided slopes
        // without granting the rejected whole-span C1/Taylor qualification.
        let both = prepared.retained_first_enclosure((0.0, 1.0), &mut work, &cancel)
            .unwrap().unwrap();
        assert!(both[2].lo <= -2.0 && both[2].hi >= 2.0);
        let first = prepared.retained_first_enclosure((0.0, 0.25), &mut work, &cancel)
            .unwrap().unwrap();
        assert!(first[0].lo <= 1.0 && first[0].hi >= 1.0);
        assert!(first[2].lo <= 2.0 && first[2].hi >= 2.0);
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
            .sampling_interval(
                &endpoints,
                None,
                &frame,
                SECTION_PLANE_TOLERANCE_MM,
                &mut work,
                &cancel,
            )
            .unwrap();
        let bound = interval.enclosure();
        // Both endpoint tangents are finite and both pieces have C''=0, but
        // the actual native midpoint is 1 mm from the returned straight chord.
        assert!(bound.chord_error_mm >= 1.0);
        assert!(bound.plane_error_mm >= 1.0);
        let smooth_endpoints = [0.0, 0.25].map(|parameter| {
            prepared
                .sampling_endpoint(parameter, &mut work, &cancel)
                .unwrap()
        });
        let smooth = prepared
            .sampling_interval(
                &smooth_endpoints,
                None,
                &frame,
                SECTION_PLANE_TOLERANCE_MM,
                &mut work,
                &cancel,
            )
            .unwrap()
            .smooth_range()
            .expect("a strict interior of the first linear piece is smooth");
        // Adjacency and overlap do not extend a closed proof, especially across
        // the real C0 corner whose complete-image error was established above.
        for range in [[0.25, 0.5], [0.125, 0.375], [0.25, 0.75], [0.0, 1.0]] {
            let endpoints = range.map(|parameter| {
                prepared
                    .sampling_endpoint(parameter, &mut work, &cancel)
                    .unwrap()
            });
            let error = prepared
                .sampling_interval(
                    &endpoints,
                    Some(smooth),
                    &frame,
                    SECTION_PLANE_TOLERANCE_MM,
                    &mut work,
                    &cancel,
                )
                .err()
                .expect("an inherited smooth range must not authorize another range");
            assert_eq!(error.code, GeometryErrorCode::InvalidGeometry);
        }
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
    fn literal_rational_uv_c1_uses_exact_weights_and_distinct_span_lengths() {
        use monstertruck_modeling::{BsplineCurve, Curve2D, KnotVector, NurbsCurve, Vector3};
        let curve = |right_x| Curve2D::NurbsCurve(NurbsCurve::new(BsplineCurve::new(
            KnotVector::from(vec![0.0, 0.0, 0.0, 1.0, 1.0, 3.0, 3.0, 3.0]),
            vec![
                Vector3::new(-1.0, 1.0, 1.0),
                Vector3::new(0.0, 0.0, 1.0),
                Vector3::new(2.0, 0.0, 2.0),
                Vector3::new(right_x, 0.0, 3.0),
                Vector3::new(8.0, 2.0, 2.0),
            ],
        )));
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(10_000_000);
        let exact = curve(5.0);
        // The native STRICT structural policy alone cannot admit this C1 join.
        assert!(monstertruck_modeling::validated_curve2d_jet(
            &exact, [0.0, 3.0],
            &mut monstertruck_modeling::ValidatedGeometryBudget::new(1_000_000),
            &cancel,
        ).is_err());
        assert!(curve2d_c1_jet(&exact, [0.0, 3.0], &mut work, &cancel)
            .unwrap().is_some());
        let knot = curve2d_c1_jet(&exact, [1.0; 2], &mut work, &cancel)
            .unwrap().unwrap();
        assert!(knot.derivative[0].lo <= 1.0 && 1.0 <= knot.derivative[0].hi);
        assert!(knot.derivative[1].lo <= 0.0 && 0.0 <= knot.derivative[1].hi);

        // Same positive tangent direction, different native t speed: G1 only.
        let g1 = curve(6.0);
        for range in [[0.0, 3.0], [1.0; 2], [0.0, 1.0], [1.0, 3.0]] {
            assert!(curve2d_c1_jet(&g1, range, &mut work, &cancel)
                .unwrap().is_none());
        }
        let sides = curve2d_piecewise_jet(&g1, [1.0; 2], &mut work, &cancel).unwrap();
        assert!(sides.derivative[0].lo <= 1.0 && 1.5 <= sides.derivative[0].hi);
        assert!(curve2d_c1_jet(&g1, [0.0, 0.5], &mut work, &cancel)
            .unwrap().is_some());

        // A one-ULP coefficient residual is not an exact identity.
        let near = curve(f64::from_bits(5.0_f64.to_bits() + 1));
        assert!(curve2d_c1_jet(&near, [0.0, 3.0], &mut work, &cancel)
            .unwrap().is_none());
    }

    #[test]
    fn literal_uv_c1_preserves_reversed_posed_wrapper_and_structural_joins() {
        use monstertruck_modeling::{
            BsplineCurve, Curve2D, Invertible, KnotVector, Matrix3, Point2,
            Processor, TrimmedCurve, Vector2,
        };
        let basis = Curve2D::BsplineCurve(BsplineCurve::new(
            KnotVector::from(vec![0.0, 0.0, 0.0, 1.0, 3.0, 3.0, 3.0]),
            vec![
                Point2::new(0.0, 0.0), Point2::new(1.0, 0.0),
                Point2::new(1.0, 2.0), Point2::new(3.0, 2.0),
            ],
        ));
        let mut wrapped = Processor::with_transform(
            TrimmedCurve::new(Box::new(basis.clone()), (0.0, 3.0)),
            Matrix3::from_translation(Vector2::new(4.0, -2.0)),
        );
        wrapped.invert();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(10_000_000);
        let original = curve2d_c1_jet(&basis, [1.0; 2], &mut work, &cancel)
            .unwrap().unwrap();
        let reflected = curve2d_c1_jet(
            &Curve2D::TrimmedCurve(wrapped), [2.0; 2], &mut work, &cancel,
        ).unwrap().unwrap();
        for axis in 0..2 {
            let shift = [4.0, -2.0][axis];
            let expected = original.position[axis].mid() + shift;
            assert!(reflected.position[axis].lo <= expected
                && expected <= reflected.position[axis].hi);
            let derivative = -original.derivative[axis].mid();
            assert!(reflected.derivative[axis].lo <= derivative
                && derivative <= reflected.derivative[axis].hi);
        }
    }

    #[test]
    fn literal_uv_jump_refuses_piecewise_even_at_closed_knot() {
        use monstertruck_modeling::{BsplineCurve, Curve2D, KnotVector, Point2};
        let jump = Curve2D::BsplineCurve(BsplineCurve::new(
            KnotVector::from(vec![0.0, 0.0, 1.0, 1.0, 2.0, 2.0]),
            vec![
                Point2::new(0.0, 0.0), Point2::new(1.0, 0.0),
                Point2::new(2.0, 0.0), Point2::new(3.0, 0.0),
            ],
        ));
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(10_000_000);
        for range in [[0.0, 2.0], [1.0; 2]] {
            assert!(curve2d_c1_jet(&jump, range, &mut work, &cancel)
                .unwrap().is_none());
            assert_eq!(
                curve2d_piecewise_jet(&jump, range, &mut work, &cancel)
                    .unwrap_err().code,
                GeometryErrorCode::SingularGeometry,
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
        let piece = prepared.pieces().iter()
            .find(|piece| piece.parameter[0] < piece.parameter[1]).unwrap();
        let [a, b] = piece.parameter;
        let range = (a * 0.75 + b * 0.25, a * 0.25 + b * 0.75);
        let tangent = mm.retained_first_enclosure(range, &mut work, &cancel).unwrap().unwrap();
        assert!(tangent[0].contains_zero() && tangent[2].contains_zero());
        assert!(tangent[1].lo <= 25.4 && tangent[1].hi >= 25.4);
        assert_eq!(
            mm.retained_first_enclosure(range, &mut GeometryWorkBudget::new(0), &cancel)
                .unwrap_err().code,
            GeometryErrorCode::ResourceLimit,
        );
        assert_eq!(
            mm.retained_first_enclosure(
                range, &mut GeometryWorkBudget::new(1_000_000), &AtomicBool::new(true),
            ).unwrap_err().code,
            GeometryErrorCode::Cancelled,
        );
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
        assert_eq!(
            first.retained_first_enclosure((0.25, 0.75), &mut work, &cancel)
                .unwrap_err().code,
            GeometryErrorCode::InvalidGeometry,
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
        let end = prepared
            .sampling_endpoint(14.0, &mut work, &cancel)
            .unwrap();
        let smooth = prepared
            .sampling_interval(
                &[start, end],
                None,
                &frame,
                SECTION_PLANE_TOLERANCE_MM,
                &mut work,
                &cancel,
            )
            .unwrap()
            .smooth_range()
            .unwrap();
        let mut area = Interval::point(0.0);
        for index in 0..16 {
            let end = prepared
                .sampling_endpoint(10.0 + f64::from(index + 1) / 4.0, &mut work, &cancel)
                .unwrap();
            let interval = prepared
                .sampling_interval(
                    &[start, end],
                    Some(smooth),
                    &frame,
                    SECTION_PLANE_TOLERANCE_MM,
                    &mut work,
                    &cancel,
                )
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
    fn sampling_smooth_ranges_refuse_foreign_sources_units_and_restrictions() {
        let native = quadratic_sampling_native();
        let other_native = quadratic_sampling_native();
        let prepared = PreparedCurveEnclosure::borrowed(&native, 1.0);
        let same_source = PreparedCurveEnclosure::borrowed(&native, 1.0);
        let other_units = PreparedCurveEnclosure::borrowed(&native, 25.4);
        let restricted = PreparedCurveEnclosure::borrowed_restricted(&native, 1.0, (10.0, 11.0));
        let other_source = PreparedCurveEnclosure::borrowed(&other_native, 1.0);
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let frame = PlaneFrameMm::from_plane(crate::PlaneMm {
            origin_mm: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
        })
        .unwrap();
        let endpoints = [10.0, 10.5].map(|parameter| {
            prepared
                .sampling_endpoint(parameter, &mut work, &cancel)
                .unwrap()
        });
        let smooth = prepared
            .sampling_interval(
                &endpoints,
                None,
                &frame,
                SECTION_PLANE_TOLERANCE_MM,
                &mut work,
                &cancel,
            )
            .unwrap()
            .smooth_range()
            .unwrap();
        for binding in [&same_source, &other_units, &restricted, &other_source] {
            // These endpoints belong to the target and lie strictly inside the
            // qualified range. Only the foreign smooth authority must refuse.
            let endpoints = [10.125, 10.25].map(|parameter| {
                binding
                    .sampling_endpoint(parameter, &mut work, &cancel)
                    .unwrap()
            });
            let error = binding
                .sampling_interval(
                    &endpoints,
                    Some(smooth),
                    &frame,
                    SECTION_PLANE_TOLERANCE_MM,
                    &mut work,
                    &cancel,
                )
                .err()
                .expect("a positive smooth proof cannot transfer prepared bindings");
            assert_eq!(error.code, GeometryErrorCode::InvalidGeometry);
        }
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
                .sampling_interval(
                    &endpoints,
                    None,
                    &frame,
                    SECTION_PLANE_TOLERANCE_MM,
                    &mut work,
                    &cancel,
                )
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
            .sampling_interval(
                &[start, end],
                None,
                &frame,
                SECTION_PLANE_TOLERANCE_MM,
                &mut work,
                &cancel,
            )
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
                .sampling_interval(
                    &[start, end],
                    interval.smooth_range(),
                    &frame,
                    SECTION_PLANE_TOLERANCE_MM,
                    &mut work,
                    &cancelled
                )
                .err()
                .unwrap()
                .code,
            GeometryErrorCode::Cancelled
        );
    }
}
