// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Original implicit curves retain one immutable continuation, independently of
//! exact explicit source/native parameter maps. Units are applied by borrowed
//! query views, never by projecting a scaled guide onto a new branch.
use super::{conversion_error, error, kernel};
use crate::kernel::OriginalBoundaryContinuation;
use crate::{GeometryError, GeometryWorkBudget, check_cancel, parametric};
use monstertruck_io::step::load::step_geometry::{Curve3D, CurveParameterMap};
use monstertruck_modeling::{BoundedCurve, Curve, ParametricSurface};
use spiling_contracts::geometry::GeometryErrorCode;
use std::sync::atomic::AtomicBool;

pub(super) fn is_original_implicit(
    mut source: &Curve3D,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    for _ in 0..=32 {
        work.charge(1, cancel)?;
        source=match source {
            Curve3D::TrimmedCurve(wrapper)=>wrapper.entity().curve().as_ref(),
            Curve3D::SurfaceCurve(wrapper) if wrapper.kind()==monstertruck_io::step::load::step_geometry::SurfaceCurveKind::IntersectionCurve
                =>return Ok(true),
            Curve3D::SurfaceCurve(wrapper)=>wrapper.leader(),
            Curve3D::IntersectionCurve(_)=>return Ok(true),
            _=>return Ok(false),
        };
    }
    Err(error(
        GeometryErrorCode::ResourceLimit,
        "original implicit source nesting exceeds bound",
    ))
}

/// Only the loader's identity restriction may be removed for a lineage-proved
/// periodic child. Actual original TRIMMED_CURVEs remain part of the source.
pub(super) fn original_source(
    source: &Curve3D,
    whole: (f64, f64),
) -> Result<&Curve3D, GeometryError> {
    if source.range_tuple() == whole {
        return Ok(source);
    }
    let Curve3D::TrimmedCurve(wrapper) = source else {
        return Err(kernel(
            "subdivided original implicit edge lost its generated restriction",
        ));
    };
    if !wrapper.orientation()
        || *wrapper.transform() != monstertruck_modeling::base::Matrix4::from_scale(1.0)
        || wrapper.raw_parameter_gauge().map_err(|cause| {
            conversion_error("generated implicit restriction gauge refused", &cause)
        })? != monstertruck_io::step::load::step_geometry::SourceParameterGauge::Internal
        || wrapper.entity().curve().range_tuple() != whole
    {
        return Err(kernel(
            "periodic child is not an identity restriction of its whole original implicit edge",
        ));
    }
    Ok(wrapper.entity().curve().as_ref())
}

pub(super) fn original_native_curve(
    curve: &Curve,
    whole: (f64, f64),
) -> Result<&Curve, GeometryError> {
    if curve.range_tuple() == whole {
        return Ok(curve);
    }
    let Curve::TrimmedCurve(wrapper) = curve else {
        return Err(kernel(
            "native implicit child lost its original basis restriction",
        ));
    };
    if wrapper.entity().curve().range_tuple() != whole {
        return Err(kernel(
            "native implicit child basis does not cover original source range",
        ));
    }
    Ok(wrapper.entity().curve().as_ref())
}

pub(super) struct ContinuationInput<'a> {
    pub source: &'a Curve3D,
    pub whole: (f64, f64),
    pub shared: Option<usize>,
    pub scale: f64,
}

/// The admitted point-width policy of the original source continuation producer.
/// Shared with base-quality proposals; this is never an achieved error bound.
pub(super) fn original_point_tolerance_native(scale: f64) -> f64 {
    monstertruck_modeling::ValidatedCurveOptions::default().endpoint_tolerance / scale
}

pub(super) fn prepare(
    input: ContinuationInput<'_>,
    proofs: &mut Vec<monstertruck_modeling::PreparedValidatedCurve<'static>>,
    used: &mut usize,
    byte_limit: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<
    (
        Curve,
        OriginalBoundaryContinuation,
        crate::kernel::BoundaryCurveMm,
    ),
    GeometryError,
> {
    let ContinuationInput {
        source,
        whole,
        shared,
        scale,
    } = input;
    check_cancel(cancel)?;
    original_source(source, whole)?;
    let (curve, metadata): (_, CurveParameterMap) =
        source.to_modeling_with_parameter_map().map_err(|cause| {
            conversion_error(
                "original implicit source conversion refused",
                cause.as_ref(),
            )
        })?;
    let range = source.range_tuple();
    if range != metadata.source_range
        || range != metadata.native_range
        || !range.0.is_finite()
        || !range.1.is_finite()
        || range.0 >= range.1
    {
        return Err(kernel(
            "original implicit conversion changed its restricted parameter interval",
        ));
    }
    let owner = if let Some(owner) = shared {
        if proofs.get(owner).is_none() {
            return Err(kernel(
                "whole original implicit continuation owner is missing",
            ));
        }
        owner
    } else {
        let binding = std::mem::size_of::<monstertruck_modeling::PreparedValidatedCurve<'static>>();
        let remaining = byte_limit.checked_sub(*used).ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "original implicit continuation capacity exhausted",
            )
        })?;
        if proofs.len() == proofs.capacity() {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                "original implicit precharged owner capacity exhausted",
            ));
        }
        let original = original_native_curve(&curve, whole)?;
        let mut native = monstertruck_modeling::ValidatedGeometryBudget::new(
            usize::try_from(work.remaining()).unwrap_or(usize::MAX),
        );
        let bytes =
            monstertruck_modeling::curve_owned_bytes(original, remaining, &mut native, cancel);
        parametric::debit_native_work(work, native.consumed(), cancel)?;
        let reserved = bytes.map_err(parametric::native_error)?;
        *used = used.checked_add(reserved).ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "original implicit capacity overflow",
            )
        })?;
        work.charge(1, cancel)?;
        let owned = original.clone();
        // The fixed binding already lives in the precharged owner vector.
        let options = monstertruck_modeling::ValidatedCurveOptions {
            max_retained_bytes: remaining + binding,
            endpoint_tolerance: original_point_tolerance_native(scale),
            ..monstertruck_modeling::ValidatedCurveOptions::default()
        };
        let mut native = monstertruck_modeling::ValidatedGeometryBudget::new(
            usize::try_from(work.remaining()).unwrap_or(usize::MAX),
        );
        let result = monstertruck_modeling::prepared_validated_curve_owned(
            owned,
            [whole.0, whole.1],
            &options,
            &mut native,
            cancel,
        );
        parametric::debit_native_work(work, native.consumed(), cancel)?;
        let prepared = result.map_err(parametric::native_error)?;
        let heap = prepared.retained_bytes().saturating_sub(binding);
        *used = used
            .checked_add(heap.saturating_sub(reserved))
            .ok_or_else(|| {
                error(
                    GeometryErrorCode::ResourceLimit,
                    "original continuation proof capacity overflow",
                )
            })?;
        let owner = proofs.len();
        proofs.push(prepared);
        owner
    };
    let view =
        parametric::PreparedCurveEnclosure::borrowed_restricted(&proofs[owner], scale, range);
    let endpoint = |t, work: &mut GeometryWorkBudget| -> Result<[f64; 3], GeometryError> {
        let enclosure = view.endpoint_position(t, work, cancel)?;
        Ok(enclosure.map(|value| value.mid()))
    };
    let witness = crate::kernel::BoundaryCurveMm::Native {
        start_mm: endpoint(range.0, work)?,
        end_mm: endpoint(range.1, work)?,
    };
    Ok((
        curve,
        OriginalBoundaryContinuation {
            prepared: owner,
            source_range: range,
            original_range: whole,
            source_edge: None,
            raw_step_gauge: metadata.raw_step_gauge,
            source_domain: metadata.source_domain,
            scale_to_mm: scale,
            edge: None,
        },
        witness,
    ))
}

struct RootChart<'a> {
    leaf: &'a monstertruck_modeling::PreparedValidatedCurve<'static>,
    carrier: usize,
    reflections: [Option<(f64, f64)>; 32],
    count: usize,
}

fn unavailable(message: &str) -> GeometryError {
    error(
        GeometryErrorCode::ParameterCorrespondenceUnavailable,
        message,
    )
}

fn root_chart<'a>(
    mut curve: &Curve,
    mut prepared: &'a monstertruck_modeling::PreparedValidatedCurve<'static>,
    surface: &monstertruck_modeling::Surface,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<RootChart<'a>, GeometryError> {
    use monstertruck_modeling::base::*;
    let mut reflections = [None; 32];
    let mut count = 0;
    // A periodic child borrows a whole original receipt. Its sole generated
    // forward identity restriction is absent from the original owned source.
    if let Curve::TrimmedCurve(wrapper) = curve {
        let whole = prepared.range();
        if curve.range_tuple() != (whole[0], whole[1])
            && wrapper.entity().curve().range_tuple() == (whole[0], whole[1])
            && wrapper.orientation()
            && *wrapper.transform() == Matrix4::from_scale(1.0)
        {
            curve = wrapper.entity().curve().as_ref();
        }
    }
    while let Some(child) = prepared.mapped_basis() {
        work.charge(1, cancel)?;
        let Curve::TrimmedCurve(wrapper) = curve else {
            return Err(unavailable(
                "retained implicit UV tube lost its original trim operator",
            ));
        };
        if *wrapper.transform() != Matrix4::from_scale(1.0) {
            return Err(unavailable(
                "retained implicit source UV carrier has a different spatial frame",
            ));
        }
        if count == reflections.len() {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                "retained implicit UV chart nesting exceeds cap",
            ));
        }
        if !wrapper.orientation() {
            reflections[count] = Some(wrapper.entity().range());
        }
        count += 1;
        curve = wrapper.entity().curve().as_ref();
        prepared = child;
    }
    let Some(Curve::IntersectionCurve(original)) = prepared.native_curve() else {
        return Err(unavailable(
            "original implicit branch has no direct source carrier UV tube",
        ));
    };
    let carrier = if parametric::same_surface_coefficients(
        original.surface0().as_ref(),
        surface,
        work,
        cancel,
    )? {
        0
    } else if parametric::same_surface_coefficients(
        original.surface1().as_ref(),
        surface,
        work,
        cancel,
    )? {
        1
    } else {
        return Err(unavailable(
            "implicit coedge carrier does not structurally match its retained root UV chart",
        ));
    };
    if prepared
        .pieces()
        .iter()
        .any(|piece| piece.source_parameters.is_none())
    {
        return Err(unavailable(
            "original implicit branch does not retain a direct carrier UV tube",
        ));
    }
    Ok(RootChart {
        leaf: prepared,
        carrier,
        reflections,
        count,
    })
}

impl RootChart<'_> {
    fn tube(
        &self,
        range: [f64; 2],
        reference: [parametric::Interval; 2],
        periods: [Option<f64>; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[parametric::Interval; 2], GeometryError> {
        use parametric::Interval;
        work.charge(1, cancel)?;
        if periods
            .iter()
            .flatten()
            .any(|period| !period.is_finite() || *period <= 0.0)
        {
            return Err(unavailable(
                "original implicit carrier has an invalid native period",
            ));
        }
        let mut parameter = Interval::new(range[0], range[1])?;
        for (a, b) in self.reflections[..self.count].iter().flatten() {
            work.charge(1, cancel)?;
            parameter = Interval::point(*a).add(Interval::point(*b)).sub(parameter);
        }
        let active = self.leaf.range();
        parameter = Interval::new(parameter.lo.max(active[0]), parameter.hi.min(active[1]))?;
        let mut result: Option<[Interval; 2]> = None;
        let pieces = self.leaf.pieces();
        let mut first = 0;
        let mut last = pieces.len();
        while first < last {
            work.charge(1, cancel)?;
            let middle = first + (last - first) / 2;
            if pieces[middle].parameter[1] < parameter.lo {
                first = middle + 1;
            } else {
                last = middle;
            }
        }
        for piece in pieces[first..]
            .iter()
            .take_while(|piece| piece.parameter[0] <= parameter.hi)
        {
            work.charge(1, cancel)?;
            let mut uv = piece.source_parameters.ok_or_else(|| {
                unavailable("original implicit source UV continuation tube is unavailable")
            })?[self.carrier]
                .map(|value| Interval {
                    lo: value.lower,
                    hi: value.upper,
                });
            for (axis, period) in periods.into_iter().enumerate() {
                let Some(period) = period else { continue };
                let turns = ((reference[axis].mid() - uv[axis].mid()) / period).round();
                if !turns.is_finite() {
                    return Err(unavailable(
                        "original implicit periodic chart lift is not finite",
                    ));
                }
                if turns != 0.0 {
                    // This translates the SAME retained root in a covering
                    // chart. The interval includes the period's rounding;
                    // whole inverse/source correspondence still proves it.
                    let period = Interval::new(parametric::down(period), parametric::up(period))?;
                    uv[axis] = uv[axis].add(period.mul(Interval::point(turns)));
                }
            }
            result = Some(match result {
                None => uv,
                Some(previous) => std::array::from_fn(|axis| previous[axis].hull(uv[axis])),
            });
        }
        result.ok_or_else(|| {
            unavailable("retained root UV tube does not cover the original coedge interval")
        })
    }
}

/// A rounded preconditioner only proposes A. The outward whole-box defect
/// proves injectivity and the inverse Lipschitz constant on the convex UV box:
/// |p-q|inf <= |A|inf |S(p)-S(q)|inf / (1-|I-AJ|inf).
/// For an affine carrier the same defect also bounds its global left inverse
/// P=(AJ)^-1 A, and therefore the coefficient-defined projection P(C-O) of a
/// complete explicit source curve. A physical residual alone does not select
/// an inverse branch on a nonlinear or periodic carrier.
pub(super) fn inverse_margin(
    jet: parametric::SurfaceEnclosure,
    residual: f64,
) -> Result<Option<f64>, GeometryError> {
    use parametric::Interval;
    let Some(metric) = inverse_metric(jet)? else {
        return Ok(None);
    };
    let margin = Interval::point(metric.rows[0].max(metric.rows[1]))
        .mul(Interval::point(residual))
        .div(Interval::point(1.0).sub(Interval::point(metric.rho)))?
        .hi;
    Ok(margin.is_finite().then_some(margin))
}

struct InverseMetric {
    rows: [f64; 2],
    defects: [f64; 2],
    rho: f64,
}

impl InverseMetric {
    fn coordinate_gains(&self) -> Result<[f64; 2], GeometryError> {
        use parametric::Interval;
        let common = Interval::point(self.rows[0].max(self.rows[1]))
            .div(Interval::point(1.0).sub(Interval::point(self.rho)))?;
        // |delta_i| <= |A_i| e + |(I-AJ)_i| |delta|inf.
        // The common inverse bound proves |delta|inf <= common*e.
        let gains = std::array::from_fn(|axis| {
            Interval::point(self.rows[axis])
                .add(Interval::point(self.defects[axis]).mul(common))
                .hi
        });
        if gains.iter().any(|value| !value.is_finite()) {
            return Err(GeometryError::new(
                GeometryErrorCode::ConditioningFailure,
                "original implicit inverse coordinate gain is not finite",
            ));
        }
        Ok(gains)
    }
}

fn inverse_metric(
    jet: parametric::SurfaceEnclosure,
) -> Result<Option<InverseMetric>, GeometryError> {
    use parametric::Interval;
    let u = jet.du.map(|value| value.mid());
    let v = jet.dv.map(|value| value.mid());
    let su = u.into_iter().map(f64::abs).fold(0.0, f64::max);
    let sv = v.into_iter().map(f64::abs).fold(0.0, f64::max);
    if su == 0.0 || sv == 0.0 || !su.is_finite() || !sv.is_finite() {
        return Ok(None);
    }
    let u = u.map(|value| value / su);
    let v = v.map(|value| value / sv);
    let dot = |a: [f64; 3], b: [f64; 3]| (0..3).map(|axis| a[axis] * b[axis]).sum::<f64>();
    let (uu, uv, vv) = (dot(u, u), dot(u, v), dot(v, v));
    let determinant = uu * vv - uv * uv;
    if !determinant.is_finite() || determinant <= 0.0 {
        return Ok(None);
    }
    let a: [[f64; 3]; 2] = [
        std::array::from_fn(|axis| (vv * u[axis] - uv * v[axis]) / determinant / su),
        std::array::from_fn(|axis| (uu * v[axis] - uv * u[axis]) / determinant / sv),
    ];
    if a.iter().flatten().any(|value| !value.is_finite()) {
        return Ok(None);
    }
    let mut rho = 0.0_f64;
    let mut rows = [0.0; 2];
    let mut defects = [0.0; 2];
    for (row, coefficients) in a.into_iter().enumerate() {
        let mut row_defect = Interval::point(0.0);
        for (column, derivative) in [jet.du, jet.dv].into_iter().enumerate() {
            let product = (0..3).fold(Interval::point(0.0), |sum, axis| {
                sum.add(Interval::point(coefficients[axis]).mul(derivative[axis]))
            });
            let defect = Interval::point(if row == column { 1.0 } else { 0.0 }).sub(product);
            row_defect = row_defect.add(Interval::point(defect.abs_upper()));
        }
        defects[row] = row_defect.hi;
        rho = rho.max(row_defect.hi);
        rows[row] = coefficients
            .into_iter()
            .fold(Interval::point(0.0), |sum, value| {
                sum.add(Interval::point(value.abs()))
            })
            .hi;
    }
    if !rho.is_finite() || rho >= 1.0 {
        return Ok(None);
    }
    Ok(Some(InverseMetric { rows, defects, rho }))
}

/// Certifies positional UV uncertainty from the SAME original root's actual
/// source-parameter tubes. Physical residual alone never supplies a UV margin.
#[derive(Clone, Copy)]
pub(super) struct UvMarginInput<'a> {
    pub curve: &'a Curve,
    pub prepared: &'a monstertruck_modeling::PreparedValidatedCurve<'static>,
    pub surface: &'a monstertruck_modeling::Surface,
    pub chart: &'a monstertruck_modeling::Curve2D,
    /// Length in the unchanged original carrier's native units, not millimetres.
    pub physical_residual: f64,
    pub coefficient_margin: [f64; 2],
}

/// Fixed completed metric receipt; absent values were not proved/computed.
#[derive(Debug, Default)]
pub(super) struct RefinementDiagnostic {
    pub coefficient_radius: Option<f64>,
    pub residual_gain: Option<f64>,
    pub point_tolerance_native: Option<f64>,
    pub point_diameter_budget_native: Option<f64>,
    pub derived_limit: Option<f64>,
}

pub(super) struct UvMarginEnclosure<'a> {
    source: UvMarginInput<'a>,
    inverse_gain: [f64; 2],
    margin_uv: [f64; 2],
}

/// Completed whole-tube first-jet metric, in the unchanged native frame.
/// Only the source/inverse traversal below can construct this receipt.
pub(super) struct RetainedRefinement {
    base_limit: f64,
    coefficient_radius: f64,
    residual_gain: f64,
}

impl RetainedRefinement {
    pub(super) fn base_limit(&self) -> f64 {
        self.base_limit
    }

    pub(super) fn completed_position(&self, achieved: f64) -> Result<f64, GeometryError> {
        use parametric::Interval;
        if !achieved.is_finite() || achieved < 0.0 || achieved > self.base_limit {
            return Err(unavailable("retained base exceeds its completed tube metric precision"));
        }
        Ok(Interval::point(achieved)
            .add(Interval::point(self.coefficient_radius))
            .add(Interval::point(self.residual_gain).mul(Interval::point(achieved)))
            .hi)
    }
}

impl std::fmt::Debug for UvMarginEnclosure<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UvMarginEnclosure")
            .field("inverse_gain", &self.inverse_gain)
            .field("margin_uv", &self.margin_uv)
            .finish()
    }
}

impl UvMarginEnclosure<'_> {
    pub(super) fn margin_uv(&self) -> [f64; 2] {
        self.margin_uv
    }

    pub(super) fn inverse_gain(&self) -> [f64; 2] {
        self.inverse_gain
    }

    pub(super) fn refined_margin(&self, achieved: f64) -> Result<[f64; 2], GeometryError> {
        use parametric::Interval;
        if !achieved.is_finite() || achieved < 0.0 || achieved > self.source.physical_residual {
            return Err(GeometryError::new(
                GeometryErrorCode::ConditioningFailure,
                "original implicit UV refinement lost its achieved source-error bound",
            ));
        }
        let margin = std::array::from_fn(|axis| {
            Interval::point(self.source.coefficient_margin[axis])
                .add(Interval::point(self.inverse_gain[axis]).mul(Interval::point(achieved)))
                .hi
        });
        if margin.iter().any(|value| !value.is_finite()) {
            return Err(GeometryError::new(
                GeometryErrorCode::ConditioningFailure,
                "original implicit inverse UV margin is not finite",
            ));
        }
        Ok(margin)
    }

    /// Completes the whole expanded-tube first-jet metric and derives one BASE
    /// quality proposal, not a new allowance. Reserve the original producer's
    /// admitted point diameter D in B+(1+G)*e+D. Only a completed same-operand
    /// forward receipt may use this metric for final position; final relative
    /// tangents must still be independently proved on the expanded tube.
    pub(super) fn refinement_limit(
        &self,
        allowed: f64,
        point_tolerance_native: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
        mut diagnostic: Option<&mut RefinementDiagnostic>,
    ) -> Result<Option<RetainedRefinement>, GeometryError> {
        use parametric::Interval;
        if !point_tolerance_native.is_finite() || point_tolerance_native <= 0.0 {
            return Err(GeometryError::new(
                GeometryErrorCode::ConditioningFailure,
                "original implicit point tolerance is not finite and positive",
            ));
        }
        // The returned box and the true point both lie in the same admitted
        // coordinate intervals. Use their full diameter, not a half-width.
        let point_diameter = Interval::point(3.0)
            .sqrt()?
            .mul(Interval::point(point_tolerance_native))
            .hi;
        if !point_diameter.is_finite() {
            return Err(GeometryError::new(
                GeometryErrorCode::ConditioningFailure,
                "original implicit point enclosure diameter is not finite",
            ));
        }
        if let Some(diagnostic) = diagnostic.as_deref_mut() {
            diagnostic.point_tolerance_native = Some(point_tolerance_native);
            diagnostic.point_diameter_budget_native = Some(point_diameter);
        }
        let source = self.source;
        let active = source.prepared.range();
        let range = source.curve.range_tuple();
        let range = [range.0.max(active[0]), range.1.min(active[1])];
        let Some([coefficient_radius, residual_gain]) = chart_physical_radii(
            source.surface, source.chart, range, self.margin_uv,
            [source.coefficient_margin, self.inverse_gain], work, cancel,
        )? else {
            return Ok(None);
        };
        if let Some(diagnostic) = diagnostic.as_deref_mut() {
            diagnostic.coefficient_radius = Some(coefficient_radius);
            diagnostic.residual_gain = Some(residual_gain);
        }
        if !allowed.is_finite() || allowed <= 0.0
            || !coefficient_radius.is_finite() || !residual_gain.is_finite()
        {
            return Err(GeometryError::new(
                GeometryErrorCode::ConditioningFailure,
                "original implicit completion metric is not finite and positive",
            ));
        }
        let remaining = Interval::point(allowed)
            .sub(Interval::point(coefficient_radius))
            .sub(Interval::point(point_diameter));
        if remaining.lo <= 0.0 {
            return Ok(None);
        }
        let limit = remaining
            .div(Interval::point(1.0).add(Interval::point(residual_gain)))?
            .lo;
        let limit = (limit.is_finite() && limit > 0.0).then_some(limit);
        if let Some(diagnostic) = diagnostic {
            diagnostic.derived_limit = limit;
        }
        Ok(limit.map(|base_limit| RetainedRefinement {
            base_limit,
            coefficient_radius,
            residual_gain,
        }))
    }
}

/// A normalized final tube has fixed UV authority already. Recompute its
/// physical metric on the ACTUAL normalized operands, never by scaling an old
/// residual or matching the original carrier to a different spatial frame.
pub(super) fn normalized_tube_refinement(
    input: super::trim_certificate::Correspondence<'_>,
    range: [f64; 2],
    source_scale: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    diagnostic: &mut RefinementDiagnostic,
) -> Result<Option<RetainedRefinement>, GeometryError> {
    use parametric::Interval;
    if !source_scale.is_finite() || source_scale <= 0.0 {
        return Err(unavailable("normalized implicit frame scale is not finite and positive"));
    }
    let Some([coefficient_radius]) = chart_physical_radii(
        input.surface, input.chart, range, input.margin_uv,
        [input.margin_uv], work, cancel,
    )? else {
        return Ok(None);
    };
    diagnostic.coefficient_radius = Some(coefficient_radius);
    diagnostic.residual_gain = Some(0.0);
    let point_diameter = Interval::point(3.0).sqrt()?
        .mul(Interval::point(original_point_tolerance_native(source_scale)))
        .mul(Interval::point(source_scale));
    diagnostic.point_tolerance_native = Some(
        parametric::up(original_point_tolerance_native(source_scale) * source_scale),
    );
    diagnostic.point_diameter_budget_native = Some(point_diameter.hi);
    let remaining = Interval::point(input.maximum_error_mm)
        .sub(Interval::point(coefficient_radius))
        .sub(point_diameter);
    diagnostic.derived_limit = (remaining.lo.is_finite() && remaining.lo > 0.0)
        .then_some(remaining.lo);
    Ok((remaining.lo.is_finite() && remaining.lo > 0.0).then_some(RetainedRefinement {
        base_limit: remaining.lo,
        coefficient_radius,
        residual_gain: 0.0,
    }))
}

fn chart_physical_radii<const N: usize>(
    surface: &monstertruck_modeling::Surface,
    chart: &monstertruck_modeling::Curve2D,
    range: [f64; 2],
    tube_margin: [f64; 2],
    radii: [[f64; 2]; N],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[f64; N]>, GeometryError> {
    use parametric::Interval;
    let uv_range = chart.range_tuple();
    let map = super::trim_certificate::AffineMap::new(range, [uv_range.0, uv_range.1])?;
    let mut stack = [(range, 0u8); 49];
    let mut live = 1;
    let mut maximum = [0.0_f64; N];
    while live != 0 {
        live -= 1;
        let (cell, depth) = stack[live];
        work.charge(1, cancel)?;
        let candidate = chart_image(chart, map.image(cell)?, work, cancel)?;
        let expanded = std::array::from_fn(|axis| {
            candidate[axis].add(Interval {
                lo: -tube_margin[axis],
                hi: tube_margin[axis],
            })
        });
        match parametric::surface_enclosure(
            surface, expanded.map(|value: Interval| [value.lo, value.hi]), work, cancel,
        ) {
            Ok(jet) => {
                for (index, radius) in radii.into_iter().enumerate() {
                    maximum[index] = maximum[index].max(physical_radius(&jet, radius)?);
                }
                continue;
            }
            Err(cause) if matches!(
                cause.code,
                GeometryErrorCode::ConditioningFailure
                    | GeometryErrorCode::SingularGeometry
                    | GeometryErrorCode::AnalysisIndeterminate
            ) => {}
            Err(cause) => return Err(cause),
        }
        let middle = cell[0] * 0.5 + cell[1] * 0.5;
        if depth == 48 || middle <= cell[0] || middle >= cell[1] {
            return Ok(None);
        }
        stack[live] = ([middle, cell[1]], depth + 1);
        stack[live + 1] = ([cell[0], middle], depth + 1);
        live += 2;
    }
    Ok(Some(maximum))
}

fn physical_radius(
    jet: &parametric::SurfaceEnclosure,
    radius: [f64; 2],
) -> Result<f64, GeometryError> {
    use parametric::Interval;
    let components: [Interval; 3] = std::array::from_fn(|axis| {
        Interval::point(jet.du[axis].abs_upper())
            .mul(Interval::point(radius[0]))
            .add(Interval::point(jet.dv[axis].abs_upper()).mul(Interval::point(radius[1])))
    });
    let squared = components.into_iter().fold(Interval::point(0.0), |sum, value| {
        sum.add(value.square())
    });
    Ok(Interval { lo: squared.lo.max(0.0), hi: squared.hi }.sqrt()?.hi)
}

fn chart_image(
    chart: &monstertruck_modeling::Curve2D,
    range: parametric::Interval,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[parametric::Interval; 2], GeometryError> {
    use parametric::Interval;
    let mut candidate: Option<[Interval; 2]> = None;
    for span in parametric::curve2d_parameter_spans(chart, [range.lo, range.hi], work, cancel)? {
        let mut native = monstertruck_modeling::ValidatedGeometryBudget::new(
            usize::try_from(work.remaining()).unwrap_or(usize::MAX),
        );
        let jet = monstertruck_modeling::validated_curve2d_jet(chart, span, &mut native, cancel);
        parametric::debit_native_work(work, native.consumed(), cancel)?;
        let position = jet.map_err(parametric::native_error)?.position
            .map(|value| Interval { lo: value.lower, hi: value.upper });
        candidate = Some(match candidate {
            None => position,
            Some(previous) => std::array::from_fn(|axis| previous[axis].hull(position[axis])),
        });
    }
    candidate.ok_or_else(|| unavailable("original implicit UV expression has no bounded image span"))
}

pub(super) fn certify_uv_margin<'a>(
    input: UvMarginInput<'a>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<UvMarginEnclosure<'a>, GeometryError> {
    if !input.physical_residual.is_finite() || input.physical_residual < 0.0
        || input.coefficient_margin.iter().any(|value| !value.is_finite() || *value < 0.0)
    {
        return Err(GeometryError::new(
            GeometryErrorCode::ConditioningFailure,
            "original implicit inverse uncertainty is not finite and nonnegative",
        ));
    }
    let UvMarginInput {
        curve,
        prepared,
        surface,
        chart,
        physical_residual,
        coefficient_margin,
    } = input;
    use parametric::Interval;
    let source = root_chart(curve, prepared, surface, work, cancel)?;
    let active = prepared.range();
    let periods = [surface.period_u(), surface.period_v()];
    let range = curve.range_tuple();
    let range = [range.0.max(active[0]), range.1.min(active[1])];
    let uv_range = chart.range_tuple();
    let map = super::trim_certificate::AffineMap::new(range, [uv_range.0, uv_range.1])?;
    let mut stack = [(range, 0u8); 49];
    let mut live = 1;
    let mut maximum = [0.0_f64; 2];
    while live != 0 {
        live -= 1;
        let (cell, depth) = stack[live];
        work.charge(1, cancel)?;
        let candidate = chart_image(chart, map.image(cell)?, work, cancel)?;
        let actual = source.tube(cell, candidate, periods, work, cancel)?;
        let box_uv = std::array::from_fn(|axis| {
            candidate[axis]
                .add(Interval {
                    lo: -coefficient_margin[axis],
                    hi: coefficient_margin[axis],
                })
                .hull(actual[axis])
        });
        let result = parametric::surface_enclosure(
            surface,
            box_uv.map(|value: Interval| [value.lo, value.hi]),
            work,
            cancel,
        )
        .and_then(inverse_metric);
        let metric = match result {
            Ok(value) => value,
            Err(cause)
                if matches!(
                    cause.code,
                    GeometryErrorCode::ConditioningFailure
                        | GeometryErrorCode::SingularGeometry
                        | GeometryErrorCode::AnalysisIndeterminate
                ) =>
            {
                None
            }
            Err(cause) => return Err(cause),
        };
        if let Some(metric) = metric {
            let gains = metric.coordinate_gains()?;
            for axis in 0..2 {
                maximum[axis] = maximum[axis].max(gains[axis]);
            }
            continue;
        }
        let middle = cell[0] * 0.5 + cell[1] * 0.5;
        if depth == 48 || middle <= cell[0] || middle >= cell[1] {
            return Err(unavailable(&format!(
                "original implicit source UV inverse chart, pole or periodic lift remains unresolved: source parameter={cell:?}, candidate={candidate:?}, retained original root={actual:?}, coefficient margin={coefficient_margin:?}, physical residual={physical_residual} native length units",
            )));
        }
        stack[live] = ([middle, cell[1]], depth + 1);
        stack[live + 1] = ([cell[0], middle], depth + 1);
        live += 2;
    }
    let mut enclosure = UvMarginEnclosure {
        source: input,
        inverse_gain: maximum,
        margin_uv: [0.0; 2],
    };
    enclosure.margin_uv = enclosure.refined_margin(physical_residual)?;
    Ok(enclosure)
}

#[cfg(test)]
mod tests {
    use super::*;
    use monstertruck_modeling::{
        Curve2D, Line, Plane, Point2, Point3, Surface, SurfaceCurve, ValidatedCurveOptions,
        ValidatedGeometryBudget,
    };

    #[test]
    fn uv_inverse_bound_does_not_treat_physical_residual_as_uv_error() {
        let surface = Surface::Plane(Plane::new(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0e-6, 0.0, 0.0),
            Point3::new(0.0, 2.0e-6, 0.0),
        ));
        let jet = parametric::surface_enclosure(
            &surface,
            [[-1.0, 1.0]; 2],
            &mut GeometryWorkBudget::new(1_000_000),
            &AtomicBool::new(false),
        )
        .unwrap();
        let margin = inverse_margin(jet, 1.0e-7).unwrap().unwrap();
        assert!((0.1..0.100001).contains(&margin));
        let gains = inverse_metric(jet).unwrap().unwrap().coordinate_gains().unwrap();
        assert!((1.0e6..1.000001e6).contains(&gains[0]));
        assert!((0.5e6..0.500001e6).contains(&gains[1]));
        let mut singular = jet;
        singular.du[0] = parametric::Interval::new(-1.0e-6, 1.0e-6).unwrap();
        assert!(inverse_margin(singular, 1.0e-7).unwrap().is_none());
    }

    #[test]
    fn original_root_uv_tube_not_guide_and_not_an_unrelated_carrier_owns_margin() {
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
            Box::new(xy.clone()),
            Box::new(yz),
            Box::new(guide),
            None,
            None,
        ));
        let cancel = AtomicBool::new(false);
        let prepared = monstertruck_modeling::prepared_validated_curve_owned(
            curve.clone(),
            [0.0, 1.0],
            &ValidatedCurveOptions::default(),
            &mut ValidatedGeometryBudget::new(1_000_000),
            &cancel,
        )
        .unwrap();
        let chart = Curve2D::Line(Line(Point2::new(0.0, 0.0), Point2::new(0.0, 1.0)));
        let mut work = GeometryWorkBudget::new(1_000_000);
        let margin = certify_uv_margin(
            UvMarginInput {
                curve: &curve,
                prepared: &prepared,
                surface: &xy,
                chart: &chart,
                physical_residual: 1.0e-9,
                coefficient_margin: [0.0; 2],
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(
            margin
                .margin_uv()
                .into_iter()
                .all(|bound| (1.0e-9..1.1e-9).contains(&bound))
        );
        // On this original orthonormal plane the whole UV rectangle has
        // exactly this Euclidean physical radius. The initial tube alone
        // exceeds T; base precision, refined tube and admitted point diameter fit.
        let allowed = 1.0e-9;
        let initial = margin.margin_uv();
        assert!(initial[0].hypot(initial[1]) > allowed);
        let point_tolerance = original_point_tolerance_native(1.0);
        let metric = margin.refinement_limit(
            allowed, point_tolerance, &mut work, &cancel, None,
        ).unwrap().unwrap();
        let goal = metric.base_limit();
        let refined = margin.refined_margin(goal).unwrap();
        assert!(
            goal + refined[0].hypot(refined[1]) + 3.0_f64.sqrt() * point_tolerance <= allowed
        );
        assert!(metric.completed_position(goal).unwrap() <= allowed);
        let other = Surface::Plane(Plane::new(
            Point3::new(0.0, 0.0, 1.0),
            Point3::new(1.0, 0.0, 1.0),
            Point3::new(0.0, 1.0, 1.0),
        ));
        let cause = certify_uv_margin(
            UvMarginInput {
                curve: &curve,
                prepared: &prepared,
                surface: &other,
                chart: &chart,
                physical_residual: 1.0e-9,
                coefficient_margin: [0.0; 2],
            },
            &mut work,
            &cancel,
        )
        .unwrap_err();
        assert_eq!(
            cause.code,
            GeometryErrorCode::ParameterCorrespondenceUnavailable
        );
    }
}
