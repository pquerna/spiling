// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Original source-use trim retention and bounded native correspondence.
//!
//! Declared PCURVEs remain retained even when no proof authorizes a consumer.
//! Derived charts require coefficient correspondence or whole physical position
//! and relative tangent proof against the retained original continuation.
//! A guide, samples and approximate inverse recovery never supply authority.
//! Periodic branch proposals are bound to individual original wire neighbors.
//! Synthetic restrictions of an unbounded UV LINE do not inherit a closed
//! spatial curve's phase; declared STEP TRIMMED_CURVE domains remain fixed.
//! Bare spatial LINE coefficients and an identical native identity restriction
//! transport the original finite edge span into analytic chart proposals. Only
//! a whole-source forward proof can activate the separately retained UV LINE.
use super::{chart_proof, charts, error, invalid};
use crate::{GeometryWorkBudget, check_cancel, kernel::SourceChartCertificate};
use monstertruck_io::step::load::step_geometry::{
    Curve2D as StepCurve2D, Curve3D, Surface as StepSurface, SurfaceParameterMap,
};
use monstertruck_modeling::{
    Conic2D, Curve, Curve2D, ExtrusionChartCurve, Line, NativeSurfaceBoundary, Plane, Processor,
    Surface, TrimDomainError, TrimmedCurve, UnitCircle, ValidatedGeometryBudget,
    ValidatedGeometryError, base::*,
};
use spiling_contracts::geometry::{GeometryError, GeometryErrorCode};
use std::sync::atomic::AtomicBool;

/// The unchanged decoded source carrier and its native conversion operator.
#[derive(Clone, Copy)]
pub(super) struct SurfaceInput<'a> {
    pub source_surface: &'a StepSurface,
    pub native_surface: &'a Surface,
    pub map: SurfaceParameterMap,
    pub scale: f64,
}

/// One original spatial association and its native restriction, not a guide.
#[derive(Clone, Copy)]
pub(super) struct TrimInput<'a> {
    pub source_curve: &'a Curve3D,
    pub native_curve: &'a Curve,
    pub surface: SurfaceInput<'a>,
    pub declared: Option<&'a StepCurve2D>,
}

/// Native spatial curve and carrier in their shared source-unit frame.
#[derive(Clone, Copy)]
struct NativeTrimGeometry<'a> {
    curve: &'a Curve,
    surface: &'a Surface,
    scale: f64,
    domain: Option<[[f64; 2]; 2]>,
}

impl<'a> TrimInput<'a> {
    fn native_geometry(self) -> NativeTrimGeometry<'a> {
        NativeTrimGeometry {
            curve: self.native_curve,
            surface: self.surface.native_surface,
            scale: self.surface.scale,
            domain: intrinsic_domain(
                self.surface.source_surface,
                self.surface.native_surface,
                self.surface.map,
            ),
        }
    }
}

use super::trim_certificate::OriginalCoedgeCorrespondence;

pub(super) struct SourceTrim {
    pub curve: Option<Curve2D>,
    pub margin_uv: [f64; 2],
    pub error_mm: f64,
    pub certificate: SourceChartCertificate,
    pending_meridian: Option<chart_proof::MeridianGraph>,
    /// Original source PCURVE (or literal source parameter curve), not a fitted
    /// inverse. Consumers still require native UV incidence/coverage.
    pub declared_source: bool,
    pub correspondence: Option<OriginalCoedgeCorrespondence>,
}
impl SourceTrim {
    pub(super) fn proves_forward(&self, tolerance: f64, margin_uv: [f64; 2]) -> bool {
        self.correspondence.as_ref().is_some_and(|proof| {
            proof.provenance() == super::trim_certificate::OriginalCorrespondenceProvenance::Forward
                && proof.position_error_mm() <= tolerance
                && (0..2).all(|axis| margin_uv[axis] <= proof.margin_uv()[axis])
        })
    }

    fn exact(
        source: &Curve,
        curve: Curve2D,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        let correspondence =
            super::trim_certificate::OriginalCoedgeCorrespondence::exact_coefficients(
                source, &curve, work, cancel,
            )?;
        Ok(Self {
            curve: Some(curve),
            margin_uv: [0.0; 2],
            error_mm: 0.0,
            certificate: SourceChartCertificate::InverseOrExact,
            pending_meridian: None,
            declared_source: false,
            correspondence: Some(correspondence),
        })
    }
    fn retained(curve: Option<Curve2D>) -> Self {
        Self {
            curve,
            margin_uv: [0.0; 2],
            error_mm: 0.0,
            certificate: SourceChartCertificate::Unproved,
            pending_meridian: None,
            declared_source: false,
            correspondence: None,
        }
    }
}

/// An independently bounded source spline chart, never a lowered sweep's
/// arbitrary default parameter box. Its endpoints are original source knots.
pub(super) fn intrinsic_domain(
    source: &StepSurface,
    surface: &Surface,
    map: SurfaceParameterMap,
) -> Option<[[f64; 2]; 2]> {
    if !matches!(map, SurfaceParameterMap::Identity)
        || !matches!(
            source,
            StepSurface::BsplineSurface(_) | StepSurface::NurbsSurface(_)
        )
    {
        return None;
    }
    let (u, v, ud, vd) = match surface {
        Surface::BsplineSurface(s) => (
            s.knot_vector_u().as_ref(),
            s.knot_vector_v().as_ref(),
            s.udegree(),
            s.vdegree(),
        ),
        Surface::NurbsSurface(s) => (
            s.knot_vector_u().as_ref(),
            s.knot_vector_v().as_ref(),
            s.udegree(),
            s.vdegree(),
        ),
        _ => return None,
    };
    let u: &[f64] = u;
    let v: &[f64] = v;
    let result = [
        [*u.get(ud)?, *u.get(u.len().checked_sub(ud + 1)?)?],
        [*v.get(vd)?, *v.get(v.len().checked_sub(vd + 1)?)?],
    ];
    result
        .iter()
        .all(|x| x[0].is_finite() && x[1].is_finite() && x[0] < x[1])
        .then_some(result)
}

/// One global nonzero homogeneous scale preserves the WHOLE rational function,
/// including its denominator zeros, every C0 knot and all one-sided tangents.
/// Independent per-control scales do not preserve a rational spline.
fn same_projective_controls(
    a: &[Vector3],
    b: &[Vector3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(
        (a.len() as u64).saturating_mul(24).saturating_add(1),
        cancel,
    )?;
    if a.len() != b.len() {
        return Ok(false);
    }
    let Some((first, second)) = a.iter().zip(b).find(|(a, b)| a.z != 0. && b.z != 0.) else {
        return Ok(false);
    };
    let (a_weight, b_weight) = (first.z, second.z);
    Ok(a.iter().zip(b).all(|(a, b)| {
        (0..3).all(|axis| chart_proof::exact_products_equal(a[axis], b_weight, b[axis], a_weight))
    }))
}

fn same_coefficients(
    a: &Curve2D,
    b: &Curve2D,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    same_coefficients_at(a, b, 0, work, cancel)
}
fn same_coefficients_at(
    a: &Curve2D,
    b: &Curve2D,
    depth: u8,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    use monstertruck_modeling::Conic2D;
    let coefficients = match a {
        Curve2D::Polyline(c) => c.len().saturating_mul(2),
        Curve2D::BsplineCurve(c) => c
            .knot_vector()
            .len()
            .saturating_add(c.control_points().len().saturating_mul(3)),
        Curve2D::NurbsCurve(c) => c
            .knot_vector()
            .len()
            .saturating_add(c.control_points().len().saturating_mul(3)),
        _ => 0,
    };
    work.charge(coefficients.saturating_add(16) as u64, cancel)?;
    Ok(match (a, b) {
        (Curve2D::Line(a), Curve2D::Line(b)) => a.0 == b.0 && a.1 == b.1,
        (Curve2D::Polyline(a), Curve2D::Polyline(b)) => a.as_slice() == b.as_slice(),
        (Curve2D::BsplineCurve(a), Curve2D::BsplineCurve(b)) => {
            a.knot_vector() == b.knot_vector() && a.control_points() == b.control_points()
        }
        (Curve2D::NurbsCurve(a), Curve2D::NurbsCurve(b)) => {
            a.knot_vector() == b.knot_vector()
                && same_projective_controls(a.control_points(), b.control_points(), work, cancel)?
        }
        (Curve2D::TrimmedCurve(a), Curve2D::TrimmedCurve(b)) => {
            if depth == 32 {
                return Err(error(
                    GeometryErrorCode::ResourceLimit,
                    "native declared UV coefficient nesting exceeds source limit",
                ));
            }
            a.transform() == b.transform()
                && a.orientation() == b.orientation()
                && a.entity().range_tuple() == b.entity().range_tuple()
                && same_coefficients_at(
                    a.entity().curve().as_ref(),
                    b.entity().curve().as_ref(),
                    depth + 1,
                    work,
                    cancel,
                )?
        }
        (Curve2D::Conic(a), Curve2D::Conic(b)) => match (a, b) {
            (Conic2D::Ellipse(a), Conic2D::Ellipse(b)) => {
                a.transform() == b.transform()
                    && a.orientation() == b.orientation()
                    && a.range_tuple() == b.range_tuple()
            }
            (Conic2D::Hyperbola(a), Conic2D::Hyperbola(b)) => {
                a.transform() == b.transform()
                    && a.orientation() == b.orientation()
                    && a.range_tuple() == b.range_tuple()
            }
            (Conic2D::Parabola(a), Conic2D::Parabola(b)) => {
                a.transform() == b.transform()
                    && a.orientation() == b.orientation()
                    && a.range_tuple() == b.range_tuple()
            }
            _ => false,
        },
        _ => false,
    })
}
fn recoverable(cause: TrimDomainError) -> Result<Option<Curve2D>, GeometryError> {
    match cause {
        TrimDomainError::Geometry(
            ValidatedGeometryError::Unsupported
            | ValidatedGeometryError::Unresolved
            | ValidatedGeometryError::ConditioningFailure,
        )
        | TrimDomainError::InvalidChart
        | TrimDomainError::UnsupportedSeam => Ok(None),
        TrimDomainError::Geometry(cause) => Err(crate::parametric::native_error(cause)),
        TrimDomainError::ResourceLimit => Err(error(
            GeometryErrorCode::ResourceLimit,
            "native source trim coefficient proof exceeded its bound",
        )),
        _ => Ok(None),
    }
}
fn native_attempt<T, F>(
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    f: F,
) -> Result<Option<T>, GeometryError>
where
    F: FnOnce(&mut ValidatedGeometryBudget) -> Result<T, TrimDomainError>,
{
    check_cancel(cancel)?;
    let mut budget =
        ValidatedGeometryBudget::new(usize::try_from(work.remaining()).unwrap_or(usize::MAX));
    let result = f(&mut budget);
    crate::parametric::debit_native_work(work, budget.consumed(), cancel)?;
    match result {
        Ok(curve) => Ok(Some(curve)),
        Err(cause) => recoverable(cause).map(|_| None),
    }
}
fn acceptable(
    candidate: Curve2D,
    declared: Option<&Curve2D>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Curve2D>, GeometryError> {
    Ok(match declared {
        Some(original) if !same_coefficients(&candidate, original, work, cancel)? => None,
        _ => Some(candidate),
    })
}

/// Structural affinity plus exact homogeneous endpoint identities proves the
/// whole declaration under the normalized affine map of its OWN finite domain.
/// Rounded endpoint proposals alone never select a tensor seam branch.
fn exact_affine_endpoints(
    chart: &Curve2D,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[Point2; 2]>, GeometryError> {
    let Some(points) = chart_proof::affine_endpoint_proposals(chart, work, cancel)? else {
        return Ok(None);
    };
    let range = chart.range_tuple();
    if exact_endpoint_coordinates(chart, [range.0, range.1], points, work, cancel)? {
        Ok(Some(points))
    } else {
        Ok(None)
    }
}

fn exact_endpoint_coordinates(
    chart: &Curve2D,
    parameters: [f64; 2],
    points: [Point2; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    Ok(native_attempt(work, cancel, |budget| {
        for (parameter, point) in parameters.into_iter().zip(points) {
            for axis in 0..2 {
                if !monstertruck_modeling::certified_curve2d_endpoint_coordinate(
                    chart,
                    parameter,
                    axis,
                    point[axis],
                    budget,
                    cancel,
                )? {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    })?
    .unwrap_or(false))
}
pub(super) fn native_chart(
    curve: &Curve,
    surface: &Surface,
    declared: Option<&Curve2D>,
    domain: Option<[[f64; 2]; 2]>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Curve2D>, GeometryError> {
    if let Surface::Plane(plane) = surface
        && let Some(candidate) = native_attempt(work, cancel, |budget| {
            monstertruck_modeling::certified_planar_parameter_curve(curve, plane, budget, cancel)
        })?
        && let Some(candidate) = acceptable(candidate, declared, work, cancel)?
    {
        return Ok(Some(candidate));
    }
    if let Some(candidate) = native_attempt(work, cancel, |budget| {
        monstertruck_modeling::certified_affine_surface_parameter_curve(
            curve, surface, budget, cancel,
        )
    })? && let Some(candidate) = acceptable(candidate, declared, work, cancel)?
    {
        return Ok(Some(candidate));
    }
    // The original source use selects a seam branch. Never choose the first
    // geometrically coincident boundary of a periodic carrier.
    let boundary_endpoints = match (declared, domain) {
        (Some(original), Some(domain)) => {
            exact_affine_endpoints(original, work, cancel)?.map(|points| (points, domain))
        }
        _ => None,
    };
    if let Some((points, domain)) = boundary_endpoints {
        let [a, b] = points.map(|point| [point.x, point.y]);
        let choices = [
            (0, domain[0][0], NativeSurfaceBoundary::UFront),
            (0, domain[0][1], NativeSurfaceBoundary::UBack),
            (1, domain[1][0], NativeSurfaceBoundary::VFront),
            (1, domain[1][1], NativeSurfaceBoundary::VBack),
        ];
        for (axis, value, boundary) in choices {
            work.charge(1, cancel)?;
            if a[axis] != value || b[axis] != value {
                continue;
            }
            let variable = [a[1 - axis].min(b[1 - axis]), a[1 - axis].max(b[1 - axis])];
            if variable[0] >= variable[1] {
                continue;
            }
            if let Some(candidate) = native_attempt(work, cancel, |budget| {
                monstertruck_modeling::certified_surface_boundary_parameter_curve(
                    curve, surface, boundary, variable, budget, cancel,
                )
            })? {
                // Native proves every homogeneous row coefficient and knot
                // under its exact affine map, retaining each spatial wrapper's
                // actual inverse anchor. The declaration has its independent
                // own range: equality of structurally affine endpoint
                // coefficients proves the complete composition, not just two
                // sampled positions. A reversed row/map is accepted only when
                // its actual oriented endpoints agree with this original use.
                let range = candidate.range_tuple();
                if exact_affine_endpoints(&candidate, work, cancel)?.is_some()
                    && exact_endpoint_coordinates(
                        &candidate,
                        [range.0, range.1],
                        points,
                        work,
                        cancel,
                    )?
                {
                    return Ok(Some(candidate));
                }
            }
        }
    }
    if matches!(surface, Surface::ExtrusionSurface(_))
        && let Some(Curve2D::Line(line)) = declared
    {
        let choice = if line.0.y == line.1.y {
            Some(ExtrusionChartCurve::ProfileAt { offset: line.0.y })
        } else if line.0.x == line.1.x {
            Some(ExtrusionChartCurve::GeneratorAt {
                profile_parameter: line.0.x,
            })
        } else {
            None
        };
        if let Some(choice) = choice
            && let Some(candidate) = native_attempt(work, cancel, |budget| {
                monstertruck_modeling::certified_extrusion_parameter_curve(
                    curve, surface, choice, budget, cancel,
                )
            })?
            && let Some(candidate) = acceptable(candidate, declared, work, cancel)?
        {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

/// A nominal raw swept-profile offset is not an exact chart coefficient. Keep
/// its original outward enclosure separate from decoded/native parameters.
fn map_margin(map: SurfaceParameterMap) -> Result<[f64; 2], GeometryError> {
    use monstertruck_io::step::load::step_geometry::SourceParameterGauge;
    let (profile, axis) = match map {
        SurfaceParameterMap::Extrusion(profile)
        | SurfaceParameterMap::Revolution {
            profile,
            inverted: false,
        } => (profile, 0),
        SurfaceParameterMap::Revolution {
            profile,
            inverted: true,
        } => (profile, 1),
        _ => return Ok([0.0; 2]),
    };
    let mut margin = [0.0; 2];
    match profile.raw_step_gauge {
        SourceParameterGauge::Internal => {}
        SourceParameterGauge::Declared {
            raw_to_internal, ..
        } => {
            let enclosure = crate::parametric::Interval::new(
                raw_to_internal.offset_enclosure.lower,
                raw_to_internal.offset_enclosure.upper,
            )?;
            margin[axis] = enclosure
                .sub(crate::parametric::Interval::point(raw_to_internal.offset))
                .abs_upper();
        }
        SourceParameterGauge::Unavailable => {
            return Err(error(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "original swept-profile raw STEP chart is unavailable",
            ));
        }
    }
    Ok(margin)
}

/// The structural seed builder evaluates decoded source geometry. Only original
/// declared PCURVEs use raw STEP swept-profile UV coordinates. Reapplying the raw
/// gauge to a decoded seed would reverse/translate its profile a second time.
fn decoded_seed_map(map: SurfaceParameterMap) -> SurfaceParameterMap {
    use monstertruck_io::step::load::step_geometry::SourceParameterGauge;
    match map {
        SurfaceParameterMap::Extrusion(mut profile) => {
            profile.raw_step_gauge = SourceParameterGauge::Internal;
            SurfaceParameterMap::Extrusion(profile)
        }
        SurfaceParameterMap::Revolution {
            mut profile,
            inverted,
        } => {
            profile.raw_step_gauge = SourceParameterGauge::Internal;
            SurfaceParameterMap::Revolution { profile, inverted }
        }
        other => other,
    }
}

fn literal_chart(
    pcurve: &monstertruck_io::step::load::step_geometry::StepParameterCurve,
    source_surface: &StepSurface,
    map: SurfaceParameterMap,
) -> Result<Option<Curve2D>, GeometryError> {
    if pcurve.surface() != source_surface
        || !matches!(map, SurfaceParameterMap::Identity)
        || pcurve.input_parameter_chart()
            != monstertruck_io::step::SurfaceParameterChart::identity()
    {
        return Ok(None);
    }
    Curve2D::try_from(pcurve.curve())
        .map(Some)
        .map_err(|cause| {
            error(
                GeometryErrorCode::UnsupportedConversion,
                &format!("literal source UV curve conversion refused: {cause}"),
            )
        })
}

/// Preserve the original curve and the affine operator as separate expressions;
/// rounded transformed endpoints/control points are not coefficient identities.
fn mapped_declaration(
    curve: Curve2D,
    range: (f64, f64),
    transform: Matrix3,
    forward: bool,
) -> Curve2D {
    let mut retained = monstertruck_modeling::Processor::with_transform(
        monstertruck_modeling::TrimmedCurve::new(Box::new(curve), range),
        transform,
    );
    if !forward {
        retained.invert();
    }
    Curve2D::TrimmedCurve(retained)
}

pub(super) fn line_branch_proposal(
    raw: Line<Point2>,
    transform: Matrix3,
    candidate: &Curve2D,
) -> Option<Curve2D> {
    let (a, b) = candidate.range_tuple();
    if !a.is_finite() || !b.is_finite() || a >= b {
        return None;
    }
    let origin = transform.transform_point(raw.0);
    let direction = <Matrix3 as Transform<Point2>>::transform_vector(&transform, raw.1 - raw.0);
    let axis = usize::from(direction.y.abs() > direction.x.abs());
    if !direction[axis].is_finite() || direction[axis] == 0. {
        return None;
    }
    let start = (candidate.subs(a)[axis] - origin[axis]) / direction[axis];
    let end = (candidate.subs(b)[axis] - origin[axis]) / direction[axis];
    if !start.is_finite() || !end.is_finite() || start == end {
        return None;
    }
    Some(mapped_declaration(
        Curve2D::Line(raw),
        (start.min(end), start.max(end)),
        transform,
        start < end,
    ))
}

fn prove_line_branch(
    raw: Line<Point2>,
    transform: Matrix3,
    candidate: &Curve2D,
    input: NativeTrimGeometry<'_>,
    margin_uv: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<SourceTrim>, GeometryError> {
    let NativeTrimGeometry {
        curve,
        surface,
        scale,
        domain,
    } = input;
    work.charge(8, cancel)?;
    let Some(chart) = line_branch_proposal(raw, transform, candidate) else {
        return Ok(None);
    };
    // A natural tensor row need not be globally C1 in its own knot gauge.
    // Reuse the whole original row/knot theorem, binding the original LINE's
    // complete affine declaration by exact endpoint coefficients. This is not
    // a forward-error receipt and cannot erase a nonzero source/chart margin.
    if domain.is_some()
        && margin_uv == [0.0; 2]
        && native_chart(curve, surface, Some(&chart), domain, work, cancel)?.is_some()
    {
        let mut witness = SourceTrim::exact(curve, chart, work, cancel)?;
        witness.declared_source = true;
        return Ok(Some(witness));
    }
    let Some(bound) = super::trim_certificate::certify_declared_coedge_correspondence(
        super::trim_certificate::Correspondence {
            curve,
            surface,
            chart: &chart,
            maximum_error_mm: 1e-7 / scale,
            margin_uv,
        },
        work,
        cancel,
        None,
    )?
    else {
        return Ok(None);
    };
    Ok(Some(SourceTrim {
        curve: Some(chart),
        margin_uv,
        error_mm: crate::parametric::up(bound.position_error_mm() * scale),
        certificate: SourceChartCertificate::InverseOrExact,
        pending_meridian: None,
        declared_source: true,
        correspondence: Some(bound),
    }))
}

/// Borrow the finite restriction of the SAME original spatial LINE. A bare
/// source LINE's nominal endpoints describe its coefficients, not the active
/// EDGE_CURVE span. Only an identity native restriction with identical basis
/// coefficients transports that span here; a converted/fitted line cannot.
///
/// This supplies search data only. The original UV expression, its chart map
/// and the complete original spatial restriction still undergo the independent
/// forward position and relative-tangent proof in `prove_line_branch`.
fn active_spatial_line_chart(
    mut source: &Curve3D,
    native: &Curve,
    surface: &StepSurface,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Curve2D>, GeometryError> {
    for depth in 0..=32 {
        work.charge(1, cancel)?;
        match source {
            Curve3D::SurfaceCurve(curve) if depth < 32 => source = curve.leader(),
            Curve3D::SurfaceCurve(_) => {
                return Err(error(
                    GeometryErrorCode::ResourceLimit,
                    "active source LINE association nesting exceeds source limit",
                ));
            }
            _ => break,
        }
    }
    let (Curve3D::Line(original), Curve::TrimmedCurve(restriction)) = (source, native) else {
        return Ok(None);
    };
    if *restriction.transform() != Matrix4::identity() || !restriction.orientation() {
        return Ok(None);
    }
    if !matches!(restriction.entity().curve().as_ref(), Curve::Line(_)) {
        return Ok(None);
    }
    let Some(basis) = chart_proof::native_affine_line_basis(native, work, cancel)? else {
        return Ok(None);
    };
    if basis != original {
        return Ok(None);
    }
    let (a, b) = restriction.entity().range_tuple();
    if !a.is_finite() || !b.is_finite() || a >= b {
        return Ok(None);
    }
    work.charge(8, cancel)?;
    // Evaluation of a LINE here is its complete affine coefficient expression,
    // not endpoint recovery from a display curve or a nearest-point search.
    let start = original.subs(a);
    let end = original.subs(b);
    if [start.x, start.y, start.z, end.x, end.y, end.z]
        .into_iter()
        .any(|value| !value.is_finite())
    {
        return Ok(None);
    }
    charts::source_chart(&Curve3D::Line(Line(start, end)), surface)
}

/// A bare original STEP LINE is unbounded. Its default native 0..1 range is
/// never the coedge's trim authority. Other source curve variants are bounded
/// declarations and may not enter this branch-selection path.
fn active_line_branch(
    raw: Line<Point2>,
    input: TrimInput<'_>,
    domain: Option<[[f64; 2]; 2]>,
    margin_uv: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<SourceTrim>, GeometryError> {
    let TrimInput {
        source_curve,
        native_curve: curve,
        surface:
            SurfaceInput {
                source_surface,
                native_surface: surface,
                map,
                ..
            },
        ..
    } = input;
    let transform = map.affine_transform().ok_or_else(|| {
        error(
            GeometryErrorCode::ParameterCorrespondenceUnavailable,
            "original source LINE needs a nonlinear chart composition",
        )
    })?;
    if let Some(candidate) = native_chart(curve, surface, None, domain, work, cancel)?
        && let Some(proof) = prove_line_branch(
            raw,
            transform,
            &candidate,
            input.native_geometry(),
            margin_uv,
            work,
            cancel,
        )?
    {
        return Ok(Some(proof));
    }
    if let Some(domain) = domain {
        let a = transform.transform_point(raw.0);
        let b = transform.transform_point(raw.1);
        for (axis, value, boundary) in [
            (0, domain[0][0], NativeSurfaceBoundary::UFront),
            (0, domain[0][1], NativeSurfaceBoundary::UBack),
            (1, domain[1][0], NativeSurfaceBoundary::VFront),
            (1, domain[1][1], NativeSurfaceBoundary::VBack),
        ] {
            work.charge(1, cancel)?;
            if a[axis] != value || b[axis] != value {
                continue;
            }
            if let Some(candidate) = native_attempt(work, cancel, |budget| {
                monstertruck_modeling::certified_surface_isocurve_parameter_curve(
                    curve, surface, boundary, budget, cancel,
                )
            })? && let Some(proof) = prove_line_branch(
                raw,
                transform,
                &candidate,
                input.native_geometry(),
                margin_uv,
                work,
                cancel,
            )? {
                return Ok(Some(proof));
            }
        }
    }
    if let Some(mut candidate) =
        active_spatial_line_chart(source_curve, curve, source_surface, work, cancel)?
        && let Some(decoded) = decoded_seed_map(map).affine_transform()
    {
        candidate.transform_by(decoded);
        if let Some(proof) = prove_line_branch(
            raw,
            transform,
            &candidate,
            input.native_geometry(),
            margin_uv,
            work,
            cancel,
        )? {
            return Ok(Some(proof));
        }
    }
    if let Some(mut candidate) = charts::source_chart(source_curve, source_surface)?
        && let Some(decoded) = decoded_seed_map(map).affine_transform()
    {
        // Candidate only: actual original-LINE coefficients plus the FULL
        // raw chart operator above must pass the independent forward proof.
        candidate.transform_by(decoded);
        if let Some(proof) = prove_line_branch(
            raw,
            transform,
            &candidate,
            input.native_geometry(),
            margin_uv,
            work,
            cancel,
        )? {
            return Ok(Some(proof));
        }
    }
    Ok(None)
}

pub(super) fn implicit_guide_chart(
    source: &Curve3D,
    source_surface: &StepSurface,
    surface: &Surface,
    map: SurfaceParameterMap,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Curve2D>, GeometryError> {
    let mut budget =
        ValidatedGeometryBudget::new(usize::try_from(work.remaining()).unwrap_or(usize::MAX));
    let result = super::source_conic::capture_guide_candidate(source, &mut budget, cancel);
    crate::parametric::debit_native_work(work, budget.consumed(), cancel)?;
    let Some(candidate) = result? else {
        return Ok(None);
    };
    let center = candidate.c.map(|value| value.mid());
    let u = candidate.u.map(|value| value.mid());
    let v = candidate.v.map(|value| value.mid());
    let witness = if candidate.circle {
        let a = candidate.phase_offset.mid() + candidate.orientation * candidate.range.0;
        let b = candidate.phase_offset.mid() + candidate.orientation * candidate.range.1;
        super::BoundaryCurveMm::CircleArc {
            center_mm: center,
            u_mm: u,
            v_mm: v,
            start_angle: a,
            end_angle: b,
        }
    } else {
        super::BoundaryCurveMm::Line {
            start_mm: center,
            end_mm: std::array::from_fn(|axis| center[axis] + u[axis]),
        }
    };
    if matches!(surface, Surface::Plane(_)) {
        // Only the compact candidate primitive is constructed. Neither original
        // carriers nor original guide coefficient buffers are copied.
        let point = |p: [f64; 3]| Point3::new(p[0], p[1], p[2]);
        let guide = match &witness {
            super::BoundaryCurveMm::Line { start_mm, end_mm } => {
                Curve::Line(Line(point(*start_mm), point(*end_mm)))
            }
            super::BoundaryCurveMm::CircleArc {
                start_angle,
                end_angle,
                ..
            } => {
                let mut circle = Processor::<_, Matrix3>::new(TrimmedCurve::new(
                    UnitCircle::<Point2>::new(),
                    (start_angle.min(*end_angle), start_angle.max(*end_angle)),
                ));
                if start_angle > end_angle {
                    circle.invert();
                }
                let carrier = Surface::Plane(Plane::new(
                    point(center),
                    point(std::array::from_fn(|axis| center[axis] + u[axis])),
                    point(std::array::from_fn(|axis| center[axis] + v[axis])),
                ));
                Curve::ParameterCurve(monstertruck_modeling::ParameterCurve::new(
                    Curve2D::Conic(Conic2D::Ellipse(circle)),
                    Box::new(carrier),
                ))
            }
            _ => return Ok(None),
        };
        return native_chart(&guide, surface, None, None, work, cancel);
    }
    let Some(chart) = charts::guide_chart(&witness, source_surface)? else {
        return Ok(None);
    };
    let map = decoded_seed_map(map);
    if let Some(transform) = map.affine_transform() {
        if matches!(map, SurfaceParameterMap::Identity) {
            Ok(Some(chart))
        } else {
            let range = chart.range_tuple();
            Ok(Some(mapped_declaration(chart, range, transform, true)))
        }
    } else {
        Ok(None)
    }
}

pub(super) fn retain_with_continuation(
    input: TrimInput<'_>,
    retained: Option<&monstertruck_modeling::PreparedValidatedCurve<'static>>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SourceTrim, GeometryError> {
    let TrimInput {
        source_curve,
        native_curve,
        surface:
            SurfaceInput {
                source_surface,
                native_surface,
                map,
                scale,
            },
        declared,
    } = input;
    let Some(prepared) = retained else {
        return retain(input, [0.0; 2], work, cancel);
    };
    work.charge(1, cancel)?;
    let margin_uv = map_margin(map)?;
    let chart = if let Some(declared) = declared {
        let chart = Curve2D::try_from(declared).map_err(|cause| {
            error(
                GeometryErrorCode::UnsupportedConversion,
                &format!("original implicit UV conversion refused: {cause}"),
            )
        })?;
        if matches!(map, SurfaceParameterMap::Identity) {
            chart
        } else if let Some(transform) = map.affine_transform() {
            let range = chart.range_tuple();
            mapped_declaration(chart, range, transform, true)
        } else {
            return Err(error(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "original implicit UV expression needs an unavailable nonlinear chart",
            ));
        }
    } else {
        implicit_guide_chart(
            source_curve,
            source_surface,
            native_surface,
            map,
            work,
            cancel,
        )?
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "original implicit edge has no whole-coedge UV proposal",
            )
        })?
    };
    let bound = super::trim_certificate::certify_retained_coedge_correspondence(
        super::trim_certificate::Correspondence {
            curve: native_curve,
            surface: native_surface,
            chart: &chart,
            maximum_error_mm: 1.0e-7 / scale,
            margin_uv,
        },
        (prepared, 1.0),
        work,
        cancel,
    )?
    .ok_or_else(|| {
        error(
            GeometryErrorCode::ParameterCorrespondenceUnavailable,
            "original implicit branch does not establish whole coedge UV correspondence",
        )
    })?;
    let margin_uv = super::implicit::certify_uv_margin(
        super::implicit::UvMarginInput {
            curve: native_curve,
            prepared,
            surface: native_surface,
            chart: &chart,
            physical_residual: bound.position_error_mm(),
            coefficient_margin: margin_uv,
        },
        work,
        cancel,
    )?;
    Ok(SourceTrim {
        curve: Some(chart),
        margin_uv,
        error_mm: crate::parametric::up(bound.position_error_mm() * scale),
        // The converter's stored chart is a candidate, not independent proof
        // that an implicit source declared a literal PCURVE expression.
        certificate: SourceChartCertificate::InverseOrExact,
        pending_meridian: None,
        declared_source: false,
        correspondence: Some(bound),
    })
}

/// Complete original physical-position and relative-tangent correspondence.
/// This derived forward receipt does not replace original wire incidence,
/// native parameter-domain containment, inverse or face-coverage evidence.
fn derived_chart_witness(
    curve: &Curve,
    surface: &Surface,
    chart: Curve2D,
    scale: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<SourceTrim>, GeometryError> {
    let result = super::trim_certificate::certify_declared_coedge_correspondence(
        super::trim_certificate::Correspondence {
            curve,
            surface,
            chart: &chart,
            maximum_error_mm: 1.0e-7 / scale,
            margin_uv: [0.0; 2],
        },
        work,
        cancel,
        None,
    )?;
    let Some(bound) = result else {
        return Ok(None);
    };
    Ok(Some(SourceTrim {
        curve: Some(chart),
        margin_uv: [0.0; 2],
        error_mm: crate::parametric::up(bound.position_error_mm() * scale),
        certificate: SourceChartCertificate::InverseOrExact,
        pending_meridian: None,
        declared_source: false,
        correspondence: Some(bound),
    }))
}

/// A latitude graph plus world tube is not a regular longitude inverse.
/// Keep it unqualified until both original source-vertex joints have received
/// their complete Native image certificates in the wire binder.
fn pole_meridian_witness(
    input: TrimInput<'_>,
    candidate: &Curve2D,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<SourceTrim>, GeometryError> {
    if input.declared.is_some()
        || !matches!(
            input.surface.source_surface,
            StepSurface::ElementarySurface(
                monstertruck_io::step::load::step_geometry::ElementarySurface::Sphere(_)
            )
        )
    {
        return Ok(None);
    }
    let Some(proof) = chart_proof::certify_pole_meridian(
        input.source_curve,
        input.surface.source_surface,
        candidate.clone(),
        decoded_seed_map(input.surface.map),
        input.surface.scale,
        work,
        cancel,
    )?
    else {
        return Ok(None);
    };
    let Some(bound) = super::trim_certificate::certify_declared_coedge_correspondence(
        super::trim_certificate::Correspondence {
            curve: input.native_curve,
            surface: input.surface.native_surface,
            chart: &proof.chart.curve,
            maximum_error_mm: 1e-7 / input.surface.scale,
            margin_uv: proof.chart.margin_uv,
        },
        work,
        cancel,
        None,
    )?
    else {
        return Ok(None);
    };
    Ok(Some(SourceTrim {
        curve: Some(proof.chart.curve),
        margin_uv: proof.chart.margin_uv,
        error_mm: proof.chart.error_mm.max(crate::parametric::up(
            bound.position_error_mm() * input.surface.scale,
        )),
        certificate: SourceChartCertificate::Unproved,
        pending_meridian: Some(proof.graph),
        declared_source: false,
        correspondence: Some(bound),
    }))
}

/// Check symbolic identity, not a positional tolerance or a conversion label.
/// Every original affine LINE operator is retained verbatim. An ellipse may
/// have only the public exact angular lowering: its actual affine columns
/// become explicit plane axes around the same oriented UnitCircle expression.
/// This excludes rationalized/fitted/chord replacements and rounded composed
/// poses. Thus coefficient-capture enclosure radii describe evaluation of the
/// SAME source/native expression, not an omitted source-to-native error.
fn same_original_affine_expression(
    mut source: &Curve3D,
    mut native: &Curve,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    use monstertruck_io::step::load::step_geometry::{Conic3D, SurfaceCurveKind};
    for depth in 0..=32 {
        work.charge(32, cancel)?;
        match (source, native) {
            (Curve3D::SurfaceCurve(curve), _)
                if depth < 32 && curve.kind() != SurfaceCurveKind::IntersectionCurve =>
            {
                source = curve.leader();
            }
            (Curve3D::TrimmedCurve(original), Curve::TrimmedCurve(retained))
                if depth < 32
                    && original.transform() == retained.transform()
                    && original.orientation() == retained.orientation()
                    && original.entity().range_tuple() == retained.entity().range_tuple() =>
            {
                source = original.entity().curve().as_ref();
                native = retained.entity().curve().as_ref();
            }
            (Curve3D::Line(original), Curve::Line(retained)) => {
                return Ok(original == retained);
            }
            (Curve3D::Conic(Conic3D::Ellipse(original)), Curve::ParameterCurve(retained)) => {
                let (Curve2D::Conic(Conic2D::Ellipse(profile)), Surface::Plane(plane)) =
                    (retained.curve(), retained.surface().as_ref())
                else {
                    return Ok(false);
                };
                let matrix = original.transform();
                return Ok(matrix.x.w == 0.0
                    && matrix.y.w == 0.0
                    && matrix.z.w == 0.0
                    && matrix.w.w == 1.0
                    && *profile.transform() == Matrix3::identity()
                    && profile.orientation() == original.orientation()
                    && profile.entity().range_tuple() == original.entity().range_tuple()
                    && plane.origin() == Point3::from_vec(matrix.w.truncate())
                    && plane.stored_axes() == Some([matrix.x.truncate(), matrix.y.truncate()]));
            }
            _ => return Ok(false),
        }
    }
    Err(error(
        GeometryErrorCode::ResourceLimit,
        "original affine source/native identity nesting exceeds source limit",
    ))
}

/// Propose the affine projection of the complete original LINE or ellipse onto
/// its own original plane or coefficient-proved affine tensor, only after
/// symbolic source/native expression AND carrier identity. Rounded frame/inverse
/// arithmetic selects coordinates only. Whole native position and relative
/// tangents prove the proposal; the ORIGINAL carrier jet bounds its true inverse.
fn source_associated_planar_chart(
    source_curve: &Curve3D,
    source_surface: &StepSurface,
    curve: &Curve,
    surface: &Surface,
    scale: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<SourceTrim>, GeometryError> {
    use monstertruck_io::step::load::step_geometry::ElementarySurface;
    let (carrier_origin, u, v, jet_domain, support, tensor_axes) = match (source_surface, surface) {
        (
            StepSurface::ElementarySurface(ElementarySurface::Plane(original)),
            Surface::Plane(plane),
        ) if original == plane => (
            plane.origin(),
            plane.axis_u(),
            plane.axis_v(),
            [[0.0, 0.0]; 2],
            None,
            None,
        ),
        _ => {
            let Some(frame) = chart_proof::affine_tensor_frame(surface, work, cancel)? else {
                return Ok(None);
            };
            let same_carrier = match (source_surface, surface) {
                (StepSurface::BsplineSurface(original), Surface::BsplineSurface(native)) => {
                    original == native
                }
                (StepSurface::NurbsSurface(original), Surface::NurbsSurface(native)) => {
                    original == native
                }
                _ => false,
            };
            if !same_carrier {
                return Ok(None);
            }
            let point = |value: [crate::parametric::Interval; 3]| {
                Vector3::new(value[0].mid(), value[1].mid(), value[2].mid())
            };
            (
                Point3::from_vec(point(frame.origin)),
                point(frame.axes[0]),
                point(frame.axes[1]),
                frame.domain,
                Some(frame.domain),
                Some(frame.axes),
            )
        }
    };
    if !same_original_affine_expression(source_curve, curve, work, cancel)? {
        return Ok(None);
    }
    let mut budget =
        ValidatedGeometryBudget::new(usize::try_from(work.remaining()).unwrap_or(usize::MAX));
    let captured = super::source_conic::capture(source_curve, &mut budget, cancel);
    crate::parametric::debit_native_work(work, budget.consumed(), cancel)?;
    let Some(source) = captured? else {
        return Ok(None);
    };
    work.charge(64, cancel)?;
    // This rounded inverse is a proposal, not the true inverse used below.
    let Some(inverse) = Matrix3::from_cols(u, v, u.cross(v)).invert() else {
        return Ok(None);
    };
    let midpoint = |value: [crate::parametric::Interval; 3]| {
        Vector3::new(value[0].mid(), value[1].mid(), value[2].mid())
    };
    let origin = inverse * (midpoint(source.c) - carrier_origin.to_vec());
    let direction = inverse * midpoint(source.u);
    let chart = if source.circle {
        let transverse = inverse * midpoint(source.v);
        // Processor inversion reflects at the actual captured outer endpoints,
        // not at a nominal conic convenience window.
        let phase = source.phase_offset.mid()
            - if source.orientation < 0.0 {
                source.range.0 + source.range.1
            } else {
                0.0
            };
        let transform = Matrix3::from_cols(
            Vector3::new(direction.x, direction.y, 0.0),
            Vector3::new(transverse.x, transverse.y, 0.0),
            Vector3::new(origin.x, origin.y, 1.0),
        ) * Matrix3::from_angle_z(Rad(phase));
        if let Some(domain) = support {
            let bounds = chart_proof::conic_support(&transform)?;
            if (0..2)
                .any(|axis| bounds[axis].lo < domain[axis][0] || bounds[axis].hi > domain[axis][1])
            {
                return Ok(None);
            }
        }
        let mut ellipse = Processor::with_transform(
            TrimmedCurve::new(UnitCircle::<Point2>::new(), source.range),
            transform,
        );
        if source.orientation < 0.0 {
            ellipse.invert();
        }
        Curve2D::Conic(Conic2D::Ellipse(ellipse))
    } else {
        let end = origin + direction;
        Curve2D::Line(Line(
            Point2::new(origin.x, origin.y),
            Point2::new(end.x, end.y),
        ))
    };
    let Some(mut witness) = derived_chart_witness(curve, surface, chart, scale, work, cancel)?
    else {
        return Ok(None);
    };
    // Let J be the ORIGINAL coefficient-defined affine derivative, A only a
    // rounded preconditioner and P=(AJ)^-1 A the true left inverse. The whole
    // carrier jet proves |I-AJ|<1 and bounds P; no rounded A*J=I is assumed.
    // Thus P(C-O)-U=P(C-S(U)) for the true P, on the complete original range.
    // The physical source mismatch stays in error_mm; finite support is not
    // extrapolated and the native original-domain/trim binder remains required.
    let mut jet = crate::parametric::surface_enclosure(surface, jet_domain, work, cancel)?;
    if let Some(axes) = tensor_axes {
        // These are outward ORIGINAL homogeneous coefficient derivatives, valid
        // on the entire proved affine patch, not derivatives of the proposed
        // rounded frame. Keep the native whole-domain denominator/support query.
        jet.du = axes[0];
        jet.dv = axes[1];
    }
    let residual = witness
        .correspondence
        .as_ref()
        .ok_or_else(|| invalid("derived affine chart lost its completed forward receipt"))?
        .position_error_mm();
    let Some(margin) = super::implicit::inverse_margin(jet, residual)? else {
        return Ok(None);
    };
    if let Some(domain) = support {
        let Some(Curve2D::Conic(Conic2D::Ellipse(chart))) = witness.curve.as_ref() else {
            return Ok(None);
        };
        let bounds = chart_proof::conic_support(chart.transform())?;
        for axis in 0..2 {
            let tube = bounds[axis].add(crate::parametric::Interval {
                lo: -margin,
                hi: margin,
            });
            if tube.lo < domain[axis][0] || tube.hi > domain[axis][1] {
                return Ok(None);
            }
        }
    }
    witness.margin_uv = [margin; 2];
    // Keep the forward receipt's original zero uncertainty. It cannot be
    // borrowed as a proof about every curve in this separately bounded tube.
    Ok(Some(witness))
}

fn extrusion_profile_domain(
    map: SurfaceParameterMap,
) -> Option<super::harmonic::ProfileParameterDomain> {
    use super::harmonic::ProfileParameterDomain;
    use monstertruck_io::step::load::step_geometry::CurveParameterDomain;
    let SurfaceParameterMap::Extrusion(profile) = map else {
        return None;
    };
    Some(match profile.source_domain {
        CurveParameterDomain::Bounded => {
            ProfileParameterDomain::Bounded([profile.native_range.0, profile.native_range.1])
        }
        CurveParameterDomain::Unbounded => ProfileParameterDomain::Unbounded,
    })
}

fn generator_chart_proposal(
    curve: &Curve,
    surface: &Surface,
    parameter: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Curve2D>, GeometryError> {
    if chart_proof::native_affine_line_basis(curve, work, cancel)?.is_none() {
        return Ok(None);
    }
    let Surface::ExtrusionSurface(extrusion) = surface else {
        return Ok(None);
    };
    let (start, end) = curve.range_tuple();
    if ![start, end, parameter].into_iter().all(f64::is_finite) || start >= end {
        return Ok(None);
    }
    let direction = extrusion.extruding_vector();
    let axis = (0..3)
        .max_by(|&a, &b| direction[a].abs().total_cmp(&direction[b].abs()))
        .ok_or_else(|| invalid("source extrusion has no coordinate axis"))?;
    if !direction[axis].is_finite() || direction[axis] == 0.0 {
        return Ok(None);
    }
    let Some(profile) = native_attempt(work, cancel, |budget| {
        monstertruck_modeling::validated_curve_jet(
            extrusion.entity_curve(),
            [parameter, parameter],
            budget,
            cancel,
        )
        .map(|jet| jet.position[axis])
        .map_err(TrimDomainError::Geometry)
    })?
    else {
        return Ok(None);
    };
    let mut values = [0.0; 2];
    for (index, t) in [start, end].into_iter().enumerate() {
        let Some(point) = native_attempt(work, cancel, |budget| {
            monstertruck_modeling::validated_curve_jet(curve, [t, t], budget, cancel)
                .map(|jet| jet.position[axis])
                .map_err(TrimDomainError::Geometry)
        })?
        else {
            return Ok(None);
        };
        values[index] = (0.5 * point.lower + 0.5 * point.upper
            - (0.5 * profile.lower + 0.5 * profile.upper))
            / direction[axis];
    }
    if !values.into_iter().all(f64::is_finite) || values[0] == values[1] {
        return Ok(None);
    }
    work.charge(
        (4 * std::mem::size_of::<f64>() + 2 * std::mem::size_of::<Point2>()) as u64,
        cancel,
    )?;
    Ok(Some(Curve2D::BsplineCurve(
        monstertruck_modeling::BsplineCurve::new(
            monstertruck_modeling::KnotVector::from(vec![start, start, end, end]),
            vec![
                Point2::new(parameter, values[0]),
                Point2::new(parameter, values[1]),
            ],
        ),
    )))
}

/// Exact native structural proposals remain the first sufficient lane.
/// Budgeted endpoint enclosures only select finite candidate coordinates;
/// complete original forward correspondence and wire/domain binding still own
/// authority. A bare LINE range never supplies the profile's source domain.
fn source_associated_extrusion_chart(
    source_surface: &StepSurface,
    curve: &Curve,
    surface: &Surface,
    scale: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<SourceTrim>, GeometryError> {
    use monstertruck_io::step::load::step_geometry::SweepSurface;
    let (
        StepSurface::SweepSurface(SweepSurface::ExtrusionSurface(original)),
        Surface::ExtrusionSurface(native),
    ) = (source_surface, surface)
    else {
        return Ok(None);
    };
    work.charge(1, cancel)?;
    let mut original_profile = original.entity_curve().as_ref();
    for depth in 0..=32 {
        match original_profile {
            Curve3D::SurfaceCurve(wrapper) if depth < 32 => {
                work.charge(1, cancel)?;
                original_profile = wrapper.leader();
            }
            Curve3D::SurfaceCurve(_) => {
                return Err(error(
                    GeometryErrorCode::ResourceLimit,
                    "source extrusion profile association nesting exceeds source limit",
                ));
            }
            _ => break,
        }
    }
    let bounded_profile = matches!(
        original_profile,
        Curve3D::BsplineCurve(_)
            | Curve3D::NurbsCurve(_)
            | Curve3D::Conic(_)
            | Curve3D::TrimmedCurve(_)
    );
    if !bounded_profile {
        return Ok(None);
    }
    let profile = native.entity_curve();
    let (a, b) = profile.range_tuple();
    let (start, end) = curve.range_tuple();
    if ![a, b, start, end].into_iter().all(f64::is_finite) || a >= b || start >= end {
        return Ok(None);
    }
    let direction = native.extruding_vector();
    let axis = (0..3)
        .max_by(|&a, &b| direction[a].abs().total_cmp(&direction[b].abs()))
        .ok_or_else(|| invalid("source extrusion has no coordinate axis"))?;
    if !direction[axis].is_finite() || direction[axis] == 0.0 {
        return Ok(None);
    }
    let Some(edge_start) = native_attempt(work, cancel, |budget| {
        let jet = monstertruck_modeling::validated_curve_jet(curve, [start, start], budget, cancel)
            .map_err(TrimDomainError::Geometry)?;
        let coordinate = jet.position[axis];
        Ok(0.5 * coordinate.lower + 0.5 * coordinate.upper)
    })?
    else {
        return Ok(None);
    };
    for parameter in [a, b] {
        work.charge(1, cancel)?;
        let Some(profile_point) = native_attempt(work, cancel, |budget| {
            let jet = monstertruck_modeling::validated_curve_jet(
                profile,
                [parameter, parameter],
                budget,
                cancel,
            )
            .map_err(TrimDomainError::Geometry)?;
            let coordinate = jet.position[axis];
            Ok(0.5 * coordinate.lower + 0.5 * coordinate.upper)
        })?
        else {
            continue;
        };
        // Midpoints of budgeted point enclosures propose only a finite offset.
        // They confer no correspondence or completeness authority.
        let offset = (edge_start - profile_point) / direction[axis];
        // Both candidates derive from the admitted original physical edge and
        // its original profile's actual bounded conversion, not a family or
        // guessed phase. Reversed profiles are handled by the native identity.
        for branch in [
            ExtrusionChartCurve::ProfileAt { offset },
            ExtrusionChartCurve::GeneratorAt {
                profile_parameter: parameter,
            },
        ] {
            if matches!(branch, ExtrusionChartCurve::ProfileAt { .. }) && !offset.is_finite() {
                continue;
            }
            let exact = native_attempt(work, cancel, |budget| {
                monstertruck_modeling::certified_extrusion_parameter_curve(
                    curve, surface, branch, budget, cancel,
                )
            })?;
            let chart = match exact {
                Some(chart) => chart,
                None if matches!(branch, ExtrusionChartCurve::GeneratorAt { .. }) => {
                    let Some(chart) =
                        generator_chart_proposal(curve, surface, parameter, work, cancel)?
                    else {
                        continue;
                    };
                    chart
                }
                None => continue,
            };
            let bound = super::trim_certificate::certify_declared_coedge_correspondence(
                super::trim_certificate::Correspondence {
                    curve,
                    surface,
                    chart: &chart,
                    maximum_error_mm: 1.0e-7 / scale,
                    margin_uv: [0.0; 2],
                },
                work,
                cancel,
                None,
            )?;
            let Some(bound) = bound else {
                continue;
            };
            // The original spatial master/active range is unchanged. This is
            // derived native UV, not an invented original PCURVE declaration;
            // the existing original-wire binder still owns periodic aliases.
            return Ok(Some(SourceTrim {
                curve: Some(chart),
                margin_uv: [0.0; 2],
                error_mm: crate::parametric::up(bound.position_error_mm() * scale),
                certificate: SourceChartCertificate::InverseOrExact,
                pending_meridian: None,
                declared_source: false,
                correspondence: Some(bound),
            }));
        }
    }
    Ok(None)
}

pub(super) fn retain(
    input: TrimInput<'_>,
    additional_margin_uv: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SourceTrim, GeometryError> {
    let TrimInput {
        source_curve,
        native_curve,
        surface:
            SurfaceInput {
                source_surface,
                native_surface,
                map,
                scale,
            },
        declared,
    } = input;
    work.charge(1, cancel)?;
    let domain = intrinsic_domain(source_surface, native_surface, map);
    let conversion_margin = map_margin(map)?;
    if additional_margin_uv
        .into_iter()
        .any(|value| !value.is_finite() || value < 0.0)
    {
        return Err(invalid(
            "selected source chart uncertainty must be finite and nonnegative",
        ));
    }
    let margin_uv = std::array::from_fn(|axis| {
        crate::parametric::Interval::point(conversion_margin[axis])
            .add(crate::parametric::Interval::point(
                additional_margin_uv[axis],
            ))
            .hi
    });
    if let Some(StepCurve2D::Line(raw)) = declared
        && let Some(proof) = active_line_branch(*raw, input, domain, margin_uv, work, cancel)?
    {
        return Ok(proof);
    }
    let mut declared = declared
        .map(|curve| {
            let curve = Curve2D::try_from(curve).map_err(|cause| {
                error(
                    GeometryErrorCode::UnsupportedConversion,
                    &format!("original source UV curve conversion refused: {cause}"),
                )
            })?;
            if let Some(transform) = map.affine_transform() {
                if matches!(map, SurfaceParameterMap::Identity) {
                    Ok(curve)
                } else {
                    let range = curve.range_tuple();
                    Ok(mapped_declaration(curve, range, transform, true))
                }
            } else {
                Err(error(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "original source PCURVE needs a nonlinear chart composition",
                ))
            }
        })
        .transpose()?;
    if let Some(curve) = native_chart(
        native_curve,
        native_surface,
        declared.as_ref(),
        domain,
        work,
        cancel,
    )? {
        let original = declared.take();
        let declared_source = original.is_some() && matches!(map, SurfaceParameterMap::Identity);
        let mut witness = SourceTrim::exact(native_curve, original.unwrap_or(curve), work, cancel)?;
        witness.declared_source = declared_source;
        // Exact native coefficient projection supplies its own native chart;
        // it does not turn the separately retained raw STEP gauge into equality.
        return Ok(witness);
    }
    if declared.is_none() {
        if let Some(witness) = source_associated_planar_chart(
            source_curve,
            source_surface,
            native_curve,
            native_surface,
            scale,
            work,
            cancel,
        )? {
            return Ok(witness);
        }
        if let Some(witness) = source_associated_extrusion_chart(
            source_surface,
            native_curve,
            native_surface,
            scale,
            work,
            cancel,
        )? {
            return Ok(witness);
        }
        if let Some(original_domain) = extrusion_profile_domain(map)
            && let Some(chart) = super::harmonic::extrusion_profile_chart_proposal(
                native_curve,
                native_surface,
                original_domain,
                work,
                cancel,
            )?
            && let Some(witness) =
                derived_chart_witness(native_curve, native_surface, chart, scale, work, cancel)?
        {
            return Ok(witness);
        }
    }
    if let Some(chart) = declared.as_ref() {
        // The ORIGINAL source association authorizes this UV expression. The
        // active affine q/range remains a proposal until complete FORWARD
        // position AND relative tangent correspondence pass. Native preparation
        // must subsequently prove neighbor incidence, winding and trim coverage.
        if let Some(bound) = super::trim_certificate::certify_declared_coedge_correspondence(
            super::trim_certificate::Correspondence {
                curve: native_curve,
                surface: native_surface,
                chart,
                maximum_error_mm: 1.0e-7 / scale,
                margin_uv,
            },
            work,
            cancel,
            None,
        )? {
            return Ok(SourceTrim {
                curve: declared,
                margin_uv,
                error_mm: crate::parametric::up(bound.position_error_mm() * scale),
                certificate: SourceChartCertificate::InverseOrExact,
                pending_meridian: None,
                declared_source: true,
                correspondence: Some(bound),
            });
        }
    }
    // A literal original PCURVE is a composed source curve, unlike a sampled
    // association. Both conversion stages must preserve the complete expression:
    // the stored UV chart is independent of the decoded-source/native map.
    // Nominally mapping a nonidentity stored chart supplies no coefficient
    // enclosure; only the separate whole-correspondence routes can authorize it.
    let mut leader = source_curve;
    while let Curve3D::SurfaceCurve(curve) = leader {
        leader = curve.leader();
    }
    if let Curve3D::ParameterCurve(pcurve) = leader
        && let Some(curve) = literal_chart(pcurve, source_surface, map)?
        && declared
            .as_ref()
            .map(|original| same_coefficients(&curve, original, work, cancel))
            .transpose()?
            .unwrap_or(true)
    {
        let mut witness = SourceTrim::exact(native_curve, curve, work, cancel)?;
        witness.declared_source = true;
        return Ok(witness);
    }
    // Derived analytic charts are considered only without an original PCURVE.
    // Reapplying a source/native map to an already mapped declared chart would
    // silently change both phase and source identity.
    let candidate = if declared.is_none() {
        charts::source_chart(source_curve, source_surface)?
    } else {
        None
    };
    if let Some(candidate) = candidate.as_ref()
        && let Some(proof) = pole_meridian_witness(input, candidate, work, cancel)?
    {
        return Ok(proof);
    }
    if let Some(candidate) = candidate
        && let Some(proof) = chart_proof::certify(
            source_curve,
            source_surface,
            candidate,
            decoded_seed_map(map),
            scale,
            work,
            cancel,
        )?
    {
        if !proof.error_mm.is_finite()
            || proof.error_mm < 0.0
            || proof
                .margin_uv
                .into_iter()
                .any(|x| !x.is_finite() || x < 0.0)
        {
            return Err(invalid("source chart proof returned invalid uncertainty"));
        }
        // The coefficient-derived inverse tube is not a source/native C1
        // correspondence. Independently prove the actual retained native edge
        // against this whole mapped UV expression, including its full tube.
        // A declined proof retains inverse authority only, never a receipt.
        let margin_uv = std::array::from_fn(|axis| {
            crate::parametric::Interval::point(proof.margin_uv[axis])
                .add(crate::parametric::Interval::point(
                    additional_margin_uv[axis],
                ))
                .hi
        });
        let correspondence = super::trim_certificate::certify_declared_coedge_correspondence(
            super::trim_certificate::Correspondence {
                curve: native_curve,
                surface: native_surface,
                chart: &proof.curve,
                maximum_error_mm: 1.0e-7 / scale,
                margin_uv,
            },
            work,
            cancel,
            None,
        )?;
        let error_mm = correspondence.as_ref().map_or(proof.error_mm, |bound| {
            proof
                .error_mm
                .max(crate::parametric::up(bound.position_error_mm() * scale))
        });
        return Ok(SourceTrim {
            curve: Some(proof.curve),
            margin_uv,
            error_mm,
            certificate: SourceChartCertificate::InverseOrExact,
            pending_meridian: None,
            declared_source: false,
            correspondence,
        });
    }
    // A bounded positional correspondence is useful diagnostic evidence, not
    // a topology/UV-inverse certificate. Retain the original use but refuse
    // operations that require an admitted native trim domain.
    let mut result = SourceTrim::retained(declared);
    if let Some(chart) = &result.curve
        && let Some(bound) = super::trim_certificate::certify_curve_on_surface(
            native_curve,
            native_surface,
            chart,
            1.0e-7 / scale,
            work,
            cancel,
        )?
    {
        result.error_mm = crate::parametric::up(bound * scale);
    }
    Ok(result)
}

/// One original oriented occurrence, including an exact native child of a
/// closed EDGE_CURVE. The source/native curves and their active ranges are
/// borrowed unchanged; only the occurrence's independent UV expression moves.
pub(super) struct PeriodicUse<'a> {
    pub face: usize,
    pub wire: usize,
    pub coedge: usize,
    pub source_edge: Option<u64>,
    pub native_edge: usize,
    pub source: &'a Curve3D,
    pub native: &'a Curve,
    pub declared: Option<&'a StepCurve2D>,
    pub retained: Option<&'a monstertruck_modeling::PreparedValidatedCurve<'static>>,
    pub orientation: bool,
    pub vertices: (usize, usize),
    /// Original shared vertex at this occurrence's oriented front, in source units.
    pub front_vertex: Point3,
}

struct PeriodicWrappers<'a, 'b>(&'a PeriodicUse<'b>);

impl std::fmt::Display for PeriodicWrappers<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Walk only the already-bounded wrapper prefix, without copying
        // geometry. Retain fixed-size outer/first-reflection summaries.
        let mut source = self.0.source;
        let mut outer = None;
        let mut reflection = None;
        let mut source_depth = 0;
        for _ in 0..32 {
            let (range, forward, basis) = match source {
                Curve3D::SurfaceCurve(curve) => {
                    source = curve.leader();
                    continue;
                }
                Curve3D::TrimmedCurve(trim) => (
                    trim.entity().range_tuple(),
                    trim.orientation(),
                    Some(trim.entity().curve().as_ref()),
                ),
                Curve3D::Conic(monstertruck_io::step::load::step_geometry::Conic3D::Ellipse(
                    circle,
                )) => (circle.range_tuple(), circle.orientation(), None),
                _ => break,
            };
            source_depth += 1;
            outer.get_or_insert((range, forward));
            if !forward {
                reflection.get_or_insert((source_depth, range));
            }
            let Some(basis) = basis else { break };
            source = basis;
        }
        write!(
            f,
            "original outer={outer:?} first_reflection={reflection:?} wrappers={source_depth}; "
        )?;
        let mut native = self.0.native;
        let mut outer = None;
        let mut reflection = None;
        let mut native_depth = 0;
        for _ in 0..32 {
            let Curve::TrimmedCurve(trim) = native else {
                break;
            };
            native_depth += 1;
            let range = trim.entity().range_tuple();
            outer.get_or_insert((range, trim.orientation()));
            if !trim.orientation() {
                reflection.get_or_insert((native_depth, range));
            }
            native = trim.entity().curve().as_ref();
        }
        let uv_outer = if let Curve::ParameterCurve(curve) = native {
            match curve.curve() {
                Curve2D::TrimmedCurve(trim) => {
                    Some((trim.entity().range_tuple(), trim.orientation()))
                }
                Curve2D::Conic(Conic2D::Ellipse(circle)) => {
                    Some((circle.range_tuple(), circle.orientation()))
                }
                _ => None,
            }
        } else {
            None
        };
        write!(
            f,
            "native outer={outer:?} first_reflection={reflection:?} wrappers={native_depth} UV_outer={uv_outer:?}"
        )
    }
}

fn periodic_error(
    cause: GeometryError,
    stage: &str,
    context: &PeriodicUse<'_>,
    shift: [f64; 2],
    started: u64,
    work: &GeometryWorkBudget,
) -> GeometryError {
    error(
        cause.code,
        &format!(
            "{stage} coedge={} src={:?} edge={} shift={shift:?} work={}/{} delta={}: {} | wrappers:{}",
            context.coedge,
            context.source_edge,
            context.native_edge,
            work.consumed(),
            work.remaining(),
            work.consumed() - started,
            cause.message,
            PeriodicWrappers(context),
        ),
    )
}

/// A converter-created restriction of an unbounded LINE is not a literal STEP
/// TRIMMED_CURVE. In particular, a circle's decoded 3D phase is not this LINE's
/// parameter map. Never cross a declared wrapper, pose, or unavailable gauge.
fn original_unbounded_line(
    mut curve: &StepCurve2D,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Line<Point2>>, GeometryError> {
    use monstertruck_io::step::load::step_geometry::SourceParameterGauge;
    for depth in 0..=32 {
        work.charge(1, cancel)?;
        match curve {
            StepCurve2D::Line(line) => return Ok(Some(*line)),
            StepCurve2D::TrimmedCurve(wrapper)
                if depth < 32
                    && *wrapper.transform() == Matrix3::identity()
                    && matches!(
                        wrapper.raw_parameter_gauge(),
                        Ok(SourceParameterGauge::Internal)
                    ) =>
            {
                curve = wrapper.entity().curve().as_ref();
            }
            StepCurve2D::TrimmedCurve(_) if depth == 32 => {
                return Err(error(
                    GeometryErrorCode::ResourceLimit,
                    "original periodic UV LINE nesting exceeds source limit",
                ));
            }
            _ => return Ok(None),
        }
    }
    Ok(None)
}

/// Propose a covering-coordinate operator around the already admitted source
/// expression. Literal bounded declarations cannot move, and no original
/// coefficient, raw chart map, active interval or reflection anchor changes.
fn periodic_lift_proposal(
    declared: Option<&StepCurve2D>,
    chart: &Curve2D,
    translation: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Curve2D>, GeometryError> {
    work.charge(1, cancel)?;
    if let Some(declared) = declared
        && original_unbounded_line(declared, work, cancel)?.is_none()
    {
        return Ok(None);
    }
    Ok(Some(mapped_declaration(
        chart.clone(),
        chart.range_tuple(),
        Matrix3::from_translation(Vector2::from(translation)),
        true,
    )))
}

fn uv_endpoint(
    witness: &SourceTrim,
    orientation: bool,
    back: bool,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[crate::parametric::Interval; 2]>, GeometryError> {
    let Some(curve) = witness.curve.as_ref() else {
        return Ok(None);
    };
    let (a, b) = curve.range_tuple();
    let t = if orientation == back { b } else { a };
    let mut budget =
        ValidatedGeometryBudget::new(usize::try_from(work.remaining()).unwrap_or(usize::MAX));
    let result = monstertruck_modeling::validated_curve2d_jet(curve, [t, t], &mut budget, cancel);
    crate::parametric::debit_native_work(work, budget.consumed(), cancel)?;
    let jet = result.map_err(crate::parametric::native_error)?;
    Ok(Some(std::array::from_fn(|axis| {
        let point = crate::parametric::Interval {
            lo: jet.position[axis].lower,
            hi: jet.position[axis].upper,
        };
        let margin = witness.margin_uv[axis];
        point.add(crate::parametric::Interval {
            lo: -margin,
            hi: margin,
        })
    })))
}

fn incident(a: [crate::parametric::Interval; 2], b: [crate::parametric::Interval; 2]) -> bool {
    (0..2).all(|axis| a[axis].lo <= b[axis].hi && b[axis].lo <= a[axis].hi)
}

/// Literal UV incidence and a certified native parameter-joint completion are
/// distinct valid joints. Source vertex identity is checked by the wire walk before this
/// gate. The native certificate encloses the WHOLE connector image relative to
/// that original vertex; endpoint proximity cannot authorize a covering-chart
/// bridge. Final native face-domain preparation still retains/proves the joint.
fn parameter_joint_incident(
    incoming: [crate::parametric::Interval; 2],
    outgoing: [crate::parametric::Interval; 2],
    surface: &Surface,
    vertex: Point3,
    scale: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    Ok(parameter_joint_status(incoming, outgoing, surface, vertex, scale, work, cancel)?.incident)
}

/// Fixed-size result of the SAME Native call, retained only for failure context.
struct ParameterJointStatus {
    incident: bool,
    native_error: Option<TrimDomainError>,
}

fn parameter_joint_status(
    incoming: [crate::parametric::Interval; 2],
    outgoing: [crate::parametric::Interval; 2],
    surface: &Surface,
    vertex: Point3,
    scale: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<ParameterJointStatus, GeometryError> {
    if incident(incoming, outgoing) {
        return Ok(ParameterJointStatus {
            incident: true,
            native_error: None,
        });
    }
    let endpoints = [incoming, outgoing].map(|point| {
        point.map(|axis| monstertruck_modeling::ValidatedInterval {
            lower: axis.lo,
            upper: axis.hi,
        })
    });
    let mut budget =
        ValidatedGeometryBudget::new(usize::try_from(work.remaining()).unwrap_or(usize::MAX));
    // `uv_endpoint` already includes each original chart's outward UV margin.
    let result = monstertruck_modeling::certified_surface_parameter_joint(
        surface,
        vertex,
        endpoints,
        [0.0; 2],
        1.0e-7 / scale,
        &mut budget,
        cancel,
    );
    crate::parametric::debit_native_work(work, budget.consumed(), cancel)?;
    match result {
        Ok(proof) => Ok(ParameterJointStatus {
            incident: proof.is_some(),
            native_error: None,
        }),
        Err(cause) => {
            recoverable(cause)?;
            Ok(ParameterJointStatus {
                incident: false,
                native_error: Some(cause),
            })
        }
    }
}

fn decoded_chart_proposal(chart: Curve2D, map: SurfaceParameterMap) -> Option<Curve2D> {
    let decoded = decoded_seed_map(map);
    if matches!(decoded, SurfaceParameterMap::Identity) {
        return Some(chart);
    }
    let inverse = decoded.affine_transform()?.invert()?;
    let range = chart.range_tuple();
    Some(mapped_declaration(chart, range, inverse, true))
}

/// Enclose the original branch's UV inverse as well as physical residual.
/// Affine inversion below builds a proposal only: the existing outward source
/// coefficient proof, not nominal inverse arithmetic, authorizes the result.
fn periodic_tube(
    source: &Curve3D,
    surface: &StepSurface,
    chart: Curve2D,
    map: SurfaceParameterMap,
    scale: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<chart_proof::ChartWitness>, GeometryError> {
    let decoded = decoded_seed_map(map);
    let Some(proposal) = decoded_chart_proposal(chart, map) else {
        return Ok(None);
    };
    chart_proof::certify(source, surface, proposal, decoded, scale, work, cancel)
}

fn widen_periodic_tube(
    witness: &mut SourceTrim,
    source: &Curve3D,
    surface: &StepSurface,
    map: SurfaceParameterMap,
    scale: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    let margin = if matches!(map, SurfaceParameterMap::Identity) {
        chart_proof::certify_in_place(
            source,
            surface,
            witness.curve.as_mut().unwrap(),
            map,
            scale,
            work,
            cancel,
        )?
        .map(|(margin, _)| margin)
    } else {
        periodic_tube(
            source,
            surface,
            witness.curve.as_ref().unwrap().clone(),
            map,
            scale,
            work,
            cancel,
        )?
        .map(|tube| tube.margin_uv)
    };
    if let Some(margin) = margin {
        for (maximum, margin) in witness.margin_uv.iter_mut().zip(margin) {
            *maximum = maximum.max(margin);
        }
    }
    Ok(())
}

fn isolated_periodic_branch(witness: &SourceTrim, periods: [Option<f64>; 2]) -> bool {
    (0..2).all(|axis| periods[axis].is_none_or(|period| witness.margin_uv[axis] < period * 0.25))
}

struct PeriodicBranch {
    chart: Curve2D,
    shift: [f64; 2],
    declared_source: bool,
}

fn prove_periodic_rebind(
    context: &PeriodicUse<'_>,
    surface: SurfaceInput<'_>,
    branch: PeriodicBranch,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    diagnostic: &mut super::trim_certificate::CorrespondenceDiagnostic,
) -> Result<Option<SourceTrim>, GeometryError> {
    let SurfaceInput {
        source_surface,
        native_surface,
        map,
        scale,
    } = surface;
    let PeriodicBranch {
        chart,
        shift,
        declared_source,
    } = branch;
    let started = work.consumed();
    let mut stage = "periodic-inverse-tube";
    let result = (|| {
        if context.declared.is_none()
            && matches!(native_surface, Surface::SphericalSurface(_))
            && let Some(proposal) = decoded_chart_proposal(chart.clone(), map)
            && let Some(proof) = pole_meridian_witness(
                TrimInput {
                    source_curve: context.source,
                    native_curve: context.native,
                    surface,
                    declared: None,
                },
                &proposal,
                work,
                cancel,
            )?
        {
            // This remains pending; neither an isolated lift nor one endpoint
            // can publish graph authority. The final wire pass proves BOTH.
            return Ok(Some(proof));
        }
        let (chart, margin_uv, coefficient_error) = if context.retained.is_some() {
            (chart, map_margin(map)?, 0.)
        } else {
            let Some(tube) = periodic_tube(
                context.source,
                source_surface,
                chart,
                map,
                scale,
                work,
                cancel,
            )?
            else {
                return Ok(None);
            };
            (tube.curve, tube.margin_uv, tube.error_mm)
        };
        stage = "periodic-whole-forward";
        diagnostic.active_source = Some(super::trim_certificate::CorrespondenceSource {
            face: context.face,
            wire: context.wire,
            coedge: context.coedge,
            source_edge: context.source_edge,
            native_edge: context.native_edge,
            shift,
            native_outer: match context.native {
                Curve::TrimmedCurve(trim) => {
                    Some((trim.entity().range_tuple(), trim.orientation()))
                }
                _ => None,
            },
        });
        let bound = if let Some(prepared) = context.retained {
            super::trim_certificate::certify_retained_coedge_correspondence(
                super::trim_certificate::Correspondence {
                    curve: context.native,
                    surface: native_surface,
                    chart: &chart,
                    maximum_error_mm: 1e-7 / scale,
                    margin_uv,
                },
                (prepared, 1.0),
                work,
                cancel,
            )?
        } else {
            super::trim_certificate::certify_declared_coedge_correspondence(
                super::trim_certificate::Correspondence {
                    curve: context.native,
                    surface: native_surface,
                    chart: &chart,
                    maximum_error_mm: 1e-7 / scale,
                    margin_uv,
                },
                work,
                cancel,
                Some(diagnostic),
            )?
        };
        let Some(bound) = bound else {
            return Ok(None);
        };
        let margin_uv = if let Some(prepared) = context.retained {
            stage = "periodic-retained-UV-margin";
            super::implicit::certify_uv_margin(
                super::implicit::UvMarginInput {
                    curve: context.native,
                    prepared,
                    surface: native_surface,
                    chart: &chart,
                    physical_residual: bound.position_error_mm(),
                    coefficient_margin: margin_uv,
                },
                work,
                cancel,
            )?
        } else {
            margin_uv
        };
        Ok(Some(SourceTrim {
            curve: Some(chart),
            margin_uv,
            error_mm: crate::parametric::up(bound.position_error_mm() * scale)
                .max(coefficient_error),
            certificate: SourceChartCertificate::InverseOrExact,
            pending_meridian: None,
            declared_source,
            correspondence: Some(bound),
        }))
    })();
    result.map_err(|cause| periodic_error(cause, stage, context, shift, started, work))
}

/// Borrow the *other literal* original seam association. No geometric carrier
/// recognition, global first-PCURVE choice, or reconstructed second branch.
fn other_seam_declaration<'a>(
    mut source: &'a Curve3D,
    surface: &StepSurface,
    orientation: bool,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<&'a StepCurve2D>, GeometryError> {
    use monstertruck_io::step::load::step_geometry::{SourceParameterGauge, SurfaceCurveKind};
    for depth in 0..=32 {
        work.charge(1, cancel)?;
        match source {
            Curve3D::TrimmedCurve(wrapper)
                if depth < 32
                    && *wrapper.transform() == Matrix4::identity()
                    && matches!(
                        wrapper.raw_parameter_gauge(),
                        Ok(SourceParameterGauge::Internal)
                    ) =>
            {
                source = wrapper.entity().curve().as_ref();
            }
            Curve3D::SurfaceCurve(curve) if curve.kind() == SurfaceCurveKind::SeamCurve => {
                return curve
                    .declared_parameter_curve_ref_on(surface, !orientation)
                    .map(|curve| curve.map(|curve| curve.curve()))
                    .map_err(|cause| {
                        error(
                            GeometryErrorCode::ParameterCorrespondenceUnavailable,
                            &format!("original periodic seam association refused: {cause}"),
                        )
                    });
            }
            _ => return Ok(None),
        }
    }
    Err(error(
        GeometryErrorCode::ResourceLimit,
        "original periodic seam nesting exceeds source limit",
    ))
}

/// Repair missing undeclared generator charts only from an already certified
/// original rim endpoint sharing this exact original topology vertex. Search
/// coordinates never replace full forward proof or the subsequent wire binder.
fn repair_source_associated_generators<'a, F>(
    witnesses: &mut [SourceTrim],
    context: &F,
    native_surface: &Surface,
    map: SurfaceParameterMap,
    scale: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError>
where
    F: Fn(usize) -> PeriodicUse<'a>,
{
    let Surface::ExtrusionSurface(extrusion) = native_surface else {
        return Ok(());
    };
    let Some(original_domain) = extrusion_profile_domain(map) else {
        return Ok(());
    };
    let count = witnesses.len();
    let vertex = |use_: &PeriodicUse<'_>, back| {
        if use_.orientation == back {
            use_.vertices.1
        } else {
            use_.vertices.0
        }
    };
    for index in 0..count {
        work.charge(1, cancel)?;
        if witnesses[index].certificate.is_certified() {
            continue;
        }
        let current = context(index);
        if current.declared.is_some()
            || chart_proof::native_affine_line_basis(current.native, work, cancel)?.is_none()
        {
            continue;
        }
        for (neighbor_index, neighbor_back, current_back) in [
            ((index + count - 1) % count, true, false),
            ((index + 1) % count, false, true),
        ] {
            work.charge(1, cancel)?;
            let neighbor = context(neighbor_index);
            if !witnesses[neighbor_index].certificate.is_certified()
                || vertex(&current, current_back) != vertex(&neighbor, neighbor_back)
            {
                continue;
            }
            let Some(rim) = witnesses[neighbor_index].curve.as_ref() else {
                continue;
            };
            let range = rim.range_tuple();
            let Some(is_rim) = native_attempt(work, cancel, |budget| {
                monstertruck_modeling::validated_curve2d_jet(
                    rim,
                    [range.0, range.1],
                    budget,
                    cancel,
                )
                .map(|jet| {
                    jet.first[1].lower == 0.0
                        && jet.first[1].upper == 0.0
                        && (jet.first[0].lower > 0.0 || jet.first[0].upper < 0.0)
                })
                .map_err(TrimDomainError::Geometry)
            })?
            else {
                continue;
            };
            if !is_rim {
                continue;
            }
            let Some(uv) = uv_endpoint(
                &witnesses[neighbor_index],
                neighbor.orientation,
                neighbor_back,
                work,
                cancel,
            )?
            else {
                continue;
            };
            let parameter = uv[0].mid();
            let native_range = extrusion.entity_curve().range_tuple();
            if parameter < native_range.0 || parameter > native_range.1 {
                continue;
            }
            if let super::harmonic::ProfileParameterDomain::Bounded(range) = original_domain
                && (parameter < range[0] || parameter > range[1])
            {
                continue;
            }
            let Some(chart) =
                generator_chart_proposal(current.native, native_surface, parameter, work, cancel)?
            else {
                continue;
            };
            if let Some(witness) =
                derived_chart_witness(current.native, native_surface, chart, scale, work, cancel)?
            {
                witnesses[index] = witness;
                break;
            }
        }
    }
    Ok(())
}

/// Bind periodic coordinates by original wire incidence, never by principal
/// wrapping each edge independently. Literal constant-parameter uses anchor the
/// chart. A synthetic LINE restriction may change its own finite active domain;
/// a literal bounded PCURVE may not. Derived lifts need whole retained-branch
/// position/tangent AND UV-tube proofs, just like the original candidates.
///
/// Endpoint overlap only selects a proposal. Every changed use is proved over
/// its entire unchanged native interval; original vertex incidence closes the
/// cycle. Nonliteral joints require the native whole-connector image
/// and original-vertex residual certificate, never UV proximity or an adapter-side
/// bridge. Native trim preparation/coverage remains mandatory for all consumers.
pub(super) fn bind_periodic_wire<'a, F>(
    witnesses: &mut [SourceTrim],
    context: F,
    surface: SurfaceInput<'_>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    diagnostic: &mut super::trim_certificate::CorrespondenceDiagnostic,
) -> Result<(), GeometryError>
where
    F: Fn(usize) -> PeriodicUse<'a>,
{
    let SurfaceInput {
        source_surface,
        native_surface,
        map,
        scale,
    } = surface;
    repair_source_associated_generators(
        witnesses,
        &context,
        native_surface,
        map,
        scale,
        work,
        cancel,
    )?;
    let periods = [native_surface.period_u(), native_surface.period_v()];
    if periods.iter().all(Option::is_none)
        || witnesses.is_empty()
        || witnesses.iter().any(|witness| {
            (!witness.certificate.is_certified() && witness.pending_meridian.is_none())
                || witness.curve.is_none()
        })
    {
        return Ok(());
    }
    if periods
        .iter()
        .flatten()
        .any(|period| !period.is_finite() || *period <= 0.)
    {
        return Err(error(
            GeometryErrorCode::ConditioningFailure,
            "original periodic chart has an invalid native period",
        ));
    }
    let mut anchor = 0;
    let mut literal_constant_anchor = false;
    for index in 0..witnesses.len() {
        let current = context(index);
        let started = work.consumed();
        work.charge(1, cancel).map_err(|cause| {
            periodic_error(
                cause,
                "periodic-source-incidence",
                &current,
                [0.0; 2],
                started,
                work,
            )
        })?;
        let next = context((index + 1) % witnesses.len());
        let back = if current.orientation {
            current.vertices.1
        } else {
            current.vertices.0
        };
        let front = if next.orientation {
            next.vertices.0
        } else {
            next.vertices.1
        };
        if back != front {
            return Err(invalid(
                "original periodic wire loses source vertex incidence",
            ));
        }
        if current.retained.is_none() && witnesses[index].declared_source {
            widen_periodic_tube(
                &mut witnesses[index],
                current.source,
                source_surface,
                map,
                scale,
                work,
                cancel,
            )
            .map_err(|cause| {
                periodic_error(
                    cause,
                    "periodic-widen-inverse-tube",
                    &current,
                    [0.0; 2],
                    started,
                    work,
                )
            })?;
        }
        if !isolated_periodic_branch(&witnesses[index], periods) {
            return Err(error(
                GeometryErrorCode::ConditioningFailure,
                "original periodic UV branch is not isolated within one half-period",
            ));
        }
        if current.declared.is_some() {
            let curve = witnesses[index].curve.as_ref().unwrap();
            let (a, b) = curve.range_tuple();
            let a = curve.subs(a);
            let b = curve.subs(b);
            if (0..2).all(|axis| periods[axis].is_none() || a[axis] == b[axis]) {
                anchor = index;
                literal_constant_anchor = true;
            }
        }
    }
    if !literal_constant_anchor {
        // Cut the search at an ORIGINAL singular endpoint, not inside the
        // ordinary chain. Otherwise an early valid pole connector can leave
        // two equivalent equator endpoints on different covering sheets.
        // The graph's complete forward/tangent receipt binds these flags to
        // its actual native curve/UV endpoint ranges, including every inner
        // source/native reflection; the use sense only selects that proved end.
        // This is search data, never pole authority or a nearest-angle choice.
        for (index, witness) in witnesses.iter().enumerate() {
            work.charge(1, cancel)?;
            if let Some(graph) = witness.pending_meridian
                && witness.proves_forward(1e-7 / scale, witness.margin_uv)
                && graph.pole_endpoints[usize::from(!context(index).orientation)]
            {
                anchor = index;
                break;
            }
        }
    }
    for offset in 1..witnesses.len() {
        let index = (anchor + offset) % witnesses.len();
        let previous = (index + witnesses.len() - 1) % witnesses.len();
        let current = context(index);
        let started = work.consumed();
        let mut stage = "previous-incoming-endpoint";
        let mut translation = [0.0; 2];
        let result = (|| {
            let incoming = uv_endpoint(
                &witnesses[previous],
                context(previous).orientation,
                true,
                work,
                cancel,
            )
            .map_err(|cause| {
                periodic_error(
                    cause,
                    "periodic-incoming-endpoint",
                    &context(previous),
                    [0.0; 2],
                    started,
                    work,
                )
            })?
            .unwrap();
            stage = "periodic-outgoing-endpoint";
            let front =
                uv_endpoint(&witnesses[index], current.orientation, false, work, cancel)?.unwrap();
            let mut candidate_front = None;
            let mut candidate_margin = None;
            let mut candidate_isolated = None;
            let mut candidate_kind = "none";
            stage = "periodic-native-joint";
            if parameter_joint_incident(
                incoming,
                front,
                native_surface,
                current.front_vertex,
                scale,
                work,
                cancel,
            )? {
                return Ok(());
            }
            stage = "periodic-alternate-seam-source";
            if let Some(other) = other_seam_declaration(
                current.source,
                source_surface,
                current.orientation,
                work,
                cancel,
            )? {
                stage = "periodic-alternate-seam-chart";
                let mut candidate = retain_with_continuation(
                    TrimInput {
                        source_curve: current.source,
                        native_curve: current.native,
                        surface,
                        declared: Some(other),
                    },
                    current.retained,
                    work,
                    cancel,
                )?;
                if candidate.certificate.is_certified() {
                    if current.retained.is_none() && candidate.declared_source {
                        stage = "periodic-alternate-seam-inverse-tube";
                        widen_periodic_tube(
                            &mut candidate,
                            current.source,
                            source_surface,
                            map,
                            scale,
                            work,
                            cancel,
                        )?;
                    }
                    stage = "periodic-alternate-seam-endpoint";
                    candidate_kind = "alternate-proved";
                    candidate_margin = Some(candidate.margin_uv);
                    let isolated = isolated_periodic_branch(&candidate, periods);
                    candidate_isolated = Some(isolated);
                    if isolated {
                        let endpoint =
                            uv_endpoint(&candidate, current.orientation, false, work, cancel)?
                                .unwrap();
                        candidate_front = Some(endpoint);
                        if parameter_joint_incident(
                            incoming,
                            endpoint,
                            native_surface,
                            current.front_vertex,
                            scale,
                            work,
                            cancel,
                        )? {
                            witnesses[index] = candidate;
                            return Ok(());
                        }
                    }
                }
            }
            translation = std::array::from_fn(|axis| {
                periods[axis].map_or(0., |period| {
                    ((incoming[axis].mid() - front[axis].mid()) / period).round() * period
                })
            });
            if translation.into_iter().any(|value| !value.is_finite()) {
                return Err(error(
                    GeometryErrorCode::ConditioningFailure,
                    "original periodic chart lift is not finite",
                ));
            }
            let curve = witnesses[index].curve.as_ref().unwrap();
            stage = "periodic-original-line-branch";
            let proposal =
                periodic_lift_proposal(current.declared, curve, translation, work, cancel)?;
            if let Some(proposal) = proposal {
                stage = "periodic-translated-rebind";
                if let Some(candidate) = prove_periodic_rebind(
                    &current,
                    surface,
                    PeriodicBranch {
                        chart: proposal,
                        shift: translation,
                        declared_source: witnesses[index].declared_source,
                    },
                    work,
                    cancel,
                    diagnostic,
                )? {
                    stage = "periodic-translated-endpoint";
                    candidate_kind = "translated-proved";
                    candidate_margin = Some(candidate.margin_uv);
                    let isolated = isolated_periodic_branch(&candidate, periods);
                    candidate_isolated = Some(isolated);
                    if isolated {
                        let endpoint =
                            uv_endpoint(&candidate, current.orientation, false, work, cancel)?
                                .unwrap();
                        candidate_front = Some(endpoint);
                        if parameter_joint_incident(
                            incoming,
                            endpoint,
                            native_surface,
                            current.front_vertex,
                            scale,
                            work,
                            cancel,
                        )? {
                            witnesses[index] = candidate;
                            return Ok(());
                        }
                    }
                }
            }
            stage = "periodic-unresolved-branch";
            Err(error(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                &format!(
                    "UV anchor={anchor}/src={:?}/decl={} r={:?}; cur decl={} r={:?}; in={:?} out={:?} m={:?}/{:?}; {candidate_kind} iso={candidate_isolated:?} cand={:?} cm={candidate_margin:?}",
                    context(anchor).source_edge,
                    context(anchor).declared.is_some(),
                    witnesses[anchor]
                        .curve
                        .as_ref()
                        .map(|curve| curve.range_tuple()),
                    current.declared.is_some(),
                    witnesses[index]
                        .curve
                        .as_ref()
                        .map(|curve| curve.range_tuple()),
                    incoming.map(|axis| [axis.lo, axis.hi]),
                    front.map(|axis| [axis.lo, axis.hi]),
                    witnesses[previous].margin_uv,
                    witnesses[index].margin_uv,
                    candidate_front.map(|point| point.map(|axis| [axis.lo, axis.hi])),
                ),
            ))
        })();
        result
            .map_err(|cause| periodic_error(cause, stage, &current, translation, started, work))?;
    }
    let last = (anchor + witnesses.len() - 1) % witnesses.len();
    let started = work.consumed();
    let result = (|| {
        let incoming = uv_endpoint(
            &witnesses[last],
            context(last).orientation,
            true,
            work,
            cancel,
        )?
        .unwrap();
        let outgoing = uv_endpoint(
            &witnesses[anchor],
            context(anchor).orientation,
            false,
            work,
            cancel,
        )?
        .unwrap();
        let current = context(anchor);
        let before = context(last);
        let status = parameter_joint_status(
            incoming,
            outgoing,
            native_surface,
            current.front_vertex,
            scale,
            work,
            cancel,
        )?;
        if !status.incident {
            return Err(error(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                &format!(
                    "original periodic coedge branches do not close in one bounded UV chart; \
                     native_joint={:?}; incoming c={last}/src={:?}/edge={}/orientation={} \
                     source_vertices={:?} spatial_range={:?} uv_range={:?} uv={:?} margin={:?} graph={:?}; \
                     outgoing c={anchor}/src={:?}/edge={}/orientation={} source_vertices={:?} \
                     spatial_range={:?} uv_range={:?} uv={:?} margin={:?} graph={:?}; \
                     shared_source_vertex={} position={:?} periods={periods:?}",
                    status.native_error,
                    before.source_edge,
                    before.native_edge,
                    before.orientation,
                    before.vertices,
                    before.native.try_range_tuple(),
                    witnesses[last].curve.as_ref().map(Curve2D::range_tuple),
                    incoming.map(|axis| [axis.lo, axis.hi]),
                    witnesses[last].margin_uv,
                    witnesses[last]
                        .pending_meridian
                        .map(|graph| (graph.latitude_axis, graph.pole_endpoints)),
                    current.source_edge,
                    current.native_edge,
                    current.orientation,
                    current.vertices,
                    current.native.try_range_tuple(),
                    witnesses[anchor].curve.as_ref().map(Curve2D::range_tuple),
                    outgoing.map(|axis| [axis.lo, axis.hi]),
                    witnesses[anchor].margin_uv,
                    witnesses[anchor]
                        .pending_meridian
                        .map(|graph| (graph.latitude_axis, graph.pole_endpoints)),
                    if current.orientation {
                        current.vertices.0
                    } else {
                        current.vertices.1
                    },
                    current.front_vertex,
                ),
            ));
        }
        Ok(())
    })();
    result.map_err(|cause| {
        periodic_error(
            cause,
            "periodic-closing-joint",
            &context(anchor),
            [0.0; 2],
            started,
            work,
        )
    })?;
    qualify_meridian_graphs(witnesses, &context, native_surface, scale, work, cancel)
}

/// Qualify all pending graphs atomically after ordinary wire binding. Source
/// vertex identity and whole connector image are mandatory even for coincident
/// UV endpoints; a pole endpoint must receive the actual Native pole theorem.
fn qualify_meridian_graphs<'a, F>(
    witnesses: &mut [SourceTrim],
    context: &F,
    surface: &Surface,
    scale: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError>
where
    F: Fn(usize) -> PeriodicUse<'a>,
{
    if witnesses.is_empty() {
        return Ok(());
    }
    for index in 0..witnesses.len() {
        let previous = (index + witnesses.len() - 1) % witnesses.len();
        let incoming_graph = witnesses[previous].pending_meridian;
        let outgoing_graph = witnesses[index].pending_meridian;
        if incoming_graph.is_none() && outgoing_graph.is_none() {
            continue;
        }
        work.charge(1, cancel)?;
        let before = context(previous);
        let current = context(index);
        let back = if before.orientation {
            before.vertices.1
        } else {
            before.vertices.0
        };
        let front = if current.orientation {
            current.vertices.0
        } else {
            current.vertices.1
        };
        if back != front {
            return Err(invalid(
                "meridian graph joint loses original source vertex identity",
            ));
        }
        let pole = incoming_graph
            .is_some_and(|graph| graph.pole_endpoints[usize::from(before.orientation)])
            || outgoing_graph
                .is_some_and(|graph| graph.pole_endpoints[usize::from(!current.orientation)]);
        let endpoints = [
            uv_endpoint(&witnesses[previous], before.orientation, true, work, cancel)?
                .ok_or_else(|| invalid("meridian incoming chart is unavailable"))?,
            uv_endpoint(&witnesses[index], current.orientation, false, work, cancel)?
                .ok_or_else(|| invalid("meridian outgoing chart is unavailable"))?,
        ]
        .map(|point| {
            point.map(|axis| monstertruck_modeling::ValidatedInterval {
                lower: axis.lo,
                upper: axis.hi,
            })
        });
        // The physical graph tube is NOT a UV inverse. Reserve its complete
        // error before asking Native to certify the connector's world image,
        // so their sum remains inside the unchanged original physical budget.
        let physical = witnesses[previous].error_mm.max(witnesses[index].error_mm);
        let allowance = crate::parametric::Interval::point(1e-7)
            .sub(crate::parametric::Interval::point(physical))
            .div(crate::parametric::Interval::point(scale))?
            .lo;
        if !allowance.is_finite() || allowance <= 0. {
            return Err(error(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "meridian graph consumes the original source-vertex error allowance",
            ));
        }
        let mut budget =
            ValidatedGeometryBudget::new(usize::try_from(work.remaining()).unwrap_or(usize::MAX));
        let result = monstertruck_modeling::certified_surface_parameter_joint(
            surface,
            current.front_vertex,
            endpoints,
            [0.; 2],
            allowance,
            &mut budget,
            cancel,
        );
        crate::parametric::debit_native_work(work, budget.consumed(), cancel)?;
        let qualified = match result {
            Ok(Some(kind)) => {
                !pole
                    || matches!(
                        kind,
                        monstertruck_modeling::TrimJointKind::SphericalPole { .. }
                    )
            }
            Ok(None) => false,
            Err(cause) => {
                recoverable(cause)?;
                false
            }
        };
        if !qualified {
            return Err(error(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                &format!(
                    "original meridian graph joint {index} has no complete {}source-vertex image certificate",
                    if pole { "spherical-pole " } else { "" }
                ),
            ));
        }
    }
    for witness in witnesses {
        if let Some(graph) = witness.pending_meridian.take() {
            witness.certificate = SourceChartCertificate::PoleMeridianGraph {
                latitude_axis: graph.latitude_axis,
                pole_endpoints: graph.pole_endpoints,
            };
            if let Some(receipt) = witness.correspondence.as_mut() {
                receipt.complete_pole_graph();
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use monstertruck_io::step::load::{
        SurfaceOfLinearExtrusionHolder, Table,
        step_geometry::{StepExtrusionSurface, StepParameterCurve, SweepSurface},
    };
    use std::str::FromStr;
    use step_p21::{ast::DataSection, tables::EntityTable};

    #[test]
    fn analytic_inverse_tube_requires_independent_whole_native_correspondence() {
        use monstertruck_io::step::load::step_geometry::{Conic3D, ElementarySurface};
        use monstertruck_modeling::RevolutionSurface;
        let cancel = AtomicBool::new(false);
        let tau = std::f64::consts::TAU;
        let original_surface =
            StepSurface::ElementarySurface(ElementarySurface::CylindricalSurface(Processor::new(
                RevolutionSurface::by_revolution(
                    Line(Point3::new(13., 10., 0.), Point3::new(13., 10., 8.)),
                    Point3::new(10., 10., 0.),
                    Vector3::unit_z(),
                ),
            )));
        let (surface, map) = original_surface.to_modeling_with_parameter_map().unwrap();
        for reflected in [false, true] {
            let mut ellipse = Processor::with_transform(
                TrimmedCurve::new(
                    UnitCircle::<Point3>::new(),
                    (tau, 3. * std::f64::consts::PI),
                ),
                Matrix4::from_translation(Vector3::new(10., 10., 4.))
                    * Matrix4::from_nonuniform_scale(3., 3., 1.),
            );
            if reflected {
                ellipse.invert();
            }
            let source = Curve3D::Conic(Conic3D::Ellipse(ellipse));
            let native = source.to_parameter_preserving_modeling().unwrap();
            let input = TrimInput {
                source_curve: &source,
                native_curve: &native,
                surface: SurfaceInput {
                    source_surface: &original_surface,
                    native_surface: &surface,
                    map,
                    scale: 1.,
                },
                declared: None,
            };
            let witness = retain(
                input,
                [0.; 2],
                &mut GeometryWorkBudget::new(4_000_000),
                &cancel,
            )
            .unwrap();
            let proof = witness
                .correspondence
                .as_ref()
                .expect("whole original analytic receipt");
            assert_eq!(
                proof.provenance(),
                super::super::trim_certificate::OriginalCorrespondenceProvenance::Forward
            );
            assert_eq!(proof.native_range(), [tau, 3. * std::f64::consts::PI]);
            assert_eq!(proof.margin_uv(), witness.margin_uv);
            assert!(proof.position_error_mm() <= 1.0e-7);
            assert!(proof.relative_derivative_error_bound() <= 0.25);
            assert!(witness.certificate.is_certified() && !witness.declared_source);
            assert!(witness.margin_uv.into_iter().any(|margin| margin > 0.));
            let wrong = native.transformed(Matrix4::from_translation(Vector3::new(0.001, 0., 0.)));
            let inverse_only = retain(
                TrimInput {
                    native_curve: &wrong,
                    ..input
                },
                [0.; 2],
                &mut GeometryWorkBudget::new(4_000_000),
                &cancel,
            )
            .unwrap();
            assert!(inverse_only.certificate.is_certified());
            assert!(
                inverse_only.correspondence.is_none(),
                "source inverse coefficients cannot prove a different NativeEdge expression"
            );
        }
    }

    #[test]
    fn blind_pocket_analytic_receipts_bind_final_original_coedges() {
        let cancel = AtomicBool::new(false);
        let source = crate::parity_fixtures::original(
            "native-parity-blind-pocket.step",
            crate::parity_fixtures::blind_pocket(),
        );
        let definition = crate::import_step(source.as_bytes(), &cancel).unwrap();
        let mut analytic_forward = 0;
        for (face_index, face) in definition.face_boundaries.iter().enumerate() {
            for (wire_index, wire) in face.wires.iter().enumerate() {
                for (coedge_index, coedge) in wire.iter().enumerate() {
                    let proof = coedge.correspondence.as_ref().unwrap_or_else(|| panic!(
                        "missing original correspondence at face {face_index}, wire {wire_index}, coedge {coedge_index}",
                    ));
                    let binding = proof.binding().expect("final normalized binding");
                    assert_eq!(binding.face, face.face);
                    assert_eq!(binding.edge, coedge.edge);
                    assert_eq!(binding.owner, [face_index, wire_index, coedge_index]);
                    assert_eq!(
                        binding.source,
                        definition.source_topology.faces[face_index].wires[wire_index].coedges
                            [coedge_index]
                    );
                    assert_eq!(proof.margin_uv(), coedge.source_chart_margin_uv);
                    assert!(proof.position_error_mm() <= 1.0e-7);
                    assert!(proof.relative_derivative_error_bound() <= 0.25);
                    if !coedge.source_chart_declared
                        && proof.provenance() == super::super::trim_certificate::OriginalCorrespondenceProvenance::Forward
                    {
                        analytic_forward += 1;
                    }
                }
            }
        }
        assert!(
            analytic_forward > 0,
            "actual derived analytic charts retain forward receipts"
        );
    }

    fn original_pole_meridian(radius: f64, longitude: f64, reflected: bool) -> Curve3D {
        use monstertruck_io::step::load::step_geometry::Conic3D;
        let (sine, cosine) = longitude.sin_cos();
        let frame = Matrix4::from_cols(
            Vector4::new(radius * cosine, radius * sine, 0., 0.),
            Vector4::new(0., 0., radius, 0.),
            Vector4::new(sine, -cosine, 0., 0.),
            Vector4::new(0., 0., 0., 1.),
        );
        let mut original = Processor::with_transform(
            TrimmedCurve::new(
                UnitCircle::<Point3>::new(),
                (0., std::f64::consts::FRAC_PI_2),
            ),
            frame,
        );
        if reflected {
            original.invert();
        }
        Curve3D::Conic(Conic3D::Ellipse(original))
    }

    #[test]
    fn pole_meridian_graph_requires_whole_source_phase_plane_and_tangent() {
        use monstertruck_io::step::load::step_geometry::ElementarySurface;
        let half_pi = std::f64::consts::FRAC_PI_2;
        let cancel = AtomicBool::new(false);
        for radius in [2., 1e-12] {
            let original = StepSurface::ElementarySurface(ElementarySurface::Sphere(
                Processor::new(monstertruck_io::step::load::step_geometry::Sphere(
                    monstertruck_modeling::Sphere::new(Point3::origin(), radius),
                )),
            ));
            // This property targets graph/tangent authority, not the STEP bridge's
            // independent analytic-family size eligibility. Keep the microscopic
            // original input and construct exactly the same Native coefficients.
            let native_surface = Surface::SphericalSurface(Processor::new(
                monstertruck_modeling::Sphere::new(Point3::origin(), radius),
            ));
            let map = SurfaceParameterMap::Affine(Matrix3::from_cols(
                Vector3::new(0., 1., 0.),
                Vector3::new(-1., 0., 0.),
                Vector3::new(half_pi, 0., 1.),
            ));
            let source = original_pole_meridian(radius, 0., false);
            let native = source.to_parameter_preserving_modeling().unwrap();
            let input = TrimInput {
                source_curve: &source,
                native_curve: &native,
                surface: SurfaceInput {
                    source_surface: &original,
                    native_surface: &native_surface,
                    map,
                    scale: 1.,
                },
                declared: None,
            };
            let mut work = GeometryWorkBudget::new(4_000_000);
            let valid = Curve2D::Line(Line(Point2::origin(), Point2::new(0., half_pi)));
            let pending = pole_meridian_witness(input, &valid, &mut work, &cancel)
                .unwrap()
                .expect("complete original meridian graph");
            assert!(!pending.certificate.is_certified());
            assert!(pending.pending_meridian.is_some());
            assert!(pending.error_mm > 0. && pending.error_mm < 1e-7);
            let declared = StepCurve2D::Line(Line(Point2::origin(), Point2::new(0., half_pi)));
            assert!(
                pole_meridian_witness(
                    TrimInput {
                        declared: Some(&declared),
                        ..input
                    },
                    &valid,
                    &mut work,
                    &cancel,
                )
                .unwrap()
                .is_none(),
                "a retained original UV declaration is never replaced by a graph",
            );
            for wrong in [
                Curve2D::Line(Line(
                    Point2::new(half_pi, 0.),
                    Point2::new(half_pi, half_pi),
                )),
                Curve2D::Line(Line(Point2::new(0., half_pi), Point2::origin())),
                Curve2D::Line(Line(Point2::new(0., -half_pi), Point2::new(0., half_pi))),
            ] {
                assert!(
                    pole_meridian_witness(input, &wrong, &mut work, &cancel)
                        .unwrap()
                        .is_none(),
                    "wrong original plane, phase or winding cannot borrow pole authority",
                );
            }
        }
    }

    #[test]
    fn pole_meridian_graph_encloses_original_affine_frame_and_both_reflection_senses() {
        use monstertruck_io::step::load::step_geometry::{ElementarySurface, SourceTrimmedCurve};
        let half_pi = std::f64::consts::FRAC_PI_2;
        let pose = Matrix4::from_cols(
            Vector4::new(-1.2, 0., 0.1, 0.),
            Vector4::new(0.3, 1.1, 0., 0.),
            Vector4::new(0.1, 0.2, 0.9, 0.),
            Vector4::new(3., -2., 1., 1.),
        );
        let original =
            StepSurface::ElementarySurface(ElementarySurface::Sphere(Processor::with_transform(
                monstertruck_io::step::load::step_geometry::Sphere(
                    monstertruck_modeling::Sphere::new(Point3::origin(), 2.),
                ),
                pose,
            )));
        let (surface, map) = original.to_modeling_with_parameter_map().unwrap();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(4_000_000);
        for reflected in [false, true] {
            let mut wrapper = Processor::with_transform(
                TrimmedCurve::new(
                    Box::new(original_pole_meridian(2., 0., false)),
                    (0., half_pi),
                ),
                pose,
            );
            if reflected {
                wrapper.invert();
            }
            let source = Curve3D::TrimmedCurve(SourceTrimmedCurve::internal(wrapper));
            let native = source.to_parameter_preserving_modeling().unwrap();
            let (first, last) = if reflected {
                (half_pi, 0.)
            } else {
                (0., half_pi)
            };
            let chart = Curve2D::Line(Line(Point2::new(0., first), Point2::new(0., last)));
            let proof = pole_meridian_witness(
                TrimInput {
                    source_curve: &source,
                    native_curve: &native,
                    surface: SurfaceInput {
                        source_surface: &original,
                        native_surface: &surface,
                        map,
                        scale: 1.,
                    },
                    declared: None,
                },
                &chart,
                &mut work,
                &cancel,
            )
            .unwrap()
            .expect("whole original inverse-frame and relative-tangent proof");
            assert!(proof.pending_meridian.is_some());
            assert!(!proof.certificate.is_certified());
            assert!(proof.error_mm > 0. && proof.error_mm < 1e-7);
        }
    }

    #[test]
    fn pole_meridian_graph_publishes_only_after_both_original_vertex_joint_images() {
        use monstertruck_io::step::load::step_geometry::{Conic3D, ElementarySurface};
        let radius = 2.;
        let half_pi = std::f64::consts::FRAC_PI_2;
        let original = StepSurface::ElementarySurface(ElementarySurface::Sphere(Processor::new(
            monstertruck_io::step::load::step_geometry::Sphere(monstertruck_modeling::Sphere::new(
                Point3::origin(),
                radius,
            )),
        )));
        let (surface, map) = original.to_modeling_with_parameter_map().unwrap();
        let sources = [
            original_pole_meridian(radius, 0., true),
            Curve3D::Conic(Conic3D::Ellipse(Processor::with_transform(
                TrimmedCurve::new(UnitCircle::<Point3>::new(), (0., half_pi)),
                Matrix4::from_scale(radius),
            ))),
            original_pole_meridian(radius, half_pi, false),
        ];
        let native: [Curve; 3] =
            std::array::from_fn(|index| sources[index].to_parameter_preserving_modeling().unwrap());
        let points = [
            Point3::new(0., 0., radius),
            Point3::new(radius, 0., 0.),
            Point3::new(0., radius, 0.),
        ];
        let vertices = [(0, 1), (1, 2), (2, 0)];
        let cancel = AtomicBool::new(false);
        for fault in 0..4 {
            let mut work = GeometryWorkBudget::new(4_000_000);
            let mut witnesses = (0..3)
                .map(|index| {
                    retain(
                        TrimInput {
                            source_curve: &sources[index],
                            native_curve: &native[index],
                            surface: SurfaceInput {
                                source_surface: &original,
                                native_surface: &surface,
                                map,
                                scale: 1.,
                            },
                            declared: None,
                        },
                        [0.; 2],
                        &mut work,
                        &cancel,
                    )
                    .unwrap()
                })
                .collect::<Vec<_>>();
            assert!(!witnesses[0].certificate.is_certified());
            assert!(!witnesses[2].certificate.is_certified());
            if fault == 3 {
                let axis = witnesses[0].pending_meridian.unwrap().latitude_axis;
                witnesses[0].margin_uv[axis] = 1e-3;
            }
            let context = |index: usize| PeriodicUse {
                face: 0,
                wire: 0,
                coedge: index,
                source_edge: Some(index as u64),
                native_edge: index,
                source: &sources[index],
                native: &native[index],
                declared: None,
                retained: None,
                orientation: true,
                vertices: if fault == 2 && index == 0 {
                    (99, 1)
                } else {
                    vertices[index]
                },
                front_vertex: if fault == 1 && index == 0 {
                    points[0] + Vector3::new(0., 0., 1e-3)
                } else {
                    points[vertices[index].0]
                },
            };
            let result = if fault == 3 {
                // Exercise the final receipt with a genuinely unsupported
                // whole connector tube, without proposing another chart.
                qualify_meridian_graphs(&mut witnesses, &context, &surface, 1., &mut work, &cancel)
            } else {
                bind_periodic_wire(
                    &mut witnesses,
                    context,
                    SurfaceInput {
                        source_surface: &original,
                        native_surface: &surface,
                        map,
                        scale: 1.,
                    },
                    &mut work,
                    &cancel,
                    &mut super::super::trim_certificate::CorrespondenceDiagnostic::default(),
                )
            };
            if fault == 0 {
                result.expect("complete original meridian wire and both Native joint images");
                for index in [0, 2] {
                    assert!(matches!(
                        witnesses[index].certificate,
                        SourceChartCertificate::PoleMeridianGraph { .. }
                    ));
                    assert!(!witnesses[index].certificate.has_regular_inverse());
                    assert!(!witnesses[index].declared_source);
                }
            } else {
                assert!(
                    result.is_err(),
                    "wrong source vertex, identity or whole pole connector must refuse"
                );
                assert!(!witnesses[0].certificate.is_certified());
                assert!(!witnesses[2].certificate.is_certified());
            }
        }
    }

    #[test]
    fn rational_declared_chart_identity_keeps_global_scale_and_internal_c0_knots() {
        use monstertruck_modeling::{BsplineCurve, KnotVector, NurbsCurve};
        let knots = KnotVector::from(vec![0., 0., 0., 1., 1., 2., 2., 2.]);
        let controls = vec![
            Vector3::new(5., 0., 1.),
            Vector3::new(5., 5., 1.),
            Vector3::new(0., 10., 2.),
            Vector3::new(-10., 10., 2.),
            Vector3::new(-20., 0., 4.),
        ];
        let chart = |controls, knots| {
            Curve2D::NurbsCurve(NurbsCurve::new(BsplineCurve::new(knots, controls)))
        };
        let original = chart(controls.clone(), knots.clone());
        let scaled = chart(
            controls.iter().map(|point| *point / 32.).collect(),
            knots.clone(),
        );
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        assert!(same_coefficients(&original, &scaled, &mut work, &cancel).unwrap());
        let mut independent_scales = controls.clone();
        independent_scales[1] *= 2.;
        assert!(
            !same_coefficients(
                &original,
                &chart(independent_scales, knots.clone()),
                &mut work,
                &cancel,
            )
            .unwrap()
        );
        let moved = KnotVector::from(vec![0., 0., 0., 1.5, 1.5, 2., 2., 2.]);
        assert!(
            !same_coefficients(
                &original,
                &chart(controls.clone(), moved),
                &mut work,
                &cancel
            )
            .unwrap()
        );
        let mut zero_weight = controls;
        zero_weight[1].z = 0.;
        let zero_original = chart(zero_weight.clone(), knots.clone());
        let zero_scaled = chart(
            zero_weight.iter().map(|point| *point * -2.).collect(),
            knots,
        );
        assert!(
            same_coefficients(&zero_original, &zero_scaled, &mut work, &cancel).unwrap(),
            "coefficient equivalence does not discard zero control weights or admit a denominator"
        );
        assert!(chart_proof::exact_products_equal(
            f64::MAX,
            2.,
            f64::MAX / 2.,
            4.
        ));
        assert!(chart_proof::exact_products_equal(
            f64::from_bits(1),
            0.5,
            f64::from_bits(1),
            0.5
        ));
        assert!(!chart_proof::exact_products_equal(
            f64::from_bits(1),
            0.5,
            0.,
            1.
        ));
        assert!(!chart_proof::exact_products_equal(
            f64::INFINITY,
            0.,
            0.,
            1.
        ));
    }

    #[test]
    fn exact_tensor_row_keeps_independent_original_gauge_and_reflected_orientation() {
        use monstertruck_modeling::{
            BsplineCurve, BsplineSurface, KnotVector, NurbsCurve, NurbsSurface,
        };
        let knots = KnotVector::from(vec![0., 0., 0., 1., 1., 1.]);
        let row = [
            Vector4::new(0., 0., 0., 1.),
            Vector4::new(1., 2., 0., 2.),
            Vector4::new(2., 0., 0., 1.),
        ];
        let tensor = NurbsSurface::new(BsplineSurface::new(
            (knots, KnotVector::from(vec![0., 0., 1., 1.])),
            row.iter()
                .map(|point| vec![*point, *point + Vector4::new(0., 0., point.w, 0.)])
                .collect(),
        ));
        let source_surface = StepSurface::NurbsSurface(tensor.clone());
        let surface = Surface::NurbsSurface(tensor);
        let leader = NurbsCurve::new(BsplineCurve::new(
            KnotVector::from(vec![10., 10., 10., 12., 12., 12.]),
            row.to_vec(),
        ));
        let cancel = AtomicBool::new(false);
        for reflected in [false, true] {
            let mut spatial = Processor::<_, Matrix4>::new(TrimmedCurve::new(
                Box::new(Curve::NurbsCurve(leader.clone())),
                (10., 12.),
            ));
            let mut uv = Processor::<_, Matrix3>::new(TrimmedCurve::new(
                Box::new(Curve2D::BsplineCurve(BsplineCurve::new(
                    KnotVector::from(vec![7., 7., 19., 19.]),
                    vec![Point2::new(0., 0.), Point2::new(1., 0.)],
                ))),
                (7., 19.),
            ));
            if reflected {
                spatial.invert();
                uv.invert();
            }
            let curve = Curve::TrimmedCurve(spatial);
            let original = Curve2D::TrimmedCurve(uv);
            let mut work = GeometryWorkBudget::new(100_000);
            let candidate = native_chart(
                &curve,
                &surface,
                Some(&original),
                intrinsic_domain(&source_surface, &surface, SurfaceParameterMap::Identity),
                &mut work,
                &cancel,
            )
            .unwrap()
            .expect("all projective coefficients and independently scaled knots agree");
            assert_eq!(candidate.range_tuple(), (10., 12.));
            assert_eq!(original.range_tuple(), (7., 19.));
            assert_eq!(
                exact_affine_endpoints(&candidate, &mut work, &cancel).unwrap(),
                exact_affine_endpoints(&original, &mut work, &cancel).unwrap(),
            );
            let wrong = mapped_declaration(
                original,
                (7., 19.),
                Matrix3::from_translation(Vector2::new(0., 0.25)),
                true,
            );
            assert!(
                native_chart(
                    &curve,
                    &surface,
                    Some(&wrong),
                    intrinsic_domain(&source_surface, &surface, SurfaceParameterMap::Identity),
                    &mut work,
                    &cancel,
                )
                .unwrap()
                .is_none(),
                "nearby original chart cannot borrow a tensor-row identity",
            );
        }
    }

    #[test]
    fn affine_endpoint_identity_refuses_bent_multispans_and_preserves_nested_anchors() {
        use monstertruck_modeling::{BsplineCurve, KnotVector};
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let bent = Curve2D::BsplineCurve(BsplineCurve::new(
            KnotVector::from(vec![0., 0., 1., 2., 2.]),
            vec![
                Point2::new(0., 0.),
                Point2::new(1., 1.),
                Point2::new(2., 0.),
            ],
        ));
        assert!(
            exact_affine_endpoints(&bent, &mut work, &cancel)
                .unwrap()
                .is_none()
        );
        let inner = mapped_declaration(
            Curve2D::BsplineCurve(BsplineCurve::new(
                KnotVector::from(vec![2., 2., 6., 6.]),
                vec![Point2::new(0., 0.), Point2::new(1., 0.)],
            )),
            (2., 6.),
            Matrix3::identity(),
            false,
        );
        let outer = mapped_declaration(inner, (3., 4.), Matrix3::identity(), false);
        assert_eq!(
            exact_affine_endpoints(&outer, &mut work, &cancel).unwrap(),
            Some([Point2::new(0.5, 0.), Point2::new(0.75, 0.)]),
            "both original reflection anchors survive the narrower outer restriction",
        );
        assert_eq!(
            exact_affine_endpoints(&outer, &mut GeometryWorkBudget::new(0), &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::ResourceLimit,
        );
        assert_eq!(
            exact_affine_endpoints(&outer, &mut work, &AtomicBool::new(true))
                .unwrap_err()
                .code,
            GeometryErrorCode::Cancelled,
        );
    }

    #[test]
    fn original_planar_line_keeps_full_posed_restriction_and_refuses_wrong_interior() {
        use monstertruck_io::step::load::step_geometry::{ElementarySurface, SourceTrimmedCurve};
        let plane = Plane::from_origin_vectors(
            Point3::new(5.0, 0.0, 2.0),
            -Vector3::unit_x(),
            Vector3::unit_y(),
        );
        let original = StepSurface::ElementarySurface(ElementarySurface::Plane(plane));
        let (surface, _) = original.to_modeling_with_parameter_map().unwrap();
        let source =
            Curve3D::TrimmedCurve(SourceTrimmedCurve::internal(Processor::with_transform(
                TrimmedCurve::new(
                    Box::new(Curve3D::Line(Line(
                        Point3::origin(),
                        Point3::new(-3.06161699785e-16, -5.0, 0.0),
                    ))),
                    (-2.0, 3.0),
                ),
                Matrix4::from_translation(Vector3::new(3.0616169979e-16, 5.0, 2.0)),
            )));
        let (native, map) = source.to_modeling_with_parameter_map().unwrap();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(2_000_000);
        let witness = source_associated_planar_chart(
            &source, &original, &native, &surface, 1.0, &mut work, &cancel,
        )
        .unwrap()
        .expect("complete original affine LINE projection");
        assert!(witness.certificate.is_certified() && !witness.declared_source);
        assert!(witness.error_mm < 1e-7);
        assert!(
            witness
                .margin_uv
                .into_iter()
                .all(|margin| margin.is_finite())
        );
        assert_eq!(source.range_tuple(), (-2.0, 3.0));
        assert_eq!(native.range_tuple(), map.native_range);
        let (a, b) = native.range_tuple();
        let start = native.subs(a);
        let end = native.subs(b);
        let rounded_chord = Curve::Line(Line(start, end));
        assert!(
            source_associated_planar_chart(
                &source,
                &original,
                &rounded_chord,
                &surface,
                1.0,
                &mut work,
                &cancel,
            )
            .unwrap()
            .is_none(),
            "rounded source endpoint chords are not symbolic source identity",
        );
        let wrong = Curve::BsplineCurve(monstertruck_modeling::BsplineCurve::new(
            monstertruck_modeling::KnotVector::bezier_knot(2),
            vec![
                start,
                start + (end - start) * 0.5 + 0.001 * Vector3::unit_z(),
                end,
            ],
        ));
        assert!(
            source_associated_planar_chart(
                &source, &original, &wrong, &surface, 1.0, &mut work, &cancel,
            )
            .unwrap()
            .is_none(),
            "matching original endpoints cannot replace whole interior proof",
        );
    }

    #[test]
    fn original_affine_tensor_conic_keeps_coefficients_gauges_and_true_inverse_tube() {
        use monstertruck_io::step::load::step_geometry::{Conic3D, SourceTrimmedCurve};
        use monstertruck_modeling::{BsplineSurface, KnotVector, NurbsSurface};
        let tensor = |lower: f64, upper: f64, defect: f64| {
            let knots = KnotVector::from(vec![0., 0., 1., 1.]);
            Surface::NurbsSurface(NurbsSurface::new(BsplineSurface::new(
                (knots.clone(), knots),
                vec![
                    vec![
                        Vector4::new(lower, lower, 4., 1.),
                        Vector4::new(lower, upper, 4., 1.),
                    ],
                    vec![
                        Vector4::new(upper, lower, 4., 1.),
                        Vector4::new(upper, upper, 4. + defect, 1.),
                    ],
                ],
            )))
        };
        let original = |surface: &Surface| {
            let Surface::NurbsSurface(surface) = surface else {
                unreachable!()
            };
            StepSurface::NurbsSurface(surface.clone())
        };
        let surface = tensor(6., 14., 0.);
        let source_surface = original(&surface);
        let cancel = AtomicBool::new(false);
        let tau = std::f64::consts::TAU;
        for reflected in [false, true] {
            let mut conic = Processor::with_transform(
                TrimmedCurve::new(
                    UnitCircle::<Point3>::new(),
                    (tau, 3. * std::f64::consts::PI),
                ),
                Matrix4::from_cols(
                    Vector4::new(-3.0000000000000018, 1.776356839410001e-15, 0., 0.),
                    Vector4::new(-1.776356839410001e-15, -3.0000000000000018, 0., 0.),
                    Vector4::unit_z(),
                    Vector4::new(10., 10., 4., 1.),
                ),
            );
            if reflected {
                conic.invert();
            }
            let source = Curve3D::TrimmedCurve(SourceTrimmedCurve::internal(
                Processor::<_, Matrix4>::new(TrimmedCurve::new(
                    Box::new(Curve3D::Conic(Conic3D::Ellipse(conic))),
                    (tau + 0.25, 3. * std::f64::consts::PI - 0.25),
                )),
            ));
            let native = source.to_parameter_preserving_modeling().unwrap();
            let mut work = GeometryWorkBudget::new(2_000_000);
            let witness = source_associated_planar_chart(
                &source,
                &source_surface,
                &native,
                &surface,
                1.,
                &mut work,
                &cancel,
            )
            .unwrap()
            .expect("true original affine frame and whole conic correspondence");
            assert!(witness.certificate.is_certified() && !witness.declared_source);
            assert!(witness.error_mm > 0. && witness.error_mm < 1e-7);
            assert!(
                witness
                    .margin_uv
                    .into_iter()
                    .all(|margin| margin > 0. && margin.is_finite())
            );
            let chart = witness.curve.unwrap();
            assert_eq!(chart.range_tuple(), source.range_tuple());
            let wrong_carrier = tensor(6., 15., 0.);
            assert!(
                source_associated_planar_chart(
                    &source,
                    &source_surface,
                    &native,
                    &wrong_carrier,
                    1.,
                    &mut work,
                    &cancel,
                )
                .unwrap()
                .is_none(),
                "physical coplanarity does not replace original carrier coefficient identity",
            );
            let nonaffine = tensor(6., 14., 0.125);
            assert!(
                source_associated_planar_chart(
                    &source,
                    &original(&nonaffine),
                    &native,
                    &nonaffine,
                    1.,
                    &mut work,
                    &cancel,
                )
                .unwrap()
                .is_none(),
                "one altered original coefficient is not an affine frame",
            );
            let unsupported = tensor(7., 13., 0.);
            assert!(
                source_associated_planar_chart(
                    &source,
                    &original(&unsupported),
                    &native,
                    &unsupported,
                    1.,
                    &mut work,
                    &cancel,
                )
                .unwrap()
                .is_none(),
                "a true inverse outside finite original support cannot be inward-rounded into success",
            );
            let mut wrong_phase = Processor::<_, Matrix4>::new(TrimmedCurve::new(
                Box::new(native.clone()),
                source.range_tuple(),
            ));
            wrong_phase.invert();
            let wrong_phase = Curve::TrimmedCurve(wrong_phase);
            assert!(
                source_associated_planar_chart(
                    &source,
                    &source_surface,
                    &wrong_phase,
                    &surface,
                    1.,
                    &mut work,
                    &cancel,
                )
                .unwrap()
                .is_none(),
                "the original spatial expression and actual reflection anchors are mandatory",
            );
            assert!(
                derived_chart_witness(&wrong_phase, &surface, chart, 1., &mut work, &cancel)
                    .unwrap()
                    .is_none(),
                "independent whole relative-tangent proof rejects the wrong phase/orientation",
            );
        }
    }

    #[test]
    fn original_planar_ellipse_keeps_reflection_anchor_and_rejects_microscopic_wrong_winding() {
        use monstertruck_io::step::load::step_geometry::{Conic3D, ElementarySurface};
        let original =
            StepSurface::ElementarySurface(ElementarySurface::Plane(Plane::from_origin_vectors(
                Point3::new(10.0, 10.0, 4.0),
                Vector3::unit_x(),
                Vector3::unit_y(),
            )));
        let (surface, _) = original.to_modeling_with_parameter_map().unwrap();
        let ellipse = |range, radius, reversed| {
            let mut curve = Processor::with_transform(
                TrimmedCurve::new(UnitCircle::<Point3>::new(), range),
                Matrix4::from_translation(Vector3::new(10.0, 10.0, 4.0))
                    * Matrix4::from_nonuniform_scale(radius, radius * 0.75, radius),
            );
            if reversed {
                curve.invert();
            }
            Curve3D::Conic(Conic3D::Ellipse(curve))
        };
        let source = ellipse((7.0, 8.0), 3.0, true);
        let native = source.to_parameter_preserving_modeling().unwrap();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(4_000_000);
        let witness = source_associated_planar_chart(
            &source, &original, &native, &surface, 1.0, &mut work, &cancel,
        )
        .unwrap()
        .expect("complete original ellipse outside a nominal one-period window");
        assert!(witness.certificate.is_certified() && !witness.declared_source);
        assert_eq!(witness.curve.unwrap().range_tuple(), (7.0, 8.0));
        let tau = std::f64::consts::TAU;
        let source = ellipse((0.0, tau), 1e-12, false);
        let native = source.to_parameter_preserving_modeling().unwrap();
        let original_chart = source_associated_planar_chart(
            &source, &original, &native, &surface, 1.0, &mut work, &cancel,
        )
        .unwrap()
        .unwrap()
        .curve
        .unwrap();
        let wrong = ellipse((0.0, 3.0 * tau), 1e-12, false)
            .to_parameter_preserving_modeling()
            .unwrap();
        assert!(
            source_associated_planar_chart(
                &source, &original, &wrong, &surface, 1.0, &mut work, &cancel,
            )
            .unwrap()
            .is_none(),
            "absolute position tolerance cannot authorize a different winding",
        );
        assert!(
            derived_chart_witness(&wrong, &surface, original_chart, 1.0, &mut work, &cancel,)
                .unwrap()
                .is_none(),
            "whole relative-tangent proof independently rejects microscopic wrong winding",
        );
    }

    #[test]
    fn missing_generator_uses_certified_rim_at_identical_original_vertex_only() {
        use monstertruck_io::step::load::step_geometry::Conic3D;
        let circle = |end| {
            Curve3D::Conic(Conic3D::Ellipse(Processor::with_transform(
                TrimmedCurve::new(UnitCircle::<Point3>::new(), (0.0, end)),
                Matrix4::from_scale(2.0),
            )))
        };
        let original = StepSurface::SweepSurface(SweepSurface::ExtrusionSurface(
            StepExtrusionSurface::by_extrusion(
                Box::new(circle(std::f64::consts::TAU)),
                Vector3::new(0.0, 0.0, 8.0),
            ),
        ));
        let (surface, map) = original.to_modeling_with_parameter_map().unwrap();
        let rim_source = circle(std::f64::consts::PI);
        let (rim, _) = rim_source.to_modeling_with_parameter_map().unwrap();
        let cancel = AtomicBool::new(false);
        let repair = |shared_vertex, dx| {
            let mut work = GeometryWorkBudget::new(2_000_000);
            let chart = super::super::harmonic::extrusion_profile_chart_proposal(
                &rim,
                &surface,
                extrusion_profile_domain(map).unwrap(),
                &mut work,
                &cancel,
            )
            .unwrap()
            .unwrap();
            let rim_witness = derived_chart_witness(&rim, &surface, chart, 1.0, &mut work, &cancel)
                .unwrap()
                .unwrap();
            let source = Curve3D::Line(Line(
                Point3::new(-2.0 + dx, 0.0, 0.0),
                Point3::new(-2.0 + dx, 0.0, 8.0),
            ));
            let (generator, _) = source.to_modeling_with_parameter_map().unwrap();
            let context = |index| PeriodicUse {
                face: 0,
                wire: 0,
                coedge: index,
                source_edge: Some(index as u64),
                native_edge: index,
                source: if index == 0 { &source } else { &rim_source },
                native: if index == 0 { &generator } else { &rim },
                declared: None,
                retained: None,
                orientation: true,
                vertices: if index == 0 {
                    (shared_vertex, 3)
                } else {
                    (1, 2)
                },
                front_vertex: if index == 0 {
                    Point3::new(-2.0 + dx, 0.0, 0.0)
                } else {
                    Point3::new(2.0, 0.0, 0.0)
                },
            };
            let mut witnesses = [SourceTrim::retained(None), rim_witness];
            repair_source_associated_generators(
                &mut witnesses,
                &context,
                &surface,
                map,
                1.0,
                &mut work,
                &cancel,
            )
            .unwrap();
            witnesses[0].certificate.is_certified()
        };
        assert!(repair(2, 0.0));
        assert!(
            !repair(99, 0.0),
            "physical nearness cannot replace original vertex identity"
        );
        assert!(
            !repair(2, 0.001),
            "shared identity cannot replace whole physical correspondence"
        );
    }

    #[test]
    fn source_associated_extrusion_requires_whole_profile_not_matching_endpoints() {
        use monstertruck_modeling::{BsplineCurve, ExtrusionSurface, KnotVector};
        let points = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 2.0, 0.0),
            Point3::new(3.0, 0.0, 0.0),
        ];
        let profile = BsplineCurve::new(KnotVector::bezier_knot(2), points.clone());
        let direction = Vector3::new(0.0, 0.0, 8.0);
        let original = StepSurface::SweepSurface(SweepSurface::ExtrusionSurface(
            StepExtrusionSurface::by_extrusion(
                Box::new(Curve3D::BsplineCurve(profile.clone())),
                direction,
            ),
        ));
        let native = Surface::ExtrusionSurface(ExtrusionSurface::by_extrusion(
            Curve::BsplineCurve(profile.clone()),
            direction,
        ));
        let translation = Matrix4::from_translation(Vector3::new(0.0, 0.0, 4.0));
        let edge = Curve::BsplineCurve(profile).transformed(translation);
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(2_000_000);
        let witness =
            source_associated_extrusion_chart(&original, &edge, &native, 1.0, &mut work, &cancel)
                .unwrap()
                .expect("original complete translated profile");
        assert!(witness.certificate.is_certified() && !witness.declared_source);
        assert_eq!(witness.curve.unwrap().range_tuple(), edge.range_tuple());
        let mut wrong = points;
        wrong[1].y += 0.001;
        let wrong = Curve::BsplineCurve(BsplineCurve::new(KnotVector::bezier_knot(2), wrong))
            .transformed(translation);
        assert!(
            source_associated_extrusion_chart(&original, &wrong, &native, 1.0, &mut work, &cancel,)
                .unwrap()
                .is_none(),
            "matching original rim endpoints do not prove its interior"
        );
    }

    #[test]
    fn bounded_original_generator_uses_complete_forward_proof_at_nonalgebraic_endpoint() {
        use monstertruck_io::step::load::step_geometry::Conic3D;
        let direction = Vector3::new(0.0, 0.0, 8.0);
        let original = |radius| {
            StepSurface::SweepSurface(SweepSurface::ExtrusionSurface(
                StepExtrusionSurface::by_extrusion(
                    Box::new(Curve3D::Conic(Conic3D::Ellipse(Processor::with_transform(
                        TrimmedCurve::new(UnitCircle::<Point3>::new(), (0.0, std::f64::consts::PI)),
                        Matrix4::from_scale(radius),
                    )))),
                    direction,
                ),
            ))
        };
        let source = original(2.0);
        let (surface, _) = source.to_modeling_with_parameter_map().unwrap();
        let edge = |dy, dx| {
            Curve::Line(Line(
                Point3::new(-2.0, dy, 0.0),
                Point3::new(-2.0 + dx, dy, 8.0),
            ))
        };
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(2_000_000);
        let exact_source_line = edge(0.0, 0.0);
        let proof = source_associated_extrusion_chart(
            &source,
            &exact_source_line,
            &surface,
            1.0,
            &mut work,
            &cancel,
        )
        .unwrap()
        .expect("complete generator at actual bounded pi endpoint");
        assert!(proof.certificate.is_certified() && !proof.declared_source);
        assert!(proof.error_mm < 1.0e-7);
        assert_eq!(
            proof.curve.unwrap().range_tuple(),
            exact_source_line.range_tuple()
        );
        for wrong in [edge(0.001, 0.0), edge(0.0, 0.1)] {
            assert!(
                source_associated_extrusion_chart(
                    &source, &wrong, &surface, 1.0, &mut work, &cancel,
                )
                .unwrap()
                .is_none(),
                "wrong original generator offset/direction must refuse"
            );
        }
        let wrong_source = original(3.0);
        let (wrong_surface, _) = wrong_source.to_modeling_with_parameter_map().unwrap();
        assert!(
            source_associated_extrusion_chart(
                &wrong_source,
                &exact_source_line,
                &wrong_surface,
                1.0,
                &mut work,
                &cancel,
            )
            .unwrap()
            .is_none(),
            "wrong original profile must refuse"
        );
    }

    #[test]
    fn periodic_neighbor_requires_whole_native_singular_joint_not_uv_proximity() {
        use crate::parametric::Interval;
        let sphere = Surface::SphericalSurface(Processor::new(monstertruck_modeling::Sphere::new(
            Point3::new(2.0, 2.0, 2.0),
            2.0,
        )));
        let pi = std::f64::consts::PI;
        let south = Interval {
            lo: pi - 2.0e-12,
            hi: pi + 2.0e-12,
        };
        let incoming = [south, Interval::point(0.0)];
        let outgoing = [south, Interval::point(pi * 0.5)];
        let vertex = Point3::new(2.0, 2.0, 0.0);
        let joint = |a, b, vertex| {
            parameter_joint_incident(
                a,
                b,
                &sphere,
                vertex,
                1.0,
                &mut GeometryWorkBudget::new(1_000_000),
                &AtomicBool::new(false),
            )
            .unwrap()
        };
        // Original meridians have different longitudes at one shared pole.
        assert!(joint(incoming, outgoing, vertex));
        // A large regular-latitude gap exceeds the whole original vertex tube.
        assert!(!joint(
            [Interval::point(pi * 0.5), incoming[1]],
            [Interval::point(pi * 0.5), outgoing[1]],
            vertex,
        ));
        assert!(!joint(incoming, outgoing, Point3::new(2.0, 2.0, 1.0),));
        // A periodic alias must still pass ordinary source branch rebinding.
        assert!(!joint(incoming, [south, Interval::point(2.0 * pi)], vertex,));
        // Endpoint inclusion of a pole is insufficient: the entire connector
        // tube must fit the original physical correspondence tolerance.
        let broad = Interval {
            lo: pi - 1.0e-3,
            hi: pi + 1.0e-3,
        };
        assert!(!joint([broad, incoming[1]], [broad, outgoing[1]], vertex,));
    }

    #[test]
    fn ordinary_parameter_joint_requires_original_vertex_and_whole_image_budget() {
        use crate::parametric::Interval;
        let pi = std::f64::consts::PI;
        let placement = Matrix4::from_translation(Vector3::new(2.0, 2.0, 2.0))
            * Matrix4::from_nonuniform_scale(-1.0, -1.0, 1.0);
        let sphere = Surface::SphericalSurface(Processor::with_transform(
            monstertruck_modeling::Sphere::new(Point3::origin(), 2.0),
            placement,
        ));
        // Actual independently exported equator/meridian endpoint intervals.
        // Their longitude boxes are disjoint, but the whole connector image is
        // bounded against their same original source vertex.
        let source_angle: f64 = "1.570796326795".parse().unwrap();
        let incoming = [Interval::point(pi * 0.5), Interval::point(source_angle)];
        let outgoing = [
            Interval {
                lo: 1.5707963267948957,
                hi: 1.5707963267948974,
            },
            Interval {
                lo: 1.5707963267948921,
                hi: 1.570796326794901,
            },
        ];
        let vertex = Point3::new(2.0, -4.440892098501e-16, 2.0);
        assert!(!incident(incoming, outgoing));
        let endpoints = [incoming, outgoing].map(|point| {
            point.map(|axis| monstertruck_modeling::ValidatedInterval {
                lower: axis.lo,
                upper: axis.hi,
            })
        });
        let proof = monstertruck_modeling::certified_surface_parameter_joint(
            &sphere,
            vertex,
            endpoints,
            [0.0; 2],
            1.0e-7,
            &mut ValidatedGeometryBudget::new(1_000_000),
            &AtomicBool::new(false),
        )
        .unwrap()
        .unwrap();
        assert!(matches!(
            proof,
            monstertruck_modeling::TrimJointKind::EndpointUncertainty
        ));
        let joint = |surface: &Surface, a, b, vertex| {
            parameter_joint_incident(
                a,
                b,
                surface,
                vertex,
                1.0,
                &mut GeometryWorkBudget::new(1_000_000),
                &AtomicBool::new(false),
            )
            .unwrap()
        };
        assert!(joint(&sphere, incoming, outgoing, vertex));
        assert!(!joint(
            &sphere,
            incoming,
            outgoing,
            Point3::new(vertex.x, vertex.y, vertex.z + 1.0e-3),
        ));
        assert!(!joint(
            &sphere,
            incoming,
            [incoming[0], Interval::point(incoming[1].mid() + 2.0 * pi)],
            vertex,
        ));
        let plane = Surface::Plane(monstertruck_modeling::Plane::xy());
        let origin = [Interval::point(0.0); 2];
        assert!(!joint(
            &plane,
            origin,
            [Interval::point(1.0e-3), Interval::point(0.0)],
            Point3::origin(),
        ));
        // A per-coordinate allowance cannot replace the Euclidean budget.
        assert!(!joint(
            &plane,
            origin,
            [Interval::point(0.8e-7); 2],
            Point3::origin(),
        ));
        let mut exhausted = GeometryWorkBudget::new(0);
        assert_eq!(
            parameter_joint_incident(
                incoming,
                outgoing,
                &sphere,
                vertex,
                1.0,
                &mut exhausted,
                &AtomicBool::new(false),
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::ResourceLimit,
        );
        assert_eq!(
            parameter_joint_incident(
                incoming,
                outgoing,
                &sphere,
                vertex,
                1.0,
                &mut GeometryWorkBudget::new(1_000_000),
                &AtomicBool::new(true),
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::Cancelled,
        );
    }

    fn physical_raw_line_sweep() -> StepSurface {
        // The physical STEP LINE has magnitude two and reversed selectors.
        // Its canonical raw range -8..-4 differs from decoded profile UV 4..8.
        let data = DataSection::from_str(
            "DATA;
            #1=SURFACE_OF_LINEAR_EXTRUSION('',#2,#7);
            #2=TRIMMED_CURVE('',#3,(PARAMETER_VALUE(8.)),(PARAMETER_VALUE(4.)),.T.,.PARAMETER.);
            #3=LINE('',#4,#5);
            #4=CARTESIAN_POINT('',(0.,0.,0.));
            #5=VECTOR('',#6,2.);
            #6=DIRECTION('',(1.,0.,0.));
            #7=VECTOR('',#8,1.);
            #8=DIRECTION('',(0.,0.,1.));
            ENDSEC;",
        )
        .unwrap();
        let table = Table::from_data_section(&data);
        let entity = EntityTable::<SurfaceOfLinearExtrusionHolder>::get_owned(&table, 1).unwrap();
        StepSurface::SweepSurface(SweepSurface::ExtrusionSurface(
            StepExtrusionSurface::try_from(&entity).unwrap(),
        ))
    }

    #[test]
    fn literal_raw_line_profile_chart_cannot_be_dropped_by_exact_lane() {
        let surface = physical_raw_line_sweep();
        let raw = StepCurve2D::Line(Line(Point2::new(-7., 0.), Point2::new(-5., 1.)));
        let composed =
            StepParameterCurve::try_from_raw(Box::new(raw), Box::new(surface.clone())).unwrap();
        let chart = composed.input_parameter_chart();
        assert_ne!(
            chart,
            monstertruck_io::step::SurfaceParameterChart::identity()
        );
        assert_eq!(chart.axes[0].offset, 12.);
        assert!(
            literal_chart(&composed, &surface, SurfaceParameterMap::Identity)
                .unwrap()
                .is_none()
        );
        // Native cached-chart evaluation, not stored UV evaluated as decoded
        // UV, remains the geometry authority.
        for t in [0., 0.5, 1.] {
            let stored = composed.curve().subs(t);
            let decoded = chart.nominal([stored.x, stored.y]).unwrap();
            let actual = composed.subs(t);
            let expected = surface.subs(decoded[0], decoded[1]);
            assert_eq!(actual, expected);
            assert_eq!(actual, Point3::new(14. - 4. * t, 0., t));
            assert_ne!(actual, surface.subs(stored.x, stored.y));
        }
    }

    #[test]
    fn literal_decoded_identity_keeps_exact_coefficients_on_raw_scaled_carrier() {
        let surface = physical_raw_line_sweep();
        let decoded = StepCurve2D::Line(Line(Point2::new(5., 0.), Point2::new(7., 1.)));
        let composed = StepParameterCurve::new(Box::new(decoded), Box::new(surface.clone()));
        assert_eq!(
            composed.input_parameter_chart(),
            monstertruck_io::step::SurfaceParameterChart::identity()
        );
        let exact = literal_chart(&composed, &surface, SurfaceParameterMap::Identity)
            .unwrap()
            .unwrap();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(10_000);
        let expected = Curve2D::Line(Line(Point2::new(5., 0.), Point2::new(7., 1.)));
        assert!(same_coefficients(&exact, &expected, &mut work, &cancel).unwrap());
        for t in [0., 0.5, 1.] {
            let uv = exact.subs(t);
            assert_eq!(composed.subs(t), surface.subs(uv.x, uv.y));
        }
    }

    #[test]
    fn nominal_identity_with_nonzero_chart_enclosure_is_not_exact() {
        let surface = physical_raw_line_sweep();
        let mut chart = monstertruck_io::step::SurfaceParameterChart::identity();
        chart.axes[0].offset_enclosure =
            monstertruck_io::step::ParameterEnclosure::new(-1e-12, 1e-12).unwrap();
        let composed = StepParameterCurve::try_from_parts(
            Box::new(StepCurve2D::Line(Line(
                Point2::new(5., 0.),
                Point2::new(7., 1.),
            ))),
            Box::new(surface.clone()),
            chart,
        )
        .unwrap();
        assert!(
            literal_chart(&composed, &surface, SurfaceParameterMap::Identity)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn original_unbounded_uv_line_uses_proved_active_length_not_unit_span() {
        let plane = monstertruck_modeling::Plane::new(
            Point3::origin(),
            Point3::new(1., 0., 0.),
            Point3::new(0., 1., 0.),
        );
        let source_surface = StepSurface::ElementarySurface(
            monstertruck_io::step::load::step_geometry::ElementarySurface::Plane(plane),
        );
        let surface = Surface::Plane(plane);
        let source = Curve3D::Line(Line(Point3::origin(), Point3::new(20., 0., 0.)));
        let curve = Curve::Line(Line(Point3::origin(), Point3::new(20., 0., 0.)));
        let original = StepCurve2D::Line(Line(Point2::origin(), Point2::new(1., 0.)));
        let proof = retain(
            TrimInput {
                source_curve: &source,
                native_curve: &curve,
                surface: SurfaceInput {
                    source_surface: &source_surface,
                    native_surface: &surface,
                    map: SurfaceParameterMap::Identity,
                    scale: 1.,
                },
                declared: Some(&original),
            },
            [0.0; 2],
            &mut GeometryWorkBudget::new(1_000_000),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(proof.certificate.is_certified() && proof.declared_source);
        let chart = proof.curve.unwrap();
        assert_eq!(chart.range_tuple(), (0., 20.));
        let Curve2D::TrimmedCurve(retained) = chart else {
            panic!("original line expression lost");
        };
        assert!(matches!(
            retained.entity().curve().as_ref(),
            Curve2D::Line(_)
        ));
        assert_eq!(retained.transform(), &Matrix3::identity());
    }

    #[test]
    fn periodic_line_binding_distinguishes_generated_restrictions_from_source_trims() {
        use monstertruck_io::step::load::{CurveAnyHolder, step_geometry::SourceTrimmedCurve};
        let data = DataSection::from_str(
            "DATA;
            #1=CARTESIAN_POINT('',(0.,0.));
            #2=DIRECTION('',(7.,0.));
            #3=VECTOR('',#2,2.);
            #4=LINE('',#1,#3);
            #5=TRIMMED_CURVE('',#4,(PARAMETER_VALUE(2.)),(PARAMETER_VALUE(5.)),.T.,.PARAMETER.);
            ENDSEC;",
        )
        .unwrap();
        let table = Table::from_data_section(&data);
        let entity = EntityTable::<CurveAnyHolder>::get_owned(&table, 5).unwrap();
        let declared = StepCurve2D::try_from(&entity).unwrap();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(10_000);
        assert!(
            original_unbounded_line(&declared, &mut work, &cancel)
                .unwrap()
                .is_none()
        );
        let chart = mapped_declaration(
            Curve2D::Line(Line(Point2::origin(), Point2::new(2., 0.))),
            (2., 5.),
            Matrix3::identity(),
            true,
        );
        assert!(
            periodic_lift_proposal(
                Some(&declared),
                &chart,
                [0., std::f64::consts::TAU],
                &mut work,
                &cancel,
            )
            .unwrap()
            .is_none(),
            "a literal bounded declaration cannot acquire a periodic lift",
        );
        let raw = Line(Point2::origin(), Point2::new(2., 0.));
        let generated = StepCurve2D::TrimmedCurve(SourceTrimmedCurve::internal(Processor::new(
            TrimmedCurve::new(Box::new(StepCurve2D::Line(raw)), (2., 5.)),
        )));
        assert_eq!(
            original_unbounded_line(&generated, &mut work, &cancel).unwrap(),
            Some(raw)
        );
        let outer = StepCurve2D::TrimmedCurve(SourceTrimmedCurve::internal(Processor::new(
            TrimmedCurve::new(Box::new(declared), (2., 3.)),
        )));
        assert!(
            original_unbounded_line(&outer, &mut work, &cancel)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn periodic_lift_keeps_fixed_source_coefficients_and_reflected_active_interval() {
        let tau = std::f64::consts::TAU;
        let raw = Line(Point2::new(tau, 0.), Point2::new(tau, 1.));
        let declaration = StepCurve2D::Line(raw);
        let original = mapped_declaration(
            Curve2D::Line(raw),
            (2., 5.),
            Matrix3::from_translation(Vector2::new(tau, 0.)),
            false,
        );
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(10_000);
        let proposal = periodic_lift_proposal(
            Some(&declaration),
            &original,
            [-4. * tau, 0.],
            &mut work,
            &cancel,
        )
        .unwrap()
        .unwrap();
        assert_eq!(proposal.range_tuple(), (2., 5.));
        assert_eq!(proposal.subs(2.), Point2::new(-2. * tau, 5.));
        assert_eq!(proposal.subs(5.), Point2::new(-2. * tau, 2.));
        let Curve2D::TrimmedCurve(covering) = proposal else {
            panic!("covering translation remains an explicit outer operator")
        };
        assert!(covering.orientation());
        assert!(
            same_coefficients(covering.entity().curve(), &original, &mut work, &cancel).unwrap(),
            "the complete original raw map, LINE and reflection anchor stay unchanged",
        );
        assert_eq!(
            periodic_lift_proposal(
                Some(&declaration),
                &original,
                [-4. * tau, 0.],
                &mut GeometryWorkBudget::new(0),
                &cancel,
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::ResourceLimit,
        );
        assert_eq!(
            periodic_lift_proposal(
                Some(&declaration),
                &original,
                [-4. * tau, 0.],
                &mut work,
                &AtomicBool::new(true),
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::Cancelled,
        );
    }

    #[test]
    fn wrong_periodic_winding_proposal_fails_whole_forward_correspondence() {
        use monstertruck_io::step::load::step_geometry::Conic3D;
        use monstertruck_modeling::{Processor, RevolutionSurface, TrimmedCurve, UnitCircle};
        let tau = std::f64::consts::TAU;
        for radius in [2., 1e-12] {
            let surface =
                Surface::RevolutionSurface(Processor::new(RevolutionSurface::by_revolution(
                    Curve::Line(Line(
                        Point3::new(radius, 0., 0.),
                        Point3::new(radius, 0., 1.),
                    )),
                    Point3::origin(),
                    Vector3::unit_z(),
                )));
            let circle = Curve3D::Conic(Conic3D::Ellipse(Processor::with_transform(
                TrimmedCurve::new(UnitCircle::<Point3>::new(), (0., tau)),
                Matrix4::from_scale(radius),
            )))
            .to_parameter_preserving_modeling()
            .unwrap();
            let raw = Line(Point2::new(0., 0.), Point2::new(0., 1.));
            // At microscopic radius even every wrong position fits the absolute
            // tolerance. Relative tangent proof must still reject both aliases.
            for winding in [-1., 2.] {
                let wrong =
                    Curve2D::Line(Line(Point2::new(0., 0.), Point2::new(0., winding * tau)));
                assert!(
                    prove_line_branch(
                        raw,
                        Matrix3::identity(),
                        &wrong,
                        NativeTrimGeometry {
                            curve: &circle,
                            surface: &surface,
                            scale: 1.,
                            domain: None,
                        },
                        [0.; 2],
                        &mut GeometryWorkBudget::new(2_000_000),
                        &AtomicBool::new(false)
                    )
                    .unwrap()
                    .is_none()
                );
            }
            let right = Curve2D::Line(Line(Point2::new(0., 0.), Point2::new(0., tau)));
            let proof = prove_line_branch(
                raw,
                Matrix3::identity(),
                &right,
                NativeTrimGeometry {
                    curve: &circle,
                    surface: &surface,
                    scale: 1.,
                    domain: None,
                },
                [0.; 2],
                &mut GeometryWorkBudget::new(2_000_000),
                &AtomicBool::new(false),
            )
            .unwrap()
            .unwrap();
            assert_eq!(proof.curve.unwrap().range_tuple(), (0., tau));
        }
    }

    #[test]
    fn full_circle_activates_original_unit_angular_line_but_bounded_line_is_not_widened() {
        use monstertruck_io::step::load::step_geometry::{Conic3D, ElementarySurface};
        use monstertruck_modeling::{Processor, RevolutionSurface, TrimmedCurve, UnitCircle};
        let tau = std::f64::consts::TAU;
        let profile = Line(Point3::new(2., 0., 0.), Point3::new(2., 0., 1.));
        let source_surface =
            StepSurface::ElementarySurface(ElementarySurface::CylindricalSurface(Processor::new(
                RevolutionSurface::by_revolution(profile, Point3::origin(), Vector3::unit_z()),
            )));
        let surface = Surface::RevolutionSurface(Processor::new(RevolutionSurface::by_revolution(
            Curve::Line(profile),
            Point3::origin(),
            Vector3::unit_z(),
        )));
        let arc = Processor::with_transform(
            TrimmedCurve::new(UnitCircle::<Point3>::new(), (0., tau)),
            Matrix4::from_scale(2.),
        );
        let source = Curve3D::Conic(Conic3D::Ellipse(arc));
        let curve = source.to_parameter_preserving_modeling().unwrap();
        let raw = Line(Point2::new(0., 0.), Point2::new(0., 1.));
        let proof = retain(
            TrimInput {
                source_curve: &source,
                native_curve: &curve,
                surface: SurfaceInput {
                    source_surface: &source_surface,
                    native_surface: &surface,
                    map: SurfaceParameterMap::Identity,
                    scale: 1.,
                },
                declared: Some(&StepCurve2D::Line(raw)),
            },
            [0.0; 2],
            &mut GeometryWorkBudget::new(2_000_000),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(proof.certificate.is_certified() && proof.declared_source);
        assert_eq!(proof.curve.unwrap().range_tuple(), (0., tau));
        let bounded = StepCurve2D::TrimmedCurve(
            monstertruck_io::step::load::step_geometry::SourceTrimmedCurve::internal(Processor::<
                _,
                Matrix3,
            >::new(
                TrimmedCurve::new(Box::new(StepCurve2D::Line(raw)), (0., 1.)),
            )),
        );
        let proof = retain(
            TrimInput {
                source_curve: &source,
                native_curve: &curve,
                surface: SurfaceInput {
                    source_surface: &source_surface,
                    native_surface: &surface,
                    map: SurfaceParameterMap::Identity,
                    scale: 1.,
                },
                declared: Some(&bounded),
            },
            [0.0; 2],
            &mut GeometryWorkBudget::new(2_000_000),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(!proof.certificate.is_certified());
        assert_eq!(proof.curve.unwrap().range_tuple(), (0., 1.));
    }

    #[test]
    fn full_native_nurbs_circle_uses_original_line_selected_boundary_span() {
        use monstertruck_modeling::{
            BsplineCurve, BsplineSurface, KnotVector, NurbsCurve, NurbsSurface,
        };
        let w = std::f64::consts::FRAC_1_SQRT_2;
        let uv = [
            (2., 0., 1.),
            (2., 2., w),
            (0., 2., 1.),
            (-2., 2., w),
            (-2., 0., 1.),
            (-2., -2., w),
            (0., -2., 1.),
            (2., -2., w),
            (2., 0., 1.),
        ];
        let points: Vec<Vector4> = uv
            .into_iter()
            .map(|(x, y, w)| Vector4::new(x * w, y * w, 0., w))
            .collect();
        let knots = KnotVector::from(vec![0., 0., 0., 1., 1., 2., 2., 3., 3., 4., 4., 4.]);
        let circle = NurbsCurve::new(BsplineCurve::new(knots.clone(), points.clone()));
        let carrier = NurbsSurface::new(BsplineSurface::new(
            (knots, KnotVector::from(vec![0., 0., 1., 1.])),
            points
                .into_iter()
                .map(|p| vec![p, Vector4::new(p.x, p.y, p.w, p.w)])
                .collect(),
        ));
        let source_curve = Curve3D::NurbsCurve(circle.clone());
        let native_curve = Curve::NurbsCurve(circle);
        let source_surface = StepSurface::NurbsSurface(carrier.clone());
        let native_surface = Surface::NurbsSurface(carrier);
        let proof = retain(
            TrimInput {
                source_curve: &source_curve,
                native_curve: &native_curve,
                surface: SurfaceInput {
                    source_surface: &source_surface,
                    native_surface: &native_surface,
                    map: SurfaceParameterMap::Identity,
                    scale: 1.,
                },
                declared: Some(&StepCurve2D::Line(Line(
                    Point2::new(0., 0.),
                    Point2::new(1., 0.),
                ))),
            },
            [0.0; 2],
            &mut GeometryWorkBudget::new(4_000_000),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(proof.certificate.is_certified() && proof.declared_source);
        assert_eq!(proof.curve.unwrap().range_tuple(), (0., 4.));
        let wrong = StepCurve2D::Line(Line(Point2::new(0., 1.), Point2::new(1., 1.)));
        let refused = retain(
            TrimInput {
                source_curve: &source_curve,
                native_curve: &native_curve,
                surface: SurfaceInput {
                    source_surface: &source_surface,
                    native_surface: &native_surface,
                    map: SurfaceParameterMap::Identity,
                    scale: 1.,
                },
                declared: Some(&wrong),
            },
            [0.; 2],
            &mut GeometryWorkBudget::new(4_000_000),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(
            !refused.certificate.is_certified(),
            "a different original tensor row is not the curve"
        );

        let mut original =
            Processor::<_, Matrix4>::new(TrimmedCurve::new(Box::new(source_curve), (0., 4.)));
        original.invert();
        let reflected_source = Curve3D::TrimmedCurve(
            monstertruck_io::step::load::step_geometry::SourceTrimmedCurve::internal(original),
        );
        let mut retained =
            Processor::<_, Matrix4>::new(TrimmedCurve::new(Box::new(native_curve), (0., 4.)));
        retained.invert();
        let reflected_native = Curve::TrimmedCurve(retained);
        let declaration = StepCurve2D::Line(Line(Point2::origin(), Point2::new(1., 0.)));
        let reflected = retain(
            TrimInput {
                source_curve: &reflected_source,
                native_curve: &reflected_native,
                surface: SurfaceInput {
                    source_surface: &source_surface,
                    native_surface: &native_surface,
                    map: SurfaceParameterMap::Identity,
                    scale: 1.,
                },
                declared: Some(&declaration),
            },
            [0.; 2],
            &mut GeometryWorkBudget::new(4_000_000),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(reflected.certificate.is_certified() && reflected.declared_source);
        let chart = reflected.curve.unwrap();
        assert_eq!(chart.range_tuple(), (0., 4.));
        assert_eq!(chart.subs(0.), Point2::new(4., 0.));
        assert_eq!(chart.subs(4.), Point2::origin());
    }
}
