// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! One immutable native parameter binding. Refinement re-evaluates the original
//! coefficients on the requested subset, never a sampled or frozen whole-span jet.

use super::{
    Interval,
    nurbs_eval::{basis_jet, binomial, conditioning, validate_knots},
};
use crate::{GeometryError, GeometryWorkBudget};
use solidcore::curve::{curve::Curve, curve::p2::CurveKind};
use solidcore::nurbs::crv_basic;
use spiling_contracts::geometry::GeometryErrorCode;
use std::sync::atomic::AtomicBool;
const ZERO: Interval = Interval::point(0.0);
pub const CURVE_EVALUATION_SCRATCH_BYTES: usize = super::nurbs_eval::BASIS_SCRATCH_BYTES
        + std::mem::size_of::<[[Interval; 4]; 4]>()
        // Source 2D MVT refinement retains its whole-span and midpoint tensors.
        + 2 * std::mem::size_of::<[[Interval; 3]; 4]>();

#[derive(Clone, Copy, Debug)]
pub struct CurveEnclosure {
    pub position: [Interval; 3],
    pub derivative: [Interval; 3],
    pub second_derivative: [Interval; 3],
}
impl CurveEnclosure {
    pub fn hull(self, other: Self) -> Self {
        Self {
            position: std::array::from_fn(|i| self.position[i].hull(other.position[i])),
            derivative: std::array::from_fn(|i| self.derivative[i].hull(other.derivative[i])),
            second_derivative: std::array::from_fn(|i| {
                self.second_derivative[i].hull(other.second_derivative[i])
            }),
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct CurveThirdEnclosure {
    pub jet: CurveEnclosure,
    pub third_derivative: [Interval; 3],
}
#[derive(Clone, Copy, Debug)]
pub struct ValidatedCurvePoint {
    pub position: [f64; 3],
    pub error: [Interval; 3],
}
#[derive(Clone, Copy, Debug)]
pub struct ValidatedScalarCurveJet {
    pub position: Interval,
    pub first: Interval,
    pub second: Interval,
}
#[derive(Clone, Copy, Debug)]
pub struct ChartCurveEnclosure {
    pub position: [Interval; 2],
    pub derivative: [Interval; 2],
    pub second_derivative: [Interval; 2],
}

pub(crate) fn native_curve_nurbs(
    curve: &Curve,
) -> Result<Option<&crv_basic::Curve>, GeometryError> {
    let base = match curve.kind() {
        CurveKind::BSpline(b) => b,
        CurveKind::Ellipse(e) => e.base(),
        CurveKind::Hyperbola(h) => h.base(),
        CurveKind::Parabola(p) => p.base(),
        CurveKind::Line(_) | CurveKind::CrvOnSurf(_) => return Ok(None),
        _ => {
            return Err(GeometryError::new(
                GeometryErrorCode::UnsupportedOperation,
                "native curve expression has no immutable interval evaluator",
            ));
        }
    };
    base.gw_nurb()
        .map(Some)
        .ok_or_else(|| conditioning("curve has no native NURBS coefficients"))
}

pub(super) fn curve_jet_order(
    curve: &Curve,
    range: [f64; 2],
    order: usize,
    depth: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[[Interval; 3]; 4], GeometryError> {
    work.charge(1, cancel)?;
    Interval::new(range[0], range[1])?;
    if depth >= 16 {
        return Err(GeometryError::new(
            GeometryErrorCode::ResourceLimit,
            "nested curve evaluation depth exceeded",
        ));
    }
    let domain = curve.natural_interval();
    if range[0] < domain.min() || range[1] > domain.max() {
        return Err(conditioning(
            "query extends outside native curve active domain",
        ));
    }
    if let CurveKind::Line(line) = curve.kind() {
        let c = line.canonical_unit();
        let t = Interval::new(range[0], range[1])?;
        let sign = Interval::point(if c.inside_out { -1.0 } else { 1.0 });
        let (mapped, slope) = if line.is_bounded() {
            let dn = Interval::point(domain.max()).sub(Interval::point(domain.min()));
            let da = Interval::point(c.domain.max()).sub(Interval::point(c.domain.min()));
            let slope = sign.mul(da.div(dn)?);
            let origin = if c.inside_out {
                domain.max()
            } else {
                domain.min()
            };
            (
                Interval::point(c.domain.min()).add(t.sub(Interval::point(origin)).mul(slope)),
                slope,
            )
        } else {
            (t.mul(sign), sign)
        };
        let p = [c.point.x, c.point.y, c.point.z];
        let v = [c.vector.x, c.vector.y, c.vector.z];
        let mut out = [[ZERO; 3]; 4];
        for k in 0..3 {
            let velocity = Interval::point(v[k]).mul(Interval::point(c.scale));
            out[0][k] = Interval::point(p[k]).add(mapped.mul(velocity));
            if order >= 1 {
                out[1][k] = slope.mul(velocity);
            }
        }
        for a in out.iter().flatten() {
            Interval::new(a.lo, a.hi)?;
        }
        return Ok(out);
    }
    if let CurveKind::CrvOnSurf(pcurve) = curve.kind() {
        let uv = curve_jet_order(pcurve.uv_curve(), range, order, depth + 1, work, cancel)?;
        return lift_surface_jet(
            pcurve.surface(),
            pcurve.uv_curve(),
            range,
            uv,
            order,
            work,
            cancel,
        );
    }
    let nurb =
        native_curve_nurbs(curve)?.ok_or_else(|| conditioning("missing curve coefficient law"))?;
    validate_knots(
        nurb.knots(),
        nurb.degree(),
        nurb.cpts().len(),
        range,
        work,
        cancel,
    )?;
    validated_nurbs_jet(curve, nurb, range, order, work, cancel)
}

/// Same coefficient arithmetic as the standalone evaluator. The private caller
/// holds completed original-knot validation for this immutable native curve.
fn validated_nurbs_jet(
    curve: &Curve,
    nurb: &crv_basic::Curve,
    range: [f64; 2],
    order: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[[Interval; 3]; 4], GeometryError> {
    work.charge(1, cancel)?;
    let knots = nurb.knots();
    let degree = nurb.degree();
    let count = nurb.cpts().len();
    Interval::new(range[0], range[1])?;
    if range[0] < knots[degree] || range[1] > knots[count] {
        return Err(conditioning(
            "prepared query exceeds complete native knot domain",
        ));
    }
    // Closed queries retain BOTH incident spans at an exact knot, including
    // all intervening repeated knots. Only proved-disjoint spans are skipped.
    work.charge(2 * (knots.len().max(1).ilog2() as u64 + 2), cancel)?;
    let first = knots
        .partition_point(|k| *k < range[0])
        .saturating_sub(1)
        .max(degree);
    let end = knots.partition_point(|k| *k <= range[1]).min(count);
    let mut result: Option<[[Interval; 3]; 4]> = None;
    for span in first..end {
        work.charge(1, cancel)?;
        if knots[span] == knots[span + 1] || knots[span] > range[1] || knots[span + 1] < range[0] {
            continue;
        }
        let clipped = [range[0].max(knots[span]), range[1].min(knots[span + 1])];
        let basis = basis_jet(knots, degree, span, clipped, order, work, cancel)?;
        let mut h = [[ZERO; 4]; 4];
        let anchor = nurb.cpts()[span - degree];
        let anchor_w = if nurb.is_rational() { anchor.w } else { 1.0 };
        let origin = if anchor_w != 0.0 {
            [
                anchor.x / anchor_w,
                anchor.y / anchor_w,
                if curve.dim() == 2 {
                    0.0
                } else {
                    anchor.z / anchor_w
                },
            ]
        } else {
            [0.0; 3]
        };
        for a in origin {
            Interval::new(a, a)?;
        }
        let mut weight_hull: Option<Interval> = None;
        #[expect(
            clippy::needless_range_loop,
            reason = "Native control points and the derivative basis tableau must retain the same span-relative coefficient index and summation order"
        )]
        for i in 0..=degree {
            work.charge(1, cancel)?;
            let c = nurb.cpts()[span - degree + i];
            if (c.w != solidcore::nurbs::math::NOW) != nurb.is_rational()
                || (curve.dim() != 2 && c.z == solidcore::nurbs::math::NOZ)
            {
                return Err(conditioning(
                    "inconsistent native homogeneous curve coefficients",
                ));
            }
            let w = if nurb.is_rational() { c.w } else { 1.0 };
            let coeff = [c.x, c.y, if curve.dim() == 2 { 0.0 } else { c.z }, w];
            for a in coeff {
                Interval::new(a, a)?;
            }
            let wi = Interval::point(w);
            weight_hull = Some(weight_hull.map_or(wi, |old| old.hull(wi)));
            for n in 0..=order {
                for k in 0..4 {
                    let coefficient = if k < 3 {
                        Interval::point(coeff[k]).sub(Interval::point(origin[k]).mul(wi))
                    } else {
                        wi
                    };
                    h[n][k] = h[n][k].add(basis[i][n].mul(coefficient));
                }
            }
        }
        if !nurb.is_rational() {
            for row in &mut h {
                row[3] = ZERO;
            }
            h[0][3] = Interval::point(1.0);
        }
        h[0][3] = h[0][3]
            .intersect(weight_hull.ok_or_else(|| conditioning("empty curve control net"))?)?;
        if h[0][3].contains_zero() {
            return Err(conditioning(
                "rational curve denominator not excluded from zero",
            ));
        }
        let mut jet = [[ZERO; 3]; 4];
        for n in 0..=order {
            for k in 0..3 {
                let mut value = h[n][k];
                for j in 1..=n {
                    value = value.sub(
                        Interval::point(binomial(n, j))
                            .mul(h[j][3])
                            .mul(jet[n - j][k]),
                    );
                }
                jet[n][k] = value.div(h[0][3])?;
                Interval::new(jet[n][k].lo, jet[n][k].hi)?;
            }
        }
        for k in 0..3 {
            jet[0][k] = jet[0][k].add(Interval::point(origin[k]));
        }
        result = Some(match result {
            None => jet,
            Some(old) => {
                std::array::from_fn(|n| std::array::from_fn(|k| old[n][k].hull(jet[n][k])))
            }
        });
    }
    result.ok_or_else(|| conditioning("no complete original curve knot span"))
}

pub struct PreparedValidatedCurve<'a> {
    curve: &'a dyn super::CurveEvaluator,
    range: [f64; 2],
    /// Spatial source-unit to output-unit conversion, not a parameter map.
    scale: f64,
    spans: super::AxisSpans<'a>,
    /// Present only after `parameter_spans` validated the original native
    /// knot inventory. Coefficients/denominators are still enclosed per query.
    native: Option<(&'a Curve, &'a crv_basic::Curve)>,
}
impl std::fmt::Debug for PreparedValidatedCurve<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedValidatedCurve")
            .field("range", &self.range)
            .field("scale", &self.scale)
            .finish_non_exhaustive()
    }
}
impl<'a> PreparedValidatedCurve<'a> {
    pub(crate) fn new(
        curve: &'a dyn super::CurveEvaluator,
        range: [f64; 2],
        scale: f64,
        max_retained_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        work.charge(1, cancel)?;
        for value in range {
            Interval::new(value, value)?;
        }
        if !scale.is_finite() || scale <= 0.0 {
            return Err(conditioning("invalid source spatial unit conversion"));
        }
        if max_retained_bytes < std::mem::size_of::<Self>() + Self::scratch_bytes() {
            return Err(GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "native curve owner and query scratch exceed byte allowance",
            ));
        }
        let spans = curve.parameter_spans(
            [range[0].min(range[1]), range[0].max(range[1])],
            work,
            cancel,
        )?;
        let native = match curve.native_curve() {
            Some(curve) => native_curve_nurbs(curve)?.map(|nurb| (curve, nurb)),
            None => None,
        };
        let owner = Self {
            curve,
            range,
            scale,
            spans,
            native,
        };
        owner.jet(range, work, cancel)?;
        Ok(owner)
    }
    pub const fn scratch_bytes() -> usize {
        16 * (CURVE_EVALUATION_SCRATCH_BYTES + super::nurbs_eval::SURFACE_EVALUATION_SCRATCH_BYTES)
            + super::continuity::SCRATCH_BYTES
    }
    fn enclosed(
        &self,
        range: [f64; 2],
        order: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[[Interval; 3]; 4], GeometryError> {
        for value in range {
            Interval::new(value, value)?;
        }
        let query = [range[0].min(range[1]), range[0].max(range[1])];
        let own = [
            self.range[0].min(self.range[1]),
            self.range[0].max(self.range[1]),
        ];
        Interval::new(query[0], query[1])?;
        if query[0] < own[0] || query[1] > own[1] {
            return Err(conditioning(
                "curve query extends outside immutable prepared binding",
            ));
        }
        let jet = match self.native {
            Some((curve, nurb)) => validated_nurbs_jet(curve, nurb, query, order, work, cancel)?,
            None => self.curve.tensor(query, order, work, cancel)?,
        };
        let result = jet.map(|row| row.map(|v| v.mul(Interval::point(self.scale))));
        for v in result.iter().flatten() {
            Interval::new(v.lo, v.hi)?;
        }
        Ok(result)
    }
    pub fn jet(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<CurveEnclosure, GeometryError> {
        let j = self.enclosed(range, 2, work, cancel)?;
        Ok(CurveEnclosure {
            position: j[0],
            derivative: j[1],
            second_derivative: j[2],
        })
    }
    pub fn third_jet(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<CurveThirdEnclosure, GeometryError> {
        let j = self.enclosed(range, 3, work, cancel)?;
        Ok(CurveThirdEnclosure {
            jet: CurveEnclosure {
                position: j[0],
                derivative: j[1],
                second_derivative: j[2],
            },
            third_derivative: j[3],
        })
    }
    pub fn position_bounds(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[Interval; 3], GeometryError> {
        Ok(self.enclosed(range, 0, work, cancel)?[0])
    }
    pub fn projected_jet(
        &self,
        range: [f64; 2],
        origin: [f64; 3],
        direction: [f64; 3],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<ValidatedScalarCurveJet, GeometryError> {
        for v in origin.into_iter().chain(direction) {
            Interval::new(v, v)?;
        }
        let jet = self.jet(range, work, cancel)?;
        let project = |v: [Interval; 3]| {
            (0..3).fold(ZERO, |sum, i| {
                sum.add(v[i].mul(Interval::point(direction[i])))
            })
        };
        let result = ValidatedScalarCurveJet {
            position: project(std::array::from_fn(|i| {
                jet.position[i].sub(Interval::point(origin[i]))
            })),
            first: project(jet.derivative),
            second: project(jet.second_derivative),
        };
        for v in [result.position, result.first, result.second] {
            Interval::new(v.lo, v.hi)?;
        }
        Ok(result)
    }
    pub fn is_smooth_on(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.enclosed(range, 0, work, cancel)?;
        if let Some((curve, nurb)) = self.native {
            return super::continuity::curve_is_c1(
                nurb,
                curve.dim(),
                [range[0].min(range[1]), range[0].max(range[1])],
                work,
                cancel,
            );
        }
        super::curve_parameter_span_smooth(
            self.curve,
            [range[0].min(range[1]), range[0].max(range[1])],
            work,
            cancel,
        )
    }
    /// Oriented projected Green integral of this same original binding. C1
    /// excludes derivative jumps; bounded piecewise second derivatives control
    /// the full arc/chord remainder without sampled area authority.
    pub fn signed_area_interval(
        &self,
        range: [f64; 2],
        frame: &crate::PlaneFrameMm,
        reference_mm: [f64; 3],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[f64; 2], GeometryError> {
        if !self.is_smooth_on(range, work, cancel)? {
            return Err(GeometryError::new(
                GeometryErrorCode::AnalysisIndeterminate,
                "native Green integral lacks closed-span C1 authority",
            ));
        }
        let jet = self.jet(range, work, cancel)?;
        let endpoints = [
            self.position_bounds([range[0]; 2], work, cancel)?,
            self.position_bounds([range[1]; 2], work, cancel)?,
        ];
        super::green::signed_area_from_curvature(
            frame,
            reference_mm,
            &endpoints,
            &jet.second_derivative,
            (range[0], range[1]),
            work,
            cancel,
        )
    }
    pub fn evaluate(
        &self,
        parameter: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<ValidatedCurvePoint, GeometryError> {
        let enclosure = self.position_bounds([parameter, parameter], work, cancel)?;
        let position = enclosure.map(|v| v.mid());
        let error = std::array::from_fn(|i| enclosure[i].sub(Interval::point(position[i])));
        Ok(ValidatedCurvePoint { position, error })
    }
    pub(crate) fn parameter_intervals(&self) -> super::AxisSpans<'a> {
        self.spans.clone()
    }
    /// Visit the complete sorted original explicit knot inventory, once each.
    /// False means a composed carrier has unisolated knot preimages; no callback
    /// has run and the caller must not treat the empty inventory as smoothness.
    pub fn visit_explicit_knots<F>(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
        visit: &mut F,
    ) -> Result<bool, GeometryError>
    where
        F: FnMut(f64, &mut GeometryWorkBudget, &AtomicBool) -> Result<(), GeometryError>,
    {
        self.enclosed(range, 0, work, cancel)?;
        let range = [range[0].min(range[1]), range[0].max(range[1])];
        let Some(mut source) = self.curve.native_curve() else {
            // A nonempty chart partition is not a complete composed-carrier
            // inventory. Establish literal source provenance before visiting it.
            let mut original = self.curve.original_curve();
            let mut literal_inventory = false;
            for _ in 0..16 {
                work.charge(1, cancel)?;
                match original {
                    Some(solidcraft_step::geom::Curve::Trimmed { basis, .. }) => {
                        original = Some(basis)
                    }
                    Some(
                        selected @ solidcraft_step::geom::Curve::SurfaceCurve { curve_3d, .. },
                    ) => {
                        if crate::kernel::source_operand::preferred_source_pcurve(selected)?
                            .is_some()
                        {
                            return Ok(false);
                        }
                        original = Some(curve_3d);
                    }
                    Some(
                        solidcraft_step::geom::Curve::BSpline(_)
                        | solidcraft_step::geom::Curve::Polyline(_),
                    ) => {
                        literal_inventory = true;
                        break;
                    }
                    Some(
                        solidcraft_step::geom::Curve::Line { .. }
                        | solidcraft_step::geom::Curve::Circle { .. }
                        | solidcraft_step::geom::Curve::Ellipse { .. }
                        | solidcraft_step::geom::Curve::Parabola { .. },
                    ) => return Ok(true),
                    _ => return Ok(false),
                }
            }
            if !literal_inventory {
                return Err(GeometryError::new(
                    GeometryErrorCode::ResourceLimit,
                    "original explicit knot wrapper depth exceeded",
                ));
            }
            if self.curve.source_parameter_map()
                != Some([Interval::point(0.0), Interval::point(1.0)])
            {
                return Ok(false);
            }
            let mut previous = None;
            for value in self.spans.knots {
                work.charge(1, cancel)?;
                if *value >= range[0] && *value <= range[1] && previous != Some(*value) {
                    visit(*value, work, cancel)?;
                    previous = Some(*value);
                }
            }
            return Ok(true);
        };
        for depth in 0..=16 {
            work.charge(1, cancel)?;
            if let CurveKind::CrvOnSurf(c) = source.kind() {
                if depth == 16 {
                    return Err(GeometryError::new(
                        GeometryErrorCode::ResourceLimit,
                        "explicit knot inventory nesting exceeded",
                    ));
                }
                if let Some(n) = super::native_surface_nurbs(c.surface())? {
                    for (knots, degree, count) in [
                        (n.u_knots(), n.p(), n.n() + 1),
                        (n.v_knots(), n.q(), n.m() + 1),
                    ] {
                        for value in knots {
                            work.charge(1, cancel)?;
                            if *value > knots[degree] && *value < knots[count] {
                                return Ok(false);
                            }
                        }
                    }
                }
                source = c.uv_curve();
                continue;
            }
            break;
        }
        if let Some(n) = native_curve_nurbs(source)? {
            let knots = n.knots();
            let degree = n.degree();
            let count = n.cpts().len();
            let mut previous = None;
            for value in knots {
                work.charge(1, cancel)?;
                if *value < range[0]
                    || *value > range[1]
                    || *value < knots[degree]
                    || *value > knots[count]
                {
                    continue;
                }
                if previous != Some(*value) {
                    visit(*value, work, cancel)?;
                    previous = Some(*value);
                }
            }
        }
        Ok(true)
    }
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
    pub(crate) fn curve(&self) -> &'a dyn super::CurveEvaluator {
        self.curve
    }
    pub fn range(&self) -> [f64; 2] {
        self.range
    }
    pub fn scale(&self) -> f64 {
        self.scale
    }
}

/// A third-order Taylor remainder needs no interior native knot, not merely
/// the existing C1 join receipt. Closed endpoint jets may include both sides;
/// the bound still contains the derivatives approached from this open cell.
pub(crate) fn native_curve_single_open_span(
    curve: &Curve,
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(1, cancel)?;
    Interval::new(range[0], range[1])?;
    if matches!(curve.kind(), CurveKind::Line(_)) {
        return Ok(true);
    }
    if matches!(curve.kind(), CurveKind::CrvOnSurf(_)) {
        return Ok(false);
    }
    let Some(n) = native_curve_nurbs(curve)? else {
        return Ok(false);
    };
    validate_knots(n.knots(), n.degree(), n.cpts().len(), range, work, cancel)?;
    for &knot in n.knots() {
        work.charge(1, cancel)?;
        if range[0] < knot && knot < range[1] {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Borrow the literal native chart composition without copying either operand.
/// The chart's parameter is independent of the spatial edge's gauge.
pub(crate) struct NativeChartCurve<'a> {
    pub(crate) surface: &'a solidcore::surface::surface::Surface,
    pub(crate) chart: &'a Curve,
}
impl NativeChartCurve<'_> {
    pub(crate) fn single_open_span(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        if !native_curve_single_open_span(self.chart, range, work, cancel)? {
            return Ok(false);
        }
        let Some(surface) = super::analytic_eval::native_surface_nurbs(self.surface)? else {
            // The literal affine/trigonometric native chart evaluator has no
            // piecewise knot interfaces. Its denominator/domain obligations
            // remain in the actual tensor and continuity evaluations.
            return Ok(true);
        };
        let uv = curve_jet_order(self.chart, range, 0, 0, work, cancel)?[0];
        let bounds = chart_image_box(self.surface, self.chart, range, uv, work, cancel)?;
        for (axis, knots) in [surface.u_knots(), surface.v_knots()]
            .into_iter()
            .enumerate()
        {
            for &knot in knots {
                work.charge(1, cancel)?;
                if bounds[axis][0] < knot && knot < bounds[axis][1] {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// A literal clamped tensor boundary and the same rational curve basis
    /// define the identical whole trace. This is positional authority only:
    /// the affine gauge is private and no original-source receipt is produced.
    pub(crate) fn isoparametric_coefficient_identity(
        &self,
        target: &Curve,
        target_range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        use super::CurveEvaluator;
        use crate::exact_coefficient::ExactCoefficient as E;

        fn products_equal(
            a: f64,
            b: f64,
            c: f64,
            d: f64,
            work: &mut GeometryWorkBudget,
            cancel: &AtomicBool,
        ) -> Result<bool, GeometryError> {
            for value in [a, b, c, d] {
                Interval::new(value, value)?;
            }
            let Some(mut left) = E::point(a).product(&E::point(b), work, cancel)? else {
                return Ok(false);
            };
            let Some(right) = E::point(c).product(&E::point(d), work, cancel)? else {
                return Ok(false);
            };
            Ok(left.subtract(&right).is_some() && left.sign() == 0)
        }

        work.charge(1, cancel)?;
        let Some(surface) = super::analytic_eval::native_surface_nurbs(self.surface)? else {
            return Ok(false);
        };
        let Some(spatial) = native_curve_nurbs(target)? else {
            return Ok(false);
        };
        // Do not infer an affine chart from endpoint samples. Only one literal
        // linear polynomial span (or the native affine LINE law) can use this
        // lane; its complete second derivative must vanish.
        if !matches!(self.chart.kind(), CurveKind::Line(_)) {
            let Some(chart) = native_curve_nurbs(self.chart)? else {
                return Ok(false);
            };
            if chart.degree() != 1 || chart.cpts().len() != 2 || chart.knots().len() != 4 {
                return Ok(false);
            }
        }
        let range = self.active_range();
        let chart = curve_jet_order(self.chart, range, 2, 0, work, cancel)?;
        if chart[2] != [ZERO; 3] {
            return Ok(false);
        }
        let first = curve_jet_order(self.chart, [range[0]; 2], 0, 0, work, cancel)?[0];
        let last = curve_jet_order(self.chart, [range[1]; 2], 0, 0, work, cancel)?[0];
        if first.iter().chain(&last).any(|value| !value.is_point()) {
            return Ok(false);
        }
        let Some(axis) = (0..2).find(|&axis| first[axis] != last[axis]) else {
            return Ok(false);
        };
        let fixed = 1 - axis;
        if first[fixed] != last[fixed] {
            return Ok(false);
        }
        let degrees = [surface.p(), surface.q()];
        let counts = [surface.n() + 1, surface.m() + 1];
        let knots = [surface.u_knots(), surface.v_knots()];
        let domain = self.surface.natural_uv_domain();
        let lo = domain.min();
        let hi = domain.max();
        for (i, interval) in [[lo.x, hi.x], [lo.y, hi.y]].into_iter().enumerate() {
            validate_knots(knots[i], degrees[i], counts[i], interval, work, cancel)?;
        }
        let a = first[axis].lo;
        let b = last[axis].lo;
        let limits = [knots[axis][degrees[axis]], knots[axis][counts[axis]]];
        if [a.min(b), a.max(b)] != limits
            || spatial.degree() != degrees[axis]
            || spatial.cpts().len() != counts[axis]
            || spatial.knots().len() != knots[axis].len()
        {
            return Ok(false);
        }
        let fixed_value = first[fixed].lo;
        let row = if fixed_value == knots[fixed][degrees[fixed]]
            && knots[fixed][..=degrees[fixed]]
                .iter()
                .all(|k| *k == fixed_value)
        {
            0
        } else if fixed_value == knots[fixed][counts[fixed]]
            && knots[fixed][counts[fixed]..]
                .iter()
                .all(|k| *k == fixed_value)
        {
            counts[fixed] - 1
        } else {
            return Ok(false);
        };
        let sorted_target = [
            target_range[0].min(target_range[1]),
            target_range[0].max(target_range[1]),
        ];
        validate_knots(
            spatial.knots(),
            spatial.degree(),
            spatial.cpts().len(),
            sorted_target,
            work,
            cancel,
        )?;
        if sorted_target
            != [
                spatial.knots()[spatial.degree()],
                spatial.knots()[spatial.cpts().len()],
            ]
        {
            return Ok(false);
        }
        let reverse = (b > a) != (target_range[1] > target_range[0]);
        let Some(source_length) = E::difference(b, a) else {
            return Ok(false);
        };
        let Some(target_length) = E::difference(target_range[1], target_range[0]) else {
            return Ok(false);
        };
        // Exact affine knot transport, including every repeated knot and both
        // literal endpoints. This fixes the complete branch and its direction.
        for (i, &knot) in knots[axis].iter().enumerate() {
            work.charge(1, cancel)?;
            let j = if reverse {
                spatial.knots().len() - 1 - i
            } else {
                i
            };
            let Some(source_delta) = E::difference(knot, a) else {
                return Ok(false);
            };
            let Some(target_delta) = E::difference(spatial.knots()[j], target_range[0]) else {
                return Ok(false);
            };
            let Some(mut left) = source_delta.product(&target_length, work, cancel)? else {
                return Ok(false);
            };
            let Some(right) = target_delta.product(&source_length, work, cancel)? else {
                return Ok(false);
            };
            if left.subtract(&right).is_none() || left.sign() != 0 {
                return Ok(false);
            }
        }
        let mut first_weights = None;
        for i in 0..counts[axis] {
            work.charge(1, cancel)?;
            let c = if axis == 0 {
                surface.cpt(i, row)
            } else {
                surface.cpt(row, i)
            }
            .map_err(|_| conditioning("missing native isoparametric coefficient"))?;
            let d = spatial.cpts()[if reverse { counts[axis] - 1 - i } else { i }];
            let cw = if surface.is_rational() { c.w } else { 1.0 };
            let dw = if spatial.is_rational() { d.w } else { 1.0 };
            if cw <= 0.0 || dw <= 0.0 || target.dim() != 3 {
                return Ok(false);
            }
            let (c0, d0) = *first_weights.get_or_insert((cw, dw));
            if !products_equal(cw, d0, dw, c0, work, cancel)? {
                return Ok(false);
            }
            for (x, y) in [c.x, c.y, c.z].into_iter().zip([d.x, d.y, d.z]) {
                if !products_equal(x, dw, y, cw, work, cancel)? {
                    return Ok(false);
                }
            }
        }
        // Identical rational coefficients still need complete C0 and active
        // chart authority; a discontinuous or out-of-domain row is no receipt.
        Ok(self.continuous(range, work, cancel)?
            && target.continuous(sorted_target, work, cancel)?)
    }
}

impl super::CurveEvaluator for NativeChartCurve<'_> {
    fn active_range(&self) -> [f64; 2] {
        let d = self.chart.natural_interval();
        [d.min(), d.max()]
    }
    fn native_chart_curve(&self) -> Option<NativeChartCurve<'_>> {
        Some(NativeChartCurve {
            surface: self.surface,
            chart: self.chart,
        })
    }
    fn tensor(
        &self,
        range: [f64; 2],
        order: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[[Interval; 3]; 4], GeometryError> {
        if order > 3 {
            return Err(conditioning("curve derivative order exceeds three"));
        }
        let uv = curve_jet_order(self.chart, range, order, 0, work, cancel)?;
        lift_surface_jet(self.surface, self.chart, range, uv, order, work, cancel)
    }
    fn continuous(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        if !super::curve_parameter_span_continuous(self.chart, range, work, cancel)? {
            return Ok(false);
        }
        let uv = curve_jet_order(self.chart, range, 0, 0, work, cancel)?[0];
        let bounds = chart_image_box(self.surface, self.chart, range, uv, work, cancel)?;
        super::surface_parameter_span_continuous(self.surface, bounds, work, cancel)
    }
    fn smooth(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        curve_on_surface_span_smooth(self.surface, self.chart, range, [0.0, 1.0], work, cancel)
    }
    fn parameter_spans(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<super::AxisSpans<'_>, GeometryError> {
        super::curve_parameter_spans(self.chart, range, work, cancel)
    }
}

fn chart_image_box(
    surface: &solidcore::surface::surface::Surface,
    chart: &Curve,
    range: [f64; 2],
    uv: [Interval; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[[f64; 2]; 2], GeometryError> {
    let mut bounds = [[uv[0].lo, uv[0].hi], [uv[1].lo, uv[1].hi]];
    let domain = super::surface_active_domain(surface)?;
    if (0..2).any(|i| bounds[i][0] < domain[i][0] || bounds[i][1] > domain[i][1])
        && super::curve2d_image_within_box(chart, range, domain, work, cancel)?
    {
        for i in 0..2 {
            bounds[i] = [
                bounds[i][0].max(domain[i][0]),
                bounds[i][1].min(domain[i][1]),
            ];
        }
    }
    Ok(bounds)
}

fn lift_surface_jet(
    surface: &solidcore::surface::surface::Surface,
    chart: &Curve,
    chart_range: [f64; 2],
    uv: [[Interval; 3]; 4],
    order: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[[Interval; 3]; 4], GeometryError> {
    let box_uv = chart_image_box(surface, chart, chart_range, uv[0], work, cancel)?;
    let s = super::analytic_eval::surface_tensor(surface, box_uv, order, work, cancel)?;
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
            let quadratic = s[2][0][k]
                .mul(u)
                .mul(uv[2][0])
                .add(s[1][1][k].mul(uv[2][0].mul(v).add(u.mul(uv[2][1]))))
                .add(s[0][2][k].mul(v).mul(uv[2][1]));
            out[3][k] = s[1][0][k]
                .mul(uv[3][0])
                .add(s[0][1][k].mul(uv[3][1]))
                .add(Interval::point(3.0).mul(quadratic))
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

/// Literal original chart-parameter correspondence, with no orientation guess.
/// Exact endpoint comparisons may justify intersecting a rounded map enclosure
/// with the known active parameter interval; nearness never does.
pub(super) fn mapped_chart_range(
    chart: &Curve,
    range: [f64; 2],
    map: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[f64; 2], GeometryError> {
    work.charge(1, cancel)?;
    Interval::new(range[0], range[1])?;
    for v in map {
        Interval::new(v, v)?;
    }
    let mapped = Interval::point(map[0])
        .add(Interval::point(map[1]).mul(Interval::new(range[0], range[1])?));
    Interval::new(mapped.lo, mapped.hi)?;
    let domain = chart.natural_interval();
    if mapped.lo >= domain.min() && mapped.hi <= domain.max() {
        return Ok([mapped.lo, mapped.hi]);
    }
    use crate::exact_coefficient::ExactCoefficient;
    for endpoint in range {
        let Some(mut exact) = ExactCoefficient::point(map[1]).product(
            &ExactCoefficient::point(endpoint),
            work,
            cancel,
        )?
        else {
            return Err(conditioning(
                "source chart affine map exceeds exact arithmetic conditioning",
            ));
        };
        if exact.push(map[0]).is_none() {
            return Err(conditioning("source chart affine map expansion overflow"));
        }
        let mut lower = exact.clone();
        let mut upper = exact;
        if lower
            .subtract(&ExactCoefficient::point(domain.min()))
            .is_none()
            || upper
                .subtract(&ExactCoefficient::point(domain.max()))
                .is_none()
            || lower.sign() < 0
            || upper.sign() > 0
        {
            return Err(conditioning(
                "literal chart parameter map exceeds native active domain",
            ));
        }
    }
    Ok([mapped.lo.max(domain.min()), mapped.hi.min(domain.max())])
}

/// Whole original composition S(UV(offset+slope*t)), with derivatives in the
/// original spatial curve parameter. It grants no C0/C1 or correspondence claim.
pub fn curve_on_surface_enclosure(
    surface: &solidcore::surface::surface::Surface,
    chart: &Curve,
    range: [f64; 2],
    chart_parameter_map: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<CurveThirdEnclosure, GeometryError> {
    let mapped = mapped_chart_range(chart, range, chart_parameter_map, work, cancel)?;
    let mut uv = curve_jet_order(chart, mapped, 3, 0, work, cancel)?;
    let slope = Interval::point(chart_parameter_map[1]);
    let mut factor = slope;
    for row in uv.iter_mut().skip(1) {
        for v in row {
            *v = v.mul(factor);
        }
        factor = factor.mul(slope);
    }
    let j = lift_surface_jet(surface, chart, mapped, uv, 3, work, cancel)?;
    Ok(CurveThirdEnclosure {
        jet: CurveEnclosure {
            position: j[0],
            derivative: j[1],
            second_derivative: j[2],
        },
        third_derivative: j[3],
    })
}

pub fn curve_on_surface_span_smooth(
    surface: &solidcore::surface::surface::Surface,
    chart: &Curve,
    range: [f64; 2],
    chart_parameter_map: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let mapped = mapped_chart_range(chart, range, chart_parameter_map, work, cancel)?;
    if !super::curve_parameter_span_smooth(chart, mapped, work, cancel)? {
        return Ok(false);
    }
    let uv = curve_jet_order(chart, mapped, 0, 0, work, cancel)?[0];
    let bounds = chart_image_box(surface, chart, mapped, uv, work, cancel)?;
    super::surface_parameter_span_smooth(surface, bounds, work, cancel)
}

/// Exact coordinate identity at a literal native endpoint. Interior spline
/// parameters may use a complete constant-coordinate support identity, never an
/// overlapping position interval. This is separate from straightness/regularity.
pub(crate) fn curve_endpoint_coordinate_equal(
    curve: &Curve,
    parameter: f64,
    axis: usize,
    expected: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    use crate::exact_coefficient::ExactCoefficient;
    work.charge(1, cancel)?;
    if axis >= 3 {
        return Err(conditioning(
            "native endpoint coordinate axis exceeds three",
        ));
    }
    Interval::new(expected, expected)?;
    curve_jet_order(curve, [parameter; 2], 0, 0, work, cancel)?;
    if let CurveKind::Line(line) = curve.kind() {
        let c = line.canonical_unit();
        let domain = curve.natural_interval();
        let p = [c.point.x, c.point.y, c.point.z][axis];
        let v = [c.vector.x, c.vector.y, c.vector.z][axis];
        let Some(velocity) =
            ExactCoefficient::point(v).product(&ExactCoefficient::point(c.scale), work, cancel)?
        else {
            return Ok(false);
        };
        let sign = if c.inside_out { -1.0 } else { 1.0 };
        let (mapped, denominator) = if line.is_bounded() {
            let Some(dn) = ExactCoefficient::difference(domain.max(), domain.min()) else {
                return Ok(false);
            };
            let Some(da) = ExactCoefficient::difference(c.domain.max(), c.domain.min()) else {
                return Ok(false);
            };
            if dn.sign() <= 0 {
                return Ok(false);
            }
            let origin = if c.inside_out {
                domain.max()
            } else {
                domain.min()
            };
            let Some(delta) = ExactCoefficient::difference(parameter, origin) else {
                return Ok(false);
            };
            let Some(delta) = delta.product(&da, work, cancel)? else {
                return Ok(false);
            };
            let Some(delta) = delta.product(&ExactCoefficient::point(sign), work, cancel)? else {
                return Ok(false);
            };
            let Some(mut numerator) =
                ExactCoefficient::point(c.domain.min()).product(&dn, work, cancel)?
            else {
                return Ok(false);
            };
            for term in &delta.terms[..delta.len] {
                work.charge(1, cancel)?;
                if numerator.push(*term).is_none() {
                    return Ok(false);
                }
            }
            (numerator, dn)
        } else {
            (
                ExactCoefficient::point(parameter * sign),
                ExactCoefficient::point(1.0),
            )
        };
        let Some(mut actual) = velocity.product(&mapped, work, cancel)? else {
            return Ok(false);
        };
        let Some(offset) = ExactCoefficient::difference(p, expected) else {
            return Ok(false);
        };
        let Some(offset) = offset.product(&denominator, work, cancel)? else {
            return Ok(false);
        };
        for term in &offset.terms[..offset.len] {
            work.charge(1, cancel)?;
            if actual.push(*term).is_none() {
                return Ok(false);
            }
        }
        return Ok(actual.sign() == 0);
    }
    let Some(n) = native_curve_nurbs(curve)? else {
        return Ok(false);
    };
    let degree = n.degree();
    let knots = n.knots();
    let count = n.cpts().len();
    let clamped = if parameter == knots[degree] && knots[..=degree].iter().all(|v| *v == parameter)
    {
        Some(0)
    } else if parameter == knots[count] && knots[count..].iter().all(|v| *v == parameter) {
        Some(count - 1)
    } else {
        None
    };
    let mut found = false;
    for span in degree..count {
        work.charge(1, cancel)?;
        if knots[span] == knots[span + 1] || parameter < knots[span] || parameter > knots[span + 1]
        {
            continue;
        }
        for i in span - degree..=span {
            if let Some(endpoint) = clamped
                && i != endpoint
            {
                continue;
            }
            work.charge(1, cancel)?;
            found = true;
            let c = n.cpts()[i];
            let value = [c.x, c.y, if curve.dim() == 2 { 0.0 } else { c.z }][axis];
            let w = if n.is_rational() { c.w } else { 1.0 };
            let Some(mut difference) = ExactCoefficient::point(expected).product(
                &ExactCoefficient::point(w),
                work,
                cancel,
            )?
            else {
                return Ok(false);
            };
            if difference
                .subtract(&ExactCoefficient::point(value))
                .is_none()
                || difference.sign() != 0
            {
                return Ok(false);
            }
        }
    }
    Ok(found)
}
