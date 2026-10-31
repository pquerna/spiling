// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Literal original STEP proof operands, separate from converted native targets.
//! All parameters retain the decoded source gauge: mm and RADIANS. A provider
//! is not a `solidcore::Curve`/`Surface`, and never dereferences to one. Original
//! placements, weights, selectors, preferred masters and wrapper genealogy stay
//! borrowed. Arithmetic errors are enclosed in every returned jet, not assigned
//! a zero conversion error. Missing active branch authority is not inferred from
//! a native convenience range or an endpoint sample.

#[cfg(test)]
use crate::parametric::SurfaceEnclosure;
use crate::parametric::nurbs_eval::{SurfaceTensor, basis_jet, binomial, validate_knots};
use crate::parametric::{
    CurveEnclosure, CurveThirdEnclosure, Interval, PreparedValidatedCurve, sin_cos_interval,
};
use crate::{GeometryError, GeometryWorkBudget};
use solidcraft_step::geom::{
    self as source, Associated, Curve, PreferredRep, Surface, Trim, TrimPreference,
};
use spiling_contracts::geometry::GeometryErrorCode;
use std::sync::atomic::AtomicBool;
const ZERO: Interval = Interval::point(0.0);
const ONE: Interval = Interval::point(1.0);
const MAX_DEPTH: usize = 16;
pub(crate) const SOURCE_OPERAND_SCRATCH_BYTES: usize = 2 * PreparedValidatedCurve::scratch_bytes();
fn conditioning(message: &str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::ConditioningFailure, message)
}
fn unavailable(message: &str) -> GeometryError {
    GeometryError::new(
        GeometryErrorCode::ParameterCorrespondenceUnavailable,
        message,
    )
}
fn unsupported(message: &str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::UnsupportedOperation, message)
}
fn resource() -> GeometryError {
    GeometryError::new(
        GeometryErrorCode::ResourceLimit,
        "original source operand live byte/depth allowance exhausted",
    )
}
fn finite(values: impl IntoIterator<Item = f64>) -> Result<(), GeometryError> {
    for value in values {
        Interval::new(value, value)?;
    }
    Ok(())
}
fn check_depth(
    depth: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    work.charge(1, cancel)?;
    if depth >= MAX_DEPTH {
        return Err(resource());
    }
    Ok(())
}
fn add(a: [Interval; 3], b: [Interval; 3]) -> [Interval; 3] {
    std::array::from_fn(|k| a[k].add(b[k]))
}
fn scale(a: [Interval; 3], b: Interval) -> [Interval; 3] {
    a.map(|v| v.mul(b))
}
fn cross(a: [Interval; 3], b: [Interval; 3]) -> [Interval; 3] {
    std::array::from_fn(|k| {
        a[(k + 1) % 3]
            .mul(b[(k + 2) % 3])
            .sub(a[(k + 2) % 3].mul(b[(k + 1) % 3]))
    })
}
fn placement(p: &source::Placement) -> Result<[[Interval; 3]; 4], GeometryError> {
    finite(p.origin.into_iter().chain(p.axis).chain(p.ref_direction))?;
    let x = p.ref_direction.map(Interval::point);
    let z = p.axis.map(Interval::point);
    // Do not round Placement::y() before enclosing its defining cross product.
    Ok([p.origin.map(Interval::point), x, cross(z, x), z])
}
/// Exact positive axial circular extrusion; no rational-chart angular inference.
pub(crate) struct OriginalCircularExtrusion<'a> {
    pub(crate) position: &'a source::Placement,
    pub(crate) radius_mm: f64,
    pub(crate) axial_scale_mm: f64,
}
pub(crate) fn original_circular_extrusion(
    surface: &source::Surface,
) -> Option<OriginalCircularExtrusion<'_>> {
    let source::Surface::Extrusion {
        profile,
        direction,
        magnitude,
    } = surface
    else {
        return None;
    };
    let source::Curve::Circle { position, radius } = profile.as_ref() else {
        return None;
    };
    if !radius.is_finite()
        || *radius <= 0.0
        || !magnitude.is_finite()
        || *magnitude <= 0.0
        || !position
            .origin
            .iter()
            .chain(&position.axis)
            .chain(&position.ref_direction)
            .all(|x| x.is_finite())
        || *direction != position.axis
        || position.axis == [0.0; 3]
    {
        return None;
    }
    Some(OriginalCircularExtrusion {
        position,
        radius_mm: *radius,
        axial_scale_mm: *magnitude,
    })
}

/// The declared ordinal is authoritative; never substitute a neighboring PCURVE.
pub(crate) fn preferred_source_pcurve(
    curve: &Curve,
) -> Result<Option<&source::Pcurve>, GeometryError> {
    let Curve::SurfaceCurve {
        master, associated, ..
    } = curve
    else {
        return Ok(None);
    };
    let index = match master {
        PreferredRep::Curve3d => return Ok(None),
        PreferredRep::PcurveS1 => 0,
        PreferredRep::PcurveS2 => 1,
    };
    match associated.get(index) {
        Some(Associated::Pcurve(pcurve)) => Ok(Some(pcurve)),
        _ => Err(unavailable(
            "declared preferred source PCURVE ordinal has no PCURVE operand",
        )),
    }
}
/// A period of the selected expression, not permission to extend a finite trim.
pub(crate) fn original_curve_parameter_period(
    curve: &Curve,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Interval>, GeometryError> {
    curve_parameter_period(curve, 0, work, cancel)
}
fn curve_parameter_period(
    mut curve: &Curve,
    start: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Interval>, GeometryError> {
    let tau = Interval {
        lo: crate::parametric::down(std::f64::consts::TAU),
        hi: crate::parametric::up(std::f64::consts::TAU),
    };
    for depth in start..MAX_DEPTH {
        check_depth(depth, work, cancel)?;
        match curve {
            Curve::Circle { .. } | Curve::Ellipse { .. } => return Ok(Some(tau)),
            Curve::Trimmed { basis, .. } => curve = basis,
            Curve::SurfaceCurve { curve_3d, .. } => {
                let Some(pcurve) = preferred_source_pcurve(curve)? else {
                    curve = curve_3d;
                    continue;
                };
                if let Some(period) =
                    curve_parameter_period(&pcurve.curve, depth + 1, work, cancel)?
                {
                    return Ok(Some(period));
                }
                let mut chart = pcurve.curve.as_ref();
                let mut surface = pcurve.surface.as_ref();
                for level in depth + 1..MAX_DEPTH {
                    check_depth(level, work, cancel)?;
                    if let Curve::Trimmed { basis, .. } = chart {
                        chart = basis;
                    } else {
                        break;
                    }
                }
                for level in depth + 1..MAX_DEPTH {
                    check_depth(level, work, cancel)?;
                    if let Surface::RectangularTrimmed { basis, .. } = surface {
                        surface = basis;
                    } else {
                        break;
                    }
                }
                let Curve::Line {
                    direction,
                    magnitude,
                    ..
                } = chart
                else {
                    return Ok(None);
                };
                finite(direction.iter().copied().chain([*magnitude]))?;
                if *magnitude == 0.0 {
                    return Ok(None);
                }
                let axis = match surface {
                    Surface::Cylinder { .. } | Surface::Cone { .. } | Surface::Sphere { .. }
                        if direction[1] == 0.0 && direction[0] != 0.0 =>
                    {
                        0
                    }
                    Surface::Torus { .. } | Surface::DegenerateTorus { .. }
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
                let velocity = Interval::point(direction[axis]).mul(Interval::point(*magnitude));
                if velocity.contains_zero() {
                    return Err(conditioning(
                        "source angular velocity is not separated from zero",
                    ));
                }
                let period = tau.div(if velocity.hi < 0.0 {
                    velocity.neg()
                } else {
                    velocity
                })?;
                if !period.lo.is_finite() || !period.hi.is_finite() || period.lo <= 0.0 {
                    return Err(conditioning("source period cannot be finitely enclosed"));
                }
                return Ok(Some(period));
            }
            _ => return Ok(None),
        }
    }
    Err(resource())
}
fn selected_trim<'a>(
    mut curve: &'a Curve,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<&'a Curve>, GeometryError> {
    for depth in 0..MAX_DEPTH {
        check_depth(depth, work, cancel)?;
        match curve {
            Curve::Trimmed { .. } => return Ok(Some(curve)),
            Curve::SurfaceCurve { curve_3d, .. } => {
                curve =
                    preferred_source_pcurve(curve)?.map_or(curve_3d.as_ref(), |p| p.curve.as_ref());
            }
            _ => return Ok(None),
        }
    }
    Err(resource())
}
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "Keep the declared source trim's literal parameters, turns and period with its immutable branch owner"
    )
)]
pub(crate) struct OriginalDeclaredTrimBranch<'a> {
    trimmed: &'a Curve,
    literal_parameters: [f64; 2],
    declared_turns: [i32; 2],
    period: Interval,
    endpoint_parameters: [Interval; 2],
}
impl OriginalDeclaredTrimBranch<'_> {
    #[cfg(test)]
    pub(crate) fn trimmed_source(&self) -> &Curve {
        self.trimmed
    }
    #[cfg(test)]
    pub(crate) fn literal_parameters(&self) -> [f64; 2] {
        self.literal_parameters
    }
    #[cfg(test)]
    pub(crate) fn declared_turns(&self) -> [i32; 2] {
        self.declared_turns
    }
    #[cfg(test)]
    pub(crate) fn period(&self) -> Interval {
        self.period
    }
    #[cfg(test)]
    pub(crate) fn endpoint_parameters(&self) -> [Interval; 2] {
        self.endpoint_parameters
    }
}
fn reserve<T>(
    values: &mut Vec<T>,
    additional: usize,
    owned: &mut usize,
    limit: usize,
) -> Result<(), GeometryError> {
    let required = values.len().checked_add(additional).ok_or_else(resource)?;
    if required <= values.capacity() {
        return Ok(());
    }
    let bytes = (required - values.capacity())
        .checked_mul(std::mem::size_of::<T>())
        .ok_or_else(resource)?;
    if owned.checked_add(bytes).is_none_or(|n| n > limit) {
        return Err(resource());
    }
    let old = values.capacity();
    values
        .try_reserve_exact(required - values.len())
        .map_err(|_| resource())?;
    *owned = owned
        .checked_add(
            (values.capacity() - old)
                .checked_mul(std::mem::size_of::<T>())
                .ok_or_else(resource)?,
        )
        .ok_or_else(resource)?;
    if *owned > limit {
        return Err(resource());
    }
    Ok(())
}
fn expanded_knots(
    values: &[f64],
    multiplicities: &[usize],
    owned: &mut usize,
    limit: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Vec<f64>, GeometryError> {
    if values.len() != multiplicities.len() || values.is_empty() {
        return Err(conditioning("invalid original distinct knot census"));
    }
    let mut count = 0usize;
    for (&k, &m) in values.iter().zip(multiplicities) {
        work.charge(1, cancel)?;
        finite([k])?;
        if m == 0 {
            return Err(conditioning("original knot has zero multiplicity"));
        }
        count = count.checked_add(m).ok_or_else(resource)?;
    }
    let mut knots = Vec::new();
    reserve(&mut knots, count, owned, limit)?;
    for (&k, &m) in values.iter().zip(multiplicities) {
        for _ in 0..m {
            work.charge(1, cancel)?;
            knots.push(k);
        }
    }
    Ok(knots)
}
struct CurveKnots<'a> {
    source: &'a source::BSplineCurve,
    knots: Vec<f64>,
}
struct PolylineKnots<'a> {
    source: &'a [[f64; 3]],
    knots: Vec<f64>,
}
struct SurfaceKnots<'a> {
    source: &'a source::BSplineSurface,
    u: Vec<f64>,
    v: Vec<f64>,
}
struct Inventory<'a> {
    curves: Vec<CurveKnots<'a>>,
    surfaces: Vec<SurfaceKnots<'a>>,
    polylines: Vec<PolylineKnots<'a>>,
    owned: usize,
    limit: usize,
    declared_branch: Option<OriginalDeclaredTrimBranch<'a>>,
}
impl<'a> Inventory<'a> {
    fn new(owner_bytes: usize, maximum_live_bytes: usize) -> Result<Self, GeometryError> {
        let limit = maximum_live_bytes
            .checked_sub(SOURCE_OPERAND_SCRATCH_BYTES)
            .ok_or_else(resource)?;
        if owner_bytes > limit {
            return Err(resource());
        }
        Ok(Self {
            curves: Vec::new(),
            surfaces: Vec::new(),
            polylines: Vec::new(),
            owned: owner_bytes,
            limit,
            declared_branch: None,
        })
    }
    fn collect_curve(
        &mut self,
        c: &'a Curve,
        depth: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        check_depth(depth, work, cancel)?;
        match c {
            Curve::BSpline(s) => {
                for entry in &self.curves {
                    work.charge(1, cancel)?;
                    if std::ptr::eq(entry.source, s.as_ref()) {
                        return Ok(());
                    }
                }
                if let Some(w) = &s.weights
                    && w.len() != s.points.len()
                {
                    return Err(conditioning("original curve weight census mismatch"));
                }
                let knots = expanded_knots(
                    &s.knots,
                    &s.multiplicities,
                    &mut self.owned,
                    self.limit,
                    work,
                    cancel,
                )?;
                let count = s.points.len();
                if count <= s.degree || knots.len() <= count {
                    return Err(conditioning("invalid original curve active knot domain"));
                }
                validate_knots(
                    &knots,
                    s.degree,
                    count,
                    [knots[s.degree], knots[count]],
                    work,
                    cancel,
                )?;
                reserve(&mut self.curves, 1, &mut self.owned, self.limit)?;
                self.curves.push(CurveKnots { source: s, knots });
            }
            Curve::Trimmed { basis, .. } => self.collect_curve(basis, depth + 1, work, cancel)?,
            Curve::SurfaceCurve { curve_3d, .. } => {
                if let Some(pcurve) = preferred_source_pcurve(c)? {
                    self.collect_curve(&pcurve.curve, depth + 1, work, cancel)?;
                    self.collect_surface(&pcurve.surface, depth + 1, work, cancel)?;
                } else {
                    self.collect_curve(curve_3d, depth + 1, work, cancel)?;
                }
            }
            Curve::Hyperbola { .. } | Curve::OffsetCurve3d { .. } => {
                return Err(unsupported(
                    "original hyperbolic/offset curve proof operand is not implemented",
                ));
            }
            Curve::Polyline(points) => {
                if points.len() < 2 {
                    return Err(conditioning("original polyline has fewer than two points"));
                }
                if (points.len() - 1) as u128 > (1_u128 << 53) {
                    return Err(resource());
                }
                for entry in &self.polylines {
                    work.charge(1, cancel)?;
                    if std::ptr::eq(entry.source, points.as_slice()) {
                        return Ok(());
                    }
                }
                let mut knots = Vec::new();
                reserve(&mut knots, points.len(), &mut self.owned, self.limit)?;
                for (i, point) in points.iter().enumerate() {
                    work.charge(1, cancel)?;
                    finite(point.iter().copied())?;
                    knots.push(i as f64);
                }
                reserve(&mut self.polylines, 1, &mut self.owned, self.limit)?;
                self.polylines.push(PolylineKnots {
                    source: points,
                    knots,
                });
            }
            Curve::Line { .. }
            | Curve::Circle { .. }
            | Curve::Ellipse { .. }
            | Curve::Parabola { .. } => {}
        }
        Ok(())
    }
    fn collect_surface(
        &mut self,
        s: &'a Surface,
        depth: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        check_depth(depth, work, cancel)?;
        match s {
            Surface::BSpline(s) => {
                for entry in &self.surfaces {
                    work.charge(1, cancel)?;
                    if std::ptr::eq(entry.source, s.as_ref()) {
                        return Ok(());
                    }
                }
                let nu = s.points.len();
                let nv = s.points.first().map_or(0, Vec::len);
                if nu == 0 || nv == 0 || s.points.iter().any(|row| row.len() != nv) {
                    return Err(conditioning(
                        "original surface control net is not rectangular",
                    ));
                }
                if let Some(weights) = &s.weights
                    && (weights.len() != nu || weights.iter().any(|row| row.len() != nv))
                {
                    return Err(conditioning("original surface weight census mismatch"));
                }
                let u = expanded_knots(
                    &s.u_knots,
                    &s.u_multiplicities,
                    &mut self.owned,
                    self.limit,
                    work,
                    cancel,
                )?;
                let v = expanded_knots(
                    &s.v_knots,
                    &s.v_multiplicities,
                    &mut self.owned,
                    self.limit,
                    work,
                    cancel,
                )?;
                if nu <= s.u_degree || nv <= s.v_degree || u.len() <= nu || v.len() <= nv {
                    return Err(conditioning("invalid original surface active knot domain"));
                }
                validate_knots(&u, s.u_degree, nu, [u[s.u_degree], u[nu]], work, cancel)?;
                validate_knots(&v, s.v_degree, nv, [v[s.v_degree], v[nv]], work, cancel)?;
                reserve(&mut self.surfaces, 1, &mut self.owned, self.limit)?;
                self.surfaces.push(SurfaceKnots { source: s, u, v });
            }
            Surface::Extrusion { profile, .. } | Surface::Revolution { profile, .. } => {
                self.collect_curve(profile, depth + 1, work, cancel)?
            }
            Surface::RectangularTrimmed { basis, .. } => {
                self.collect_surface(basis, depth + 1, work, cancel)?
            }
            Surface::Offset { .. } => {
                return Err(unsupported(
                    "original offset surface proof operand is not implemented",
                ));
            }
            Surface::Plane { .. }
            | Surface::Cylinder { .. }
            | Surface::Cone { .. }
            | Surface::Sphere { .. }
            | Surface::Torus { .. }
            | Surface::DegenerateTorus { .. } => {}
        }
        Ok(())
    }
    fn curve_knots(
        &self,
        s: &source::BSplineCurve,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<&[f64], GeometryError> {
        for entry in &self.curves {
            work.charge(1, cancel)?;
            if std::ptr::eq(entry.source, s) {
                return Ok(&entry.knots);
            }
        }
        Err(conditioning(
            "original curve does not belong to this immutable inventory",
        ))
    }
    fn polyline_knots(
        &self,
        points: &[[f64; 3]],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<&[f64], GeometryError> {
        for entry in &self.polylines {
            work.charge(1, cancel)?;
            if std::ptr::eq(entry.source, points) {
                return Ok(&entry.knots);
            }
        }
        Err(conditioning(
            "original polyline does not belong to this immutable inventory",
        ))
    }
    fn surface_knots(
        &self,
        s: &source::BSplineSurface,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(&[f64], &[f64]), GeometryError> {
        for entry in &self.surfaces {
            work.charge(1, cancel)?;
            if std::ptr::eq(entry.source, s) {
                return Ok((&entry.u, &entry.v));
            }
        }
        Err(conditioning(
            "original surface does not belong to this immutable inventory",
        ))
    }
}

/// Literal PARAMETER_VALUE selectors only. Cartesian selectors are retained in
/// the source object but require the owning original branch correspondence proof.
pub(crate) fn original_curve_parameter_restriction(
    curve: &Curve,
) -> Result<Option<[f64; 2]>, GeometryError> {
    let mut curve = curve;
    for _ in 0..MAX_DEPTH {
        match curve {
            Curve::Trimmed {
                trim_1,
                trim_2,
                preference,
                ..
            } => {
                if *preference == TrimPreference::Cartesian {
                    return Ok(None);
                }
                let parameter = |selectors: &[Trim]| -> Result<Option<f64>, GeometryError> {
                    let mut parameter = None;
                    for selector in selectors {
                        if let Trim::Parameter(p) = selector {
                            finite([*p])?;
                            if parameter.is_some_and(|v| v != *p) {
                                return Err(unavailable(
                                    "contradictory original parameter trim selectors",
                                ));
                            }
                            parameter = Some(*p);
                        }
                    }
                    Ok(parameter)
                };
                return match (parameter(trim_1)?, parameter(trim_2)?) {
                    (Some(a), Some(b)) => Ok(Some([a, b])),
                    _ => Ok(None),
                };
            }
            Curve::SurfaceCurve { curve_3d, .. } => {
                curve = if let Some(pcurve) = preferred_source_pcurve(curve)? {
                    &pcurve.curve
                } else {
                    curve_3d
                };
            }
            _ => return Ok(None),
        }
    }
    Err(resource())
}
fn check_trim(
    curve: &Curve,
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    if let Some([a, b]) = original_curve_parameter_restriction(curve)? {
        if let Some(Curve::Trimmed { basis, sense, .. }) = selected_trim(curve, work, cancel)?
            && ((*sense && a >= b) || (!*sense && a <= b))
            && original_curve_parameter_period(basis, work, cancel)?.is_some()
        {
            return Err(unavailable(
                "periodic trim requires its retained declared-turn expression",
            ));
        }
        if range[0] < a.min(b) || range[1] > a.max(b) {
            return Err(unavailable(
                "query exceeds literal original parameter trim selectors",
            ));
        }
    }
    Ok(())
}

pub(crate) struct PreparedOriginalCurve<'a> {
    source: &'a Curve,
    active: [f64; 2],
    inventory: Inventory<'a>,
}
pub(crate) struct PreparedOriginalSurface<'a> {
    source: &'a Surface,
    active: [[f64; 2]; 2],
    inventory: Inventory<'a>,
}
impl<'a> PreparedOriginalCurve<'a> {
    pub(crate) fn new(
        source: &'a Curve,
        active: [f64; 2],
        maximum_live_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        work.charge(1, cancel)?;
        Interval::new(active[0], active[1])?;
        if let Some(trimmed @ Curve::Trimmed { basis, sense, .. }) =
            selected_trim(source, work, cancel)?
            && let Some([a, b]) = original_curve_parameter_restriction(trimmed)?
        {
            let turn = if *sense && a >= b {
                1
            } else if !*sense && a <= b {
                -1
            } else {
                0
            };
            if turn != 0 && original_curve_parameter_period(basis, work, cancel)?.is_some() {
                return Self::new_with_declared_turns(
                    source,
                    active,
                    [0, turn],
                    maximum_live_bytes,
                    work,
                    cancel,
                );
            }
        }
        check_trim(source, active, work, cancel)?;
        let mut inventory = Inventory::new(std::mem::size_of::<Self>(), maximum_live_bytes)?;
        inventory.collect_curve(source, 0, work, cancel)?;
        let owner = Self {
            source,
            active,
            inventory,
        };
        owner.evaluate_order(active, 0, work, cancel)?;
        Ok(owner)
    }
    /// The outward query cover retains exact literal-plus-period endpoints;
    /// it is not substituted for the declared branch expression.
    pub(crate) fn new_with_declared_turns(
        source: &'a Curve,
        active: [f64; 2],
        declared_turns: [i32; 2],
        maximum_live_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        work.charge(1, cancel)?;
        Interval::new(active[0], active[1])?;
        let trimmed = selected_trim(source, work, cancel)?
            .ok_or_else(|| unavailable("source has no authored parameter trim to lift"))?;
        let Curve::Trimmed { basis, sense, .. } = trimmed else {
            unreachable!()
        };
        if selected_trim(basis, work, cancel)?.is_some() {
            return Err(unavailable(
                "nested declared trim lifts require separate containment proof",
            ));
        }
        let literal = original_curve_parameter_restriction(trimmed)?.ok_or_else(|| {
            unavailable("declared turn lift requires preferred literal parameter selectors")
        })?;
        // The declaring trim's own basis must be periodic. A LINE PCURVE does
        // not acquire permission to cross its finite trim from S(gamma)'s period.
        let period = original_curve_parameter_period(basis, work, cancel)?.ok_or_else(|| {
            unavailable("the declaring trim basis has no coefficient-proved period")
        })?;
        let turn = if *sense && literal[0] >= literal[1] {
            1
        } else if !*sense && literal[0] <= literal[1] {
            -1
        } else {
            0
        };
        if declared_turns != [0, turn] {
            return Err(unavailable(
                "declared source turn disagrees with authored trim and sense",
            ));
        }
        let endpoint_parameters = [
            Interval::point(literal[0]),
            Interval::point(literal[1]).add(period.mul(Interval::point(turn as f64))),
        ];
        let cover = endpoint_parameters[0].hull(endpoint_parameters[1]);
        if active[0] < cover.lo || active[1] > cover.hi {
            return Err(unavailable(
                "source query cover exceeds its declared lifted trim",
            ));
        }
        let mut inventory = Inventory::new(std::mem::size_of::<Self>(), maximum_live_bytes)?;
        inventory.collect_curve(source, 0, work, cancel)?;
        inventory.declared_branch = Some(OriginalDeclaredTrimBranch {
            trimmed,
            literal_parameters: literal,
            declared_turns,
            period,
            endpoint_parameters,
        });
        let owner = Self {
            source,
            active,
            inventory,
        };
        owner.evaluate_order(active, 0, work, cancel)?;
        Ok(owner)
    }
    #[cfg(test)]
    pub(crate) fn declared_trim_branch(&self) -> Option<&OriginalDeclaredTrimBranch<'a>> {
        self.inventory.declared_branch.as_ref()
    }
    pub(crate) fn source(&self) -> &'a Curve {
        self.source
    }
    pub(crate) fn active_range(&self) -> [f64; 2] {
        self.active
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        self.inventory.owned
    }
    pub(crate) fn evaluate_order(
        &self,
        range: [f64; 2],
        order: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[[Interval; 3]; 4], GeometryError> {
        work.charge(1, cancel)?;
        if order > 3 {
            return Err(conditioning(
                "original curve derivative order exceeds three",
            ));
        }
        Interval::new(range[0], range[1])?;
        if range[0] < self.active[0] || range[1] > self.active[1] {
            return Err(unavailable(
                "original curve query exceeds its authorized branch",
            ));
        }
        curve_jet_selected(
            self.source,
            range,
            order,
            &self.inventory,
            0,
            self.inventory
                .declared_branch
                .as_ref()
                .map(|b| b.trimmed as *const Curve),
            work,
            cancel,
        )
    }
    pub(crate) fn jet(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<CurveThirdEnclosure, GeometryError> {
        let j = self.evaluate_order(range, 3, work, cancel)?;
        Ok(CurveThirdEnclosure {
            jet: CurveEnclosure {
                position: j[0],
                derivative: j[1],
                second_derivative: j[2],
            },
            third_derivative: j[3],
        })
    }
    pub(crate) fn position_bounds(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[Interval; 3], GeometryError> {
        Ok(self.evaluate_order(range, 0, work, cancel)?[0])
    }
}
impl<'a> PreparedOriginalSurface<'a> {
    pub(crate) fn new(
        source: &'a Surface,
        active: [[f64; 2]; 2],
        maximum_live_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        work.charge(1, cancel)?;
        for range in active {
            Interval::new(range[0], range[1])?;
        }
        let mut inventory = Inventory::new(std::mem::size_of::<Self>(), maximum_live_bytes)?;
        inventory.collect_surface(source, 0, work, cancel)?;
        let owner = Self {
            source,
            active,
            inventory,
        };
        owner.evaluate_order(active, 0, work, cancel)?;
        Ok(owner)
    }
    pub(crate) fn source(&self) -> &'a Surface {
        self.source
    }
    pub(crate) fn active_domain(&self) -> [[f64; 2]; 2] {
        self.active
    }
    /// Narrow the original chart without rebuilding or cloning its inventory.
    pub(crate) fn restrict_active(
        mut self,
        domain: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        work.charge(1, cancel)?;
        if domain.iter().flatten().any(|value| !value.is_finite()) {
            return Err(conditioning("original surface restriction is not finite"));
        }
        self.evaluate_order(domain, 0, work, cancel)?;
        self.active = domain;
        Ok(self)
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        self.inventory.owned
    }
    pub(crate) fn evaluate_order(
        &self,
        uv: [[f64; 2]; 2],
        order: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<SurfaceTensor, GeometryError> {
        work.charge(1, cancel)?;
        if order > 3 {
            return Err(conditioning(
                "original surface derivative order exceeds three",
            ));
        }
        for i in 0..2 {
            Interval::new(uv[i][0], uv[i][1])?;
            if uv[i][0] < self.active[i][0] || uv[i][1] > self.active[i][1] {
                return Err(unavailable(&format!(
                    "original surface query exceeds its authorized domain: axis={i} query={uv:?} authorized={:?}",
                    self.active
                )));
            }
        }
        surface_jet(self.source, uv, order, &self.inventory, 0, work, cancel)
    }
    #[cfg(test)]
    pub(crate) fn jet(
        &self,
        uv: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<SurfaceEnclosure, GeometryError> {
        let j = self.evaluate_order(uv, 2, work, cancel)?;
        Ok(SurfaceEnclosure {
            position: j[0][0],
            du: j[1][0],
            dv: j[0][1],
            duu: j[2][0],
            duv: j[1][1],
            dvv: j[0][2],
        })
    }

    /// Evaluate the captured coefficient sheet on this finite query, not
    /// membership in its selected trim envelope. Source realization homotopies
    /// need this distinct operation; it grants no material or native parity.
    /// Literal rectangular restrictions, spline domains/denominators and C1
    /// admission remain enforced by the actual original coefficient operators.
    #[cfg(test)]
    pub(crate) fn carrier_jet(
        &self,
        uv: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<SurfaceEnclosure, GeometryError> {
        work.charge(1, cancel)?;
        for range in uv {
            Interval::new(range[0], range[1])?;
        }
        if !surface_continuity(self.source, uv, 1, &self.inventory, 0, work, cancel)? {
            return Err(GeometryError::new(
                GeometryErrorCode::AnalysisIndeterminate,
                "original coefficient carrier lacks closed-box C1 authority",
            ));
        }
        let j = surface_jet(self.source, uv, 2, &self.inventory, 0, work, cancel)?;
        Ok(SurfaceEnclosure {
            position: j[0][0],
            du: j[1][0],
            dv: j[0][1],
            duu: j[2][0],
            duv: j[1][1],
            dvv: j[0][2],
        })
    }

    /// Piecewise first bounds on the same captured coefficient sheet. C0 is
    /// required; an original derivative jump stays in the closed first hull.
    #[cfg(test)]
    pub(crate) fn carrier_piecewise_first(
        &self,
        uv: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<crate::parametric::SurfaceFirstEnclosure, GeometryError> {
        work.charge(1, cancel)?;
        for range in uv {
            Interval::new(range[0], range[1])?;
        }
        if !surface_continuity(self.source, uv, 0, &self.inventory, 0, work, cancel)? {
            return Err(GeometryError::new(
                GeometryErrorCode::AnalysisIndeterminate,
                "original coefficient carrier lacks closed-box C0 authority",
            ));
        }
        let j = surface_jet(self.source, uv, 1, &self.inventory, 0, work, cancel)?;
        Ok(crate::parametric::SurfaceFirstEnclosure {
            position: j[0][0],
            du: j[1][0],
            dv: j[0][1],
        })
    }
    /// Position-only image of the captured coefficient sheet. Every original
    /// span and literal/natural restriction still applies; neither continuity
    /// nor selected-material membership is inferred from this enclosure.
    #[cfg(test)]
    pub(crate) fn carrier_position_bounds(
        &self,
        uv: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[Interval; 3], GeometryError> {
        work.charge(1, cancel)?;
        for range in uv {
            Interval::new(range[0], range[1])?;
        }
        Ok(surface_jet(self.source, uv, 0, &self.inventory, 0, work, cancel)?[0][0])
    }

    pub(crate) fn position_bounds(
        &self,
        uv: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[Interval; 3], GeometryError> {
        Ok(self.evaluate_order(uv, 0, work, cancel)?[0][0])
    }
}

fn curve_jet(
    c: &Curve,
    range: [f64; 2],
    order: usize,
    inventory: &Inventory<'_>,
    depth: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[[Interval; 3]; 4], GeometryError> {
    curve_jet_selected(c, range, order, inventory, depth, None, work, cancel)
}
#[expect(
    clippy::too_many_arguments,
    reason = "Recursive coefficient evaluation keeps the literal source range, derivative order, retained inventory and selected trim identity explicit"
)]
fn curve_jet_selected(
    c: &Curve,
    range: [f64; 2],
    order: usize,
    inventory: &Inventory<'_>,
    depth: usize,
    lifted_trim: Option<*const Curve>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[[Interval; 3]; 4], GeometryError> {
    check_depth(depth, work, cancel)?;
    Interval::new(range[0], range[1])?;
    let t = Interval::new(range[0], range[1])?;
    let mut out = [[ZERO; 3]; 4];
    match c {
        Curve::Line {
            origin,
            direction,
            magnitude,
        } => {
            finite(
                origin
                    .iter()
                    .copied()
                    .chain(direction.iter().copied())
                    .chain([*magnitude]),
            )?;
            let velocity = direction.map(|v| Interval::point(v).mul(Interval::point(*magnitude)));
            out[0] = add(origin.map(Interval::point), scale(velocity, t));
            if order >= 1 {
                out[1] = velocity;
            }
        }
        Curve::Circle { position, radius }
        | Curve::Ellipse {
            position,
            semi_axis_1: radius,
            semi_axis_2: _,
        } => {
            let ry = match c {
                Curve::Ellipse { semi_axis_2, .. } => *semi_axis_2,
                _ => *radius,
            };
            finite([*radius, ry])?;
            let p = placement(position)?;
            let (s, co) = sin_cos_interval(t);
            let cosines = [co, s.neg(), co.neg(), s];
            let sines = [s, co, s.neg(), co.neg()];
            for n in 0..=order {
                out[n] = add(
                    scale(p[1], Interval::point(*radius).mul(cosines[n])),
                    scale(p[2], Interval::point(ry).mul(sines[n])),
                );
            }
            out[0] = add(out[0], p[0]);
        }
        Curve::Parabola {
            position,
            focal_dist,
        } => {
            finite([*focal_dist])?;
            let p = placement(position)?;
            let f = Interval::point(*focal_dist);
            out[0] = add(
                p[0],
                add(
                    scale(p[1], f.mul(t.square())),
                    scale(p[2], Interval::point(2.0).mul(f).mul(t)),
                ),
            );
            if order >= 1 {
                out[1] = add(
                    scale(p[1], Interval::point(2.0).mul(f).mul(t)),
                    scale(p[2], Interval::point(2.0).mul(f)),
                );
            }
            if order >= 2 {
                out[2] = scale(p[1], Interval::point(2.0).mul(f));
            }
        }
        Curve::BSpline(s) => {
            return spline_curve(
                s,
                inventory.curve_knots(s, work, cancel)?,
                range,
                order,
                work,
                cancel,
            );
        }
        Curve::Polyline(points) => {
            if points.len() < 2 || range[0] < 0.0 || range[1] > (points.len() - 1) as f64 {
                return Err(unavailable(
                    "original polyline query exceeds its literal segments",
                ));
            }
            if order == 0 && range[0] == range[1] {
                let at = range[0] as usize;
                if at < points.len() && at as f64 == range[0] {
                    finite(points[at])?;
                    out[0] = points[at].map(Interval::point);
                    return Ok(out);
                }
            }
            let mut result: Option<[[Interval; 3]; 4]> = None;
            for i in 0..points.len() - 1 {
                work.charge(1, cancel)?;
                let a = i as f64;
                let b = (i + 1) as f64;
                if a > range[1] || b < range[0] {
                    continue;
                }
                finite(points[i].into_iter().chain(points[i + 1]))?;
                let delta = std::array::from_fn(|k| {
                    Interval::point(points[i + 1][k]).sub(Interval::point(points[i][k]))
                });
                let mut j = [[ZERO; 3]; 4];
                j[0] = add(
                    points[i].map(Interval::point),
                    scale(
                        delta,
                        Interval::new(range[0].max(a), range[1].min(b))?.sub(Interval::point(a)),
                    ),
                );
                if order >= 1 {
                    j[1] = delta;
                }
                result = Some(result.map_or(j, |old| hull_curve(old, j)));
            }
            return result
                .ok_or_else(|| unavailable("original polyline contains no selected segment"));
        }
        Curve::Trimmed { basis, .. } => {
            if lifted_trim != Some(c as *const Curve) {
                check_trim(c, range, work, cancel)?;
            }
            return curve_jet_selected(
                basis,
                range,
                order,
                inventory,
                depth + 1,
                lifted_trim,
                work,
                cancel,
            );
        }
        Curve::SurfaceCurve { curve_3d, .. } => {
            if let Some(pcurve) = preferred_source_pcurve(c)? {
                let uv = curve_jet_selected(
                    &pcurve.curve,
                    range,
                    order,
                    inventory,
                    depth + 1,
                    lifted_trim,
                    work,
                    cancel,
                )?;
                let box_uv = [[uv[0][0].lo, uv[0][0].hi], [uv[0][1].lo, uv[0][1].hi]];
                let surface = surface_jet(
                    &pcurve.surface,
                    box_uv,
                    order,
                    inventory,
                    depth + 1,
                    work,
                    cancel,
                )?;
                return lift(surface, uv, order);
            }
            return curve_jet_selected(
                curve_3d,
                range,
                order,
                inventory,
                depth + 1,
                lifted_trim,
                work,
                cancel,
            );
        }
        _ => {
            return Err(unsupported(
                "original curve expression has no qualified interval operand",
            ));
        }
    }
    for v in out.iter().flatten() {
        Interval::new(v.lo, v.hi)?;
    }
    Ok(out)
}
fn hull_curve(a: [[Interval; 3]; 4], b: [[Interval; 3]; 4]) -> [[Interval; 3]; 4] {
    std::array::from_fn(|n| std::array::from_fn(|k| a[n][k].hull(b[n][k])))
}
fn spline_curve(
    s: &source::BSplineCurve,
    knots: &[f64],
    range: [f64; 2],
    order: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[[Interval; 3]; 4], GeometryError> {
    let p = s.degree;
    let count = s.points.len();
    validate_knots(knots, p, count, range, work, cancel)?;
    let mut result = None;
    for span in p..count {
        work.charge(1, cancel)?;
        if knots[span] == knots[span + 1] || knots[span] > range[1] || knots[span + 1] < range[0] {
            continue;
        }
        let basis = basis_jet(
            knots,
            p,
            span,
            [range[0].max(knots[span]), range[1].min(knots[span + 1])],
            order,
            work,
            cancel,
        )?;
        let origin = s.points[span - p];
        finite(origin)?;
        let mut h = [[ZERO; 4]; 4];
        let mut weight_hull: Option<Interval> = None;
        #[expect(
            clippy::needless_range_loop,
            reason = "Original control points and every derivative basis row share this span-relative coefficient index and accumulation order"
        )]
        for i in 0..=p {
            work.charge(1, cancel)?;
            let at = span - p + i;
            finite(s.points[at])?;
            let w = s.weights.as_ref().map_or(1.0, |weights| weights[at]);
            finite([w])?;
            let w = Interval::point(w);
            weight_hull = Some(weight_hull.map_or(w, |old| old.hull(w)));
            let coefficients: [Interval; 4] = std::array::from_fn(|k| {
                if k == 3 {
                    w
                } else {
                    Interval::point(s.points[at][k])
                        .sub(Interval::point(origin[k]))
                        .mul(w)
                }
            });
            for n in 0..=order {
                for k in 0..4 {
                    h[n][k] = h[n][k].add(basis[i][n].mul(coefficients[k]));
                }
            }
        }
        if s.weights.is_none() {
            for row in &mut h {
                row[3] = ZERO;
            }
            h[0][3] = ONE;
        }
        h[0][3] = h[0][3]
            .intersect(weight_hull.ok_or_else(|| conditioning("empty original curve support"))?)?;
        if h[0][3].contains_zero() {
            return Err(conditioning(
                "original rational curve denominator not excluded from zero",
            ));
        }
        let mut j = [[ZERO; 3]; 4];
        for n in 0..=order {
            for k in 0..3 {
                let mut value = h[n][k];
                for d in 1..=n {
                    value = value.sub(
                        Interval::point(binomial(n, d))
                            .mul(h[d][3])
                            .mul(j[n - d][k]),
                    );
                }
                j[n][k] = value.div(h[0][3])?;
            }
        }
        j[0] = add(j[0], origin.map(Interval::point));
        for v in j.iter().flatten() {
            Interval::new(v.lo, v.hi)?;
        }
        result = Some(result.map_or(j, |old| hull_curve(old, j)));
    }
    result.ok_or_else(|| unavailable("original curve has no complete selected knot span"))
}

fn surface_jet(
    s: &Surface,
    uv: [[f64; 2]; 2],
    order: usize,
    inventory: &Inventory<'_>,
    depth: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SurfaceTensor, GeometryError> {
    check_depth(depth, work, cancel)?;
    for r in uv {
        Interval::new(r[0], r[1])?;
    }
    let u = Interval::new(uv[0][0], uv[0][1])?;
    let v = Interval::new(uv[1][0], uv[1][1])?;
    let mut out = [[[ZERO; 3]; 4]; 4];
    match s {
        Surface::BSpline(s) => {
            let (uk, vk) = inventory.surface_knots(s, work, cancel)?;
            return spline_surface(s, uk, vk, uv, order, work, cancel);
        }
        Surface::RectangularTrimmed {
            basis,
            u1,
            u2,
            v1,
            v2,
            ..
        } => {
            finite([*u1, *u2, *v1, *v2])?;
            if u.lo < u1.min(*u2) || u.hi > u1.max(*u2) || v.lo < v1.min(*v2) || v.hi > v1.max(*v2)
            {
                return Err(unavailable(
                    "original rectangular trim restriction exceeded",
                ));
            }
            return surface_jet(basis, uv, order, inventory, depth + 1, work, cancel);
        }
        Surface::Extrusion {
            profile,
            direction,
            magnitude,
        } => {
            finite(direction.iter().copied().chain([*magnitude]))?;
            let p = curve_jet(profile, uv[0], order, inventory, depth + 1, work, cancel)?;
            let vector = direction.map(|x| Interval::point(x).mul(Interval::point(*magnitude)));
            for a in 0..=order {
                out[a][0] = p[a];
            }
            out[0][0] = add(out[0][0], scale(vector, v));
            if order >= 1 {
                out[0][1] = vector;
            }
        }
        Surface::Revolution { profile, axis } => {
            finite(axis.origin.into_iter().chain(axis.direction))?;
            let p = curve_jet(profile, uv[1], order, inventory, depth + 1, work, cancel)?;
            let a = axis.direction.map(Interval::point);
            let origin = axis.origin.map(Interval::point);
            let (si, co) = sin_cos_interval(u);
            let cosines = [co, si.neg(), co.neg(), si];
            let sines = [si, co, si.neg(), co.neg()];
            for du in 0..=order {
                for dv in 0..=order - du {
                    let q = if dv == 0 {
                        std::array::from_fn(|k| p[0][k].sub(origin[k]))
                    } else {
                        p[dv]
                    };
                    let dot = (0..3).fold(ZERO, |sum, k| sum.add(a[k].mul(q[k])));
                    let axial = scale(a, dot);
                    let perpendicular = std::array::from_fn(|k| q[k].sub(axial[k]));
                    let mut value = add(
                        scale(perpendicular, cosines[du]),
                        scale(cross(a, q), sines[du]),
                    );
                    if du == 0 {
                        value = add(value, axial);
                    }
                    if du == 0 && dv == 0 {
                        value = add(value, origin);
                    }
                    out[du][dv] = value;
                }
            }
        }
        Surface::Plane { position } => {
            let p = placement(position)?;
            out[0][0] = add(p[0], add(scale(p[1], u), scale(p[2], v)));
            if order >= 1 {
                out[1][0] = p[1];
                out[0][1] = p[2];
            }
        }
        Surface::Cylinder { position, radius }
        | Surface::Cone {
            position, radius, ..
        }
        | Surface::Sphere { position, radius }
        | Surface::Torus {
            position,
            major_radius: radius,
            ..
        }
        | Surface::DegenerateTorus {
            position,
            major_radius: radius,
            ..
        } => {
            finite([*radius])?;
            let p = placement(position)?;
            let (su, cu) = sin_cos_interval(u);
            let cosine_u = [cu, su.neg(), cu.neg(), su];
            let sine_u = [su, cu, su.neg(), cu.neg()];
            let (radial, axial) = match s {
                Surface::Cylinder { .. } => (
                    [Interval::point(*radius), ZERO, ZERO, ZERO],
                    [v, ONE, ZERO, ZERO],
                ),
                Surface::Cone { semi_angle, .. } => {
                    finite([*semi_angle])?;
                    let (sa, ca) = sin_cos_interval(Interval::point(*semi_angle));
                    let slope = sa.div(ca)?;
                    (
                        [
                            Interval::point(*radius).add(v.mul(slope)),
                            slope,
                            ZERO,
                            ZERO,
                        ],
                        [v, ONE, ZERO, ZERO],
                    )
                }
                _ => {
                    let (sv, cv) = sin_cos_interval(v);
                    let (major, minor) = match s {
                        Surface::Sphere { .. } => (0.0, *radius),
                        Surface::Torus { minor_radius, .. }
                        | Surface::DegenerateTorus { minor_radius, .. } => (*radius, *minor_radius),
                        _ => unreachable!(),
                    };
                    finite([major, minor])?;
                    let r = Interval::point(minor);
                    (
                        [
                            Interval::point(major).add(r.mul(cv)),
                            r.mul(sv.neg()),
                            r.mul(cv.neg()),
                            r.mul(sv),
                        ],
                        [r.mul(sv), r.mul(cv), r.mul(sv.neg()), r.mul(cv.neg())],
                    )
                }
            };
            for a in 0..=order {
                for b in 0..=order - a {
                    let direction = add(scale(p[1], cosine_u[a]), scale(p[2], sine_u[a]));
                    out[a][b] = scale(direction, radial[b]);
                    if a == 0 {
                        out[a][b] = add(out[a][b], scale(p[3], axial[b]));
                    }
                    if a == 0 && b == 0 {
                        out[a][b] = add(out[a][b], p[0]);
                    }
                }
            }
        }
        _ => {
            return Err(unsupported(
                "original surface expression has no qualified interval operand",
            ));
        }
    }
    for row in &out {
        for derivative in row {
            for value in derivative {
                Interval::new(value.lo, value.hi)?;
            }
        }
    }
    Ok(out)
}
fn spline_surface(
    s: &source::BSplineSurface,
    uk: &[f64],
    vk: &[f64],
    uv: [[f64; 2]; 2],
    order: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SurfaceTensor, GeometryError> {
    let p = s.u_degree;
    let q = s.v_degree;
    let nu = s.points.len();
    let nv = s.points[0].len();
    validate_knots(uk, p, nu, uv[0], work, cancel)?;
    validate_knots(vk, q, nv, uv[1], work, cancel)?;
    let mut result: Option<SurfaceTensor> = None;
    for us in p..nu {
        work.charge(1, cancel)?;
        if uk[us] == uk[us + 1] || uk[us] > uv[0][1] || uk[us + 1] < uv[0][0] {
            continue;
        }
        let ub = basis_jet(
            uk,
            p,
            us,
            [uv[0][0].max(uk[us]), uv[0][1].min(uk[us + 1])],
            order,
            work,
            cancel,
        )?;
        for vs in q..nv {
            work.charge(1, cancel)?;
            if vk[vs] == vk[vs + 1] || vk[vs] > uv[1][1] || vk[vs + 1] < uv[1][0] {
                continue;
            }
            let vb = basis_jet(
                vk,
                q,
                vs,
                [uv[1][0].max(vk[vs]), uv[1][1].min(vk[vs + 1])],
                order,
                work,
                cancel,
            )?;
            let origin = s.points[us - p][vs - q];
            finite(origin)?;
            let mut h = [[[ZERO; 4]; 4]; 4];
            let mut weights: Option<Interval> = None;
            #[expect(
                clippy::needless_range_loop,
                reason = "The original surface coefficient net and derivative basis tables share explicit u/v indices and a fixed summation order"
            )]
            for i in 0..=p {
                for j in 0..=q {
                    work.charge(1, cancel)?;
                    let at = [us - p + i, vs - q + j];
                    let point = s.points[at[0]][at[1]];
                    finite(point)?;
                    let w = s
                        .weights
                        .as_ref()
                        .map_or(1.0, |weights| weights[at[0]][at[1]]);
                    finite([w])?;
                    let w = Interval::point(w);
                    weights = Some(weights.map_or(w, |old| old.hull(w)));
                    let c: [Interval; 4] = std::array::from_fn(|k| {
                        if k == 3 {
                            w
                        } else {
                            Interval::point(point[k])
                                .sub(Interval::point(origin[k]))
                                .mul(w)
                        }
                    });
                    for a in 0..=order {
                        for b in 0..=order - a {
                            let basis = ub[i][a].mul(vb[j][b]);
                            for k in 0..4 {
                                h[a][b][k] = h[a][b][k].add(basis.mul(c[k]));
                            }
                        }
                    }
                }
            }
            if s.weights.is_none() {
                #[expect(
                    clippy::needless_range_loop,
                    reason = "Initialize exactly the triangular mixed-derivative catalog in the homogeneous weight component"
                )]
                for a in 0..=order {
                    for b in 0..=order - a {
                        h[a][b][3] = ZERO;
                    }
                }
                h[0][0][3] = ONE;
            }
            h[0][0][3] = h[0][0][3].intersect(
                weights.ok_or_else(|| conditioning("empty original surface support"))?,
            )?;
            if h[0][0][3].contains_zero() {
                return Err(conditioning(
                    "original rational surface denominator not excluded from zero",
                ));
            }
            let mut jet = [[[ZERO; 3]; 4]; 4];
            for total in 0..=order {
                for a in 0..=total {
                    let b = total - a;
                    for k in 0..3 {
                        let mut value = h[a][b][k];
                        for i in 0..=a {
                            for j in 0..=b {
                                if i == 0 && j == 0 {
                                    continue;
                                }
                                value = value.sub(
                                    Interval::point(binomial(a, i) * binomial(b, j))
                                        .mul(h[i][j][3])
                                        .mul(jet[a - i][b - j][k]),
                                );
                            }
                        }
                        jet[a][b][k] = value.div(h[0][0][3])?;
                    }
                }
            }
            jet[0][0] = add(jet[0][0], origin.map(Interval::point));
            for row in &jet {
                for derivative in row {
                    for v in derivative {
                        Interval::new(v.lo, v.hi)?;
                    }
                }
            }
            result = Some(result.map_or(jet, |old| {
                std::array::from_fn(|a| {
                    std::array::from_fn(|b| {
                        std::array::from_fn(|k| old[a][b][k].hull(jet[a][b][k]))
                    })
                })
            }));
        }
    }
    result.ok_or_else(|| unavailable("original surface has no complete selected knot span"))
}
pub(crate) fn lift(
    s: SurfaceTensor,
    uv: [[Interval; 3]; 4],
    order: usize,
) -> Result<[[Interval; 3]; 4], GeometryError> {
    let mut out = [[ZERO; 3]; 4];
    out[0] = s[0][0];
    for k in 0..3 {
        if order >= 1 {
            out[1][k] = s[1][0][k].mul(uv[1][0]).add(s[0][1][k].mul(uv[1][1]));
        }
        if order >= 2 {
            out[2][k] = s[2][0][k]
                .mul(uv[1][0].square())
                .add(
                    Interval::point(2.0)
                        .mul(s[1][1][k])
                        .mul(uv[1][0])
                        .mul(uv[1][1]),
                )
                .add(s[0][2][k].mul(uv[1][1].square()))
                .add(s[1][0][k].mul(uv[2][0]))
                .add(s[0][1][k].mul(uv[2][1]));
        }
        if order >= 3 {
            let u = uv[1][0];
            let v = uv[1][1];
            let second = s[2][0][k]
                .mul(u)
                .mul(uv[2][0])
                .add(s[1][1][k].mul(uv[2][0].mul(v).add(u.mul(uv[2][1]))))
                .add(s[0][2][k].mul(v).mul(uv[2][1]));
            out[3][k] = s[1][0][k]
                .mul(uv[3][0])
                .add(s[0][1][k].mul(uv[3][1]))
                .add(Interval::point(3.0).mul(second))
                .add(s[3][0][k].mul(u.square()).mul(u))
                .add(Interval::point(3.0).mul(s[2][1][k]).mul(u.square()).mul(v))
                .add(Interval::point(3.0).mul(s[1][2][k]).mul(u).mul(v.square()))
                .add(s[0][3][k].mul(v.square()).mul(v));
        }
    }
    for v in out.iter().flatten() {
        Interval::new(v.lo, v.hi)?;
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) fn original_curve_on_surface_enclosure(
    surface: &PreparedOriginalSurface<'_>,
    chart: &PreparedOriginalCurve<'_>,
    range: [f64; 2],
    parameter_map: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<CurveThirdEnclosure, GeometryError> {
    work.charge(1, cancel)?;
    finite(parameter_map)?;
    let t = Interval::new(range[0], range[1])?;
    let mapped = Interval::point(parameter_map[0]).add(Interval::point(parameter_map[1]).mul(t));
    let mut uv = chart.evaluate_order([mapped.lo, mapped.hi], 3, work, cancel)?;
    let slope = Interval::point(parameter_map[1]);
    let mut factor = slope;
    for derivative in uv.iter_mut().skip(1) {
        for coordinate in derivative {
            *coordinate = coordinate.mul(factor);
        }
        factor = factor.mul(slope);
    }
    let box_uv = [[uv[0][0].lo, uv[0][0].hi], [uv[0][1].lo, uv[0][1].hi]];
    let s = surface.evaluate_order(box_uv, 3, work, cancel)?;
    let j = lift(s, uv, 3)?;
    Ok(CurveThirdEnclosure {
        jet: CurveEnclosure {
            position: j[0],
            derivative: j[1],
            second_derivative: j[2],
        },
        third_derivative: j[3],
    })
}

fn curve_axis<'a>(
    curve: &Curve,
    inventory: &'a Inventory<'_>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    depth: usize,
) -> Result<(&'a [f64], usize), GeometryError> {
    check_depth(depth, work, cancel)?;
    match curve {
        Curve::BSpline(s) => Ok((inventory.curve_knots(s, work, cancel)?, s.degree)),
        Curve::Trimmed { basis, .. } => curve_axis(basis, inventory, work, cancel, depth + 1),
        Curve::SurfaceCurve { curve_3d, .. } => {
            let selected = preferred_source_pcurve(curve)?
                .map_or(curve_3d.as_ref(), |pcurve| pcurve.curve.as_ref());
            curve_axis(selected, inventory, work, cancel, depth + 1)
        }
        Curve::Polyline(points) => Ok((inventory.polyline_knots(points, work, cancel)?, 0)),
        Curve::Line { .. }
        | Curve::Circle { .. }
        | Curve::Ellipse { .. }
        | Curve::Parabola { .. } => Ok((&[], 0)),
        _ => Err(unsupported(
            "original curve knot inventory is not available",
        )),
    }
}
fn surface_axes<'a>(
    surface: &Surface,
    inventory: &'a Inventory<'_>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    depth: usize,
) -> Result<[(&'a [f64], usize); 2], GeometryError> {
    check_depth(depth, work, cancel)?;
    match surface {
        Surface::BSpline(s) => {
            let (u, v) = inventory.surface_knots(s, work, cancel)?;
            Ok([(u, s.u_degree), (v, s.v_degree)])
        }
        Surface::RectangularTrimmed { basis, .. } => {
            surface_axes(basis, inventory, work, cancel, depth + 1)
        }
        Surface::Extrusion { profile, .. } => Ok([
            curve_axis(profile, inventory, work, cancel, depth + 1)?,
            (&[], 0),
        ]),
        Surface::Revolution { profile, .. } => Ok([
            (&[], 0),
            curve_axis(profile, inventory, work, cancel, depth + 1)?,
        ]),
        Surface::Plane { .. }
        | Surface::Cylinder { .. }
        | Surface::Cone { .. }
        | Surface::Sphere { .. }
        | Surface::Torus { .. }
        | Surface::DegenerateTorus { .. } => Ok([(&[], 0); 2]),
        _ => Err(unsupported(
            "original surface knot inventory is not available",
        )),
    }
}
fn intrinsic_continuity(
    knots: &[f64],
    degree: usize,
    range: [f64; 2],
    required: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    if knots.is_empty() {
        return Ok(true);
    }
    let count = knots.len() - degree - 1;
    let mut i = 0;
    while i < knots.len() {
        work.charge(1, cancel)?;
        let start = i;
        let value = knots[i];
        while i < knots.len() && knots[i] == value {
            work.charge(1, cancel)?;
            i += 1;
        }
        if value > knots[degree]
            && value < knots[count]
            && value >= range[0]
            && value <= range[1]
            && i - start + required > degree
        {
            return Ok(false);
        }
    }
    Ok(true)
}
fn original_spline_curve_continuity(
    s: &source::BSplineCurve,
    knots: &[f64],
    range: [f64; 2],
    required: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    use crate::exact_coefficient::ExactCoefficient;
    let degree = s.degree;
    let count = s.points.len();
    let mut start = 0;
    while start < knots.len() {
        work.charge(1, cancel)?;
        let value = knots[start];
        let mut end = start + 1;
        while end < knots.len() && knots[end] == value {
            work.charge(1, cancel)?;
            end += 1;
        }
        if value > knots[degree] && value < knots[count] && value >= range[0] && value <= range[1] {
            let multiplicity = end - start;
            if required == 0 && multiplicity == degree + 1 {
                let left = end - degree - 2;
                if s.points[left] != s.points[left + 1] {
                    return Ok(false);
                }
            } else if required == 1 && multiplicity >= degree {
                if degree == 0 || multiplicity != degree {
                    return Ok(false);
                }
                let center = end - degree - 1;
                if center == 0 || center + 1 >= count {
                    return Ok(false);
                }
                let Some(left_span) = ExactCoefficient::difference(value, knots[start - 1]) else {
                    return Ok(false);
                };
                let Some(right_span) = ExactCoefficient::difference(knots[end], value) else {
                    return Ok(false);
                };
                let wl = s.weights.as_ref().map_or(1.0, |w| w[center - 1]);
                let wr = s.weights.as_ref().map_or(1.0, |w| w[center + 1]);
                for k in 0..3 {
                    let Some(left) =
                        ExactCoefficient::difference(s.points[center][k], s.points[center - 1][k])
                    else {
                        return Ok(false);
                    };
                    let Some(right) =
                        ExactCoefficient::difference(s.points[center + 1][k], s.points[center][k])
                    else {
                        return Ok(false);
                    };
                    let Some(left) = left.product(&ExactCoefficient::point(wl), work, cancel)?
                    else {
                        return Ok(false);
                    };
                    let Some(right) = right.product(&ExactCoefficient::point(wr), work, cancel)?
                    else {
                        return Ok(false);
                    };
                    let Some(mut left) = left.product(&right_span, work, cancel)? else {
                        return Ok(false);
                    };
                    let Some(right) = right.product(&left_span, work, cancel)? else {
                        return Ok(false);
                    };
                    if left.subtract(&right).is_none() || left.sign() != 0 {
                        return Ok(false);
                    }
                }
            } else if multiplicity + required > degree {
                return Ok(false);
            }
        }
        start = end;
    }
    Ok(true)
}
fn curve_continuity(
    curve: &Curve,
    range: [f64; 2],
    required: usize,
    inventory: &Inventory<'_>,
    depth: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    curve_continuity_selected(curve, range, required, inventory, depth, None, work, cancel)
}
#[expect(
    clippy::too_many_arguments,
    reason = "Continuity recursion retains the requested order and selected trim branch independently of its source inventory and query budget"
)]
fn curve_continuity_selected(
    curve: &Curve,
    range: [f64; 2],
    required: usize,
    inventory: &Inventory<'_>,
    depth: usize,
    lifted_trim: Option<*const Curve>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    check_depth(depth, work, cancel)?;
    match curve {
        Curve::BSpline(s) => original_spline_curve_continuity(
            s,
            inventory.curve_knots(s, work, cancel)?,
            range,
            required,
            work,
            cancel,
        ),
        Curve::Trimmed { basis, .. } => curve_continuity_selected(
            basis,
            range,
            required,
            inventory,
            depth + 1,
            lifted_trim,
            work,
            cancel,
        ),
        Curve::SurfaceCurve { curve_3d, .. } => {
            if let Some(pcurve) = preferred_source_pcurve(curve)? {
                if !curve_continuity_selected(
                    &pcurve.curve,
                    range,
                    required,
                    inventory,
                    depth + 1,
                    lifted_trim,
                    work,
                    cancel,
                )? {
                    return Ok(false);
                }
                let uv = curve_jet_selected(
                    &pcurve.curve,
                    range,
                    0,
                    inventory,
                    depth + 1,
                    lifted_trim,
                    work,
                    cancel,
                )?;
                return surface_continuity(
                    &pcurve.surface,
                    [[uv[0][0].lo, uv[0][0].hi], [uv[0][1].lo, uv[0][1].hi]],
                    required,
                    inventory,
                    depth + 1,
                    work,
                    cancel,
                );
            }
            curve_continuity_selected(
                curve_3d,
                range,
                required,
                inventory,
                depth + 1,
                lifted_trim,
                work,
                cancel,
            )
        }
        Curve::Polyline(points) => {
            if required == 0 {
                return Ok(true);
            }
            use crate::exact_coefficient::ExactCoefficient;
            for i in 1..points.len().saturating_sub(1) {
                work.charge(1, cancel)?;
                if (i as f64) < range[0] || (i as f64) > range[1] {
                    continue;
                }
                #[expect(
                    clippy::needless_range_loop,
                    reason = "Compare adjacent polyline control points in the identical native coordinate axis while keeping both neighbor indices visible"
                )]
                for k in 0..3 {
                    let Some(mut left) =
                        ExactCoefficient::difference(points[i][k], points[i - 1][k])
                    else {
                        return Ok(false);
                    };
                    let Some(right) = ExactCoefficient::difference(points[i + 1][k], points[i][k])
                    else {
                        return Ok(false);
                    };
                    if left.subtract(&right).is_none() || left.sign() != 0 {
                        return Ok(false);
                    }
                }
            }
            Ok(true)
        }
        Curve::Line { .. }
        | Curve::Circle { .. }
        | Curve::Ellipse { .. }
        | Curve::Parabola { .. } => Ok(true),
        _ => Err(unsupported("original curve continuity is not implemented")),
    }
}
fn surface_continuity(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    required: usize,
    inventory: &Inventory<'_>,
    depth: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    check_depth(depth, work, cancel)?;
    match surface {
        Surface::BSpline(s) => {
            let (u, v) = inventory.surface_knots(s, work, cancel)?;
            // Intrinsic knot smoothness is sufficient. Non-intrinsic tensor
            // joins need the owning exact whole-trace identity certificate.
            Ok(
                intrinsic_continuity(u, s.u_degree, uv[0], required, work, cancel)?
                    && intrinsic_continuity(v, s.v_degree, uv[1], required, work, cancel)?,
            )
        }
        Surface::RectangularTrimmed { basis, .. } => {
            surface_continuity(basis, uv, required, inventory, depth + 1, work, cancel)
        }
        Surface::Extrusion { profile, .. } => {
            curve_continuity(profile, uv[0], required, inventory, depth + 1, work, cancel)
        }
        Surface::Revolution { profile, .. } => {
            curve_continuity(profile, uv[1], required, inventory, depth + 1, work, cancel)
        }
        Surface::Plane { .. }
        | Surface::Cylinder { .. }
        | Surface::Cone { .. }
        | Surface::Sphere { .. }
        | Surface::Torus { .. }
        | Surface::DegenerateTorus { .. } => Ok(true),
        _ => Err(unsupported(
            "original surface continuity is not implemented",
        )),
    }
}
impl PreparedOriginalCurve<'_> {
    pub(crate) fn parameter_axis(
        &self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(&[f64], usize), GeometryError> {
        curve_axis(self.source, &self.inventory, work, cancel, 0)
    }
    pub(crate) fn span_is_continuous(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.evaluate_order(range, 0, work, cancel)?;
        curve_continuity_selected(
            self.source,
            range,
            0,
            &self.inventory,
            0,
            self.inventory
                .declared_branch
                .as_ref()
                .map(|b| b.trimmed as *const Curve),
            work,
            cancel,
        )
    }
    pub(crate) fn span_is_smooth(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.evaluate_order(range, 0, work, cancel)?;
        curve_continuity_selected(
            self.source,
            range,
            1,
            &self.inventory,
            0,
            self.inventory
                .declared_branch
                .as_ref()
                .map(|b| b.trimmed as *const Curve),
            work,
            cancel,
        )
    }
}
impl PreparedOriginalSurface<'_> {
    pub(crate) fn parameter_axes(
        &self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[(&[f64], usize); 2], GeometryError> {
        surface_axes(self.source, &self.inventory, work, cancel, 0)
    }
    pub(crate) fn span_is_continuous(
        &self,
        uv: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.evaluate_order(uv, 0, work, cancel)?;
        surface_continuity(self.source, uv, 0, &self.inventory, 0, work, cancel)
    }
    pub(crate) fn span_is_smooth(
        &self,
        uv: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.evaluate_order(uv, 0, work, cancel)?;
        surface_continuity(self.source, uv, 1, &self.inventory, 0, work, cancel)
    }
}

/// Certified branch of atan2(y,x). The libm value proposes a bracket only;
/// endpoint signs, nonzero derivative and interval Newton enclose the actual
/// angular root for every coefficient value in the input intervals.
pub(crate) fn original_angular_parameter(
    y: Interval,
    x: Interval,
    maximum_width_rad: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Interval, GeometryError> {
    work.charge(1, cancel)?;
    Interval::new(x.lo, x.hi)?;
    Interval::new(y.lo, y.hi)?;
    if !maximum_width_rad.is_finite() || maximum_width_rad <= 0.0 || maximum_width_rad > 0.25 {
        return Err(conditioning("invalid original angular isolation policy"));
    }
    if x.contains_zero() && y.contains_zero() {
        return Err(unavailable(
            "original angular endpoint is not separated from the axis",
        ));
    }
    if y == ZERO && x.lo > 0.0 {
        return Ok(ZERO);
    }
    let seed = y.mid().atan2(x.mid());
    let mut bracket = Interval::point(seed).add(Interval {
        lo: -0.25,
        hi: 0.25,
    });
    let f = |angle: Interval| {
        let (s, c) = sin_cos_interval(angle);
        x.mul(s).sub(y.mul(c))
    };
    let derivative = |angle: Interval| {
        let (s, c) = sin_cos_interval(angle);
        x.mul(c).add(y.mul(s))
    };
    if f(Interval::point(bracket.lo)).hi >= 0.0
        || f(Interval::point(bracket.hi)).lo <= 0.0
        || derivative(bracket).lo <= 0.0
    {
        return Err(conditioning(
            "original angular branch lacks strict existence/uniqueness bounds",
        ));
    }
    for _ in 0..64 {
        work.charge(1, cancel)?;
        if bracket.width() <= maximum_width_rad {
            return Ok(bracket);
        }
        let midpoint = bracket.mid();
        let slope = derivative(bracket);
        if slope.lo <= 0.0 {
            return Err(conditioning(
                "original angular root derivative lost exclusion",
            ));
        }
        let newton = Interval::point(midpoint).sub(f(Interval::point(midpoint)).div(slope)?);
        let contracted = bracket.intersect(newton)?;
        if contracted.width() >= bracket.width() {
            return Err(conditioning(
                "original angular root exhausted coefficient/parameter precision",
            ));
        }
        bracket = contracted;
    }
    Err(conditioning(
        "original angular root isolation depth exhausted",
    ))
}

/// Canonical analytic pullback expression, not an original-trim certificate.
/// The expression is bound to the retained carrier and original vertex point;
/// its nonzero residual must be consumed by the owning forward/joint proof.
#[cfg(test)]
#[expect(
    dead_code,
    reason = "The original pullback probe retains its carrier, phase lifts and full lifted position together with the tested residual"
)]
pub(crate) struct OriginalSurfacePullback<'a> {
    pub(crate) carrier: &'a Surface,
    pub(crate) point_mm: [f64; 3],
    pub(crate) phase_lifts: [i32; 2],
    pub(crate) uv: [Interval; 2],
    pub(crate) position_mm: [Interval; 3],
    pub(crate) residual_mm: [Interval; 3],
}
#[cfg(test)]
fn dot(a: [Interval; 3], b: [Interval; 3]) -> Interval {
    (0..3).fold(ZERO, |v, k| v.add(a[k].mul(b[k])))
}
#[cfg(test)]
fn phase_lift(angle: Interval, turns: i32) -> Result<Interval, GeometryError> {
    let pi = Interval {
        lo: crate::parametric::down(std::f64::consts::PI),
        hi: crate::parametric::up(std::f64::consts::PI),
    };
    let lifted = angle.add(
        pi.mul(Interval::point(2.0))
            .mul(Interval::point(turns as f64)),
    );
    Interval::new(lifted.lo, lifted.hi)
}
#[cfg(test)]
pub(crate) fn original_analytic_surface_pullback<'a>(
    surface: &'a PreparedOriginalSurface<'a>,
    point_mm: [f64; 3],
    phase_lifts: [i32; 2],
    maximum_angular_width_rad: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<OriginalSurfacePullback<'a>, GeometryError> {
    work.charge(1, cancel)?;
    finite(point_mm)?;
    let mut source = surface.source;
    for depth in 0..MAX_DEPTH {
        work.charge(1, cancel)?;
        match source {
            Surface::RectangularTrimmed { basis, .. } => source = basis,
            _ => break,
        }
        if depth + 1 == MAX_DEPTH {
            return Err(resource());
        }
    }
    let position = match source {
        Surface::Plane { position }
        | Surface::Cylinder { position, .. }
        | Surface::Cone { position, .. }
        | Surface::Sphere { position, .. }
        | Surface::Torus { position, .. }
        | Surface::DegenerateTorus { position, .. } => position,
        _ => {
            return Err(unavailable(
                "original carrier has no analytic coefficient pullback",
            ));
        }
    };
    let p = placement(position)?;
    let displacement = std::array::from_fn(|k| Interval::point(point_mm[k]).sub(p[0][k]));
    let uv = if matches!(source, Surface::Plane { .. }) {
        if phase_lifts != [0, 0] {
            return Err(unavailable("a plane has no declared angular period"));
        }
        let xx = dot(p[1], p[1]);
        let xy = dot(p[1], p[2]);
        let yy = dot(p[2], p[2]);
        let determinant = xx.mul(yy).sub(xy.square());
        if determinant.lo <= 0.0 {
            return Err(conditioning(
                "original plane pullback Gram determinant is singular",
            ));
        }
        let x = dot(p[1], displacement);
        let y = dot(p[2], displacement);
        [
            x.mul(yy).sub(y.mul(xy)).div(determinant)?,
            y.mul(xx).sub(x.mul(xy)).div(determinant)?,
        ]
    } else {
        let cofactor = [cross(p[2], p[3]), cross(p[3], p[1]), cross(p[1], p[2])];
        let determinant = dot(p[1], cofactor[0]);
        if determinant.contains_zero() {
            return Err(conditioning("original analytic placement is singular"));
        }
        let local = [
            dot(cofactor[0], displacement).div(determinant)?,
            dot(cofactor[1], displacement).div(determinant)?,
            dot(cofactor[2], displacement).div(determinant)?,
        ];
        let mut x = local[0];
        let mut y = local[1];
        match source {
            Surface::Cylinder { radius, .. } => {
                finite([*radius])?;
                if *radius == 0.0 {
                    return Err(conditioning("original cylinder radius is zero"));
                }
                if *radius < 0.0 {
                    x = x.neg();
                    y = y.neg();
                }
            }
            Surface::Cone {
                radius, semi_angle, ..
            } => {
                finite([*radius, *semi_angle])?;
                let (s, c) = sin_cos_interval(Interval::point(*semi_angle));
                let radius = Interval::point(*radius).add(local[2].mul(s.div(c)?));
                if radius.contains_zero() {
                    return Err(unavailable(
                        "original cone apex needs source chart incidence, not an angular guess",
                    ));
                }
                if radius.hi < 0.0 {
                    x = x.neg();
                    y = y.neg();
                }
            }
            Surface::Sphere { radius, .. } if (!radius.is_finite() || *radius <= 0.0) => {
                return Err(conditioning("invalid original sphere radius"));
            }
            _ => {}
        }
        let u = phase_lift(
            original_angular_parameter(y, x, maximum_angular_width_rad, work, cancel)?,
            phase_lifts[0],
        )?;
        let v = match source {
            Surface::Cylinder { .. } | Surface::Cone { .. } => {
                if phase_lifts[1] != 0 {
                    return Err(unavailable("original analytic height has no period"));
                }
                local[2]
            }
            Surface::Sphere { .. } => {
                if phase_lifts[1] != 0 {
                    return Err(unavailable(
                        "original sphere latitude branch has no independent covering shift",
                    ));
                }
                original_angular_parameter(
                    local[2],
                    x.square().add(y.square()).sqrt()?,
                    maximum_angular_width_rad,
                    work,
                    cancel,
                )?
            }
            Surface::Torus {
                major_radius,
                minor_radius,
                ..
            }
            | Surface::DegenerateTorus {
                major_radius,
                minor_radius,
                ..
            } => {
                if !minor_radius.is_finite() || *minor_radius <= 0.0 {
                    return Err(conditioning("invalid original torus minor radius"));
                }
                let radial = x
                    .square()
                    .add(y.square())
                    .sqrt()?
                    .sub(Interval::point(*major_radius));
                phase_lift(
                    original_angular_parameter(
                        local[2],
                        radial,
                        maximum_angular_width_rad,
                        work,
                        cancel,
                    )?,
                    phase_lifts[1],
                )?
            }
            _ => {
                return Err(unavailable(
                    "original analytic pullback branch is unsupported",
                ));
            }
        };
        [u, v]
    };
    let position_mm =
        surface.position_bounds([[uv[0].lo, uv[0].hi], [uv[1].lo, uv[1].hi]], work, cancel)?;
    let residual_mm = std::array::from_fn(|k| position_mm[k].sub(Interval::point(point_mm[k])));
    Ok(OriginalSurfacePullback {
        carrier: surface.source,
        point_mm,
        phase_lifts,
        uv,
        position_mm,
        residual_mm,
    })
}

/// Actual affine physical curve, C(q)=(position+q*derivative)/denominator.
/// Every coefficient is an unrounded bounded exact expansion.
pub(crate) struct ExactAffineCurve {
    pub(crate) position: [crate::exact_coefficient::ExactCoefficient; 3],
    pub(crate) derivative: [crate::exact_coefficient::ExactCoefficient; 3],
    pub(crate) denominator: crate::exact_coefficient::ExactCoefficient,
}
impl ExactAffineCurve {
    /// Algebraic identity only; callers validate the owning curve's active domain.
    pub(crate) fn endpoint_coordinate_equal(
        &self,
        parameter: f64,
        axis: usize,
        expected: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        use crate::exact_coefficient::ExactCoefficient as E;
        work.charge(1, cancel)?;
        if axis >= 3 {
            return Err(conditioning("exact affine coordinate axis exceeds three"));
        }
        finite([parameter, expected])?;
        if self.denominator.sign() == 0 {
            return Err(conditioning("exact affine source denominator is zero"));
        }
        let Some(mut value) = self.derivative[axis].product(&E::point(parameter), work, cancel)?
        else {
            return Ok(false);
        };
        for term in &self.position[axis].terms[..self.position[axis].len] {
            if value.push(*term).is_none() {
                return Ok(false);
            }
        }
        let Some(reference) = self
            .denominator
            .product(&E::point(expected), work, cancel)?
        else {
            return Ok(false);
        };
        Ok(value.subtract(&reference).is_some() && value.sign() == 0)
    }
}
pub(crate) fn exact_source_line_projection_coefficients(
    curve: &Curve,
    vertices: &[[f64; 3]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<ExactAffineCurve>, GeometryError> {
    use crate::exact_coefficient::ExactCoefficient as E;
    let Curve::Line {
        origin,
        direction,
        magnitude,
    } = curve
    else {
        return Err(unavailable(
            "exact LINE projection requires the retained literal LINE",
        ));
    };
    finite(
        origin
            .iter()
            .copied()
            .chain(direction.iter().copied())
            .chain(vertices.iter().flatten().copied())
            .chain([*magnitude]),
    )?;
    if *magnitude == 0.0 {
        return Err(conditioning("original LINE magnitude is zero"));
    }
    let mut denominator = E::point(0.0);
    let mut start = E::point(0.0);
    let mut delta = E::point(0.0);
    for k in 0..3 {
        let Some(square) = E::point(direction[k]).product(&E::point(direction[k]), work, cancel)?
        else {
            return Ok(None);
        };
        for term in &square.terms[..square.len] {
            if denominator.push(*term).is_none() {
                return Ok(None);
            }
        }
        let Some(a) = E::difference(vertices[0][k], origin[k]) else {
            return Ok(None);
        };
        let Some(b) = E::difference(vertices[1][k], vertices[0][k]) else {
            return Ok(None);
        };
        let Some(a) = a.product(&E::point(direction[k]), work, cancel)? else {
            return Ok(None);
        };
        let Some(b) = b.product(&E::point(direction[k]), work, cancel)? else {
            return Ok(None);
        };
        for term in &a.terms[..a.len] {
            if start.push(*term).is_none() {
                return Ok(None);
            }
        }
        for term in &b.terms[..b.len] {
            if delta.push(*term).is_none() {
                return Ok(None);
            }
        }
    }
    if denominator.sign() <= 0 {
        return Err(conditioning(
            "original LINE direction has zero squared norm",
        ));
    }
    let mut position = std::array::from_fn(|_| E::point(0.0));
    let mut derivative = std::array::from_fn(|_| E::point(0.0));
    for k in 0..3 {
        let Some(mut p) = E::point(origin[k]).product(&denominator, work, cancel)? else {
            return Ok(None);
        };
        let Some(v) = E::point(direction[k]).product(&start, work, cancel)? else {
            return Ok(None);
        };
        let Some(d) = E::point(direction[k]).product(&delta, work, cancel)? else {
            return Ok(None);
        };
        for term in &v.terms[..v.len] {
            if p.push(*term).is_none() {
                return Ok(None);
            }
        }
        position[k] = p;
        derivative[k] = d;
    }
    Ok(Some(ExactAffineCurve {
        position,
        derivative,
        denominator,
    }))
}
impl PreparedOriginalCurve<'_> {
    pub(crate) fn exact_affine_coefficients(
        &self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<ExactAffineCurve>, GeometryError> {
        Self::exact_expression_affine(self.source, 0, work, cancel)
    }
    fn exact_expression_affine(
        mut curve: &Curve,
        start_depth: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<ExactAffineCurve>, GeometryError> {
        use crate::exact_coefficient::ExactCoefficient as E;
        for depth in start_depth..MAX_DEPTH {
            check_depth(depth, work, cancel)?;
            match curve {
                Curve::Trimmed { basis, .. } => curve = basis,
                Curve::SurfaceCurve { curve_3d, .. } => {
                    if let Some(pcurve) = preferred_source_pcurve(curve)? {
                        let Some(chart) =
                            Self::exact_expression_affine(&pcurve.curve, depth + 1, work, cancel)?
                        else {
                            return Ok(None);
                        };
                        return exact_plane_curve_affine(
                            &pcurve.surface,
                            chart,
                            depth + 1,
                            work,
                            cancel,
                        );
                    }
                    curve = curve_3d;
                }
                Curve::Line {
                    origin,
                    direction,
                    magnitude,
                } => {
                    finite(
                        origin
                            .iter()
                            .copied()
                            .chain(direction.iter().copied())
                            .chain([*magnitude]),
                    )?;
                    let mut derivative = std::array::from_fn(|_| E::point(0.0));
                    for k in 0..3 {
                        let Some(d) =
                            E::point(direction[k]).product(&E::point(*magnitude), work, cancel)?
                        else {
                            return Ok(None);
                        };
                        derivative[k] = d;
                    }
                    return Ok(Some(ExactAffineCurve {
                        position: origin.map(E::point),
                        derivative,
                        denominator: E::point(1.0),
                    }));
                }
                Curve::BSpline(s) => {
                    if s.degree != 1
                        || s.points.len() != 2
                        || s.knots.len() != 2
                        || s.multiplicities.as_slice() != [2, 2]
                    {
                        return Ok(None);
                    }
                    let [a, b] = [s.knots[0], s.knots[1]];
                    finite([a, b].into_iter().chain(s.points.iter().flatten().copied()))?;
                    if a >= b {
                        return Ok(None);
                    }
                    if let Some(w) = &s.weights {
                        if w.len() != 2 {
                            return Err(conditioning(
                                "original affine spline weight census mismatch",
                            ));
                        }
                        finite(w.iter().copied())?;
                        if w[0] <= 0.0 || w[0] != w[1] {
                            return Ok(None);
                        }
                    }
                    let Some(denominator) = E::difference(b, a) else {
                        return Ok(None);
                    };
                    let mut position = std::array::from_fn(|_| E::point(0.0));
                    let mut derivative = std::array::from_fn(|_| E::point(0.0));
                    for k in 0..3 {
                        let Some(mut p) =
                            E::point(s.points[0][k]).product(&E::point(b), work, cancel)?
                        else {
                            return Ok(None);
                        };
                        let Some(q) =
                            E::point(s.points[1][k]).product(&E::point(a), work, cancel)?
                        else {
                            return Ok(None);
                        };
                        if p.subtract(&q).is_none() {
                            return Ok(None);
                        }
                        let Some(d) = E::difference(s.points[1][k], s.points[0][k]) else {
                            return Ok(None);
                        };
                        position[k] = p;
                        derivative[k] = d;
                    }
                    return Ok(Some(ExactAffineCurve {
                        position,
                        derivative,
                        denominator,
                    }));
                }
                _ => return Ok(None),
            }
        }
        Err(resource())
    }
    #[cfg(test)]
    pub(crate) fn tensor_coefficients(
        &self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<(&source::BSplineCurve, &[f64])>, GeometryError> {
        let mut curve = self.source;
        for depth in 0..MAX_DEPTH {
            check_depth(depth, work, cancel)?;
            match curve {
                Curve::Trimmed { basis, .. } => curve = basis,
                Curve::SurfaceCurve { curve_3d, .. } => {
                    if preferred_source_pcurve(curve)?.is_some() {
                        return Ok(None);
                    }
                    curve = curve_3d;
                }
                Curve::BSpline(s) => {
                    return Ok(Some((s, self.inventory.curve_knots(s, work, cancel)?)));
                }
                _ => return Ok(None),
            }
        }
        Err(resource())
    }
}
pub(crate) fn exact_plane_curve_affine(
    mut surface: &Surface,
    chart: ExactAffineCurve,
    start_depth: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<ExactAffineCurve>, GeometryError> {
    use crate::exact_coefficient::ExactCoefficient as E;
    for depth in start_depth..MAX_DEPTH {
        check_depth(depth, work, cancel)?;
        match surface {
            Surface::RectangularTrimmed { basis, .. } => surface = basis,
            Surface::Plane { position: frame } => {
                finite(
                    frame
                        .origin
                        .into_iter()
                        .chain(frame.axis)
                        .chain(frame.ref_direction),
                )?;
                let mut position = std::array::from_fn(|_| E::point(0.0));
                let mut derivative = std::array::from_fn(|_| E::point(0.0));
                for k in 0..3 {
                    let x = E::point(frame.ref_direction[k]);
                    let Some(mut y) = E::point(frame.axis[(k + 1) % 3]).product(
                        &E::point(frame.ref_direction[(k + 2) % 3]),
                        work,
                        cancel,
                    )?
                    else {
                        return Ok(None);
                    };
                    let Some(other) = E::point(frame.axis[(k + 2) % 3]).product(
                        &E::point(frame.ref_direction[(k + 1) % 3]),
                        work,
                        cancel,
                    )?
                    else {
                        return Ok(None);
                    };
                    if y.subtract(&other).is_none() {
                        return Ok(None);
                    }
                    let Some(mut p) =
                        E::point(frame.origin[k]).product(&chart.denominator, work, cancel)?
                    else {
                        return Ok(None);
                    };
                    let mut d = E::point(0.0);
                    for (axis, coefficient) in [(0, x), (1, y)] {
                        let Some(value) =
                            coefficient.product(&chart.position[axis], work, cancel)?
                        else {
                            return Ok(None);
                        };
                        for term in &value.terms[..value.len] {
                            if p.push(*term).is_none() {
                                return Ok(None);
                            }
                        }
                        let Some(value) =
                            coefficient.product(&chart.derivative[axis], work, cancel)?
                        else {
                            return Ok(None);
                        };
                        for term in &value.terms[..value.len] {
                            if d.push(*term).is_none() {
                                return Ok(None);
                            }
                        }
                    }
                    position[k] = p;
                    derivative[k] = d;
                }
                return Ok(Some(ExactAffineCurve {
                    position,
                    derivative,
                    denominator: chart.denominator,
                }));
            }
            _ => return Ok(None),
        }
    }
    Err(resource())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame() -> source::Placement {
        source::Placement {
            origin: [0.0; 3],
            axis: [0.0, 0.0, 1.0],
            ref_direction: [1.0, 0.0, 0.0],
        }
    }
    #[test]
    fn original_radian_operators_do_not_masquerade_as_native_degree_curves() {
        let curve = Curve::Circle {
            position: frame(),
            radius: 2.0,
        };
        let surface = Surface::Cylinder {
            position: frame(),
            radius: 2.0,
        };
        let chart = Curve::Line {
            origin: [0.0, 2.0, 0.0],
            direction: [1.0, 0.0, 0.0],
            magnitude: 1.0,
        };
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        let c =
            PreparedOriginalCurve::new(&curve, [0.0, 1.0], usize::MAX, &mut work, &cancel).unwrap();
        let s = PreparedOriginalSurface::new(
            &surface,
            [[0.0, 1.0], [0.0, 4.0]],
            usize::MAX,
            &mut work,
            &cancel,
        )
        .unwrap();
        let p =
            PreparedOriginalCurve::new(&chart, [0.0, 1.0], usize::MAX, &mut work, &cancel).unwrap();
        let j = c.jet([0.0; 2], &mut work, &cancel).unwrap();
        assert!(j.jet.position[0].contains(2.0) && j.jet.derivative[1].contains(2.0));
        assert!(j.jet.second_derivative[0].contains(-2.0) && j.third_derivative[1].contains(-2.0));
        assert!(c.jet([90.0; 2], &mut work, &cancel).is_err());
        let j =
            original_curve_on_surface_enclosure(&s, &p, [0.0; 2], [0.0, 1.0], &mut work, &cancel)
                .unwrap();
        assert!(j.jet.position[2].contains(2.0) && j.jet.derivative[1].contains(2.0));
        assert!(j.third_derivative[1].contains(-2.0));
        let evaluator: &dyn crate::parametric::SurfaceEvaluator = &s;
        assert!(evaluator.native_surface().is_none());
        assert!(
            crate::parametric::surface_enclosure(
                evaluator,
                [[0.0; 2], [2.0; 2]],
                &mut work,
                &cancel
            )
            .unwrap()
            .du[1]
                .contains(2.0)
        );
    }
    #[test]
    fn canonical_analytic_pullback_retains_nonzero_original_vertex_gap() {
        let source = Surface::Cylinder {
            position: frame(),
            radius: 2.0,
        };
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        let surface = PreparedOriginalSurface::new(
            &source,
            [[-1.0, 1.0], [0.0, 4.0]],
            usize::MAX,
            &mut work,
            &cancel,
        )
        .unwrap();
        let p = original_analytic_surface_pullback(
            &surface,
            [2.125, 0.0, 3.0],
            [0, 0],
            1.0e-10,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(p.uv[0].contains(0.0) && p.uv[1].contains(3.0));
        assert!(p.residual_mm[0].contains(-0.125) && p.residual_mm[0].hi < 0.0);
        assert_eq!(p.point_mm, [2.125, 0.0, 3.0]);
        assert!(
            PreparedOriginalSurface::new(&source, [[0.0, 1.0]; 2], 0, &mut work, &cancel).is_err()
        );
        assert_eq!(
            PreparedOriginalSurface::new(
                &source,
                [[0.0, 1.0]; 2],
                usize::MAX,
                &mut work,
                &AtomicBool::new(true)
            )
            .err()
            .unwrap()
            .code,
            GeometryErrorCode::Cancelled
        );
    }

    #[test]
    fn coefficient_carrier_queries_preserve_selected_domain_and_literal_restrictions() {
        let plane = Surface::Plane { position: frame() };
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let active = [[-1.0, 1.0]; 2];
        let prepared =
            PreparedOriginalSurface::new(&plane, active, usize::MAX, &mut work, &cancel).unwrap();
        let outside_trim = [[2.0, 2.0], [3.0, 3.0]];
        assert!(prepared.jet(outside_trim, &mut work, &cancel).is_err());
        let sheet = prepared
            .carrier_jet(outside_trim, &mut work, &cancel)
            .unwrap();
        let position = prepared
            .carrier_position_bounds(outside_trim, &mut work, &cancel)
            .unwrap();
        assert!(position[0].contains(2.0));
        assert!(position[1].contains(3.0));
        assert_eq!(position[2], Interval::point(0.0));
        assert!(sheet.du[0].contains(1.0) && sheet.dv[1].contains(1.0));
        assert_eq!(prepared.active_domain(), active);
        assert!(prepared.jet(outside_trim, &mut work, &cancel).is_err());

        let literal = Surface::RectangularTrimmed {
            basis: std::sync::Arc::new(Surface::Plane { position: frame() }),
            u1: -1.0,
            u2: 1.0,
            v1: -1.0,
            v2: 1.0,
            u_sense: true,
            v_sense: true,
        };
        let literal =
            PreparedOriginalSurface::new(&literal, active, usize::MAX, &mut work, &cancel).unwrap();
        assert!(
            literal
                .carrier_jet(outside_trim, &mut work, &cancel)
                .is_err()
        );
        assert!(
            literal
                .carrier_position_bounds(outside_trim, &mut work, &cancel)
                .is_err(),
            "coefficient queries cannot widen the literal source restriction"
        );
    }
}

#[cfg(test)]
mod affine_spline_tests {
    use super::*;
    #[test]
    fn source_affine_spline_keeps_exact_parameter_law_and_rejects_unequal_weights() {
        use source::{BSplineCurve, CurveForm, KnotSpec, Logical};
        let points = [[1.0, 1.0e16, 0.0], [2.0, -1.0e16, 0.0]];
        for (weights, affine) in [
            (None, true),
            (Some(vec![2.0, 2.0]), true),
            (
                Some(vec![2.0, f64::from_bits(2.0_f64.to_bits() + 1)]),
                false,
            ),
        ] {
            let source = Curve::BSpline(Box::new(BSplineCurve {
                degree: 1,
                points: points.to_vec(),
                weights,
                knots: vec![-2.0, 4.0],
                multiplicities: vec![2, 2],
                form: CurveForm::Unspecified,
                closed: Logical::False,
                self_intersect: Logical::False,
                knot_spec: KnotSpec::Unspecified,
            }));
            let mut work = GeometryWorkBudget::new(1_000_000);
            let cancel = AtomicBool::new(false);
            let provider =
                PreparedOriginalCurve::new(&source, [-2.0, 4.0], usize::MAX, &mut work, &cancel)
                    .unwrap();
            let coefficients = provider
                .exact_affine_coefficients(&mut work, &cancel)
                .unwrap();
            assert_eq!(coefficients.is_some(), affine);
            let Some(c) = coefficients else {
                continue;
            };
            // Both endpoint equalities hold in the original t gauge despite the
            // non-dyadic derivative and cancellation of large control ordinates.
            for (i, t) in [-2.0, 4.0].into_iter().enumerate() {
                for (k, &coordinate) in points[i][..3].iter().enumerate() {
                    assert!(
                        c.endpoint_coordinate_equal(t, k, coordinate, &mut work, &cancel)
                            .unwrap()
                    );
                    let nearby = f64::from_bits(coordinate.to_bits() + 1);
                    assert!(
                        !c.endpoint_coordinate_equal(t, k, nearby, &mut work, &cancel)
                            .unwrap()
                    );
                }
            }
            assert!(
                crate::parametric::curve_endpoint_coordinate_equal(
                    &provider, -3.0, 0, 1.0, &mut work, &cancel
                )
                .is_err()
            );
            assert!(
                c.endpoint_coordinate_equal(0.0, 0, f64::NAN, &mut work, &cancel)
                    .is_err()
            );
        }
    }
}

#[cfg(test)]
mod preferred_master_tests {
    use super::*;
    use std::sync::Arc;
    #[test]
    fn preferred_pcurve_is_the_actual_composed_operator_not_the_spatial_neighbor() {
        use source::{Pcurve, SurfaceCurveKind};
        let frame = source::Placement {
            origin: [0.0; 3],
            axis: [0.0, 0.0, 1.0],
            ref_direction: [1.0, 0.0, 0.0],
        };
        let chart = Arc::new(Curve::Line {
            origin: [0.0, 2.0, 0.0],
            direction: [1.0, 0.0, 0.0],
            magnitude: 1.0,
        });
        let cylinder = Arc::new(Surface::Cylinder {
            position: frame,
            radius: 2.0,
        });
        let plane = Arc::new(Surface::Plane {
            position: source::Placement {
                origin: [10.0, 20.0, 30.0],
                ..frame
            },
        });
        let spatial = Arc::new(Curve::Line {
            origin: [1000.0; 3],
            direction: [0.0, 0.0, 1.0],
            magnitude: 1.0,
        });
        for (master, expected) in [
            (PreferredRep::Curve3d, [1000.0; 3]),
            (PreferredRep::PcurveS1, [2.0, 0.0, 2.0]),
            (PreferredRep::PcurveS2, [10.0, 22.0, 30.0]),
        ] {
            let source = Curve::SurfaceCurve {
                kind: SurfaceCurveKind::Surface,
                curve_3d: spatial.clone(),
                associated: vec![
                    Associated::Pcurve(Pcurve {
                        surface: cylinder.clone(),
                        curve: chart.clone(),
                    }),
                    Associated::Pcurve(Pcurve {
                        surface: plane.clone(),
                        curve: chart.clone(),
                    }),
                ],
                master,
            };
            let mut work = GeometryWorkBudget::new(1_000_000);
            let cancel = AtomicBool::new(false);
            let provider =
                PreparedOriginalCurve::new(&source, [0.0, 1.0], usize::MAX, &mut work, &cancel)
                    .unwrap();
            let jet = provider.jet([0.0; 2], &mut work, &cancel).unwrap();
            for (k, &coordinate) in expected[..3].iter().enumerate() {
                assert!(jet.jet.position[k].contains(coordinate));
            }
            assert!(
                provider
                    .span_is_continuous([0.0, 1.0], &mut work, &cancel)
                    .unwrap()
            );
            assert!(
                provider
                    .span_is_smooth([0.0, 1.0], &mut work, &cancel)
                    .unwrap()
            );
            assert!(
                provider
                    .tensor_coefficients(&mut work, &cancel)
                    .unwrap()
                    .is_none()
            );
            let affine = provider
                .exact_affine_coefficients(&mut work, &cancel)
                .unwrap();
            if master == PreferredRep::PcurveS1 {
                assert!(
                    jet.jet.derivative[1].contains(2.0) && jet.third_derivative[1].contains(-2.0)
                );
                assert!(affine.is_none());
            } else {
                let affine = affine.unwrap();
                for (k, &coordinate) in expected[..3].iter().enumerate() {
                    assert!(
                        affine
                            .endpoint_coordinate_equal(0.0, k, coordinate, &mut work, &cancel)
                            .unwrap()
                    );
                }
            }
        }
        let wrong_ordinal = Curve::SurfaceCurve {
            kind: SurfaceCurveKind::Surface,
            curve_3d: spatial,
            associated: vec![
                Associated::Surface(cylinder),
                Associated::Pcurve(Pcurve {
                    surface: plane,
                    curve: chart,
                }),
            ],
            master: PreferredRep::PcurveS1,
        };
        assert!(
            PreparedOriginalCurve::new(
                &wrong_ordinal,
                [0.0, 1.0],
                usize::MAX,
                &mut GeometryWorkBudget::new(1_000_000),
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }
    #[test]
    fn preferred_cartesian_trim_does_not_apply_nonpreferred_parameter_selectors() {
        let curve = Curve::Trimmed {
            basis: Arc::new(Curve::Line {
                origin: [0.0; 3],
                direction: [1.0, 0.0, 0.0],
                magnitude: 1.0,
            }),
            trim_1: vec![Trim::Point([0.0; 3]), Trim::Parameter(500.0)],
            trim_2: vec![Trim::Point([1.0, 0.0, 0.0]), Trim::Parameter(501.0)],
            sense: true,
            preference: TrimPreference::Cartesian,
        };
        assert!(
            original_curve_parameter_restriction(&curve)
                .unwrap()
                .is_none()
        );
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        let provider =
            PreparedOriginalCurve::new(&curve, [0.0, 1.0], usize::MAX, &mut work, &cancel).unwrap();
        assert!(
            provider
                .jet([1.0; 2], &mut work, &cancel)
                .unwrap()
                .jet
                .position[0]
                .contains(1.0)
        );
    }
}

#[cfg(test)]
mod composed_knot_tests {
    use super::*;
    use std::sync::Arc;
    #[test]
    fn preferred_master_checks_carrier_knots_even_when_chart_has_no_knots() {
        use source::{BSplineSurface, KnotSpec, Logical, Pcurve, SurfaceCurveKind, SurfaceForm};
        let carrier = Surface::BSpline(Box::new(BSplineSurface {
            u_degree: 1,
            v_degree: 1,
            points: vec![
                vec![[0.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                vec![[1.0, 0.0, 0.0], [1.0, 1.0, 0.0]],
                vec![[1.0, 0.0, 1.0], [1.0, 1.0, 1.0]],
            ],
            weights: None,
            u_knots: vec![0.0, 0.5, 1.0],
            u_multiplicities: vec![2, 1, 2],
            v_knots: vec![0.0, 1.0],
            v_multiplicities: vec![2, 2],
            form: SurfaceForm::Unspecified,
            u_closed: Logical::False,
            v_closed: Logical::False,
            self_intersect: Logical::False,
            knot_spec: KnotSpec::Unspecified,
        }));
        {
            let cancel = AtomicBool::new(false);
            let mut work = GeometryWorkBudget::new(1_000_000);
            let prepared = PreparedOriginalSurface::new(
                &carrier,
                [[0.1, 0.4], [0.1, 0.9]],
                usize::MAX,
                &mut work,
                &cancel,
            )
            .unwrap();
            assert!(
                prepared
                    .carrier_jet([[0.45, 0.55], [0.25, 0.75]], &mut work, &cancel,)
                    .is_err()
            );
            let first = prepared
                .carrier_piecewise_first([[0.45, 0.55], [0.25, 0.75]], &mut work, &cancel)
                .unwrap();
            assert!(first.du[0].contains(2.0) && first.du[0].contains(0.0));
            assert!(first.du[2].contains(0.0) && first.du[2].contains(2.0));
            assert!(
                prepared
                    .carrier_jet([[0.9, 1.1], [0.25, 0.75]], &mut work, &cancel,)
                    .is_err()
            );
            assert!(
                prepared
                    .carrier_piecewise_first([[0.9, 1.1], [0.25, 0.75]], &mut work, &cancel,)
                    .is_err()
            );
            assert!(
                prepared
                    .carrier_position_bounds([[0.9, 1.1], [0.25, 0.75]], &mut work, &cancel)
                    .is_err()
            );
        }
        let curve = Curve::SurfaceCurve {
            kind: SurfaceCurveKind::Surface,
            curve_3d: Arc::new(Curve::Line {
                origin: [0.0; 3],
                direction: [1.0, 0.0, 0.0],
                magnitude: 1.0,
            }),
            associated: vec![Associated::Pcurve(Pcurve {
                surface: Arc::new(carrier),
                curve: Arc::new(Curve::Line {
                    origin: [0.0, 0.5, 0.0],
                    direction: [1.0, 0.0, 0.0],
                    magnitude: 1.0,
                }),
            })],
            master: PreferredRep::PcurveS1,
        };
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        let provider =
            PreparedOriginalCurve::new(&curve, [0.0, 1.0], usize::MAX, &mut work, &cancel).unwrap();
        assert!(
            provider
                .parameter_axis(&mut work, &cancel)
                .unwrap()
                .0
                .is_empty()
        );
        assert!(
            provider
                .span_is_continuous([0.25, 0.75], &mut work, &cancel)
                .unwrap()
        );
        assert!(
            !provider
                .span_is_smooth([0.25, 0.75], &mut work, &cancel)
                .unwrap()
        );
        let positions = provider
            .position_bounds([0.25, 0.75], &mut work, &cancel)
            .unwrap();
        for endpoint in [[0.5, 0.5, 0.0], [1.0, 0.5, 0.5]] {
            for k in 0..3 {
                assert!(positions[k].contains(endpoint[k]));
            }
        }
    }
}

#[cfg(test)]
mod declared_turn_tests {
    use super::*;
    use std::sync::Arc;
    fn trim(basis: Arc<Curve>, a: f64, b: f64, sense: bool) -> Curve {
        Curve::Trimmed {
            basis,
            trim_1: vec![Trim::Parameter(a)],
            trim_2: vec![Trim::Parameter(b)],
            sense,
            preference: TrimPreference::Parameter,
        }
    }
    fn frame() -> source::Placement {
        source::Placement {
            origin: [0.0; 3],
            axis: [0.0, 0.0, 1.0],
            ref_direction: [1.0, 0.0, 0.0],
        }
    }
    #[test]
    fn authored_wrapping_keeps_literal_endpoints_and_refuses_the_complement() {
        let circle = Arc::new(Curve::Circle {
            position: frame(),
            radius: 2.0,
        });
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        for (a, b, sense, turn) in [
            (5.5, 0.5, true, 1),
            (0.5, 5.5, false, -1),
            (1.0, 1.0, true, 1),
            (1.0, 1.0, false, -1),
        ] {
            let curve = trim(circle.clone(), a, b, sense);
            let period = original_curve_parameter_period(&curve, &mut work, &cancel)
                .unwrap()
                .unwrap();
            let end = Interval::point(b).add(period.mul(Interval::point(turn as f64)));
            let cover = Interval::point(a).hull(end);
            let provider = PreparedOriginalCurve::new(
                &curve,
                [cover.lo, cover.hi],
                usize::MAX,
                &mut work,
                &cancel,
            )
            .unwrap();
            let branch = provider.declared_trim_branch().unwrap();
            assert!(std::ptr::eq(branch.trimmed_source(), &curve));
            assert_eq!(branch.literal_parameters(), [a, b]);
            assert_eq!(branch.declared_turns(), [0, turn]);
            assert_eq!(branch.period(), period);
            assert_eq!(branch.endpoint_parameters(), [Interval::point(a), end]);
            assert!(
                provider
                    .span_is_smooth([cover.lo, cover.hi], &mut work, &cancel)
                    .unwrap()
            );
            assert!(
                provider
                    .position_bounds(
                        [crate::parametric::down(cover.lo), cover.hi],
                        &mut work,
                        &cancel
                    )
                    .is_err()
            );
            assert!(
                PreparedOriginalCurve::new_with_declared_turns(
                    &curve,
                    [cover.lo, cover.hi],
                    [0, 2 * turn],
                    usize::MAX,
                    &mut work,
                    &cancel
                )
                .is_err()
            );
        }
        let forward = trim(circle, 5.5, 0.5, true);
        assert!(
            PreparedOriginalCurve::new(&forward, [0.5, 5.5], usize::MAX, &mut work, &cancel)
                .is_err()
        );
    }
    #[test]
    fn world_period_cannot_extend_a_finite_nonperiodic_pcurve_trim() {
        use source::{Pcurve, SurfaceCurveKind};
        let line = Arc::new(Curve::Line {
            origin: [0.0; 3],
            direction: [1.0, 0.0, 0.0],
            magnitude: 1.0,
        });
        let carrier = Arc::new(Surface::Cylinder {
            position: frame(),
            radius: 2.0,
        });
        let selected = |chart| Curve::SurfaceCurve {
            kind: SurfaceCurveKind::Surface,
            curve_3d: line.clone(),
            associated: vec![Associated::Pcurve(Pcurve {
                surface: carrier.clone(),
                curve: chart,
            })],
            master: PreferredRep::PcurveS1,
        };
        let outer_trim = trim(Arc::new(selected(line.clone())), 5.5, 0.5, true);
        let inner_trim = selected(Arc::new(trim(line.clone(), 5.5, 0.5, true)));
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        assert!(
            original_curve_parameter_period(&inner_trim, &mut work, &cancel)
                .unwrap()
                .is_some()
        );
        assert!(
            PreparedOriginalCurve::new(&outer_trim, [5.5, 6.5], usize::MAX, &mut work, &cancel)
                .is_ok()
        );
        assert!(
            PreparedOriginalCurve::new(&inner_trim, [5.5, 6.5], usize::MAX, &mut work, &cancel)
                .is_err()
        );
        assert!(
            PreparedOriginalCurve::new_with_declared_turns(
                &inner_trim,
                [5.5, 6.5],
                [0, 1],
                usize::MAX,
                &mut work,
                &cancel
            )
            .is_err()
        );
    }
}

#[cfg(test)]
mod polyline_tests {
    use super::*;
    #[test]
    fn polyline_retains_every_integer_cut_and_literal_control_identity() {
        let source = Curve::Polyline(vec![
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [2.0, 1.0, 0.0],
            [3.0, 0.0, 0.0],
        ]);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        let provider =
            PreparedOriginalCurve::new(&source, [0.0, 3.0], usize::MAX, &mut work, &cancel)
                .unwrap();
        assert_eq!(
            provider.parameter_axis(&mut work, &cancel).unwrap().0,
            &[0.0, 1.0, 2.0, 3.0]
        );
        let prepared =
            PreparedValidatedCurve::new(&provider, [0.0, 3.0], 1.0, usize::MAX, &mut work, &cancel)
                .unwrap();
        assert_eq!(
            prepared.parameter_intervals().collect::<Vec<_>>(),
            vec![[0.0, 1.0], [1.0, 2.0], [2.0, 3.0]]
        );
        let mut visited = Vec::new();
        assert!(
            prepared
                .visit_explicit_knots([0.0, 3.0], &mut work, &cancel, &mut |k, _, _| {
                    visited.push(k);
                    Ok(())
                })
                .unwrap()
        );
        assert_eq!(visited, vec![0.0, 1.0, 2.0, 3.0]);
        assert!(
            crate::parametric::curve_endpoint_coordinate_equal(
                &provider, 1.0, 1, 1.0, &mut work, &cancel
            )
            .unwrap()
        );
        assert!(
            !crate::parametric::curve_endpoint_coordinate_equal(
                &provider,
                1.0,
                1,
                f64::from_bits(1.0_f64.to_bits() + 1),
                &mut work,
                &cancel
            )
            .unwrap()
        );
        assert!(
            provider
                .span_is_continuous([0.5, 1.5], &mut work, &cancel)
                .unwrap()
        );
        assert!(
            !provider
                .span_is_smooth([0.5, 1.5], &mut work, &cancel)
                .unwrap()
        );
        assert!(
            provider
                .position_bounds([-f64::MIN_POSITIVE, 1.0], &mut work, &cancel)
                .is_err()
        );
        let insufficient = SOURCE_OPERAND_SCRATCH_BYTES
            + std::mem::size_of::<PreparedOriginalCurve<'_>>()
            + 3 * std::mem::size_of::<f64>();
        assert!(
            PreparedOriginalCurve::new(&source, [0.0, 3.0], insufficient, &mut work, &cancel)
                .is_err()
        );
    }
    #[test]
    fn nonempty_chart_cuts_cannot_claim_a_complete_carrier_preimage_inventory() {
        use source::{BSplineSurface, KnotSpec, Logical, Pcurve, SurfaceCurveKind, SurfaceForm};
        use std::sync::Arc;
        let carrier = Surface::BSpline(Box::new(BSplineSurface {
            u_degree: 1,
            v_degree: 1,
            points: vec![
                vec![[0.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                vec![[1.0, 0.0, 0.0], [1.0, 1.0, 0.0]],
                vec![[1.0, 0.0, 1.0], [1.0, 1.0, 1.0]],
            ],
            weights: None,
            u_knots: vec![0.0, 0.5, 1.0],
            u_multiplicities: vec![2, 1, 2],
            v_knots: vec![0.0, 1.0],
            v_multiplicities: vec![2, 2],
            form: SurfaceForm::Unspecified,
            u_closed: Logical::False,
            v_closed: Logical::False,
            self_intersect: Logical::False,
            knot_spec: KnotSpec::Unspecified,
        }));
        let chart = Arc::new(Curve::Polyline(vec![[0.0, 0.5, 0.0], [1.0, 0.5, 0.0]]));
        let source = Curve::SurfaceCurve {
            kind: SurfaceCurveKind::Surface,
            curve_3d: Arc::new(Curve::Line {
                origin: [0.0; 3],
                direction: [1.0, 0.0, 0.0],
                magnitude: 1.0,
            }),
            associated: vec![Associated::Pcurve(Pcurve {
                surface: Arc::new(carrier),
                curve: chart,
            })],
            master: PreferredRep::PcurveS1,
        };
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        let provider =
            PreparedOriginalCurve::new(&source, [0.0, 1.0], usize::MAX, &mut work, &cancel)
                .unwrap();
        assert_eq!(
            provider.parameter_axis(&mut work, &cancel).unwrap().0,
            &[0.0, 1.0]
        );
        let prepared =
            PreparedValidatedCurve::new(&provider, [0.0, 1.0], 1.0, usize::MAX, &mut work, &cancel)
                .unwrap();
        let mut visited = Vec::new();
        assert!(
            !prepared
                .visit_explicit_knots([0.0, 1.0], &mut work, &cancel, &mut |k, _, _| {
                    visited.push(k);
                    Ok(())
                })
                .unwrap()
        );
        assert!(visited.is_empty());
        assert!(
            !provider
                .span_is_smooth([0.0, 1.0], &mut work, &cancel)
                .unwrap()
        );
    }
}
