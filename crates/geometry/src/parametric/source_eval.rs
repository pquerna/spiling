// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! One borrowed query interface; a native chart and an original radian chart
//! remain distinct actual operands, never aliases or retagged geometry.
use super::nurbs_eval::SurfaceTensor;
use super::*;
pub(crate) use crate::kernel::source_operand::ExactAffineCurve;
use crate::kernel::source_operand::{PreparedOriginalCurve, PreparedOriginalSurface};

pub(crate) trait SurfaceEvaluator {
    fn tensor(
        &self,
        uv: [[f64; 2]; 2],
        order: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<SurfaceTensor, GeometryError>;
    fn continuous(
        &self,
        uv: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError>;
    fn smooth(
        &self,
        uv: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError>;
    fn parameter_axes(
        &self,
        uv: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[(&[f64], usize); 2], GeometryError>;
    fn native_surface(&self) -> Option<&Surface> {
        None
    }
    fn original_surface(&self) -> Option<&solidcraft_step::geom::Surface> {
        None
    }
}
pub(crate) trait CurveEvaluator {
    fn active_range(&self) -> [f64; 2];
    fn exact_affine_coefficients(
        &self,
        _work: &mut GeometryWorkBudget,
        _cancel: &AtomicBool,
    ) -> Result<Option<ExactAffineCurve>, GeometryError> {
        Ok(None)
    }
    /// Query-parameter -> original raw source [offset,slope] enclosure. None
    /// means there is no established original gauge map, never identity by default.
    fn source_parameter_map(&self) -> Option<[Interval; 2]> {
        None
    }
    fn source_parameter_image(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Interval>, GeometryError> {
        work.charge(1, cancel)?;
        let input = Interval::new(range[0], range[1])?;
        let active = self.active_range();
        if range[0] < active[0] || range[1] > active[1] {
            return Err(nurbs_eval::conditioning(
                "source parameter image exceeds its authorized query domain",
            ));
        }
        Ok(self
            .source_parameter_map()
            .map(|m| m[0].add(m[1].mul(input))))
    }
    fn tensor(
        &self,
        range: [f64; 2],
        order: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[[Interval; 3]; 4], GeometryError>;
    fn continuous(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError>;
    fn smooth(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError>;
    fn parameter_spans(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<AxisSpans<'_>, GeometryError>;
    fn native_curve(&self) -> Option<&Curve> {
        None
    }
    fn native_chart_curve(&self) -> Option<curve_prepared::NativeChartCurve<'_>> {
        None
    }
    fn original_curve(&self) -> Option<&solidcraft_step::geom::Curve> {
        None
    }
}
impl SurfaceEvaluator for Surface {
    fn tensor(
        &self,
        uv: [[f64; 2]; 2],
        order: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<SurfaceTensor, GeometryError> {
        analytic_eval::surface_tensor(self, uv, order, work, cancel)
    }
    fn continuous(
        &self,
        uv: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        super::native_surface_parameter_span_continuous(self, uv, work, cancel)
    }
    fn smooth(
        &self,
        uv: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        super::native_surface_parameter_span_smooth(self, uv, work, cancel)
    }
    fn parameter_axes(
        &self,
        uv: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[(&[f64], usize); 2], GeometryError> {
        super::validate_surface_partition(self, uv, work, cancel)?;
        let a = super::surface_knots(self, work, cancel)?;
        Ok([(a[0].values, a[0].degree), (a[1].values, a[1].degree)])
    }
    fn native_surface(&self) -> Option<&Surface> {
        Some(self)
    }
}
impl SurfaceEvaluator for PreparedOriginalSurface<'_> {
    fn tensor(
        &self,
        uv: [[f64; 2]; 2],
        order: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<SurfaceTensor, GeometryError> {
        self.evaluate_order(uv, order, work, cancel)
    }
    fn continuous(
        &self,
        uv: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.span_is_continuous(uv, work, cancel)
    }
    fn smooth(
        &self,
        uv: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.span_is_smooth(uv, work, cancel)
    }
    fn parameter_axes(
        &self,
        uv: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[(&[f64], usize); 2], GeometryError> {
        let active = self.active_domain();
        for i in 0..2 {
            Interval::new(uv[i][0], uv[i][1])?;
            if uv[i][0] < active[i][0] || uv[i][1] > active[i][1] {
                return Err(nurbs_eval::conditioning(
                    "original surface partition exceeds its authorized domain",
                ));
            }
        }
        PreparedOriginalSurface::parameter_axes(self, work, cancel)
    }
    fn original_surface(&self) -> Option<&solidcraft_step::geom::Surface> {
        Some(self.source())
    }
}

impl CurveEvaluator for Curve {
    fn active_range(&self) -> [f64; 2] {
        if let Ok(Some(n)) = curve_prepared::native_curve_nurbs(self)
            && let (Some(a), Some(b)) = (n.knots().get(n.degree()), n.knots().get(n.cpts().len()))
        {
            return [*a, *b];
        }
        let d = self.natural_interval();
        [d.min(), d.max()]
    }
    fn exact_affine_coefficients(
        &self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<ExactAffineCurve>, GeometryError> {
        use crate::exact_coefficient::ExactCoefficient as E;
        work.charge(1, cancel)?;
        let solidcore::curve::curve::CurveKind::Line(line) = self.kind() else {
            return Ok(None);
        };
        // Bounded native lines have a separate NURB-to-analytic map, including
        // a native short-domain branch. No affine receipt is claimed for it.
        if line.is_bounded() {
            return Ok(None);
        }
        let c = line.canonical_unit();
        let point = [c.point.x, c.point.y, c.point.z];
        let vector = [c.vector.x, c.vector.y, c.vector.z];
        for value in point.into_iter().chain(vector).chain([c.scale]) {
            Interval::new(value, value)?;
        }
        // The actual unbounded native gauge is +/-q. Preserve the separately
        // stored vector and scale: their rounded product is not a coefficient.
        let mut derivative = std::array::from_fn(|_| E::point(0.0));
        for axis in 0..3 {
            let Some(mut velocity) =
                E::point(vector[axis]).product(&E::point(c.scale), work, cancel)?
            else {
                return Ok(None);
            };
            if c.inside_out {
                for term in &mut velocity.terms[..velocity.len] {
                    *term = -*term;
                }
            }
            derivative[axis] = velocity;
        }
        Ok(Some(ExactAffineCurve {
            position: point.map(E::point),
            derivative,
            denominator: E::point(1.0),
        }))
    }
    fn tensor(
        &self,
        range: [f64; 2],
        order: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[[Interval; 3]; 4], GeometryError> {
        curve_prepared::curve_jet_order(self, range, order, 0, work, cancel)
    }
    fn continuous(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        super::native_curve_parameter_span_continuous(self, range, work, cancel)
    }
    fn smooth(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        super::native_curve_parameter_span_smooth(self, range, work, cancel)
    }
    fn parameter_spans(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<AxisSpans<'_>, GeometryError> {
        super::native_curve_parameter_spans(self, range, work, cancel)
    }
    fn native_curve(&self) -> Option<&Curve> {
        Some(self)
    }
}
impl CurveEvaluator for PreparedOriginalCurve<'_> {
    fn active_range(&self) -> [f64; 2] {
        PreparedOriginalCurve::active_range(self)
    }
    fn source_parameter_map(&self) -> Option<[Interval; 2]> {
        Some([Interval::point(0.0), Interval::point(1.0)])
    }
    fn exact_affine_coefficients(
        &self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<ExactAffineCurve>, GeometryError> {
        PreparedOriginalCurve::exact_affine_coefficients(self, work, cancel)
    }
    fn tensor(
        &self,
        range: [f64; 2],
        order: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[[Interval; 3]; 4], GeometryError> {
        self.evaluate_order(range, order, work, cancel)
    }
    fn continuous(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.span_is_continuous(range, work, cancel)
    }
    fn smooth(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.span_is_smooth(range, work, cancel)
    }
    fn parameter_spans(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<AxisSpans<'_>, GeometryError> {
        let active = self.active_range();
        Interval::new(range[0], range[1])?;
        if range[0] < active[0] || range[1] > active[1] {
            return Err(nurbs_eval::conditioning(
                "original curve partition exceeds its authorized branch",
            ));
        }
        let (values, degree) = self.parameter_axis(work, cancel)?;
        Ok(AxisSpans::new(KnotAxis { values, degree }, range))
    }
    fn original_curve(&self) -> Option<&solidcraft_step::geom::Curve> {
        Some(self.source())
    }
}
fn surface_jet(j: SurfaceTensor) -> SurfaceEnclosure {
    SurfaceEnclosure {
        position: j[0][0],
        du: j[1][0],
        dv: j[0][1],
        duu: j[2][0],
        duv: j[1][1],
        dvv: j[0][2],
    }
}
pub(crate) fn native_surface_nurbs<S: SurfaceEvaluator + ?Sized>(
    surface: &S,
) -> Result<Option<&solidcore::nurbs::srf_basic::Surface>, GeometryError> {
    surface
        .native_surface()
        .map_or(Ok(None), analytic_eval::native_surface_nurbs)
}
pub(crate) fn native_curve_nurbs<C: CurveEvaluator + ?Sized>(
    curve: &C,
) -> Result<Option<&solidcore::nurbs::crv_basic::Curve>, GeometryError> {
    curve
        .native_curve()
        .map_or(Ok(None), curve_prepared::native_curve_nurbs)
}
pub(crate) fn surface_piecewise_enclosure<S: SurfaceEvaluator + ?Sized>(
    surface: &S,
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SurfaceEnclosure, GeometryError> {
    if let Some(native) = surface.native_surface() {
        return super::native_surface_piecewise_enclosure(native, uv, work, cancel);
    }
    Ok(surface_jet(surface.tensor(uv, 2, work, cancel)?))
}
pub(crate) fn surface_enclosure<S: SurfaceEvaluator + ?Sized>(
    surface: &S,
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SurfaceEnclosure, GeometryError> {
    if let Some(native) = surface.native_surface() {
        return super::native_surface_enclosure(native, uv, work, cancel);
    }
    if !surface.smooth(uv, work, cancel)? {
        return Err(GeometryError::new(
            GeometryErrorCode::AnalysisIndeterminate,
            "original surface lacks closed-box C1 authority",
        ));
    }
    Ok(surface_jet(surface.tensor(uv, 2, work, cancel)?))
}
pub(crate) fn surface_position_enclosure<S: SurfaceEvaluator + ?Sized>(
    surface: &S,
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[Interval; 3], GeometryError> {
    if let Some(native) = surface.native_surface() {
        return super::native_surface_position_enclosure(native, uv, work, cancel);
    }
    Ok(surface.tensor(uv, 0, work, cancel)?[0][0])
}
pub(crate) fn surface_piecewise_first_enclosure<S: SurfaceEvaluator + ?Sized>(
    surface: &S,
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SurfaceFirstEnclosure, GeometryError> {
    if let Some(native) = surface.native_surface() {
        return super::native_surface_piecewise_first_enclosure(native, uv, work, cancel);
    }
    if !surface.continuous(uv, work, cancel)? {
        return Err(GeometryError::new(
            GeometryErrorCode::AnalysisIndeterminate,
            "original surface lacks whole-box C0 authority",
        ));
    }
    let j = surface.tensor(uv, 1, work, cancel)?;
    Ok(SurfaceFirstEnclosure {
        position: j[0][0],
        du: j[1][0],
        dv: j[0][1],
    })
}
pub(crate) fn surface_functional_enclosures<const N: usize>(
    surface: &(impl SurfaceEvaluator + ?Sized),
    uv: [[f64; 2]; 2],
    functionals: &[SurfaceFunctional; N],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[SurfaceFunctionalEnclosure; N], GeometryError> {
    if let Some(native) = surface.native_surface() {
        return super::native_surface_functional_enclosures(native, uv, functionals, work, cancel);
    }
    let j = surface_piecewise_first_enclosure(surface, uv, work, cancel)?;
    work.charge(N as u64, cancel)?;
    for f in functionals {
        for v in f.direction.into_iter().chain(f.origin).chain([f.offset]) {
            Interval::new(v.lo, v.hi)?;
        }
    }
    let rows = std::array::from_fn(|n| {
        let f = functionals[n];
        let project = |v: [Interval; 3]| {
            (0..3).fold(Interval::point(0.0), |s, k| s.add(v[k].mul(f.direction[k])))
        };
        SurfaceFunctionalEnclosure {
            position: project(std::array::from_fn(|k| j.position[k].sub(f.origin[k])))
                .add(f.offset),
            du: project(j.du),
            dv: project(j.dv),
        }
    });
    for r in rows {
        for v in [r.position, r.du, r.dv] {
            Interval::new(v.lo, v.hi)?;
        }
    }
    Ok(rows)
}
pub(crate) fn surface_geometric_normal_enclosure<S: SurfaceEvaluator + ?Sized>(
    surface: &S,
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[Interval; 3], GeometryError> {
    if let Some(native) = surface.native_surface() {
        return super::native_surface_geometric_normal_enclosure(native, uv, work, cancel);
    }
    if !surface.smooth(uv, work, cancel)? {
        return Err(GeometryError::new(
            GeometryErrorCode::AnalysisIndeterminate,
            "original normal lacks C1 authority",
        ));
    }
    let j = surface.tensor(uv, 1, work, cancel)?;
    let normal = super::cross(j[1][0], j[0][1]);
    let length = normal
        .into_iter()
        .fold(Interval::point(0.0), |s, v| s.add(v.square()))
        .sqrt()?;
    if length.lo <= 0.0 {
        return Err(GeometryError::new(
            GeometryErrorCode::SingularGeometry,
            "original source normal has no nonsingular enclosure",
        ));
    }
    Ok([
        normal[0].div(length)?,
        normal[1].div(length)?,
        normal[2].div(length)?,
    ])
}
pub(crate) fn surface_parameter_span_smooth<S: SurfaceEvaluator + ?Sized>(
    surface: &S,
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    surface.smooth(uv, work, cancel)
}
pub(crate) fn surface_parameter_span_continuous<S: SurfaceEvaluator + ?Sized>(
    surface: &S,
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    surface.continuous(uv, work, cancel)
}
pub(crate) fn curve_parameter_span_smooth<C: CurveEvaluator + ?Sized>(
    curve: &C,
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    curve.smooth(range, work, cancel)
}
pub(crate) fn curve_parameter_span_continuous<C: CurveEvaluator + ?Sized>(
    curve: &C,
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    curve.continuous(range, work, cancel)
}
pub(crate) fn surface_parameter_boxes<'a, S: SurfaceEvaluator + ?Sized>(
    surface: &'a S,
    domain: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SurfaceParameterBoxes<'a>, GeometryError> {
    if let Some(native) = surface.native_surface() {
        return super::native_surface_parameter_boxes(native, domain, work, cancel);
    }
    let axes = surface.parameter_axes(domain, work, cancel)?;
    let u = AxisSpans::new(
        KnotAxis {
            values: axes[0].0,
            degree: axes[0].1,
        },
        domain[0],
    );
    let v = AxisSpans::new(
        KnotAxis {
            values: axes[1].0,
            degree: axes[1].1,
        },
        domain[1],
    );
    let count = u
        .clone()
        .count()
        .checked_mul(v.clone().count())
        .ok_or_else(|| {
            GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "original knot product overflow",
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
pub(crate) fn surface_crease_interfaces<'a, S: SurfaceEvaluator + ?Sized>(
    surface: &'a S,
    domain: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SurfaceCreaseInterfaces<'a>, GeometryError> {
    if let Some(native) = surface.native_surface() {
        return super::native_surface_crease_interfaces(native, domain, work, cancel);
    }
    let raw = surface.parameter_axes(domain, work, cancel)?;
    let axes = raw.map(|(values, degree)| KnotAxis { values, degree });
    let count = axes[0]
        .values
        .len()
        .checked_mul(axes[1].values.len().max(1))
        .ok_or_else(|| {
            GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "original crease inventory overflow",
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
pub(crate) fn curve_parameter_spans<'a, C: CurveEvaluator + ?Sized>(
    curve: &'a C,
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<AxisSpans<'a>, GeometryError> {
    curve.parameter_spans(range, work, cancel)
}
pub(crate) fn curve2d_parameter_spans<'a, C: CurveEvaluator + ?Sized>(
    curve: &'a C,
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<AxisSpans<'a>, GeometryError> {
    if let Some(native) = curve.native_curve() {
        return super::native_curve2d_parameter_spans(native, range, work, cancel);
    }
    curve.parameter_spans(range, work, cancel)
}
pub(crate) fn curve_enclosure<C: CurveEvaluator + ?Sized>(
    curve: &C,
    range: (f64, f64),
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<CurveEnclosure, GeometryError> {
    if let Some(native) = curve.native_curve() {
        return super::native_curve_enclosure(native, range, work, cancel);
    }
    for v in [range.0, range.1] {
        Interval::new(v, v)?;
    }
    let j = curve.tensor(
        [range.0.min(range.1), range.0.max(range.1)],
        2,
        work,
        cancel,
    )?;
    Ok(CurveEnclosure {
        position: j[0],
        derivative: j[1],
        second_derivative: j[2],
    })
}
pub(crate) fn curve2d_piecewise_jet<C: CurveEvaluator + ?Sized>(
    curve: &C,
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<ChartCurveEnclosure, GeometryError> {
    if let Some(native) = curve.native_curve() {
        return super::native_curve2d_piecewise_jet(native, range, work, cancel);
    }
    let mut j = curve.tensor(range, 2, work, cancel)?;
    if range[0] < range[1]
        && (0..2)
            .any(|axis| j[0][axis].width() > up((range[1] - range[0]) * j[1][axis].abs_upper()))
        && curve.continuous(range, work, cancel)?
    {
        // The midpoint proposes no geometry: its original position enclosure
        // plus the COMPLETE derivative hull encloses every point by the MVT.
        // C0 admits derivative jumps; without C0 retain the natural overcover.
        // Use raw tensor evaluation, not recursive refined-jet evaluation.
        let middle = range[0] * 0.5 + range[1] * 0.5;
        let at_middle = curve.tensor([middle; 2], 0, work, cancel)?;
        let delta = Interval::new(range[0], range[1])?.sub(Interval::point(middle));
        for axis in 0..2 {
            j[0][axis] = j[0][axis].intersect(at_middle[0][axis].add(delta.mul(j[1][axis])))?;
        }
    }
    Ok(ChartCurveEnclosure {
        position: [j[0][0], j[0][1]],
        derivative: [j[1][0], j[1][1]],
        second_derivative: [j[2][0], j[2][1]],
    })
}

impl CurveEvaluator for crate::analysis::source_restriction::SourceLineRestriction<'_> {
    fn active_range(&self) -> [f64; 2] {
        [0.0, 1.0]
    }
    fn source_parameter_map(&self) -> Option<[Interval; 2]> {
        let p = self.endpoint_parameters();
        Some([p[0], p[1].sub(p[0])])
    }
    fn source_parameter_image(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Interval>, GeometryError> {
        Ok(Some(self.parameter_image(range, work, cancel)?))
    }
    fn exact_affine_coefficients(
        &self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<ExactAffineCurve>, GeometryError> {
        crate::kernel::source_operand::exact_source_line_projection_coefficients(
            self.source_curve(),
            self.source_vertices(),
            work,
            cancel,
        )
    }
    fn tensor(
        &self,
        range: [f64; 2],
        order: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[[Interval; 3]; 4], GeometryError> {
        if order > 3 {
            return Err(nurbs_eval::conditioning(
                "source LINE derivative order exceeds three",
            ));
        }
        let j = self.jet(range, work, cancel)?;
        Ok([
            j.jet.position,
            j.jet.derivative,
            j.jet.second_derivative,
            j.third_derivative,
        ])
    }
    fn continuous(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.jet(range, work, cancel)?;
        Ok(true)
    }
    fn smooth(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.jet(range, work, cancel)?;
        Ok(true)
    }
    fn parameter_spans(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<AxisSpans<'_>, GeometryError> {
        self.jet(range, work, cancel)?;
        AxisSpans::single(range)
    }
    fn original_curve(&self) -> Option<&solidcraft_step::geom::Curve> {
        Some(self.source_curve())
    }
}

pub(crate) fn curve_endpoint_coordinate_equal<C: CurveEvaluator + ?Sized>(
    curve: &C,
    parameter: f64,
    axis: usize,
    expected: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(1, cancel)?;
    if let Some(native) = curve.native_curve() {
        return curve_prepared::curve_endpoint_coordinate_equal(
            native, parameter, axis, expected, work, cancel,
        );
    }
    if axis >= 3 {
        return Err(nurbs_eval::conditioning(
            "original endpoint coordinate axis exceeds three",
        ));
    }
    Interval::new(expected, expected)?;
    Interval::new(parameter, parameter)?;
    let active = curve.active_range();
    if parameter < active[0] || parameter > active[1] {
        return Err(nurbs_eval::conditioning(
            "original endpoint parameter exceeds active curve domain",
        ));
    }
    if let Some(affine) = curve.exact_affine_coefficients(work, cancel)? {
        return affine.endpoint_coordinate_equal(parameter, axis, expected, work, cancel);
    }
    let position = curve.tensor([parameter; 2], 0, work, cancel)?[0];
    // An exact singleton interval is an arithmetic identity, unlike overlap or
    // a rounded point sample. No derivative/straightness authority follows.
    if position[axis] == Interval::point(expected) {
        return Ok(true);
    }
    if curve.source_parameter_map() != Some([Interval::point(0.0), Interval::point(1.0)]) {
        return Ok(false);
    }
    let Some(mut original) = curve.original_curve() else {
        return Ok(false);
    };
    for _ in 0..16 {
        work.charge(1, cancel)?;
        match original {
            solidcraft_step::geom::Curve::Trimmed { basis, .. } => original = basis,
            solidcraft_step::geom::Curve::SurfaceCurve { curve_3d, .. } => {
                if crate::kernel::source_operand::preferred_source_pcurve(original)?.is_some() {
                    return Ok(false);
                }
                original = curve_3d;
            }
            solidcraft_step::geom::Curve::BSpline(s) => {
                let spans = curve.parameter_spans([parameter; 2], work, cancel)?;
                let k = spans.knots;
                let degree = s.degree;
                let count = s.points.len();
                let endpoint =
                    if parameter == k[degree] && k[..=degree].iter().all(|v| *v == parameter) {
                        Some(0)
                    } else if parameter == k[count] && k[count..].iter().all(|v| *v == parameter) {
                        Some(count - 1)
                    } else {
                        None
                    };
                if let Some(at) = endpoint {
                    return Ok(s.points[at][axis] == expected);
                }
                return Ok(s.points.iter().all(|p| p[axis] == expected));
            }
            solidcraft_step::geom::Curve::Polyline(points) => {
                let at = parameter as usize;
                if at < points.len() && at as f64 == parameter {
                    return Ok(points[at][axis] == expected);
                }
                return Ok(at + 1 < points.len()
                    && points[at][axis] == expected
                    && points[at + 1][axis] == expected);
            }
            _ => return Ok(false),
        }
    }
    Err(GeometryError::new(
        GeometryErrorCode::ResourceLimit,
        "original endpoint wrapper depth exceeded",
    ))
}
pub(crate) fn curve2d_image_within_box<C: CurveEvaluator + ?Sized>(
    curve: &C,
    range: [f64; 2],
    domain: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    if let Some(native) = curve.native_curve() {
        return super::native_curve2d_image_within_box(native, range, domain, work, cancel);
    }
    for r in domain {
        Interval::new(r[0], r[1])?;
    }
    let image = curve.tensor(range, 0, work, cancel)?[0];
    if (0..2).all(|i| image[i].lo >= domain[i][0] && image[i].hi <= domain[i][1]) {
        return Ok(true);
    }
    if curve.source_parameter_map() != Some([Interval::point(0.0), Interval::point(1.0)]) {
        return Ok(false);
    }
    let Some(mut original) = curve.original_curve() else {
        return Ok(false);
    };
    for _ in 0..16 {
        work.charge(1, cancel)?;
        match original {
            solidcraft_step::geom::Curve::Trimmed { basis, .. } => original = basis,
            solidcraft_step::geom::Curve::SurfaceCurve { curve_3d, .. } => {
                if crate::kernel::source_operand::preferred_source_pcurve(original)?.is_some() {
                    return Ok(false);
                }
                original = curve_3d;
            }
            solidcraft_step::geom::Curve::BSpline(s) => {
                let mut sign = 0i8;
                for (i, p) in s.points.iter().enumerate() {
                    work.charge(1, cancel)?;
                    let w = s.weights.as_ref().map_or(1.0, |weights| weights[i]);
                    let next = if w > 0.0 {
                        1
                    } else if w < 0.0 {
                        -1
                    } else {
                        return Ok(false);
                    };
                    if sign != 0 && sign != next {
                        return Ok(false);
                    }
                    sign = next;
                    if (0..2).any(|axis| p[axis] < domain[axis][0] || p[axis] > domain[axis][1]) {
                        return Ok(false);
                    }
                }
                return Ok(sign != 0);
            }
            _ => return Ok(false),
        }
    }
    Err(GeometryError::new(
        GeometryErrorCode::ResourceLimit,
        "original UV containment wrapper depth exceeded",
    ))
}

/// Prove same-chart coefficient identity, not approximate coincidence.
/// False provides no identity receipt; cross-representation equivalence belongs
/// to source qualification, not to this artificial-cut cancellation shortcut.
pub(crate) fn same_surface_coefficients<
    A: SurfaceEvaluator + ?Sized,
    B: SurfaceEvaluator + ?Sized,
>(
    first: &A,
    second: &B,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(1, cancel)?;
    let (Some(first), Some(second)) = (first.native_surface(), second.native_surface()) else {
        return Ok(
            match (first.original_surface(), second.original_surface()) {
                (Some(a), Some(b)) => std::ptr::eq(a, b),
                _ => false,
            },
        );
    };
    if std::ptr::eq(first, second) {
        return Ok(true);
    }
    fn operand<'a>(
        mut surface: &'a Surface,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(&'a Surface, bool), GeometryError> {
        let mut step = false;
        for _ in 0..32 {
            work.charge(1, cancel)?;
            match surface.kind() {
                solidcore::surface::surface::p2::SurfaceKind::CurveBounded(b) => {
                    surface = b.basis()
                }
                solidcore::surface::surface::p2::SurfaceKind::Step(s) => {
                    surface = s.surface().ok_or_else(|| {
                        nurbs_eval::conditioning("empty native STEP coefficient operand")
                    })?;
                    step = true;
                }
                _ => return Ok((surface, step)),
            }
        }
        Err(GeometryError::new(
            GeometryErrorCode::ResourceLimit,
            "native coefficient operand nesting exceeded",
        ))
    }
    let (a, a_step) = operand(first, work, cancel)?;
    let (b, b_step) = operand(second, work, cancel)?;
    use solidcore::surface::surface::p2::SurfaceKind;
    // STEP analytic charts use degrees; stored NURBS charts do not. Plane and
    // literal BSpline evaluation are unchanged by a STEP wrapper.
    if a_step != b_step
        && !matches!(
            (a.kind(), b.kind()),
            (SurfaceKind::Plane(_), SurfaceKind::Plane(_))
                | (SurfaceKind::BSpline(_), SurfaceKind::BSpline(_))
        )
    {
        return Ok(false);
    }
    if let (SurfaceKind::Plane(x), SurfaceKind::Plane(y)) = (a.kind(), b.kind()) {
        let (x_pos, y_pos) = (x.position(), y.position());
        return Ok(x_pos.origin() == y_pos.origin()
            && x_pos.x_axis() == y_pos.x_axis()
            && x_pos.y_axis() == y_pos.y_axis()
            && x.uv_scale() == y.uv_scale());
    }
    if a_step && b_step {
        let same_frame =
            |x: &solidcore::geom::axis2_placement::Axis2Placement,
             y: &solidcore::geom::axis2_placement::Axis2Placement| {
                x.origin() == y.origin()
                    && x.x_axis() == y.x_axis()
                    && x.y_axis() == y.y_axis()
                    && x.z_axis() == y.z_axis()
            };
        match (a.kind(), b.kind()) {
            (SurfaceKind::Cone(x), SurfaceKind::Cone(y)) => {
                let (x, y) = (x.canonical(), y.canonical());
                return Ok(same_frame(&x.origin, &y.origin)
                    && x.radius == y.radius
                    && x.semi_angle_deg == y.semi_angle_deg);
            }
            (SurfaceKind::Sphere(x), SurfaceKind::Sphere(y)) => {
                return Ok(x.radius() == y.radius()
                    && same_frame(x.base().position(), y.base().position()));
            }
            (SurfaceKind::Torus(x), SurfaceKind::Torus(y)) => {
                return Ok(x.major_radius() == y.major_radius()
                    && x.minor_radius() == y.minor_radius()
                    && same_frame(x.base().position(), y.base().position()));
            }
            (SurfaceKind::BSpline(_), SurfaceKind::BSpline(_)) => {}
            _ => return Ok(false),
        }
    }
    let (Some(x), Some(y)) = (
        analytic_eval::native_surface_nurbs(a)?,
        analytic_eval::native_surface_nurbs(b)?,
    ) else {
        return Ok(false);
    };
    if x.degrees() != y.degrees()
        || x.n() != y.n()
        || x.m() != y.m()
        || x.is_rational() != y.is_rational()
        || x.u_knots().len() != y.u_knots().len()
        || x.v_knots().len() != y.v_knots().len()
    {
        return Ok(false);
    }
    for (a, b) in x
        .u_knots()
        .iter()
        .zip(y.u_knots())
        .chain(x.v_knots().iter().zip(y.v_knots()))
    {
        work.charge(1, cancel)?;
        if a != b {
            return Ok(false);
        }
    }
    for i in 0..=x.n() {
        for j in 0..=x.m() {
            work.charge(1, cancel)?;
            let a = x.cpt(i, j).map_err(|_| {
                nurbs_eval::conditioning("missing first native identity coefficient")
            })?;
            let b = y.cpt(i, j).map_err(|_| {
                nurbs_eval::conditioning("missing second native identity coefficient")
            })?;
            if a.x != b.x || a.y != b.y || a.z != b.z || a.w != b.w {
                return Ok(false);
            }
        }
    }
    Ok(true)
}
