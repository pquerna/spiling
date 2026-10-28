// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Shared immutable native coefficient bounds. Parameters are the original
//! natural/NURBS chart, never implicit STEP angles, normalized edge fractions,
//! face-use orientation or a guessed UV correspondence. Spatial unit conversion
//! belongs to an explicit prepared curve view, not its parameter derivatives.

pub mod analytic_eval;
mod continuity;
pub mod coplane;
pub mod curve_prepared;
mod green;
pub mod interval;
pub mod nurbs_eval;
pub mod plane_roots;
mod source_eval;
pub(crate) use source_eval::*;
#[cfg(test)]
mod tests;

pub use analytic_eval::{evaluate_analytic_normal_enclosure, evaluate_analytic_surface_jet};
pub use coplane::{COPLANE_ANGULAR_TOLERANCE, CoplaneClassification, classify_face_coplane};
pub use curve_prepared::{
    ChartCurveEnclosure, CurveEnclosure, CurveThirdEnclosure, PreparedValidatedCurve,
    ValidatedCurvePoint, ValidatedScalarCurveJet, curve_on_surface_enclosure,
    curve_on_surface_span_smooth,
};
pub use interval::*;
pub use nurbs_eval::{SurfaceEnclosure, evaluate_bspline_surface_jet, evaluate_nurbs_surface_jet};
pub use plane_roots::{
    BoundaryCoedgeRoots, FacePlaneCell, FacePlaneRelation, FacePlaneTrimRoot,
    PreparedBoundaryBinding, PreparedFacePlaneRoots,
};

use crate::{GeometryError, GeometryWorkBudget, PlaneFrameMm, check_cancel};
use solidcore::{curve::curve::Curve, surface::surface::Surface};
use spiling_contracts::geometry::GeometryErrorCode;
use std::sync::atomic::AtomicBool;

pub(crate) fn dot_point(point: [f64; 3], direction: [f64; 3]) -> Interval {
    (0..3).fold(Interval::point(0.0), |sum, axis| {
        sum.add(Interval::point(point[axis]).mul(Interval::point(direction[axis])))
    })
}
#[derive(Debug, Clone, Copy)]
pub(crate) struct SurfaceFirstEnclosure {
    pub position: [Interval; 3],
    pub du: [Interval; 3],
    pub dv: [Interval; 3],
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
    nurbs_eval::SURFACE_EVALUATION_SCRATCH_BYTES
        + continuity::SCRATCH_BYTES
        + std::mem::size_of::<[SurfaceFunctional; N]>()
        + 3 * std::mem::size_of::<[SurfaceFunctionalEnclosure; N]>()
}
pub(crate) const SURFACE_FIRST_ENCLOSURE_SCRATCH_BYTES: usize =
    surface_functional_scratch_bytes::<3>() + std::mem::size_of::<SurfaceFirstEnclosure>();

/// Full piecewise second-jet bounds; this grants no global differentiability.
fn native_surface_piecewise_enclosure(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SurfaceEnclosure, GeometryError> {
    analytic_eval::surface_order(surface, uv, 2, work, cancel)
}
/// A Hessian consumer requires C1 over the closed box, not a selected knot side.
fn native_surface_enclosure(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SurfaceEnclosure, GeometryError> {
    if !surface_parameter_span_smooth(surface, uv, work, cancel)? {
        return Err(GeometryError::new(
            GeometryErrorCode::AnalysisIndeterminate,
            "native surface closed query lacks C1 smoothness authority",
        ));
    }
    surface_piecewise_enclosure(surface, uv, work, cancel)
}
fn native_surface_piecewise_first_enclosure(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SurfaceFirstEnclosure, GeometryError> {
    let jet = analytic_eval::surface_order(surface, uv, 1, work, cancel)?;
    if !surface_coefficients_continuous(surface, uv, work, cancel)? {
        return Err(GeometryError::new(
            GeometryErrorCode::AnalysisIndeterminate,
            "native surface query crosses an unproved discontinuous knot",
        ));
    }
    Ok(SurfaceFirstEnclosure {
        position: jet.position,
        du: jet.du,
        dv: jet.dv,
    })
}
fn native_surface_functional_enclosures<const N: usize>(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    functionals: &[SurfaceFunctional; N],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[SurfaceFunctionalEnclosure; N], GeometryError> {
    work.charge(N as u64, cancel)?;
    for f in functionals {
        for v in f.direction.into_iter().chain(f.origin).chain([f.offset]) {
            Interval::new(v.lo, v.hi)?;
        }
    }
    validate_surface_partition(surface, uv, work, cancel)?;
    if !surface_coefficients_continuous(surface, uv, work, cancel)? {
        return Err(GeometryError::new(
            GeometryErrorCode::AnalysisIndeterminate,
            "surface functional crosses an unproved source discontinuity",
        ));
    }
    if let Some(n) = analytic_eval::native_surface_nurbs(surface)? {
        return nurbs_eval::evaluate_nurbs_surface_functionals(n, uv, functionals, work, cancel);
    }
    let mut source = surface;
    for _ in 0..32 {
        match source.kind() {
            solidcore::surface::surface::p2::SurfaceKind::CurveBounded(s) => source = s.basis(),
            solidcore::surface::surface::p2::SurfaceKind::Step(s) => {
                source = s.surface().ok_or_else(|| {
                    nurbs_eval::conditioning("empty native STEP functional surface")
                })?
            }
            solidcore::surface::surface::p2::SurfaceKind::Plane(p) => {
                let jet = surface_piecewise_first_enclosure(surface, uv, work, cancel)?;
                let origin = p.position().origin();
                let origin = [origin.x, origin.y, origin.z].map(Interval::point);
                let result = functionals.map(|f| {
                    let project = |row: [Interval; 3]| {
                        (0..3).fold(Interval::point(0.0), |s, i| {
                            s.add(f.direction[i].mul(row[i]))
                        })
                    };
                    let du = project(jet.du);
                    let dv = project(jet.dv);
                    let constant =
                        project(std::array::from_fn(|i| origin[i].sub(f.origin[i]))).add(f.offset);
                    SurfaceFunctionalEnclosure {
                        position: constant
                            .add(
                                Interval {
                                    lo: uv[0][0],
                                    hi: uv[0][1],
                                }
                                .mul(du),
                            )
                            .add(
                                Interval {
                                    lo: uv[1][0],
                                    hi: uv[1][1],
                                }
                                .mul(dv),
                            ),
                        du,
                        dv,
                    }
                });
                for r in result {
                    for v in [r.position, r.du, r.dv] {
                        Interval::new(v.lo, v.hi)?;
                    }
                }
                return Ok(result);
            }
            _ => break,
        }
    }
    let jet = surface_piecewise_first_enclosure(surface, uv, work, cancel)?;
    let carrier = analytic_eval::step_sphere_functional_bounds(surface, functionals, work, cancel)?;
    let mut result = functionals.map(|f| {
        let project = |row: [Interval; 3]| {
            (0..3).fold(Interval::point(0.0), |s, i| {
                s.add(f.direction[i].mul(row[i]))
            })
        };
        SurfaceFunctionalEnclosure {
            position: project(std::array::from_fn(|i| jet.position[i].sub(f.origin[i])))
                .add(f.offset),
            du: project(jet.du),
            dv: project(jet.dv),
        }
    });
    if let Some(carrier) = carrier {
        for (result, carrier) in result.iter_mut().zip(carrier) {
            result.position = result.position.intersect(carrier)?;
        }
    }
    for r in result {
        for v in [r.position, r.du, r.dv] {
            Interval::new(v.lo, v.hi)?;
        }
    }
    Ok(result)
}

/// Whole-carrier scalar enclosure from native rational control coefficients.
/// Uniform-sign weights give a convex combination of Euclidean controls. This
/// is an outer bound only; a control value is never a feasible curve witness.
pub(crate) fn native_curve_functional_control_bounds(
    curve: &dyn CurveEvaluator,
    range: [f64; 2],
    direction: [Interval; 3],
    offset: Interval,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Interval>, GeometryError> {
    let Some(native) = curve.native_curve() else {
        return Ok(None);
    };
    let Some(nurbs) = native_curve_nurbs(curve)? else {
        return Ok(None);
    };
    if !curve_parameter_span_continuous(curve, range, work, cancel)? {
        return Ok(None);
    }
    let mut weight_sign = None;
    let mut support: Option<Interval> = None;
    for point in nurbs.cpts() {
        work.charge(1, cancel)?;
        let weight = if nurbs.is_rational() { point.w } else { 1.0 };
        let coefficient = [
            point.x,
            point.y,
            if native.dim() == 2 { 0.0 } else { point.z },
        ];
        for value in coefficient.into_iter().chain([weight]) {
            Interval::new(value, value)?;
        }
        if weight == 0.0 || weight_sign.is_some_and(|sign| sign != (weight > 0.0)) {
            return Ok(None);
        }
        weight_sign = Some(weight > 0.0);
        let bound = (0..3)
            .fold(Interval::point(0.0), |sum, axis| {
                sum.add(direction[axis].mul(Interval::point(coefficient[axis])))
            })
            .div(Interval::point(weight))?
            .add(offset);
        support = Some(support.map_or(bound, |previous| previous.hull(bound)));
    }
    Ok(support)
}
fn native_surface_geometric_normal_enclosure(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[Interval; 3], GeometryError> {
    if !surface_parameter_span_smooth(surface, uv, work, cancel)? {
        return Err(GeometryError::new(
            GeometryErrorCode::AnalysisIndeterminate,
            "normal query lacks whole-span C1 authority",
        ));
    }
    evaluate_analytic_normal_enclosure(surface, uv, work, cancel)
}

#[derive(Clone, Copy)]
struct KnotAxis<'a> {
    values: &'a [f64],
    degree: usize,
}
#[derive(Clone)]
pub(crate) struct AxisSpans<'a> {
    knots: &'a [f64],
    range: [f64; 2],
    front: usize,
    back: usize,
    single: bool,
}
impl<'a> AxisSpans<'a> {
    pub(crate) fn single(range: [f64; 2]) -> Result<Self, GeometryError> {
        Interval::new(range[0], range[1])?;
        Ok(Self::new(
            KnotAxis {
                values: &[],
                degree: 0,
            },
            range,
        ))
    }
    /// A borrowed evaluation cover, not an inventory of exact source knots.
    /// Knot-band fences preserve uncertainty; continuity is proved by the provider.
    pub(crate) fn from_cover_cuts(
        cuts: &'a [f64],
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        work.charge(1, cancel)?;
        Interval::new(range[0], range[1])?;
        if cuts.len() < 2 {
            return Err(nurbs_eval::conditioning(
                "curve cover needs both domain fences",
            ));
        }
        for (i, value) in cuts.iter().copied().enumerate() {
            work.charge(1, cancel)?;
            if !value.is_finite() || (i > 0 && value < cuts[i - 1]) {
                return Err(nurbs_eval::conditioning(
                    "curve cover fences are not finite and sorted",
                ));
            }
        }
        if range[0] < cuts[0] || range[1] > cuts[cuts.len() - 1] {
            return Err(nurbs_eval::conditioning(
                "curve cover does not contain the requested domain",
            ));
        }
        if cuts[0] == cuts[cuts.len() - 1] {
            return Self::single(range);
        }
        Ok(Self::new(
            KnotAxis {
                values: cuts,
                degree: 0,
            },
            range,
        ))
    }
    fn new(axis: KnotAxis<'a>, range: [f64; 2]) -> Self {
        Self {
            knots: axis.values,
            range,
            front: axis.degree,
            back: axis.values.len().saturating_sub(axis.degree + 1),
            single: axis.values.is_empty(),
        }
    }
    pub(super) fn from_validated_curve(curve: &'a Curve, range: [f64; 2]) -> Self {
        let mut source = curve;
        while let solidcore::curve::curve::p2::CurveKind::CrvOnSurf(c) = source.kind() {
            source = c.uv_curve();
        }
        let axis = curve_prepared::native_curve_nurbs(source)
            .ok()
            .flatten()
            .map_or(
                KnotAxis {
                    values: &[],
                    degree: 0,
                },
                |n| KnotAxis {
                    values: n.knots(),
                    degree: n.degree(),
                },
            );
        Self::new(axis, [range[0].min(range[1]), range[0].max(range[1])])
    }
    fn clip(&self, i: usize) -> Option<[f64; 2]> {
        let lo = self.knots[i];
        let hi = self.knots[i + 1];
        (lo < hi && lo <= self.range[1] && hi >= self.range[0])
            .then_some([lo.max(self.range[0]), hi.min(self.range[1])])
    }
}
impl Iterator for AxisSpans<'_> {
    type Item = [f64; 2];
    fn next(&mut self) -> Option<Self::Item> {
        if self.single {
            self.single = false;
            return Some(self.range);
        }
        while self.front < self.back {
            let i = self.front;
            self.front += 1;
            if let Some(v) = self.clip(i) {
                return Some(v);
            }
        }
        None
    }
}
impl DoubleEndedIterator for AxisSpans<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.single {
            self.single = false;
            return Some(self.range);
        }
        while self.front < self.back {
            self.back -= 1;
            if let Some(v) = self.clip(self.back) {
                return Some(v);
            }
        }
        None
    }
}
fn surface_knots<'a>(
    surface: &'a Surface,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[KnotAxis<'a>; 2], GeometryError> {
    work.charge(1, cancel)?;
    if let Some(n) = analytic_eval::native_surface_nurbs(surface)? {
        let (p, q) = n.degrees();
        Ok([
            KnotAxis {
                values: n.u_knots(),
                degree: p,
            },
            KnotAxis {
                values: n.v_knots(),
                degree: q,
            },
        ])
    } else {
        Ok([KnotAxis {
            values: &[],
            degree: 0,
        }; 2])
    }
}
fn axis_continuity(
    axis: KnotAxis<'_>,
    range: [f64; 2],
    continuity: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    Interval::new(range[0], range[1])?;
    let knots = axis.values;
    let mut i = 0;
    while i < knots.len() {
        work.charge(1, cancel)?;
        let start = i;
        let value = knots[i];
        while i < knots.len() && knots[i] == value {
            i += 1;
        }
        if start == 0 || i == knots.len() {
            continue;
        }
        if value >= range[0] && value <= range[1] && i - start + continuity > axis.degree {
            return Ok(false);
        }
    }
    Ok(true)
}
fn native_surface_parameter_span_smooth(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    analytic_eval::surface_order(surface, uv, 0, work, cancel)?;
    let axes = surface_knots(surface, work, cancel)?;
    if axis_continuity(axes[0], uv[0], 1, work, cancel)?
        && axis_continuity(axes[1], uv[1], 1, work, cancel)?
    {
        return Ok(true);
    }
    if let Some(n) = analytic_eval::native_surface_nurbs(surface)? {
        continuity::surface_is_c1(n, uv, work, cancel)
    } else {
        Ok(true)
    }
}
fn native_surface_parameter_span_continuous(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    analytic_eval::surface_order(surface, uv, 0, work, cancel)?;
    surface_coefficients_continuous(surface, uv, work, cancel)
}
fn surface_coefficients_continuous(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let axes = surface_knots(surface, work, cancel)?;
    if axis_continuity(axes[0], uv[0], 0, work, cancel)?
        && axis_continuity(axes[1], uv[1], 0, work, cancel)?
    {
        return Ok(true);
    }
    if let Some(n) = analytic_eval::native_surface_nurbs(surface)? {
        continuity::surface_is_c0(n, uv, work, cancel)
    } else {
        Ok(true)
    }
}
fn native_curve_parameter_span_continuous(
    curve: &Curve,
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    curve_span_continuous_at(curve, range, 0, work, cancel)
}
fn curve_span_continuous_at(
    curve: &Curve,
    range: [f64; 2],
    depth: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(1, cancel)?;
    if depth >= 16 {
        return Err(GeometryError::new(
            GeometryErrorCode::ResourceLimit,
            "nested curve continuity depth exceeded",
        ));
    }
    curve_prepared::curve_jet_order(curve, range, 0, depth, work, cancel)?;
    if let solidcore::curve::curve::p2::CurveKind::CrvOnSurf(c) = curve.kind() {
        if !curve_span_continuous_at(c.uv_curve(), range, depth + 1, work, cancel)? {
            return Ok(false);
        }
        let uv = curve_prepared::curve_jet_order(c.uv_curve(), range, 0, depth + 1, work, cancel)?;
        let domain = [[uv[0][0].lo, uv[0][0].hi], [uv[0][1].lo, uv[0][1].hi]];
        return surface_parameter_span_continuous(c.surface(), domain, work, cancel);
    }
    if let Some(n) = curve_prepared::native_curve_nurbs(curve)? {
        if axis_continuity(
            KnotAxis {
                values: n.knots(),
                degree: n.degree(),
            },
            range,
            0,
            work,
            cancel,
        )? {
            Ok(true)
        } else {
            continuity::curve_is_c0(n, curve.dim(), range, work, cancel)
        }
    } else {
        Ok(true)
    }
}
fn native_curve_parameter_span_smooth(
    curve: &Curve,
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    curve_span_smooth_at(curve, range, 0, work, cancel)
}
fn curve_span_smooth_at(
    curve: &Curve,
    range: [f64; 2],
    depth: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(1, cancel)?;
    if depth >= 16 {
        return Err(GeometryError::new(
            GeometryErrorCode::ResourceLimit,
            "nested curve smoothness depth exceeded",
        ));
    }
    if let solidcore::curve::curve::p2::CurveKind::CrvOnSurf(c) = curve.kind() {
        if !curve_span_smooth_at(c.uv_curve(), range, depth + 1, work, cancel)? {
            return Ok(false);
        }
        let uv = curve_prepared::curve_jet_order(c.uv_curve(), range, 0, depth + 1, work, cancel)?;
        return surface_parameter_span_smooth(
            c.surface(),
            [[uv[0][0].lo, uv[0][0].hi], [uv[0][1].lo, uv[0][1].hi]],
            work,
            cancel,
        );
    }
    curve_prepared::curve_jet_order(curve, range, 0, depth, work, cancel)?;
    if let Some(n) = curve_prepared::native_curve_nurbs(curve)? {
        continuity::curve_is_c1(n, curve.dim(), range, work, cancel)
    } else {
        Ok(true)
    }
}
fn native_curve_parameter_spans<'a>(
    curve: &'a Curve,
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<AxisSpans<'a>, GeometryError> {
    Interval::new(range[0], range[1])?;
    let mut source = curve;
    for depth in 0..=16 {
        work.charge(1, cancel)?;
        let active = source.natural_interval();
        if range[0] < active.min() || range[1] > active.max() {
            return Err(nurbs_eval::conditioning(
                "curve partition exceeds native active domain",
            ));
        }
        if let solidcore::curve::curve::p2::CurveKind::CrvOnSurf(c) = source.kind() {
            if depth == 16 {
                return Err(GeometryError::new(
                    GeometryErrorCode::ResourceLimit,
                    "curve partition nesting limit exceeded",
                ));
            }
            source = c.uv_curve();
            continue;
        }
        if let Some(n) = curve_prepared::native_curve_nurbs(source)? {
            nurbs_eval::validate_knots(n.knots(), n.degree(), n.cpts().len(), range, work, cancel)?;
        }
        break;
    }
    let spans = AxisSpans::from_validated_curve(curve, range);
    work.charge(spans.clone().count() as u64, cancel)?;
    Ok(spans)
}
fn native_curve2d_parameter_spans<'a>(
    curve: &'a Curve,
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<AxisSpans<'a>, GeometryError> {
    curve_parameter_spans(curve, range, work, cancel)
}
/// Positive/negative homogeneous convex-hull containment, independent of an
/// overwide evaluated UV interval. A true result permits intersection with this
/// domain; false supplies no chart authority and is never a tolerance waiver.
fn native_curve2d_image_within_box(
    curve: &Curve,
    range: [f64; 2],
    domain: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    for d in domain {
        Interval::new(d[0], d[1])?;
    }
    let image = curve_prepared::curve_jet_order(curve, range, 0, 0, work, cancel)?[0];
    if (0..2).all(|i| image[i].lo >= domain[i][0] && image[i].hi <= domain[i][1]) {
        return Ok(true);
    }
    if native_curve2d_control_hull_within_box(curve, range, domain, work, cancel)? {
        return Ok(true);
    }
    // An outlying control is not an outlying curve point. Exhaust the actual
    // knot spans with bounded whole-cell images before declining containment.
    // This is a positive-only proof: unresolved leaves grant no chart authority.
    const MAX_CONTAINMENT_DEPTH: u8 = 64;
    let mut pending = [([0.0; 2], 0u8); MAX_CONTAINMENT_DEPTH as usize + 1];
    let mut covered = false;
    for span in native_curve2d_parameter_spans(curve, range, work, cancel)? {
        covered = true;
        let mut live = 1;
        pending[0] = (span, 0);
        while live != 0 {
            live -= 1;
            let (cell, depth) = pending[live];
            let jet = curve_prepared::curve_jet_order(curve, cell, 1, 0, work, cancel)?;
            if (0..2)
                .any(|axis| jet[0][axis].hi < domain[axis][0] || jet[0][axis].lo > domain[axis][1])
            {
                return Ok(false);
            }
            let mut contained = true;
            let mut endpoints = [None; 2];
            for axis in 0..2 {
                for side in 0..2 {
                    let bound = domain[axis][side];
                    if (side == 0 && jet[0][axis].lo >= bound)
                        || (side == 1 && jet[0][axis].hi <= bound)
                    {
                        continue;
                    }
                    // A complete one-sided derivative and its literal endpoint
                    // can close a boundary-touching cell without interval
                    // padding turning exact equality into a false excursion.
                    let derivative = jet[1][axis];
                    let endpoint = if derivative.lo >= 0.0 {
                        Some(side)
                    } else if derivative.hi <= 0.0 {
                        Some(1 - side)
                    } else {
                        None
                    };
                    let Some(endpoint) = endpoint else {
                        contained = false;
                        break;
                    };
                    let point = match endpoints[endpoint] {
                        Some(point) => point,
                        None => {
                            let point = curve_prepared::curve_jet_order(
                                curve,
                                [cell[endpoint]; 2],
                                0,
                                0,
                                work,
                                cancel,
                            )?[0];
                            endpoints[endpoint] = Some(point);
                            point
                        }
                    };
                    if (side == 0 && point[axis].lo >= bound)
                        || (side == 1 && point[axis].hi <= bound)
                        || curve_prepared::curve_endpoint_coordinate_equal(
                            curve,
                            cell[endpoint],
                            axis,
                            bound,
                            work,
                            cancel,
                        )?
                    {
                        continue;
                    }
                    contained = false;
                    break;
                }
                if !contained {
                    break;
                }
            }
            if contained {
                continue;
            }
            let middle = cell[0] * 0.5 + cell[1] * 0.5;
            if depth >= MAX_CONTAINMENT_DEPTH || middle <= cell[0] || middle >= cell[1] {
                return Ok(false);
            }
            work.charge(2, cancel)?;
            pending[live] = ([middle, cell[1]], depth + 1);
            pending[live + 1] = ([cell[0], middle], depth + 1);
            live += 2;
        }
    }
    Ok(covered)
}

fn native_curve2d_control_hull_within_box(
    curve: &Curve,
    range: [f64; 2],
    domain: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let Some(n) = curve_prepared::native_curve_nurbs(curve)? else {
        return Ok(false);
    };
    let knots = n.knots();
    let degree = n.degree();
    let count = n.cpts().len();
    let mut sign = 0i8;
    for span in degree..count {
        work.charge(1, cancel)?;
        if knots[span] == knots[span + 1] || knots[span] > range[1] || knots[span + 1] < range[0] {
            continue;
        }
        for p in &n.cpts()[span - degree..=span] {
            work.charge(1, cancel)?;
            let w = if n.is_rational() { p.w } else { 1.0 };
            let this_sign = if w > 0.0 {
                1
            } else if w < 0.0 {
                -1
            } else {
                return Ok(false);
            };
            if sign != 0 && sign != this_sign {
                return Ok(false);
            }
            sign = this_sign;
            let coordinates = [p.x, p.y];
            for axis in 0..2 {
                for side in 0..2 {
                    let Some(mut difference) =
                        crate::exact_coefficient::ExactCoefficient::point(domain[axis][side])
                            .product(
                                &crate::exact_coefficient::ExactCoefficient::point(w),
                                work,
                                cancel,
                            )?
                    else {
                        return Ok(false);
                    };
                    if difference
                        .subtract(&crate::exact_coefficient::ExactCoefficient::point(
                            coordinates[axis],
                        ))
                        .is_none()
                    {
                        return Ok(false);
                    }
                    let comparison = difference.sign() * sign;
                    if (side == 0 && comparison > 0) || (side == 1 && comparison < 0) {
                        return Ok(false);
                    }
                }
            }
        }
    }
    Ok(sign != 0)
}
fn native_curve2d_piecewise_jet(
    curve: &Curve,
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<ChartCurveEnclosure, GeometryError> {
    let j = curve_prepared::curve_jet_order(curve, range, 2, 0, work, cancel)?;
    Ok(ChartCurveEnclosure {
        position: [j[0][0], j[0][1]],
        derivative: [j[1][0], j[1][1]],
        second_derivative: [j[2][0], j[2][1]],
    })
}
fn native_curve2d_c1_jet(
    curve: &Curve,
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<ChartCurveEnclosure>, GeometryError> {
    if !curve_parameter_span_smooth(curve, range, work, cancel)? {
        return Ok(None);
    }
    Ok(Some(curve2d_piecewise_jet(curve, range, work, cancel)?))
}
pub(crate) struct SurfaceParameterBoxes<'a> {
    u: AxisSpans<'a>,
    v: AxisSpans<'a>,
    active_u: Option<[f64; 2]>,
    active_v: AxisSpans<'a>,
}
impl Iterator for SurfaceParameterBoxes<'_> {
    type Item = [[f64; 2]; 2];
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(u) = self.active_u {
                if let Some(v) = self.active_v.next() {
                    return Some([u, v]);
                }
            }
            self.active_u = Some(self.u.next()?);
            self.active_v = self.v.clone();
        }
    }
}
fn validate_surface_partition(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    work.charge(1, cancel)?;
    for range in uv {
        Interval::new(range[0], range[1])?;
    }
    let active = surface.natural_uv_domain();
    let lo = active.min();
    let hi = active.max();
    if uv[0][0] < lo.x || uv[0][1] > hi.x || uv[1][0] < lo.y || uv[1][1] > hi.y {
        return Err(nurbs_eval::conditioning(
            "surface partition exceeds native active domain",
        ));
    }
    if let Some(n) = analytic_eval::native_surface_nurbs(surface)? {
        nurbs_eval::validate_knots(n.u_knots(), n.p(), n.n() + 1, uv[0], work, cancel)?;
        nurbs_eval::validate_knots(n.v_knots(), n.q(), n.m() + 1, uv[1], work, cancel)?;
    }
    Ok(())
}
fn native_surface_parameter_boxes<'a>(
    surface: &'a Surface,
    domain: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SurfaceParameterBoxes<'a>, GeometryError> {
    validate_surface_partition(surface, domain, work, cancel)?;
    let axes = surface_knots(surface, work, cancel)?;
    let u = AxisSpans::new(axes[0], domain[0]);
    let v = AxisSpans::new(axes[1], domain[1]);
    let count = u
        .clone()
        .count()
        .checked_mul(v.clone().count())
        .ok_or_else(|| {
            GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "surface knot product overflow",
            )
        })?;
    work.charge(count as u64, cancel)?;
    Ok(SurfaceParameterBoxes {
        u,
        active_v: v.clone(),
        v,
        active_u: None,
    })
}
pub(crate) struct SurfaceCreaseInterfaces<'a> {
    axes: [KnotAxis<'a>; 2],
    domain: [[f64; 2]; 2],
    axis: usize,
    index: usize,
    active: Option<(usize, f64, AxisSpans<'a>)>,
}
impl Iterator for SurfaceCreaseInterfaces<'_> {
    type Item = [[f64; 2]; 2];
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some((axis, value, spans)) = &mut self.active {
                if let Some(span) = spans.next() {
                    let mut result = self.domain;
                    result[*axis] = [*value; 2];
                    result[1 - *axis] = span;
                    return Some(result);
                }
                self.active = None;
            }
            if self.axis >= 2 {
                return None;
            }
            let a = self.axes[self.axis];
            if self.index >= a.values.len() {
                self.axis += 1;
                self.index = 0;
                continue;
            }
            let start = self.index;
            let value = a.values[start];
            while self.index < a.values.len() && a.values[self.index] == value {
                self.index += 1;
            }
            if value > self.domain[self.axis][0]
                && value < self.domain[self.axis][1]
                && self.index - start >= a.degree
            {
                self.active = Some((
                    self.axis,
                    value,
                    AxisSpans::new(self.axes[1 - self.axis], self.domain[1 - self.axis]),
                ));
            }
        }
    }
}
fn native_surface_crease_interfaces<'a>(
    surface: &'a Surface,
    domain: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SurfaceCreaseInterfaces<'a>, GeometryError> {
    validate_surface_partition(surface, domain, work, cancel)?;
    let axes = surface_knots(surface, work, cancel)?;
    let count = axes[0]
        .values
        .len()
        .checked_mul(axes[1].values.len().max(1))
        .ok_or_else(|| {
            GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "crease knot product overflow",
            )
        })?;
    work.charge(count as u64, cancel)?;
    Ok(SurfaceCreaseInterfaces {
        axes,
        domain,
        axis: 0,
        index: 0,
        active: None,
    })
}
fn native_curve_enclosure(
    curve: &Curve,
    range: (f64, f64),
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<CurveEnclosure, GeometryError> {
    for value in [range.0, range.1] {
        Interval::new(value, value)?;
    }
    let j = curve_prepared::curve_jet_order(
        curve,
        [range.0.min(range.1), range.0.max(range.1)],
        2,
        0,
        work,
        cancel,
    )?;
    Ok(CurveEnclosure {
        position: j[0],
        derivative: j[1],
        second_derivative: j[2],
    })
}
pub(crate) fn prepare_curve_enclosure<'a>(
    curve: &'a dyn CurveEvaluator,
    range: (f64, f64),
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<PreparedValidatedCurve<'a>, GeometryError> {
    PreparedValidatedCurve::new(
        curve,
        [range.0, range.1],
        1.0,
        remaining_bytes,
        work,
        cancel,
    )
}
fn norm_upper(vector: [Interval; 3]) -> Result<f64, GeometryError> {
    Ok(vector
        .into_iter()
        .fold(Interval::point(0.0), |sum, v| sum.add(v.square()))
        .sqrt()?
        .hi)
}

pub(crate) fn cross(a: [Interval; 3], b: [Interval; 3]) -> [Interval; 3] {
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
        direction: std::array::from_fn(|coordinate| {
            Interval::point(if coordinate == axis { 1.0 } else { 0.0 })
        }),
        origin,
        offset: Interval::point(0.0),
    });
    let jets = surface_functional_enclosures::<3>(surface, domain, &functionals, work, cancel)?;
    Ok(regular_derivatives(
        jets.map(|jet| jet.du),
        jets.map(|jet| jet.dv),
    ))
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
                    jet.du,
                    jet.dv,
                    cross(jet.du, jet.dv),
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
    mesh_uvs: &'a [solidcore::Vec2],
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
    pub(crate) fn key_for(domain: [[f64; 2]; 2]) -> [u64; 4] {
        // Signed zeros are the same geometric endpoint. Keep every nonzero
        // bit unchanged; this is cache identity, never UV quantization.
        [domain[0][0], domain[0][1], domain[1][0], domain[1][1]]
            .map(|value| if value == 0.0 { 0 } else { value.to_bits() })
    }
    pub(crate) fn new(key: [u64; 4]) -> Self {
        Self {
            key,
            proof: None,
            conditioned: None,
        }
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
const DISPLAY_EVALUATION_SCRATCH_BYTES: usize = nurbs_eval::SURFACE_EVALUATION_SCRATCH_BYTES
    + continuity::SCRATCH_BYTES
    + std::mem::size_of::<[([[f64; 2]; 2], u8); 50]>()
    + 4 * std::mem::size_of::<SurfaceEnclosure>();
impl<'a> DisplayTriangleProof<'a> {
    pub(crate) fn base_retained_bytes_for_vertices(count: usize) -> Result<usize, GeometryError> {
        count
            .checked_mul(std::mem::size_of::<Option<[Interval; 3]>>())
            .and_then(|bytes| {
                bytes.checked_add(
                    std::mem::size_of::<Self>()
                        + std::mem::size_of::<DisplayHessianScratch>()
                        + std::mem::size_of::<DisplaySubdivisionScratch>()
                        + DISPLAY_EVALUATION_SCRATCH_BYTES,
                )
            })
            .ok_or_else(|| {
                GeometryError::new(
                    GeometryErrorCode::ResourceLimit,
                    "display UV-point proof storage overflow",
                )
            })
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        (std::mem::size_of::<Self>()
            + std::mem::size_of::<DisplayHessianScratch>()
            + std::mem::size_of::<DisplaySubdivisionScratch>()
            + DISPLAY_EVALUATION_SCRATCH_BYTES)
            .saturating_add(
                self.mesh_vertices
                    .capacity()
                    .saturating_mul(std::mem::size_of::<Option<[Interval; 3]>>()),
            )
            .saturating_add(
                self.triangle_boxes
                    .capacity()
                    .saturating_mul(std::mem::size_of::<u32>()),
            )
            .saturating_add(
                self.mesh_boxes
                    .capacity()
                    .saturating_mul(std::mem::size_of::<DisplayMeshBox>()),
            )
    }
    #[cfg(test)]
    pub(crate) fn new(
        surface: &'a Surface,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        Self::with_mesh_inventory(
            surface,
            &[],
            Vec::new(),
            Vec::new(),
            usize::MAX,
            work,
            cancel,
        )
    }
    pub(crate) fn with_mesh_inventory(
        surface: &'a Surface,
        mesh_uvs: &'a [solidcore::Vec2],
        triangle_boxes: Vec<u32>,
        mesh_boxes: Vec<DisplayMeshBox>,
        maximum_live_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        let uv_count = mesh_uvs.len();
        work.charge((64 + 256 + 1_u64).saturating_add(uv_count as u64), cancel)?;
        let inventory_bytes = triangle_boxes
            .capacity()
            .checked_mul(std::mem::size_of::<u32>())
            .and_then(|n| {
                mesh_boxes
                    .capacity()
                    .checked_mul(std::mem::size_of::<DisplayMeshBox>())
                    .and_then(|b| n.checked_add(b))
            })
            .ok_or_else(|| {
                GeometryError::new(
                    GeometryErrorCode::ResourceLimit,
                    "display inventory byte size overflow",
                )
            })?;
        let anticipated = Self::base_retained_bytes_for_vertices(uv_count)?
            .checked_add(inventory_bytes)
            .ok_or_else(|| {
                GeometryError::new(
                    GeometryErrorCode::ResourceLimit,
                    "display proof live bytes overflow",
                )
            })?;
        if anticipated > maximum_live_bytes {
            return Err(GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "display proof owner and scratch exceed byte allowance",
            ));
        }
        let mut mesh_vertices = Vec::new();
        mesh_vertices.try_reserve_exact(uv_count).map_err(|_| {
            GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "display UV-point proof allocation failed",
            )
        })?;
        let actual = Self::base_retained_bytes_for_vertices(mesh_vertices.capacity())?
            .checked_add(inventory_bytes)
            .ok_or_else(|| {
                GeometryError::new(
                    GeometryErrorCode::ResourceLimit,
                    "display proof allocated bytes overflow",
                )
            })?;
        if actual > maximum_live_bytes {
            return Err(GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "display allocator capacity exceeds proof allowance",
            ));
        }
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
        self.triangle_error_bound_at(
            uv,
            points,
            None,
            true,
            spiling_contracts::geometry::MESH_CARRIER_TOLERANCE_MM,
            work,
            cancel,
        )
    }
    /// `maximum_error` is the caller's remaining carrier allowance after
    /// independent transport has been debited, never a wider geometry policy.
    /// It selects existing proof paths; the caller still checks the final sum.
    pub(crate) fn indexed_triangle_error_bound(
        &mut self,
        uv: &[[f64; 2]; 3],
        points: &[[f64; 3]; 3],
        triangle_ordinal: usize,
        uv_indices: &[usize; 3],
        maximum_error: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<f64, GeometryError> {
        self.triangle_error_bound_at(
            uv,
            points,
            Some((triangle_ordinal, uv_indices)),
            true,
            maximum_error,
            work,
            cancel,
        )
    }
    fn triangle_error_bound_at(
        &mut self,
        uv: &[[f64; 2]; 3],
        points: &[[f64; 3]; 3],
        uv_indices: Option<(usize, &[usize; 3])>,
        allow_auxiliary: bool,
        maximum_error: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<f64, GeometryError> {
        work.charge(1, cancel)?;
        if !maximum_error.is_finite()
            || maximum_error <= 0.0
            || maximum_error > spiling_contracts::geometry::MESH_CARRIER_TOLERANCE_MM
        {
            return Err(GeometryError::new(
                GeometryErrorCode::ShapeErrorExceeded,
                "display triangle proof has no positive remaining carrier allowance",
            ));
        }
        if uv.iter().flatten().any(|value| !value.is_finite()) {
            return Err(GeometryError::new(
                GeometryErrorCode::InvalidGeometry,
                "display triangle proof requires finite native UV coordinates",
            ));
        }
        let domain = std::array::from_fn(|axis| {
            [
                uv.iter().map(|p| p[axis]).fold(f64::INFINITY, f64::min),
                uv.iter().map(|p| p[axis]).fold(f64::NEG_INFINITY, f64::max),
            ]
        });
        let key = DisplayMeshBox::key_for(domain);
        let box_index = if let Some((ordinal, _)) = uv_indices {
            let index = *self.triangle_boxes.get(ordinal).ok_or_else(|| {
                GeometryError::new(
                    GeometryErrorCode::InvalidGeometry,
                    "display triangle is outside its captured UV-box inventory",
                )
            })? as usize;
            let entry = self.mesh_boxes.get(index).ok_or_else(|| {
                GeometryError::new(
                    GeometryErrorCode::InvalidGeometry,
                    "display triangle references an absent UV-box slot",
                )
            })?;
            if entry.key != key {
                return Err(GeometryError::new(
                    GeometryErrorCode::InvalidGeometry,
                    format!(
                        "display triangle changed its exact captured UV box: triangle={ordinal}; captured={:?}; actual={key:?}; uv={uv:?}",
                        entry.key,
                    ),
                ));
            }
            DisplayBoxIndex::Mesh(index)
        } else {
            let slot = display_proof_slot(&key, self.boxes.len());
            if self.boxes[slot]
                .as_ref()
                .is_some_and(|entry| entry.key != key)
            {
                self.boxes[slot] = None;
            }
            DisplayBoxIndex::Auxiliary(slot)
        };
        if self.box_proof(box_index).is_none() {
            let anchor = std::array::from_fn(|axis| {
                let low = points
                    .iter()
                    .map(|point| point[axis])
                    .fold(f64::INFINITY, f64::min);
                let high = points
                    .iter()
                    .map(|point| point[axis])
                    .fold(f64::NEG_INFINITY, f64::max);
                low * 0.5 + high * 0.5
            });
            let first = surface_coordinate_first(self.surface, domain, anchor, work, cancel)
                .map_err(|cause| {
                    display_proof_failure(cause, "box first jets", box_index, domain)
                })?;
            let du = first.map(|jet| jet.du);
            let dv = first.map(|jet| jet.dv);
            let full = if regular_derivatives(du, dv) {
                None
            } else {
                let bound = surface_piecewise_enclosure(self.surface, domain, work, cancel)
                    .map_err(|cause| {
                        display_proof_failure(cause, "box rank Jet2", box_index, domain)
                    })?;
                certify_surface_regular(self.surface, domain, &bound, work, cancel).map_err(|cause| {
                    GeometryError::new(cause.code, format!(
                        "obligation=box rank cache={box_index:?} triangle_uv={uv:?} triangle_points={points:?}: {}", cause.message,
                    ))
                })?;
                let smooth = surface_parameter_span_smooth(self.surface, domain, work, cancel)
                    .map_err(|cause| {
                        display_proof_failure(cause, "box rank smooth span", box_index, domain)
                    })?;
                Some(DisplayFullBoxProof {
                    bound,
                    smooth,
                    partitioned: false,
                })
            };
            let proof = DisplayBoxProof { du, dv, full };
            match box_index {
                DisplayBoxIndex::Mesh(index) => self.mesh_boxes[index].proof = Some(proof),
                DisplayBoxIndex::Auxiliary(slot) => {
                    self.boxes[slot] = Some(DisplayAuxBox { key, proof })
                }
            }
        }
        let positions = self.triangle_positions(uv, uv_indices, box_index, work, cancel)?;
        let proof = self
            .box_proof(box_index)
            .expect("completed same-box rank proof");
        let first_error = first_jet_triangle_error(points, domain, proof.du, proof.dv, &positions)?;
        if first_error <= maximum_error {
            return Ok(first_error);
        }
        // A first-order bound/rank decline is not an error. Only then request
        // the existing Hessian/smooth-span proof. Native failures stay terminal.
        if self.box_proof(box_index).expect("same box").full.is_none() {
            let bound = surface_piecewise_enclosure(self.surface, domain, work, cancel).map_err(
                |cause| display_proof_failure(cause, "box Hessian jets", box_index, domain),
            )?;
            // This exact box already has a completed first-jet rank proof.
            let smooth = surface_parameter_span_smooth(self.surface, domain, work, cancel)
                .map_err(|cause| {
                    display_proof_failure(cause, "box Hessian smooth span", box_index, domain)
                })?;
            self.box_proof_mut(box_index).expect("same box").full = Some(DisplayFullBoxProof {
                bound,
                smooth,
                partitioned: false,
            });
        }
        let proof = self
            .box_proof(box_index)
            .expect("same box")
            .full
            .as_ref()
            .expect("completed Hessian/smooth-span proof");
        let mut error = first_error.min(triangle_interpolation_error_prepared(
            uv,
            points,
            domain,
            &proof.bound,
            proof.smooth,
            &positions,
            (&mut *work, cancel),
        )?);
        if !allow_auxiliary && proof.smooth && !proof.partitioned && error > maximum_error {
            // Tighten only the single auxiliary comparison's Hessian, once,
            // over four closed subboxes covering this SAME qualified box.
            let hessian = partition_display_hessian(self.surface, domain, proof, work, cancel)
                .map_err(|cause| {
                    display_proof_failure(cause, "four-child Hessian", box_index, domain)
                })?;
            let proof = self
                .box_proof_mut(box_index)
                .expect("same box")
                .full
                .as_mut()
                .expect("same smooth-span proof");
            [proof.bound.duu, proof.bound.duv, proof.bound.dvv] = hessian;
            proof.partitioned = true;
            error = first_error.min(triangle_interpolation_error_prepared(
                uv,
                points,
                domain,
                &proof.bound,
                proof.smooth,
                &positions,
                (&mut *work, cancel),
            )?);
        }
        if !allow_auxiliary || error <= maximum_error {
            return Ok(error);
        }
        let Some(auxiliary) = auxiliary_smooth_triangle(self.surface, uv, domain, work, cancel)
            .map_err(|cause| {
                display_proof_failure(cause, "auxiliary span qualification", box_index, domain)
            })?
        else {
            work.charge(1, cancel)?;
            let proof = self
                .box_proof(box_index)
                .expect("same box")
                .full
                .as_ref()
                .expect("completed same-box derivative proof");
            if proof.smooth
                && let Some(refined) = subdivide_display_triangle(
                    self.surface,
                    uv,
                    points,
                    domain,
                    [&proof.bound.duu, &proof.bound.duv, &proof.bound.dvv],
                    &positions,
                    (work, cancel),
                )
                .map_err(|cause| {
                    display_proof_failure(cause, "filled-triangle subdivision", box_index, domain)
                })?
            {
                error = error.min(refined);
            }
            return Ok(error);
        };
        let proof = self
            .box_proof(box_index)
            .expect("same box")
            .full
            .as_ref()
            .expect("completed same-box derivative proof");
        // The same barycentric weights pair every original UV point with an
        // auxiliary UV point. Convexity bounds the entire displacement by the
        // largest vertex displacement on each axis. Both triangles remain in
        // the ORIGINAL closed box, whose continuous piecewise first jets bound
        // the complete transport; no C1 claim is made at its crossed knots.
        let delta: [f64; 2] = std::array::from_fn(|axis| {
            (0..3)
                .map(|vertex| {
                    Interval::point(uv[vertex][axis])
                        .sub(Interval::point(auxiliary[vertex][axis]))
                        .abs_upper()
                })
                .fold(0.0_f64, f64::max)
        });
        let transport = norm_upper(std::array::from_fn(|axis| {
            Interval::point(proof.bound.du[axis].abs_upper())
                .mul(Interval::point(delta[0]))
                .add(
                    Interval::point(proof.bound.dv[axis].abs_upper())
                        .mul(Interval::point(delta[1])),
                )
        }))?;
        let auxiliary_domain: [[f64; 2]; 2] = std::array::from_fn(|axis| {
            [
                auxiliary
                    .iter()
                    .map(|point| point[axis])
                    .fold(f64::INFINITY, f64::min),
                auxiliary
                    .iter()
                    .map(|point| point[axis])
                    .fold(f64::NEG_INFINITY, f64::max),
            ]
        });
        let auxiliary_key = [
            auxiliary_domain[0][0].to_bits(),
            auxiliary_domain[0][1].to_bits(),
            auxiliary_domain[1][0].to_bits(),
            auxiliary_domain[1][1].to_bits(),
        ];
        work.charge(1, cancel)?;
        let parent_index = match box_index {
            DisplayBoxIndex::Mesh(index) => Some(index),
            DisplayBoxIndex::Auxiliary(_) => None,
        };
        let mut retained_error = None;
        if let Some(index) = parent_index
            && self.mesh_boxes[index]
                .conditioned
                .as_ref()
                .is_some_and(|receipt| receipt.key == auxiliary_key)
        {
            let positions = self.triangle_positions(&auxiliary, None, box_index, work, cancel)?;
            let receipt = self.mesh_boxes[index]
                .conditioned
                .as_ref()
                .expect("matching completed auxiliary smooth-span proof");
            let bound = smooth_triangle_interpolation_error(
                &auxiliary,
                points,
                auxiliary_domain,
                [
                    &receipt.hessian[0],
                    &receipt.hessian[1],
                    &receipt.hessian[2],
                ],
                &positions,
                work,
                cancel,
            )?;
            let complete = up(bound + transport);
            if complete <= maximum_error {
                return Ok(error.min(complete));
            }
            // Retaining a valid Hessian must not discard another existing
            // certificate when this triangle's fresh residual still declines.
            retained_error = Some(bound);
        }
        // Exactly one independently qualified comparison. Even if its bound
        // remains too large, there is no precision or conditioning ladder.
        let auxiliary_limit = Interval::point(maximum_error)
            .sub(Interval::point(transport))
            .lo;
        let auxiliary_error = if auxiliary_limit > 0.0 {
            self.triangle_error_bound_at(
                &auxiliary,
                points,
                None,
                false,
                auxiliary_limit,
                work,
                cancel,
            )?
        } else {
            f64::INFINITY
        };
        if let Some(index) = parent_index {
            let slot = display_proof_slot(&auxiliary_key, self.boxes.len());
            let receipt = self.boxes[slot]
                .as_ref()
                .filter(|entry| entry.key == auxiliary_key)
                .and_then(|entry| entry.proof.full.as_ref().filter(|full| full.smooth))
                .map(|full| DisplayConditionedHessian {
                    key: auxiliary_key,
                    hessian: [full.bound.duu, full.bound.duv, full.bound.dvv],
                });
            if let Some(receipt) = receipt {
                self.mesh_boxes[index].conditioned = Some(receipt);
            }
        }
        let auxiliary_error =
            retained_error.map_or(auxiliary_error, |old: f64| old.min(auxiliary_error));
        let mut complete = error.min(up(auxiliary_error + transport));
        if complete <= maximum_error {
            return Ok(complete);
        }
        // All existing scalar/conditioning paths have declined. At most ONE
        // exact filled-triangle proof subdivision may now use an already
        // completed smooth Hessian, preferably the actual auxiliary box.
        work.charge(4, cancel)?;
        let slot = display_proof_slot(&auxiliary_key, self.boxes.len());
        let has_retained = parent_index.is_some_and(|index| {
            self.mesh_boxes[index]
                .conditioned
                .as_ref()
                .is_some_and(|receipt| receipt.key == auxiliary_key)
        });
        let has_auxiliary = self.boxes[slot]
            .as_ref()
            .filter(|entry| entry.key == auxiliary_key)
            .and_then(|entry| entry.proof.full.as_ref())
            .is_some_and(|full| full.smooth);
        if auxiliary_limit > 0.0 && (has_retained || has_auxiliary) {
            let positions = self.triangle_positions(&auxiliary, None, box_index, work, cancel)?;
            let hessian = parent_index
                .and_then(|index| self.mesh_boxes[index].conditioned.as_ref())
                .filter(|receipt| receipt.key == auxiliary_key)
                .map(|receipt| {
                    [
                        &receipt.hessian[0],
                        &receipt.hessian[1],
                        &receipt.hessian[2],
                    ]
                })
                .or_else(|| {
                    self.boxes[slot]
                        .as_ref()
                        .filter(|entry| entry.key == auxiliary_key)
                        .and_then(|entry| entry.proof.full.as_ref().filter(|full| full.smooth))
                        .map(|full| [&full.bound.duu, &full.bound.duv, &full.bound.dvv])
                })
                .expect("completed same-source auxiliary smooth Hessian");
            if let Some(refined) = subdivide_display_triangle(
                self.surface,
                &auxiliary,
                points,
                auxiliary_domain,
                hessian,
                &positions,
                (work, cancel),
            )
            .map_err(|cause| {
                display_proof_failure(
                    cause,
                    "auxiliary filled-triangle subdivision",
                    box_index,
                    auxiliary_domain,
                )
            })? {
                // This is still the original affine physical triangle. Pair
                // every original UV point with its auxiliary point only once.
                complete = complete.min(up(refined + transport));
            }
        } else if let Some(proof) = self
            .box_proof(box_index)
            .and_then(|proof| proof.full.as_ref())
            .filter(|full| full.smooth)
            && let Some(refined) = subdivide_display_triangle(
                self.surface,
                uv,
                points,
                domain,
                [&proof.bound.duu, &proof.bound.duv, &proof.bound.dvv],
                &positions,
                (work, cancel),
            )
            .map_err(|cause| {
                display_proof_failure(cause, "filled-triangle subdivision", box_index, domain)
            })?
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
                let captured = self.mesh_uvs.get(index).ok_or_else(|| {
                    GeometryError::new(
                        GeometryErrorCode::InvalidGeometry,
                        "display UV proof index is outside its captured mesh",
                    )
                })?;
                if [captured.x.to_bits(), captured.y.to_bits()] != key {
                    return Err(GeometryError::new(
                        GeometryErrorCode::InvalidGeometry,
                        "display UV index changed its exact parameter bits",
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
                None => self.vertices[auxiliary_slot]
                    .as_ref()
                    .filter(|proof| proof.key == key)
                    .map(|proof| proof.position),
            };
            let position = if let Some(position) = cached {
                position
            } else {
                let position = surface_position_enclosure(
                    self.surface,
                    [[uv[vertex][0]; 2], [uv[vertex][1]; 2]],
                    work,
                    cancel,
                )
                .map_err(|cause| {
                    GeometryError::new(
                        cause.code,
                        format!(
                            "obligation=point position cache={box_index:?} uv={:?}: {}",
                            uv[vertex], cause.message,
                        ),
                    )
                })?;
                match index {
                    Some(index) => self.mesh_vertices[index] = Some(position),
                    None => {
                        self.vertices[auxiliary_slot] = Some(DisplayVertexProof { key, position })
                    }
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
            DisplayBoxIndex::Auxiliary(index) => {
                self.boxes[index].as_ref().map(|entry| &entry.proof)
            }
        }
    }
    fn box_proof_mut(&mut self, index: DisplayBoxIndex) -> Option<&mut DisplayBoxProof> {
        match index {
            DisplayBoxIndex::Mesh(index) => self.mesh_boxes[index].proof.as_mut(),
            DisplayBoxIndex::Auxiliary(index) => {
                self.boxes[index].as_mut().map(|entry| &mut entry.proof)
            }
        }
    }
    /// Failure-only inspection of the captured original triangle/box binding.
    pub(crate) fn smooth_span_for_triangle(
        &self,
        ordinal: usize,
        uv: &[[f64; 2]; 3],
    ) -> Option<bool> {
        let domain: [[f64; 2]; 2] = std::array::from_fn(|axis| {
            [
                uv.iter().map(|p| p[axis]).fold(f64::INFINITY, f64::min),
                uv.iter().map(|p| p[axis]).fold(f64::NEG_INFINITY, f64::max),
            ]
        });
        let key = [
            domain[0][0].to_bits(),
            domain[0][1].to_bits(),
            domain[1][0].to_bits(),
            domain[1][1].to_bits(),
        ];
        let index = *self.triangle_boxes.get(ordinal)? as usize;
        self.mesh_boxes
            .get(index)
            .filter(|entry| entry.key == key)?
            .proof
            .as_ref()?
            .full
            .as_ref()
            .map(|full| full.smooth)
    }
}

fn display_proof_failure(
    cause: GeometryError,
    obligation: &str,
    index: DisplayBoxIndex,
    domain: [[f64; 2]; 2],
) -> GeometryError {
    GeometryError::new(
        cause.code,
        format!(
            "obligation={obligation} cache={index:?} uv_box={domain:?}: {}",
            cause.message,
        ),
    )
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
                [
                    if u == 0 { domain[0][0] } else { middle[0] },
                    if u == 0 { middle[0] } else { domain[0][1] },
                ],
                [
                    if v == 0 { domain[1][0] } else { middle[1] },
                    if v == 0 { middle[1] } else { domain[1][1] },
                ],
            ]
        }),
        hull: None,
        child: None,
    };
    for index in 0..4 {
        work.charge(1, cancel)?;
        scratch.child = None;
        scratch.child = Some(
            surface_piecewise_enclosure(surface, scratch.boxes[index], work, cancel).map_err(
                |cause| {
                    GeometryError::new(
                        cause.code,
                        format!(
                            "Hessian child={index} uv_box={:?}: {}",
                            scratch.boxes[index], cause.message,
                        ),
                    )
                },
            )?,
        );
        let child = scratch
            .child
            .as_ref()
            .expect("completed closed-subbox enclosure");
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
    let span = domain.map(|range| {
        Interval::point(range[1])
            .sub(Interval::point(range[0]))
            .abs_upper()
    });
    norm_upper(std::array::from_fn(|axis| {
        let vertex_error = (0..3)
            .map(|vertex| {
                positions[vertex][axis]
                    .sub(Interval::point(points[vertex][axis]))
                    .abs_upper()
            })
            .fold(0.0_f64, f64::max);
        Interval::point(vertex_error)
            .add(
                Interval::point(du[axis].width())
                    .mul(Interval::point(span[0]))
                    .mul(Interval::point(0.25)),
            )
            .add(
                Interval::point(dv[axis].width())
                    .mul(Interval::point(span[1]))
                    .mul(Interval::point(0.25)),
            )
    }))
}

/// Evaluate only the native source position; derivatives and regularity remain
/// separate obligations. Debit completed work even if evaluation fails.
fn native_surface_position_enclosure(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[Interval; 3], GeometryError> {
    Ok(analytic_eval::surface_order(surface, uv, 0, work, cancel)?.position)
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
        direction: std::array::from_fn(|coordinate| {
            Interval::point(if coordinate == axis { 1.0 } else { 0.0 })
        }),
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
        work.charge(axes[axis].values.len() as u64 + 1, cancel)?;
        let mut selected: Option<[f64; 2]> = None;
        for span in AxisSpans::new(axes[axis], domain[axis]) {
            work.charge(1, cancel)?;
            if selected.is_none_or(|old| span[1] - span[0] > old[1] - old[0]) {
                selected = Some(span);
            }
        }
        let Some(span) = selected else {
            return Ok(None);
        };
        let low = span[0].next_up();
        let high = span[1].next_down();
        if !low.is_finite() || !high.is_finite() || low >= high {
            return Ok(None);
        }
        interior[axis] = [low, high];
    }
    work.charge(6, cancel)?;
    let auxiliary = uv.map(|point| {
        std::array::from_fn(|axis| point[axis].clamp(interior[axis][0], interior[axis][1]))
    });
    let auxiliary_domain = std::array::from_fn(|axis| {
        [
            auxiliary
                .iter()
                .map(|point| point[axis])
                .fold(f64::INFINITY, f64::min),
            auxiliary
                .iter()
                .map(|point| point[axis])
                .fold(f64::NEG_INFINITY, f64::max),
        ]
    });
    if !surface_parameter_span_smooth(surface, auxiliary_domain, work, cancel)? {
        return Ok(None);
    }
    Ok(Some(auxiliary))
}
fn display_proof_slot(key: &[u64], slots: usize) -> usize {
    let hash = key.iter().fold(0_u64, |hash, bits| {
        (hash ^ *bits)
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
            .rotate_left(27)
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
            surface,
            [[uv[vertex][0]; 2], [uv[vertex][1]; 2]],
            work,
            cancel,
        )?
        .position;
    }
    triangle_interpolation_error_prepared(
        uv,
        points,
        domain,
        bound,
        smooth,
        &positions,
        (work, cancel),
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
        uv,
        points,
        domain,
        [&bound.duu, &bound.duv, &bound.dvv],
        positions,
        work,
        cancel,
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
    let Some(metric_remainder) = triangle_metric_remainder(uv, [du, dv], hessian, work, cancel)?
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

fn rectangle_triangle_remainder([du, dv]: [f64; 2], hessian: [&[Interval; 3]; 3]) -> [Interval; 3] {
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
                || !domain[axis][0].is_finite()
                || !domain[axis][1].is_finite()
                || uv[vertex][axis] < domain[axis][0]
                || uv[vertex][axis] > domain[axis][1]
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
            let average = scratch.uv[a][axis]
                .mul(half)
                .add(scratch.uv[b][axis].mul(half));
            // The exact average lies in BOTH the endpoint hull and the parent
            // box. This algebraic intersection is not source/chart authority.
            let lo = average
                .lo
                .max(uv[a][axis].min(uv[b][axis]))
                .max(domain[axis][0]);
            let hi = average
                .hi
                .min(uv[a][axis].max(uv[b][axis]))
                .min(domain[axis][1]);
            let Ok(average) = Interval::new(lo, hi) else {
                return Ok(None);
            };
            scratch.uv[midpoint][axis] = average;
        }
        for axis in 0..3 {
            // Never substitute F(midpoint): these are the exact averages on
            // the original affine physical triangle, enclosed outward.
            let average = scratch.points[a][axis]
                .mul(half)
                .add(scratch.points[b][axis].mul(half));
            if !average.lo.is_finite() || !average.hi.is_finite() {
                return Ok(None);
            }
            scratch.points[midpoint][axis] = average;
        }
    }
    for midpoint in 3..6 {
        work.charge(1, cancel)?;
        scratch.domain = scratch.uv[midpoint].map(|range| [range.lo, range.hi]);
        scratch.positions[midpoint] =
            surface_position_enclosure(surface, scratch.domain, work, cancel).map_err(|cause| {
                GeometryError::new(
                    cause.code,
                    format!(
                        "triangle proof midpoint={} uv_box={:?}: {}",
                        midpoint - 3,
                        scratch.domain,
                        cause.message,
                    ),
                )
            })?;
    }
    let mut complete = 0.0_f64;
    // 3=01, 4=12, 5=20. These EXACT conceptual children partition the parent;
    // their interval bounding boxes merely enclose them.
    for child in [[0, 3, 5], [3, 1, 4], [5, 4, 2], [3, 4, 5]] {
        work.charge(1, cancel)?;
        scratch.domain = std::array::from_fn(|axis| {
            [
                child
                    .iter()
                    .map(|&vertex| scratch.uv[vertex][axis].lo)
                    .fold(f64::INFINITY, f64::min)
                    .max(domain[axis][0]),
                child
                    .iter()
                    .map(|&vertex| scratch.uv[vertex][axis].hi)
                    .fold(f64::NEG_INFINITY, f64::max)
                    .min(domain[axis][1]),
            ]
        });
        let spans = scratch.domain.map(|range| {
            Interval::point(range[1])
                .sub(Interval::point(range[0]))
                .abs_upper()
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
    if !r.is_finite()
        || r <= 0.0
        || spans
            .into_iter()
            .any(|span| !span.is_finite() || span <= 0.0)
    {
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
        if diagonal
            .iter()
            .any(|value| !value.hi.is_finite() || value.hi < 0.0)
        {
            return Ok(None);
        }
        let diagonal = diagonal.map(|value| Interval::point(value.hi));
        let distance = |a: [f64; 2], b: [f64; 2]| {
            Interval::point(a[0])
                .sub(Interval::point(b[0]))
                .square()
                .mul(diagonal[0])
                .add(
                    Interval::point(a[1])
                        .sub(Interval::point(b[1]))
                        .square()
                        .mul(diagonal[1]),
                )
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
