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
use crate::{GeometryWorkBudget, check_cancel};
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

/// Completed forward position AND relative-tangent proof for the exact chart
/// returned by this retention call. Coefficient projections and inverse tubes
/// deliberately cannot construct this receipt.
struct ForwardCorrespondence {
    error: f64,
    margin_uv: [f64; 2],
}

pub(super) struct SourceTrim {
    pub curve: Option<Curve2D>,
    pub margin_uv: [f64; 2],
    pub error_mm: f64,
    pub certified: bool,
    /// Original source PCURVE (or literal source parameter curve), not a fitted
    /// inverse. Consumers still require native UV incidence/coverage.
    pub declared_source: bool,
    forward: Option<ForwardCorrespondence>,
}
impl SourceTrim {
    pub(super) fn proves_forward(&self, tolerance: f64, margin_uv: [f64; 2]) -> bool {
        self.forward.as_ref().is_some_and(|proof| {
            proof.error <= tolerance && (0..2).all(|axis| margin_uv[axis] <= proof.margin_uv[axis])
        })
    }

    fn exact(curve: Curve2D) -> Self {
        Self {
            curve: Some(curve),
            margin_uv: [0.0; 2],
            error_mm: 0.0,
            certified: true,
            declared_source: false,
            forward: None,
        }
    }
    fn retained(curve: Option<Curve2D>) -> Self {
        Self {
            curve,
            margin_uv: [0.0; 2],
            error_mm: 0.0,
            certified: false,
            declared_source: false,
            forward: None,
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
            a.knot_vector() == b.knot_vector() && a.control_points() == b.control_points()
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
pub(super) fn native_chart(
    curve: &Curve,
    surface: &Surface,
    declared: Option<&Curve2D>,
    domain: Option<[[f64; 2]; 2]>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Curve2D>, GeometryError> {
    if let Surface::Plane(plane) = surface {
        if let Some(candidate) = native_attempt(work, cancel, |budget| {
            monstertruck_modeling::certified_planar_parameter_curve(curve, plane, budget, cancel)
        })? {
            if let Some(candidate) = acceptable(candidate, declared, work, cancel)? {
                return Ok(Some(candidate));
            }
        }
    }
    if let Some(candidate) = native_attempt(work, cancel, |budget| {
        monstertruck_modeling::certified_affine_surface_parameter_curve(
            curve, surface, budget, cancel,
        )
    })? {
        if let Some(candidate) = acceptable(candidate, declared, work, cancel)? {
            return Ok(Some(candidate));
        }
    }
    // The original source use selects a seam branch. Never choose the first
    // geometrically coincident boundary of a periodic carrier.
    if let (Some(Curve2D::Line(line)), Some(domain)) = (declared, domain) {
        let a = [line.0.x, line.0.y];
        let b = [line.1.x, line.1.y];
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
                if let Some(candidate) =
                    acceptable(candidate, Some(&Curve2D::Line(*line)), work, cancel)?
                {
                    return Ok(Some(candidate));
                }
            }
        }
    }
    if matches!(surface, Surface::ExtrusionSurface(_)) {
        if let Some(Curve2D::Line(line)) = declared {
            let choice = if line.0.y == line.1.y {
                Some(ExtrusionChartCurve::ProfileAt { offset: line.0.y })
            } else if line.0.x == line.1.x {
                Some(ExtrusionChartCurve::GeneratorAt {
                    profile_parameter: line.0.x,
                })
            } else {
                None
            };
            if let Some(choice) = choice {
                if let Some(candidate) = native_attempt(work, cancel, |budget| {
                    monstertruck_modeling::certified_extrusion_parameter_curve(
                        curve, surface, choice, budget, cancel,
                    )
                })? {
                    if let Some(candidate) = acceptable(candidate, declared, work, cancel)? {
                        return Ok(Some(candidate));
                    }
                }
            }
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
    curve: &Curve,
    surface: &Surface,
    margin_uv: [f64; 2],
    scale: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<SourceTrim>, GeometryError> {
    work.charge(8, cancel)?;
    let Some(chart) = line_branch_proposal(raw, transform, candidate) else {
        return Ok(None);
    };
    let Some(bound) = super::trim_certificate::certify_declared_coedge_correspondence(
        curve,
        surface,
        &chart,
        1e-7 / scale,
        margin_uv,
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
        error_mm: crate::parametric::up(bound * scale),
        certified: true,
        declared_source: true,
        forward: Some(ForwardCorrespondence {
            error: bound,
            margin_uv,
        }),
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
    source_curve: &Curve3D,
    source_surface: &StepSurface,
    curve: &Curve,
    surface: &Surface,
    map: SurfaceParameterMap,
    domain: Option<[[f64; 2]; 2]>,
    margin_uv: [f64; 2],
    scale: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<SourceTrim>, GeometryError> {
    let transform = map.affine_transform().ok_or_else(|| {
        error(
            GeometryErrorCode::ParameterCorrespondenceUnavailable,
            "original source LINE needs a nonlinear chart composition",
        )
    })?;
    if let Some(candidate) = native_chart(curve, surface, None, domain, work, cancel)? {
        if let Some(proof) = prove_line_branch(
            raw, transform, &candidate, curve, surface, margin_uv, scale, work, cancel,
        )? {
            return Ok(Some(proof));
        }
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
            })? {
                if let Some(proof) = prove_line_branch(
                    raw, transform, &candidate, curve, surface, margin_uv, scale, work, cancel,
                )? {
                    return Ok(Some(proof));
                }
            }
        }
    }
    if let Some(mut candidate) =
        active_spatial_line_chart(source_curve, curve, source_surface, work, cancel)?
    {
        if let Some(decoded) = decoded_seed_map(map).affine_transform() {
            candidate.transform_by(decoded);
            if let Some(proof) = prove_line_branch(
                raw, transform, &candidate, curve, surface, margin_uv, scale, work, cancel,
            )? {
                return Ok(Some(proof));
            }
        }
    }
    if let Some(mut candidate) = charts::source_chart(source_curve, source_surface)? {
        if let Some(decoded) = decoded_seed_map(map).affine_transform() {
            // Candidate only: actual original-LINE coefficients plus the FULL
            // raw chart operator above must pass the independent forward proof.
            candidate.transform_by(decoded);
            if let Some(proof) = prove_line_branch(
                raw, transform, &candidate, curve, surface, margin_uv, scale, work, cancel,
            )? {
                return Ok(Some(proof));
            }
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
    source_curve: &Curve3D,
    native_curve: &Curve,
    source_surface: &StepSurface,
    native_surface: &Surface,
    map: SurfaceParameterMap,
    declared: Option<&StepCurve2D>,
    scale: f64,
    retained: Option<&monstertruck_modeling::PreparedValidatedCurve<'static>>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SourceTrim, GeometryError> {
    let Some(prepared) = retained else {
        return retain(
            source_curve,
            native_curve,
            source_surface,
            native_surface,
            map,
            declared,
            scale,
            [0.0; 2],
            work,
            cancel,
        );
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
        native_curve,
        prepared,
        native_surface,
        &chart,
        1.0e-7 / scale,
        margin_uv,
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
        native_curve,
        prepared,
        native_surface,
        &chart,
        bound,
        margin_uv,
        work,
        cancel,
    )?;
    Ok(SourceTrim {
        curve: Some(chart),
        margin_uv,
        error_mm: crate::parametric::up(bound * scale),
        // The converter's stored chart is a candidate, not independent proof
        // that an implicit source declared a literal PCURVE expression.
        certified: true,
        declared_source: false,
        forward: None,
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
        curve,
        surface,
        &chart,
        1.0e-7 / scale,
        [0.0; 2],
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
        error_mm: crate::parametric::up(bound * scale),
        certified: true,
        declared_source: false,
        forward: Some(ForwardCorrespondence {
            error: bound,
            margin_uv: [0.0; 2],
        }),
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
/// its own original plane, only after symbolic source/native expression identity.
/// Captured coefficient midpoints and phase arithmetic select search coordinates
/// only. Whole native position AND relative-tangent correspondence then proves
/// the same original expression, not an independent approximate conversion.
/// The globally injective plane chart separately bounds its source projection.
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
    let (StepSurface::ElementarySurface(ElementarySurface::Plane(original)), Surface::Plane(plane)) =
        (source_surface, surface)
    else {
        return Ok(None);
    };
    if original != plane || !same_original_affine_expression(source_curve, curve, work, cancel)? {
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
    let u = plane.axis_u();
    let v = plane.axis_v();
    let Some(inverse) = Matrix3::from_cols(u, v, u.cross(v)).invert() else {
        return Ok(None);
    };
    let midpoint = |value: [crate::parametric::Interval; 3]| {
        Vector3::new(value[0].mid(), value[1].mid(), value[2].mid())
    };
    let origin = inverse * (midpoint(source.c) - plane.origin().to_vec());
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
    // For affine S(q)=O+Jq, the validated left inverse P=(AJ)^-1 A
    // exists globally when |I-AJ|<1. Hence P(C-O)-U=P(C-S(U)) on the
    // complete original range. This binds the coefficient-defined source
    // projection, not an arbitrary nearby point or a periodic inverse branch.
    let jet = crate::parametric::surface_enclosure(surface, [[0.0, 0.0]; 2], work, cancel)?;
    let residual = witness
        .forward
        .as_ref()
        .ok_or_else(|| invalid("derived plane chart lost its completed forward receipt"))?
        .error;
    let Some(margin) = super::implicit::inverse_margin(jet, residual)? else {
        return Ok(None);
    };
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
                curve,
                surface,
                &chart,
                1.0e-7 / scale,
                [0.0; 2],
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
                error_mm: crate::parametric::up(bound * scale),
                certified: true,
                declared_source: false,
                forward: Some(ForwardCorrespondence {
                    error: bound,
                    margin_uv: [0.0; 2],
                }),
            }));
        }
    }
    Ok(None)
}

pub(super) fn retain(
    source_curve: &Curve3D,
    native_curve: &Curve,
    source_surface: &StepSurface,
    native_surface: &Surface,
    map: SurfaceParameterMap,
    declared: Option<&StepCurve2D>,
    scale: f64,
    additional_margin_uv: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SourceTrim, GeometryError> {
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
    if let Some(StepCurve2D::Line(raw)) = declared {
        if let Some(proof) = active_line_branch(
            *raw,
            source_curve,
            source_surface,
            native_curve,
            native_surface,
            map,
            domain,
            margin_uv,
            scale,
            work,
            cancel,
        )? {
            return Ok(proof);
        }
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
        let mut witness = SourceTrim::exact(original.unwrap_or(curve));
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
        if let Some(original_domain) = extrusion_profile_domain(map) {
            if let Some(chart) = super::harmonic::extrusion_profile_chart_proposal(
                native_curve,
                native_surface,
                original_domain,
                work,
                cancel,
            )? {
                if let Some(witness) =
                    derived_chart_witness(native_curve, native_surface, chart, scale, work, cancel)?
                {
                    return Ok(witness);
                }
            }
        }
    }
    if let Some(chart) = declared.as_ref() {
        // The ORIGINAL source association authorizes this UV expression. The
        // active affine q/range remains a proposal until complete FORWARD
        // position AND relative tangent correspondence pass. Native preparation
        // must subsequently prove neighbor incidence, winding and trim coverage.
        if let Some(bound) = super::trim_certificate::certify_declared_coedge_correspondence(
            native_curve,
            native_surface,
            chart,
            1.0e-7 / scale,
            margin_uv,
            work,
            cancel,
            None,
        )? {
            return Ok(SourceTrim {
                curve: declared,
                margin_uv,
                error_mm: crate::parametric::up(bound * scale),
                certified: true,
                declared_source: true,
                forward: Some(ForwardCorrespondence {
                    error: bound,
                    margin_uv,
                }),
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
    if let Curve3D::ParameterCurve(pcurve) = leader {
        if let Some(curve) = literal_chart(pcurve, source_surface, map)? {
            if declared
                .as_ref()
                .map(|original| same_coefficients(&curve, original, work, cancel))
                .transpose()?
                .unwrap_or(true)
            {
                let mut witness = SourceTrim::exact(curve);
                witness.declared_source = true;
                return Ok(witness);
            }
        }
    }
    // Derived analytic charts are considered only without an original PCURVE.
    // Reapplying a source/native map to an already mapped declared chart would
    // silently change both phase and source identity.
    let candidate = if declared.is_none() {
        charts::source_chart(source_curve, source_surface)?
    } else {
        None
    };
    if let Some(candidate) = candidate {
        if let Some(proof) = chart_proof::certify(
            source_curve,
            source_surface,
            candidate,
            decoded_seed_map(map),
            scale,
            work,
            cancel,
        )? {
            if !proof.error_mm.is_finite()
                || proof.error_mm < 0.0
                || proof
                    .margin_uv
                    .into_iter()
                    .any(|x| !x.is_finite() || x < 0.0)
            {
                return Err(invalid("source chart proof returned invalid uncertainty"));
            }
            return Ok(SourceTrim {
                curve: Some(proof.curve),
                margin_uv: proof.margin_uv,
                error_mm: proof.error_mm,
                certified: true,
                declared_source: false,
                forward: None,
            });
        }
    }
    // A bounded positional correspondence is useful diagnostic evidence, not
    // a topology/UV-inverse certificate. Retain the original use but refuse
    // operations that require an admitted native trim domain.
    let mut result = SourceTrim::retained(declared);
    if let Some(chart) = &result.curve {
        if let Some(bound) = super::trim_certificate::certify_curve_on_surface(
            native_curve,
            native_surface,
            chart,
            1.0e-7 / scale,
            work,
            cancel,
        )? {
            result.error_mm = crate::parametric::up(bound * scale);
        }
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
    if incident(incoming, outgoing) {
        return Ok(true);
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
        Ok(proof) => Ok(proof.is_some()),
        Err(cause) => recoverable(cause).map(|_| false),
    }
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
    let proposal = if matches!(decoded, SurfaceParameterMap::Identity) {
        chart
    } else {
        let Some(inverse) = decoded
            .affine_transform()
            .and_then(|matrix| matrix.invert())
        else {
            return Ok(None);
        };
        let range = chart.range_tuple();
        mapped_declaration(chart, range, inverse, true)
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
        for axis in 0..2 {
            witness.margin_uv[axis] = witness.margin_uv[axis].max(margin[axis]);
        }
    }
    Ok(())
}

fn isolated_periodic_branch(witness: &SourceTrim, periods: [Option<f64>; 2]) -> bool {
    (0..2).all(|axis| periods[axis].is_none_or(|period| witness.margin_uv[axis] < period * 0.25))
}

fn prove_periodic_rebind(
    context: &PeriodicUse<'_>,
    source_surface: &StepSurface,
    native_surface: &Surface,
    map: SurfaceParameterMap,
    chart: Curve2D,
    shift: [f64; 2],
    scale: f64,
    declared_source: bool,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    diagnostic: &mut super::trim_certificate::CorrespondenceDiagnostic,
) -> Result<Option<SourceTrim>, GeometryError> {
    let started = work.consumed();
    let mut stage = "periodic-inverse-tube";
    let result = (|| {
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
                context.native,
                prepared,
                native_surface,
                &chart,
                1e-7 / scale,
                margin_uv,
                work,
                cancel,
            )?
        } else {
            super::trim_certificate::certify_declared_coedge_correspondence(
                context.native,
                native_surface,
                &chart,
                1e-7 / scale,
                margin_uv,
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
                context.native,
                prepared,
                native_surface,
                &chart,
                bound,
                margin_uv,
                work,
                cancel,
            )?
        } else {
            margin_uv
        };
        Ok(Some(SourceTrim {
            curve: Some(chart),
            margin_uv,
            error_mm: crate::parametric::up(bound * scale).max(coefficient_error),
            certified: true,
            declared_source,
            forward: None,
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
        if witnesses[index].certified {
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
            if !witnesses[neighbor_index].certified
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
            if let super::harmonic::ProfileParameterDomain::Bounded(range) = original_domain {
                if parameter < range[0] || parameter > range[1] {
                    continue;
                }
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
    source_surface: &StepSurface,
    native_surface: &Surface,
    map: SurfaceParameterMap,
    scale: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    diagnostic: &mut super::trim_certificate::CorrespondenceDiagnostic,
) -> Result<(), GeometryError>
where
    F: Fn(usize) -> PeriodicUse<'a>,
{
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
        || witnesses
            .iter()
            .any(|witness| !witness.certified || witness.curve.is_none())
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
                    current.source,
                    current.native,
                    source_surface,
                    native_surface,
                    map,
                    Some(other),
                    scale,
                    current.retained,
                    work,
                    cancel,
                )?;
                if candidate.certified {
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
            let range = curve.range_tuple();
            let delta = Vector2::new(translation[0], translation[1]);
            stage = "periodic-original-line-branch";
            let proposal = if let Some(declared) = current.declared {
                if let Some(raw) = original_unbounded_line(declared, work, cancel)? {
                    // Endpoint arithmetic constructs a seed only. The admitted
                    // expression below keeps the original LINE and its chart
                    // operator, then proves the complete independently mapped range.
                    let shifted = Curve2D::Line(Line(
                        curve.subs(range.0) + delta,
                        curve.subs(range.1) + delta,
                    ));
                    map.affine_transform()
                        .and_then(|transform| line_branch_proposal(raw, transform, &shifted))
                } else {
                    None
                }
            } else {
                Some(mapped_declaration(
                    curve.clone(),
                    range,
                    Matrix3::from_translation(delta),
                    true,
                ))
            };
            if let Some(proposal) = proposal {
                stage = "periodic-translated-rebind";
                if let Some(candidate) = prove_periodic_rebind(
                    &current,
                    source_surface,
                    native_surface,
                    map,
                    proposal,
                    translation,
                    scale,
                    witnesses[index].declared_source,
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
        if !parameter_joint_incident(
            uv_endpoint(
                &witnesses[last],
                context(last).orientation,
                true,
                work,
                cancel,
            )?
            .unwrap(),
            uv_endpoint(
                &witnesses[anchor],
                context(anchor).orientation,
                false,
                work,
                cancel,
            )?
            .unwrap(),
            native_surface,
            context(anchor).front_vertex,
            scale,
            work,
            cancel,
        )? {
            return Err(error(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "original periodic coedge branches do not close in one bounded UV chart",
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
    })
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
        assert!(witness.certified && !witness.declared_source);
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
        assert!(witness.certified && !witness.declared_source);
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
            witnesses[0].certified
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
        assert!(witness.certified && !witness.declared_source);
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
        assert!(proof.certified && !proof.declared_source);
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
        let incoming = [Interval::point(pi * 0.5), Interval::point(1.570796326795)];
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
            &source,
            &curve,
            &source_surface,
            &surface,
            SurfaceParameterMap::Identity,
            Some(&original),
            1.,
            [0.0; 2],
            &mut GeometryWorkBudget::new(1_000_000),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(proof.certified && proof.declared_source);
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
                        &circle,
                        &surface,
                        [0.; 2],
                        1.,
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
                &circle,
                &surface,
                [0.; 2],
                1.,
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
            &source,
            &curve,
            &source_surface,
            &surface,
            SurfaceParameterMap::Identity,
            Some(&StepCurve2D::Line(raw)),
            1.,
            [0.0; 2],
            &mut GeometryWorkBudget::new(2_000_000),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(proof.certified && proof.declared_source);
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
            &source,
            &curve,
            &source_surface,
            &surface,
            SurfaceParameterMap::Identity,
            Some(&bounded),
            1.,
            [0.0; 2],
            &mut GeometryWorkBudget::new(2_000_000),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(!proof.certified);
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
            &source_curve,
            &native_curve,
            &source_surface,
            &native_surface,
            SurfaceParameterMap::Identity,
            Some(&StepCurve2D::Line(Line(
                Point2::new(0., 0.),
                Point2::new(1., 0.),
            ))),
            1.,
            [0.0; 2],
            &mut GeometryWorkBudget::new(4_000_000),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(proof.certified && proof.declared_source);
        assert_eq!(proof.curve.unwrap().range_tuple(), (0., 4.));
    }
}
