// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Original STEP coefficient restrictions for raw inspection and finish
//! authoring only. They preserve the original parameter/endpoint expressions.
//! Native material queries use PreparedNativeTrimSource::prepare_material and
//! native spatial restrictions instead; none of these operators is material
//! authority or a fallback for native admission.
use crate::exact_coefficient::ExactCoefficient;
use crate::parametric::{CurveEnclosure, CurveThirdEnclosure, Interval};
use crate::{GeometryError, GeometryWorkBudget};
use solidcraft_step::geom::Curve;
use spiling_contracts::geometry::GeometryErrorCode;
use std::sync::atomic::AtomicBool;

mod affine;
pub(crate) mod carrier;
pub(crate) mod census;
pub(crate) mod pcurve;
mod roots;
mod selection;
use crate::kernel::source::OriginalCoedgeSource;
use crate::kernel::source_operand::PreparedOriginalCurve;
use crate::kernel::trim::{SourceCurveCalibration, calibrate_source_curves};
use crate::parametric::{AxisSpans, CurveEvaluator, ExactAffineCurve};
use affine::SourceAffineRestriction;
use roots::EndpointRoot;

/// One selected original spatial operator. Native parameters never enter its
/// normalized query gauge; the trim owner proves their separate association.
#[expect(
    clippy::large_enum_variant,
    reason = "Keep the selected spatial proof inline in its charged retained owner rather than allocate a boxed variant"
)]
pub(crate) enum SourceSpatialRestriction<'a> {
    Line {
        source: &'a OriginalCoedgeSource,
        operator: SourceLineRestriction<'a>,
    },
    Affine {
        source: &'a OriginalCoedgeSource,
        operator: SourceAffineRestriction<'a>,
        original_master: Option<SourceCurveCalibration>,
    },
    Endpoint(EndpointRestrictedOriginal<'a>),
}

#[expect(
    clippy::large_enum_variant,
    reason = "The temporary original reference is admitted as one inline proof before correspondence calibration"
)]
enum OriginalSpatialReference<'a> {
    Line(SourceLineRestriction<'a>),
    Affine(SourceAffineRestriction<'a>, Option<SourceCurveCalibration>),
    Endpoint(EndpointRestrictedOriginal<'a>),
}
impl<'a> OriginalSpatialReference<'a> {
    fn prepare(
        source: &'a OriginalCoedgeSource,
        allowance: f64,
        remaining: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        let Some((curve, sense)) = selection::original_3d_reference(source, work, cancel)? else {
            return Ok(None);
        };
        Self::prepare_curve(source, curve, sense, allowance, remaining, 0, work, cancel).map(Some)
    }
    fn prepare_curve(
        source: &'a OriginalCoedgeSource,
        curve: &'a Curve,
        sense: bool,
        allowance: f64,
        remaining: usize,
        depth: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        work.charge(1, cancel)?;
        if depth >= 16
            || remaining < std::mem::size_of::<Self>() + SOURCE_LINE_RESTRICTION_SCRATCH_BYTES
        {
            return Err(source_capacity());
        }
        if let Some(line) = SourceLineRestriction::new(
            curve,
            &source.vertex_points_mm,
            sense,
            allowance,
            work,
            cancel,
        )? {
            return Ok(Self::Line(line));
        }
        // Every preferred layer keeps its own actual 3D declaration. Parameter
        // selectors are never transplanted across an independently gauged master.
        let reference = if let Some((basis, basis_sense)) =
            selection::original_3d_reference_curve(curve, sense, work, cancel)?
        {
            let budget = remaining
                .checked_sub(std::mem::size_of::<Self>())
                .ok_or_else(source_capacity)?;
            Some(Self::prepare_curve(
                source,
                basis,
                basis_sense,
                allowance,
                budget,
                depth + 1,
                work,
                cancel,
            )?)
        } else {
            None
        };
        let reference_bytes = reference.as_ref().map_or(0, Self::retained_bytes);
        let available = remaining
            .checked_sub(reference_bytes)
            .ok_or_else(source_capacity)?;
        let affine_budget = available
            .checked_sub(std::mem::size_of::<Self>())
            .and_then(|n| n.checked_add(std::mem::size_of::<SourceAffineRestriction<'a>>()))
            .ok_or_else(source_capacity)?;
        let mut selected = if let Some(operator) = SourceAffineRestriction::try_new_curve(
            source,
            curve,
            sense,
            allowance,
            affine_budget,
            work,
            cancel,
        )? {
            Self::Affine(operator, None)
        } else {
            let selection = selection::select_spatial_curve(
                source, curve, sense, allowance, available, work, cancel,
            )?;
            let budget = available
                .checked_sub(std::mem::size_of::<Self>())
                .and_then(|n| n.checked_add(std::mem::size_of::<EndpointRestrictedOriginal<'a>>()))
                .ok_or_else(source_capacity)?;
            Self::Endpoint(EndpointRestrictedOriginal::from_selection(
                source,
                curve,
                sense,
                selection,
                reference.as_ref().map(Self::evaluator),
                allowance,
                budget,
                work,
                cancel,
            )?)
        };
        if let Some(reference) = &reference {
            let live = reference
                .retained_bytes()
                .checked_add(selected.retained_bytes())
                .ok_or_else(source_capacity)?;
            let budget = remaining.checked_sub(live).ok_or_else(source_capacity)?;
            let proof = calibrate_source_curves(
                reference.evaluator(),
                selected.evaluator(),
                [0.0, 1.0],
                allowance,
                32,
                budget,
                work,
                cancel,
            )?;
            match &mut selected {
                Self::Affine(_, association) => *association = Some(proof),
                Self::Endpoint(operator) => operator.original_master = Some(proof),
                Self::Line(_) => {
                    return Err(source_unavailable(
                        "literal reference LINE cannot replace a preferred spatial expression",
                    ));
                }
            }
        }
        if selected.retained_bytes() > remaining {
            return Err(source_capacity());
        }
        Ok(selected)
    }
    fn evaluator(&self) -> &dyn CurveEvaluator {
        match self {
            Self::Line(operator) => operator,
            Self::Affine(operator, _) => operator,
            Self::Endpoint(operator) => operator,
        }
    }
    fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + match self {
                Self::Line(_) => 0,
                Self::Affine(operator, association) => operator
                    .retained_bytes()
                    .saturating_sub(std::mem::size_of::<SourceAffineRestriction<'a>>())
                    .saturating_add(association.as_ref().map_or(0, |proof| {
                        proof
                            .retained_bytes()
                            .saturating_sub(std::mem::size_of::<SourceCurveCalibration>())
                    })),
                Self::Endpoint(operator) => operator.heap_bytes(),
            }
    }
}

#[derive(Clone, Copy)]
enum SourceEndpoint {
    Literal {
        value: f64,
        turns: i32,
    },
    Root {
        root: EndpointRoot,
        branch: usize,
        turns: i32,
    },
    ClosedFence(roots::ClosedFenceEndpoint),
    FirstShift {
        turns: i32,
    },
}
pub(crate) struct EndpointRestrictedOriginal<'a> {
    source: &'a OriginalCoedgeSource,
    operand: PreparedOriginalCurve<'a>,
    endpoints: [SourceEndpoint; 2],
    period: Option<Interval>,
    source_error_mm: f64,
    exact_endpoints: bool,
    original_knots: Vec<f64>,
    knot_bands: Vec<Interval>,
    cover_cuts: Vec<f64>,
    original_master: Option<SourceCurveCalibration>,
}
fn source_capacity() -> GeometryError {
    failure(
        GeometryErrorCode::ResourceLimit,
        "selected original spatial restriction exceeds its live byte allowance",
    )
}
fn source_unavailable(message: &'static str) -> GeometryError {
    failure(
        GeometryErrorCode::ParameterCorrespondenceUnavailable,
        message,
    )
}
fn admit_vec<T>(
    values: &mut Vec<T>,
    additional: usize,
    live: usize,
    limit: usize,
) -> Result<(), GeometryError> {
    let required = values
        .len()
        .checked_add(additional)
        .ok_or_else(source_capacity)?;
    if required <= values.capacity() {
        return Ok(());
    }
    let extra = required
        .checked_sub(values.capacity())
        .and_then(|n| n.checked_mul(std::mem::size_of::<T>()))
        .ok_or_else(source_capacity)?;
    if live.checked_add(extra).is_none_or(|n| n > limit) {
        return Err(source_capacity());
    }
    let old = values.capacity();
    values
        .try_reserve_exact(required - values.len())
        .map_err(|_| source_capacity())?;
    let actual = (values.capacity() - old)
        .checked_mul(std::mem::size_of::<T>())
        .ok_or_else(source_capacity)?;
    if live.checked_add(actual).is_none_or(|n| n > limit) {
        return Err(source_capacity());
    }
    Ok(())
}
impl<'a> SourceSpatialRestriction<'a> {
    pub(crate) fn new(
        source: &'a OriginalCoedgeSource,
        allowance: f64,
        remaining: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        work.charge(1, cancel)?;
        if remaining < std::mem::size_of::<Self>() + SOURCE_LINE_RESTRICTION_SCRATCH_BYTES {
            return Err(source_capacity());
        }
        if let Some(operator) = SourceLineRestriction::new(
            source.spatial_curve.as_ref(),
            &source.vertex_points_mm,
            source.same_sense,
            allowance,
            work,
            cancel,
        )? {
            return Ok(Self::Line { source, operator });
        }
        let reference_budget = remaining
            .checked_sub(std::mem::size_of::<Self>())
            .ok_or_else(source_capacity)?;
        let reference =
            OriginalSpatialReference::prepare(source, allowance, reference_budget, work, cancel)?;
        let reference_bytes = reference
            .as_ref()
            .map_or(0, OriginalSpatialReference::retained_bytes);
        let selected_budget = remaining
            .checked_sub(reference_bytes)
            .ok_or_else(source_capacity)?;
        let affine_budget = selected_budget
            .checked_sub(std::mem::size_of::<Self>())
            .and_then(|n| n.checked_add(std::mem::size_of::<SourceAffineRestriction<'a>>()))
            .ok_or_else(source_capacity)?;
        let mut selected = if let Some(operator) =
            SourceAffineRestriction::try_new(source, allowance, affine_budget, work, cancel)?
        {
            Self::Affine {
                source,
                operator,
                original_master: None,
            }
        } else {
            let endpoint_budget = selected_budget
                .checked_sub(std::mem::size_of::<Self>())
                .and_then(|n| n.checked_add(std::mem::size_of::<EndpointRestrictedOriginal<'a>>()))
                .ok_or_else(source_capacity)?;
            Self::Endpoint(EndpointRestrictedOriginal::new(
                source,
                reference.as_ref().map(OriginalSpatialReference::evaluator),
                allowance,
                endpoint_budget,
                work,
                cancel,
            )?)
        };
        if let Some(reference) = &reference {
            let live = selected
                .retained_bytes()
                .checked_add(reference.retained_bytes())
                .ok_or_else(source_capacity)?;
            let budget = remaining.checked_sub(live).ok_or_else(source_capacity)?;
            let association = calibrate_source_curves(
                reference.evaluator(),
                &selected,
                [0.0, 1.0],
                allowance,
                32,
                budget,
                work,
                cancel,
            )?;
            match &mut selected {
                Self::Affine {
                    original_master, ..
                } => *original_master = Some(association),
                Self::Endpoint(operator) => operator.original_master = Some(association),
                Self::Line { .. } => {
                    return Err(source_unavailable(
                        "literal LINE cannot carry an unrelated selected spatial master",
                    ));
                }
            }
        }
        if selected.retained_bytes() > remaining {
            return Err(source_capacity());
        }
        Ok(selected)
    }
    fn evaluator(&self) -> &dyn CurveEvaluator {
        match self {
            Self::Line { operator, .. } => operator,
            Self::Affine { operator, .. } => operator,
            Self::Endpoint(operator) => operator,
        }
    }
    fn source(&self) -> &'a OriginalCoedgeSource {
        match self {
            Self::Line { source, .. } | Self::Affine { source, .. } => source,
            Self::Endpoint(operator) => operator.source,
        }
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + match self {
                Self::Line { .. } => 0,
                Self::Affine {
                    operator,
                    original_master,
                    ..
                } => operator
                    .retained_bytes()
                    .saturating_sub(std::mem::size_of::<SourceAffineRestriction<'a>>())
                    .saturating_add(original_master.as_ref().map_or(0, |proof| {
                        proof
                            .retained_bytes()
                            .saturating_sub(std::mem::size_of::<SourceCurveCalibration>())
                    })),
                Self::Endpoint(operator) => operator.heap_bytes(),
            }
    }
    #[cfg(test)]
    pub(crate) fn original_master_association(&self) -> Option<&SourceCurveCalibration> {
        match self {
            Self::Line { .. } => None,
            Self::Affine {
                original_master, ..
            } => original_master.as_ref(),
            Self::Endpoint(operator) => operator.original_master.as_ref(),
        }
    }
    pub(crate) fn source_error_mm(&self) -> f64 {
        match self {
            Self::Line { operator, .. } => operator.source_error_mm(),
            Self::Affine { operator, .. } => operator.source_error_mm(),
            Self::Endpoint(operator) => operator.source_error_mm,
        }
    }
    pub(crate) fn exact_source_endpoints(&self) -> bool {
        match self {
            Self::Line { operator, .. } => operator.exact_source_endpoints(),
            Self::Affine { operator, .. } => operator.exact_source_endpoints(),
            Self::Endpoint(operator) => operator.exact_endpoints,
        }
    }
    pub(crate) fn endpoint_parameters(&self) -> Result<[Interval; 2], GeometryError> {
        match self {
            Self::Line { operator, .. } => Ok(operator.endpoint_parameters()),
            Self::Affine { operator, .. } => Ok(operator.endpoint_parameters()),
            Self::Endpoint(operator) => operator.parameters(),
        }
    }
    pub(crate) fn parameter_derivative(
        &self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Interval, GeometryError> {
        work.charge(1, cancel)?;
        match self {
            Self::Line { operator, .. } => operator.parameter_derivative(work, cancel),
            Self::Affine { operator, .. } => operator.parameter_derivative(work, cancel),
            Self::Endpoint(operator) => operator.derivative(),
        }
    }
    pub(crate) fn original_knots(&self) -> &[f64] {
        match self {
            Self::Line { .. } | Self::Affine { .. } => &[],
            Self::Endpoint(operator) => &operator.original_knots,
        }
    }
    pub(crate) fn knot_bands(&self) -> &[Interval] {
        match self {
            Self::Line { .. } | Self::Affine { .. } => &[],
            Self::Endpoint(operator) => &operator.knot_bands,
        }
    }
    #[cfg(test)]
    pub(crate) fn original_knot_is_active(
        &self,
        index: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        work.charge(1, cancel)?;
        let &parameter = self.original_knots().get(index).ok_or_else(|| {
            source_unavailable("original spatial knot index is outside its sealed census")
        })?;
        let [a, b] = self.endpoint_parameters()?;
        Ok((a.hi <= parameter && parameter <= b.lo) || (b.hi <= parameter && parameter <= a.lo))
    }
    #[cfg(test)]
    pub(crate) fn original_knot_jet(
        &self,
        index: usize,
        order: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[[Interval; 3]; 4], GeometryError> {
        if !self.original_knot_is_active(index, work, cancel)? {
            return Err(source_unavailable(
                "possible original knot band does not prove an active source incidence",
            ));
        }
        match self {
            Self::Endpoint(operator) => {
                let &parameter = operator.original_knots.get(index).ok_or_else(|| {
                    source_unavailable("original spatial knot index is outside its sealed census")
                })?;
                operator
                    .operand
                    .evaluate_order([parameter; 2], order, work, cancel)
            }
            Self::Line { .. } | Self::Affine { .. } => {
                Err(source_unavailable("affine source has no original knot"))
            }
        }
    }
    #[cfg(test)]
    pub(crate) fn same_operator(
        &self,
        other: &Self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        work.charge(1, cancel)?;
        let (a, b) = (self.source(), other.source());
        if !std::ptr::eq(a.spatial_curve.as_ref(), b.spatial_curve.as_ref())
            || a.edge_curve_id != b.edge_curve_id
            || a.curve_id != b.curve_id
            || a.vertex_ids != b.vertex_ids
            || a.vertex_points_mm != b.vertex_points_mm
            || a.same_sense != b.same_sense
        {
            return Ok(false);
        }
        Ok(match (self, other) {
            (Self::Line { .. }, Self::Line { .. }) => true,
            (Self::Affine { .. }, Self::Affine { .. }) => true,
            (Self::Endpoint(a), Self::Endpoint(b)) => {
                a.endpoints
                    .iter()
                    .zip(&b.endpoints)
                    .all(|(a, b)| match (a, b) {
                        (
                            SourceEndpoint::Literal {
                                value: a,
                                turns: at,
                            },
                            SourceEndpoint::Literal {
                                value: b,
                                turns: bt,
                            },
                        ) => a == b && at == bt,
                        (
                            SourceEndpoint::Root {
                                branch: a,
                                turns: at,
                                ..
                            },
                            SourceEndpoint::Root {
                                branch: b,
                                turns: bt,
                                ..
                            },
                        ) => a == b && at == bt,
                        (SourceEndpoint::ClosedFence(a), SourceEndpoint::ClosedFence(b)) => {
                            a.parameter() == b.parameter()
                        }
                        (
                            SourceEndpoint::FirstShift { turns: a },
                            SourceEndpoint::FirstShift { turns: b },
                        ) => a == b,
                        _ => false,
                    })
            }
            _ => false,
        })
    }
}
impl EndpointRestrictedOriginal<'_> {
    fn endpoint(&self, index: usize) -> Result<Interval, GeometryError> {
        let (base, turns) = match self.endpoints[index] {
            SourceEndpoint::Literal { value, turns } => (Interval::point(value), turns),
            SourceEndpoint::Root { root, turns, .. } => (root.enclosure(), turns),
            SourceEndpoint::ClosedFence(fence) => (Interval::point(fence.parameter()), 0),
            SourceEndpoint::FirstShift { turns } => (self.endpoint(0)?, turns),
        };
        if turns == 0 {
            return Ok(base);
        }
        let period = self.period.ok_or_else(|| {
            source_unavailable(
                "original endpoint period shift has no coefficient periodicity proof",
            )
        })?;
        finite(base.add(period.mul(Interval::point(f64::from(turns)))))
    }
    fn parameters(&self) -> Result<[Interval; 2], GeometryError> {
        Ok([self.endpoint(0)?, self.endpoint(1)?])
    }
    fn derivative(&self) -> Result<Interval, GeometryError> {
        if let SourceEndpoint::FirstShift { turns } = self.endpoints[1] {
            return finite(
                self.period
                    .ok_or_else(|| {
                        source_unavailable("closed original curve has no coefficient period")
                    })?
                    .mul(Interval::point(f64::from(turns))),
            );
        }
        finite(self.endpoint(1)?.sub(self.endpoint(0)?))
    }
    fn image(&self, range: [f64; 2]) -> Result<Interval, GeometryError> {
        Interval::new(range[0], range[1])?;
        if range[0] < 0.0 || range[1] > 1.0 {
            return Err(source_unavailable(
                "original spatial query exceeds its selected normalized edge",
            ));
        }
        let p = self.parameters()?;
        let at = |value: f64| -> Result<Interval, GeometryError> {
            let s = Interval::point(value);
            if matches!(self.endpoints[1], SourceEndpoint::FirstShift { .. }) {
                Ok(p[0].add(self.derivative()?.mul(s)))
            } else {
                Ok(p[0].mul(Interval::point(1.0).sub(s)).add(p[1].mul(s)))
            }
        };
        // A fixed affine map is monotone. Evaluating its two query fences
        // avoids treating s and 1-s as independent variables on the whole span.
        let image = finite(at(range[0])?.hull(at(range[1])?))?;
        let active = self.operand.active_range();
        // Constructor-proved endpoint containment and convexity establish this
        // constraint on the actual map. This removes arithmetic over-enclosure,
        // not any source endpoint/root uncertainty.
        image
            .intersection(Interval::new(active[0], active[1])?)
            .ok_or_else(|| {
                source_unavailable("original parameter image misses its proved source domain")
            })
    }
    fn heap_bytes(&self) -> usize {
        self.operand
            .retained_bytes()
            .saturating_sub(std::mem::size_of::<PreparedOriginalCurve<'_>>())
            .saturating_add(self.original_knots.capacity() * std::mem::size_of::<f64>())
            .saturating_add(self.knot_bands.capacity() * std::mem::size_of::<Interval>())
            .saturating_add(self.cover_cuts.capacity() * std::mem::size_of::<f64>())
            .saturating_add(self.original_master.as_ref().map_or(0, |proof| {
                proof
                    .retained_bytes()
                    .saturating_sub(std::mem::size_of::<SourceCurveCalibration>())
            }))
    }
}
fn endpoint_identity(
    curve: &dyn CurveEvaluator,
    parameter: f64,
    point: [f64; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    for axis in 0..3 {
        if !crate::parametric::curve_endpoint_coordinate_equal(
            curve,
            parameter,
            axis,
            point[axis],
            work,
            cancel,
        )? {
            return Ok(false);
        }
    }
    Ok(true)
}
fn endpoint_error(
    curve: &dyn CurveEvaluator,
    range: Interval,
    point: [f64; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<f64, GeometryError> {
    if range.is_point() && endpoint_identity(curve, range.lo, point, work, cancel)? {
        return Ok(0.0);
    }
    let p = curve.tensor([range.lo, range.hi], 0, work, cancel)?[0];
    finite(
        (0..3)
            .fold(Interval::point(0.0), |sum, axis| {
                sum.add(p[axis].sub(Interval::point(point[axis])).square())
            })
            .sqrt()?,
    )
    .map(|v| v.hi)
}
fn endpoint_root(
    curve: &dyn CurveEvaluator,
    vertex: [f64; 3],
    search: [f64; 2],
    period: Option<Interval>,
    closed_domain: Option<selection::ClosedEndpointDomain>,
    allowance: f64,
    remaining: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SourceEndpoint, GeometryError> {
    // Active equality alone is not provenance: carrier-derived PCURVE LINE
    // proposals also install active==search. Require selector-owned bounds.
    // This complete-domain theorem proves strict monotonic distance and thus
    // excludes EVERY stationary root. It is disjoint from successful existing
    // stationary recipes, so it may precede their isolator without changing
    // those choices. Nonproof enters the unchanged isolator; errors propagate.
    if period.is_none()
        && let Some(domain) = closed_domain.filter(|domain| domain.range() == search)
        && let Some(fence) = roots::closed_fence_endpoint(
            curve,
            vertex,
            domain.range(),
            allowance,
            remaining,
            work,
            cancel,
        )?
    {
        return Ok(SourceEndpoint::ClosedFence(fence));
    }
    let mut roots =
        roots::isolate_endpoint(curve, vertex, search, allowance, remaining, work, cancel)?;
    work.charge(
        (roots.len() as u64).saturating_mul(usize::BITS as u64),
        cancel,
    )?;
    roots.sort_unstable_by(|a, b| a.enclosure().lo.total_cmp(&b.enclosure().lo));
    let Some(first) = roots.first().copied() else {
        return Err(GeometryError::new(
            GeometryErrorCode::ParameterCorrespondenceUnavailable,
            format!(
                "original endpoint has no proved stationary root or closed-domain endpoint: target={vertex:?}, active={:?}, search={search:?}, closed_domain={closed_domain:?}, allowance_mm={allowance:?}, stationary_roots=0",
                curve.active_range(),
            ),
        ));
    };
    for root in roots.iter().skip(1) {
        work.charge(1, cancel)?;
        let Some(period) = period else {
            return Err(source_unavailable(
                "original endpoint has more than one source branch",
            ));
        };
        let shifted = first.enclosure().add(period);
        let tube = shifted.hull(root.enclosure());
        let j = curve.tensor([tube.lo, tube.hi], 2, work, cancel)?;
        let delta = std::array::from_fn(|axis| j[0][axis].sub(Interval::point(vertex[axis])));
        if shifted.intersection(root.enclosure()).is_none()
            || dot(j[1], j[1]).add(dot(delta, j[2])).lo <= 0.0
        {
            return Err(source_unavailable(
                "periodic source endpoint branches lack a unique shared root",
            ));
        }
    }
    Ok(SourceEndpoint::Root {
        root: first,
        branch: 0,
        turns: 0,
    })
}
fn endpoint_context(
    error: GeometryError,
    source: &OriginalCoedgeSource,
    endpoint: usize,
) -> GeometryError {
    GeometryError::new(
        error.code,
        format!(
            "original source face {:?}, coedge {:?}, edge {:?}, curve {:?}, endpoint {endpoint}, vertex {:?}: {}",
            source.source_face,
            source.oriented_edge_id,
            source.edge_curve_id,
            source.curve_id,
            source.vertex_ids[endpoint],
            error.message,
        ),
    )
}
impl<'a> EndpointRestrictedOriginal<'a> {
    fn new(
        source: &'a OriginalCoedgeSource,
        reference: Option<&dyn CurveEvaluator>,
        allowance: f64,
        remaining: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        Self::from_selection(
            source,
            source.spatial_curve.as_ref(),
            source.same_sense,
            selection::select(source, allowance, remaining, work, cancel)?,
            reference,
            allowance,
            remaining,
            work,
            cancel,
        )
    }
    fn from_selection(
        source: &'a OriginalCoedgeSource,
        curve: &'a Curve,
        same_sense: bool,
        mut selection: selection::SourceSelection,
        reference: Option<&dyn CurveEvaluator>,
        allowance: f64,
        remaining: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        if !allowance.is_finite() || allowance < 0.0 {
            return Err(source_unavailable(
                "original spatial restriction needs finite nonnegative source admission",
            ));
        }
        let fixed = std::mem::size_of::<Self>();
        let available = remaining
            .checked_sub(fixed)
            .and_then(|n| n.checked_add(std::mem::size_of::<PreparedOriginalCurve<'a>>()))
            .ok_or_else(source_capacity)?;
        let operand = PreparedOriginalCurve::new(curve, selection.active, available, work, cancel)?;
        let root_bytes = remaining
            .checked_sub(fixed)
            .and_then(|n| {
                n.checked_sub(
                    operand
                        .retained_bytes()
                        .saturating_sub(std::mem::size_of::<PreparedOriginalCurve<'a>>()),
                )
            })
            .ok_or_else(source_capacity)?;
        if let Some(reference) = reference {
            let mut phase = None;
            let mut candidates = selection.active.map(Some);
            if let Some(parameters) = selection.declared_endpoints {
                candidates = [None; 2];
                // These are candidate evaluation points only. A lifted endpoint
                // enclosure is handled by the complete root search below.
                for (index, (parameter, turns)) in parameters
                    .into_iter()
                    .zip(selection.declared_turns)
                    .enumerate()
                {
                    if turns == 0 {
                        candidates[index] = Some(parameter);
                    }
                }
            }
            for parameter in candidates.into_iter().flatten() {
                work.charge(1, cancel)?;
                if endpoint_error(
                    &operand,
                    Interval::point(parameter),
                    source.vertex_points_mm[0],
                    work,
                    cancel,
                )? <= allowance
                {
                    phase = Some(Interval::point(parameter));
                    break;
                }
            }
            let phase = match phase {
                Some(phase) => phase,
                None => match endpoint_root(
                    &operand,
                    source.vertex_points_mm[0],
                    selection.search,
                    selection.periodic,
                    selection.closed_domain,
                    allowance,
                    root_bytes,
                    work,
                    cancel,
                )
                .map_err(|error| endpoint_context(error, source, 0))?
                {
                    SourceEndpoint::Root { root, .. } => root.enclosure(),
                    SourceEndpoint::ClosedFence(fence) => Interval::point(fence.parameter()),
                    _ => {
                        return Err(source_unavailable(
                            "original master direction has no bounded source endpoint",
                        ));
                    }
                },
            };
            let reference_jet = reference.tensor([0.0; 2], 1, work, cancel)?;
            let selected_jet = operand.evaluate_order([phase.lo, phase.hi], 1, work, cancel)?;
            let alignment = dot(reference_jet[1], selected_jet[1]);
            selection.increasing = if alignment.lo > 0.0 {
                true
            } else if alignment.hi < 0.0 {
                false
            } else {
                return Err(source_unavailable(
                    "preferred master has no proved directed source germ",
                ));
            };
        }
        let endpoints = if let Some(parameters) = selection.declared_endpoints {
            let mut endpoints = [
                SourceEndpoint::Literal {
                    value: parameters[0],
                    turns: selection.declared_turns[0],
                },
                SourceEndpoint::Literal {
                    value: parameters[1],
                    turns: selection.declared_turns[1],
                },
            ];
            if reference.is_some() {
                let endpoint = |value: f64, turns: i32| -> Result<Interval, GeometryError> {
                    if turns == 0 {
                        return Ok(Interval::point(value));
                    }
                    Ok(Interval::point(value).add(
                        selection
                            .periodic
                            .ok_or_else(|| {
                                source_unavailable("declared original turn has no actual period")
                            })?
                            .mul(Interval::point(f64::from(turns))),
                    ))
                };
                let a = endpoint(parameters[0], selection.declared_turns[0])?;
                let b = endpoint(parameters[1], selection.declared_turns[1])?;
                if (selection.increasing && a.lo > b.hi) || (!selection.increasing && a.hi < b.lo) {
                    endpoints.swap(0, 1);
                }
            } else if !same_sense {
                endpoints.swap(0, 1);
            }
            endpoints
        } else {
            let mut targets = selection
                .endpoint_targets
                .unwrap_or(source.vertex_points_mm);
            if selection.endpoint_targets.is_some() {
                if reference.is_some() && targets[0] != targets[1] {
                    let distance = |target: [f64; 3]| -> Result<Interval, GeometryError> {
                        finite((0..3).fold(Interval::point(0.0), |sum, axis| {
                            sum.add(
                                Interval::point(target[axis])
                                    .sub(Interval::point(source.vertex_points_mm[0][axis]))
                                    .square(),
                            )
                        }))
                    };
                    let a = distance(targets[0])?;
                    let b = distance(targets[1])?;
                    if b.hi < a.lo {
                        targets.swap(0, 1);
                    } else if a.hi >= b.lo {
                        return Err(source_unavailable(
                            "Cartesian master endpoints have ambiguous original vertex ownership",
                        ));
                    }
                } else if !same_sense {
                    targets.swap(0, 1);
                }
            }
            let natural = operand.active_range();
            let candidates = if selection.increasing {
                natural
            } else {
                [natural[1], natural[0]]
            };
            let mut endpoints = [SourceEndpoint::Literal {
                value: 0.0,
                turns: 0,
            }; 2];
            for endpoint in 0..2 {
                work.charge(1, cancel)?;
                endpoints[endpoint] = if selection.periodic.is_none()
                    && endpoint_identity(
                        &operand,
                        candidates[endpoint],
                        targets[endpoint],
                        work,
                        cancel,
                    )? {
                    SourceEndpoint::Literal {
                        value: candidates[endpoint],
                        turns: 0,
                    }
                } else if endpoint == 1
                    && source.vertex_ids[0] == source.vertex_ids[1]
                    && targets[0] == targets[1]
                    && selection.periodic.is_some()
                {
                    SourceEndpoint::FirstShift {
                        turns: if selection.increasing { 1 } else { -1 },
                    }
                } else {
                    endpoint_root(
                        &operand,
                        targets[endpoint],
                        selection.search,
                        selection.periodic,
                        selection.closed_domain,
                        allowance,
                        root_bytes,
                        work,
                        cancel,
                    )
                    .map_err(|error| endpoint_context(error, source, endpoint))?
                };
            }
            endpoints
        };
        let mut operator = Self {
            source,
            operand,
            endpoints,
            period: selection.periodic,
            source_error_mm: 0.0,
            exact_endpoints: false,
            original_knots: Vec::new(),
            knot_bands: Vec::new(),
            cover_cuts: Vec::new(),
            original_master: None,
        };
        let mut derivative = operator.derivative()?;
        if let Some(period) = operator.period {
            if !matches!(operator.endpoints[1], SourceEndpoint::FirstShift { .. })
                && ((selection.increasing && derivative.hi <= 0.0)
                    || (!selection.increasing && derivative.lo >= 0.0))
            {
                let turns = if selection.increasing { 1 } else { -1 };
                match &mut operator.endpoints[1] {
                    SourceEndpoint::Root { turns: shift, .. } => *shift += turns,
                    _ => {
                        return Err(source_unavailable(
                            "declared original endpoints contradict selected source sense",
                        ));
                    }
                }
                derivative = operator.derivative()?;
            }
            if derivative.lo.abs().max(derivative.hi.abs()) > period.hi
                && selection.declared_endpoints.is_none()
            {
                return Err(source_unavailable(
                    "original endpoint restriction exceeds its unique source period",
                ));
            }
        }
        if (selection.increasing && derivative.lo <= 0.0)
            || (!selection.increasing && derivative.hi >= 0.0)
        {
            return Err(source_unavailable(
                "original endpoint roots do not prove the selected nonzero direction",
            ));
        }
        let p = operator.parameters()?;
        let active = operator.operand.active_range();
        if p.iter()
            .any(|endpoint| endpoint.lo < active[0] || endpoint.hi > active[1])
        {
            return Err(source_unavailable(
                "selected original endpoint recipe exceeds its actual coefficient domain",
            ));
        }
        operator.exact_endpoints = true;
        for endpoint in 0..2 {
            let error = endpoint_error(
                &operator.operand,
                p[endpoint],
                source.vertex_points_mm[endpoint],
                work,
                cancel,
            )?;
            operator.source_error_mm = operator.source_error_mm.max(error);
            if let SourceEndpoint::ClosedFence(fence) = operator.endpoints[endpoint] {
                operator.source_error_mm = operator.source_error_mm.max(fence.source_error_mm());
            }
            operator.exact_endpoints &= error == 0.0
                && !matches!(operator.endpoints[endpoint], SourceEndpoint::ClosedFence(_));
        }
        if operator.source_error_mm > allowance {
            return Err(source_unavailable(
                "selected original endpoint trace exceeds its proved source admission",
            ));
        }
        let (knots, _) = operator.operand.parameter_axis(work, cancel)?;
        let knot_count = knots.len();
        // Metadata is admitted with the source provider still live. Each raw
        // knot retains its own inverse band; cover fences are not incidences.
        for index in 0..knot_count {
            work.charge(1, cancel)?;
            let knot = operator.operand.parameter_axis(work, cancel)?.0[index];
            if operator.original_knots.last() == Some(&knot) {
                continue;
            }
            let raw = Interval::point(knot);
            let band = raw.sub(p[0]).div(derivative)?;
            let Some(band) = band.intersection(Interval::new(0.0, 1.0)?) else {
                continue;
            };
            let live = fixed + operator.heap_bytes();
            admit_vec(&mut operator.original_knots, 1, live, remaining)?;
            let live = fixed + operator.heap_bytes();
            admit_vec(&mut operator.knot_bands, 1, live, remaining)?;
            operator.original_knots.push(knot);
            operator.knot_bands.push(band);
        }
        let fence_count = operator
            .knot_bands
            .len()
            .checked_mul(2)
            .and_then(|n| n.checked_add(2))
            .ok_or_else(source_capacity)?;
        let live = fixed + operator.heap_bytes();
        admit_vec(&mut operator.cover_cuts, fence_count, live, remaining)?;
        operator.cover_cuts.extend([0.0, 1.0]);
        for band in &operator.knot_bands {
            operator.cover_cuts.extend([band.lo, band.hi]);
        }
        work.charge(
            (operator.cover_cuts.len() as u64).saturating_mul(usize::BITS as u64),
            cancel,
        )?;
        operator.cover_cuts.sort_unstable_by(f64::total_cmp);
        operator.cover_cuts.dedup();
        Ok(operator)
    }
}
impl CurveEvaluator for EndpointRestrictedOriginal<'_> {
    fn active_range(&self) -> [f64; 2] {
        [0.0, 1.0]
    }
    fn exact_affine_coefficients(
        &self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<ExactAffineCurve>, GeometryError> {
        let parameter = |endpoint: SourceEndpoint| match endpoint {
            SourceEndpoint::Literal { value, turns: 0 } => Some(value),
            SourceEndpoint::Root { root, turns: 0, .. } => root.exact_parameter(),
            SourceEndpoint::ClosedFence(fence) => Some(fence.parameter()),
            _ => None,
        };
        let (Some(a), Some(b)) = (parameter(self.endpoints[0]), parameter(self.endpoints[1]))
        else {
            return Ok(None);
        };
        let Some(mut affine) = self.operand.exact_affine_coefficients(work, cancel)? else {
            return Ok(None);
        };
        let Some(slope) = ExactCoefficient::difference(b, a) else {
            return Ok(None);
        };
        for axis in 0..3 {
            let Some(offset) =
                affine.derivative[axis].product(&ExactCoefficient::point(a), work, cancel)?
            else {
                return Ok(None);
            };
            for &term in &offset.terms[..offset.len] {
                if affine.position[axis].push(term).is_none() {
                    return Ok(None);
                }
            }
            let Some(derivative) = affine.derivative[axis].product(&slope, work, cancel)? else {
                return Ok(None);
            };
            affine.derivative[axis] = derivative;
        }
        Ok(Some(affine))
    }
    fn source_parameter_map(&self) -> Option<[Interval; 2]> {
        Some([self.endpoint(0).ok()?, self.derivative().ok()?])
    }
    fn source_parameter_image(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Interval>, GeometryError> {
        work.charge(1, cancel)?;
        self.image(range).map(Some)
    }
    fn tensor(
        &self,
        range: [f64; 2],
        order: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[[Interval; 3]; 4], GeometryError> {
        if order > 3 {
            return Err(source_unavailable("original curve jet order exceeds three"));
        }
        let parameter = self.image(range)?;
        let mut tensor =
            self.operand
                .evaluate_order([parameter.lo, parameter.hi], order, work, cancel)?;
        if order != 0 {
            let derivative = self.derivative()?;
            let mut factor = Interval::point(1.0);
            for row in tensor.iter_mut().take(order + 1).skip(1) {
                factor = factor.mul(derivative);
                for coordinate in row {
                    *coordinate = coordinate.mul(factor);
                }
            }
        }
        Ok(tensor)
    }
    fn continuous(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let p = self.image(range)?;
        self.operand.span_is_continuous([p.lo, p.hi], work, cancel)
    }
    fn smooth(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let p = self.image(range)?;
        self.operand.span_is_smooth([p.lo, p.hi], work, cancel)
    }
    fn parameter_spans(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<AxisSpans<'_>, GeometryError> {
        self.image(range)?;
        AxisSpans::from_cover_cuts(&self.cover_cuts, range, work, cancel)
    }
    fn original_curve(&self) -> Option<&Curve> {
        Some(self.operand.source())
    }
}
impl CurveEvaluator for SourceSpatialRestriction<'_> {
    fn active_range(&self) -> [f64; 2] {
        [0.0, 1.0]
    }
    fn exact_affine_coefficients(
        &self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<ExactAffineCurve>, GeometryError> {
        self.evaluator().exact_affine_coefficients(work, cancel)
    }
    fn source_parameter_map(&self) -> Option<[Interval; 2]> {
        self.evaluator().source_parameter_map()
    }
    fn source_parameter_image(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Interval>, GeometryError> {
        self.evaluator().source_parameter_image(range, work, cancel)
    }
    fn tensor(
        &self,
        range: [f64; 2],
        order: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[[Interval; 3]; 4], GeometryError> {
        self.evaluator().tensor(range, order, work, cancel)
    }
    fn continuous(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.evaluator().continuous(range, work, cancel)
    }
    fn smooth(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.evaluator().smooth(range, work, cancel)
    }
    fn parameter_spans(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<AxisSpans<'_>, GeometryError> {
        self.evaluator().parameter_spans(range, work, cancel)
    }
    fn original_curve(&self) -> Option<&Curve> {
        Some(self.source().spatial_curve.as_ref())
    }
}

/// Fixed scratch reserved by source trim admission; the restriction owns no heap.
pub(crate) const SOURCE_LINE_RESTRICTION_SCRATCH_BYTES: usize =
    8 * std::mem::size_of::<ExactCoefficient>() + 24 * std::mem::size_of::<Interval>();

#[derive(Clone, Copy)]
pub(crate) struct SourceLineRestriction<'a> {
    curve: &'a Curve,
    vertices: &'a [[f64; 3]; 2],
    denominator: Interval,
    direction_norm_squared: Interval,
    parameters: [Interval; 2],
    exact_endpoints: bool,
    source_error_mm: f64,
}
fn failure(code: GeometryErrorCode, message: &'static str) -> GeometryError {
    GeometryError::new(code, message)
}
fn conditioning(message: &'static str) -> GeometryError {
    failure(GeometryErrorCode::ConditioningFailure, message)
}
fn dot(a: [Interval; 3], b: [Interval; 3]) -> Interval {
    (0..3).fold(Interval::point(0.0), |sum, axis| {
        sum.add(a[axis].mul(b[axis]))
    })
}
fn finite(value: Interval) -> Result<Interval, GeometryError> {
    Interval::new(value.lo, value.hi)
}
fn exact_collinear(
    origin: [f64; 3],
    direction: [f64; 3],
    point: [f64; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    // Fixed-size exact expansions: no coefficient-dependent allocation.
    let displacement: [Option<ExactCoefficient>; 3] =
        std::array::from_fn(|axis| ExactCoefficient::difference(point[axis], origin[axis]));
    let [Some(x), Some(y), Some(z)] = displacement else {
        return Ok(false);
    };
    let displacement = [x, y, z];
    for (a, b) in [(0, 1), (0, 2), (1, 2)] {
        let Some(mut value) =
            displacement[a].product(&ExactCoefficient::point(direction[b]), work, cancel)?
        else {
            return Ok(false);
        };
        let Some(other) =
            displacement[b].product(&ExactCoefficient::point(direction[a]), work, cancel)?
        else {
            return Ok(false);
        };
        if value.subtract(&other).is_none() || value.sign() != 0 {
            return Ok(false);
        }
    }
    Ok(true)
}
/// Tighten a proposed floating quotient only after an exact coefficient
/// identity proves numerator = proposal * denominator. Rounded products alone
/// cannot turn 1/3 into an exact trim parameter.
fn exact_line_parameter(
    origin: [f64; 3],
    direction: [f64; 3],
    magnitude: f64,
    point: [f64; 3],
    proposal: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<f64>, GeometryError> {
    if !proposal.is_finite() {
        return Ok(None);
    }
    let mut numerator = ExactCoefficient::point(0.0);
    let mut norm = ExactCoefficient::point(0.0);
    for axis in 0..3 {
        let Some(delta) = ExactCoefficient::difference(point[axis], origin[axis]) else {
            return Ok(None);
        };
        let d = ExactCoefficient::point(direction[axis]);
        let Some(product) = delta.product(&d, work, cancel)? else {
            return Ok(None);
        };
        for &term in &product.terms[..product.len] {
            if numerator.push(term).is_none() {
                return Ok(None);
            }
        }
        let Some(product) = d.product(&d, work, cancel)? else {
            return Ok(None);
        };
        for &term in &product.terms[..product.len] {
            if norm.push(term).is_none() {
                return Ok(None);
            }
        }
    }
    let Some(denominator) = norm.product(&ExactCoefficient::point(magnitude), work, cancel)? else {
        return Ok(None);
    };
    let Some(product) = denominator.product(&ExactCoefficient::point(proposal), work, cancel)?
    else {
        return Ok(None);
    };
    Ok((numerator.subtract(&product).is_some() && numerator.sign() == 0).then_some(proposal))
}
impl<'a> SourceLineRestriction<'a> {
    /// Only a literal selected LINE may enter this constructor. In particular,
    /// it does not unwrap a TRIMMED_CURVE, override a preferred source master,
    /// choose a PCURVE, or infer a source branch from native proximity.
    pub(crate) fn new(
        curve: &'a Curve,
        vertices: &'a [[f64; 3]; 2],
        same_sense: bool,
        maximum_source_error_mm: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        work.charge(1, cancel)?;
        let Curve::Line {
            origin,
            direction,
            magnitude,
        } = curve
        else {
            return Ok(None);
        };
        if !maximum_source_error_mm.is_finite() || maximum_source_error_mm < 0.0 {
            return Err(failure(
                GeometryErrorCode::InvalidGeometry,
                "source LINE restriction needs its finite nonnegative source allowance",
            ));
        }
        for value in origin
            .iter()
            .chain(direction)
            .chain(vertices.iter().flatten())
            .chain([magnitude])
        {
            Interval::new(*value, *value)?;
        }
        work.charge(32, cancel)?;
        let d = direction.map(Interval::point);
        let norm = finite(dot(d, d))?;
        if norm.lo <= 0.0 || *magnitude == 0.0 {
            return Err(failure(
                GeometryErrorCode::SingularGeometry,
                "original source LINE has no nonzero coefficient direction",
            ));
        }
        let denominator = finite(Interval::point(*magnitude).mul(norm))?;
        if denominator.contains_zero() {
            return Err(conditioning(
                "original LINE parameter denominator is unresolved",
            ));
        }
        let mut parameters = [Interval::point(0.0); 2];
        let mut source_error_mm = 0.0_f64;
        let mut exact_endpoints = true;
        for endpoint in 0..2 {
            work.charge(32, cancel)?;
            let delta = std::array::from_fn(|axis| {
                Interval::point(vertices[endpoint][axis]).sub(Interval::point(origin[axis]))
            });
            let numerator = dot(delta, d);
            parameters[endpoint] = finite(numerator.div(denominator)?)?;
            if let Some(parameter) = exact_line_parameter(
                *origin,
                *direction,
                *magnitude,
                vertices[endpoint],
                parameters[endpoint].mid(),
                work,
                cancel,
            )? {
                parameters[endpoint] = Interval::point(parameter);
            }
            let exact = exact_collinear(*origin, *direction, vertices[endpoint], work, cancel)?;
            exact_endpoints &= exact;
            if !exact {
                // Magnitude cancels from the actual projection expression; do
                // not multiply an uncertain reciprocal back by a large scale.
                let scalar = numerator.div(norm)?;
                let residual: [Interval; 3] =
                    std::array::from_fn(|axis| d[axis].mul(scalar).sub(delta[axis]));
                let squared = residual
                    .into_iter()
                    .fold(Interval::point(0.0), |sum, value| sum.add(value.square()));
                source_error_mm = source_error_mm.max(finite(squared.sqrt()?)?.hi);
            }
        }
        let ordered = if same_sense {
            parameters[0].hi < parameters[1].lo
        } else {
            parameters[1].hi < parameters[0].lo
        };
        if !ordered {
            return Err(failure(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "original LINE endpoints do not prove the declared source direction and a nonzero active span",
            ));
        }
        if source_error_mm > maximum_source_error_mm {
            return Err(failure(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "original LINE endpoint projection exceeds the captured source allowance",
            ));
        }
        Ok(Some(Self {
            curve,
            vertices,
            denominator,
            direction_norm_squared: norm,
            parameters,
            exact_endpoints,
            source_error_mm,
        }))
    }
    pub(crate) fn source_curve(&self) -> &'a Curve {
        self.curve
    }
    pub(crate) fn source_vertices(&self) -> &'a [[f64; 3]; 2] {
        self.vertices
    }
    pub(crate) fn endpoint_parameters(&self) -> [Interval; 2] {
        self.parameters
    }
    pub(crate) fn source_error_mm(&self) -> f64 {
        self.source_error_mm
    }
    pub(crate) fn exact_source_endpoints(&self) -> bool {
        self.exact_endpoints
    }
    /// Actual original t of the selected reference C(s), s in [0,1]. This is an
    /// expression enclosure, never a rounded scalar trim or a native gauge.
    pub(crate) fn parameter_image(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Interval, GeometryError> {
        work.charge(32, cancel)?;
        let s = Interval::new(range[0], range[1])?;
        if range[0] < 0.0 || range[1] > 1.0 {
            return Err(failure(
                GeometryErrorCode::InvalidGeometry,
                "source LINE query exceeds its normalized captured edge",
            ));
        }
        let Curve::Line {
            origin, direction, ..
        } = self.curve
        else {
            unreachable!("sealed LINE operand");
        };
        let delta = std::array::from_fn(|axis| {
            Interval::point(self.vertices[0][axis])
                .sub(Interval::point(origin[axis]))
                .add(
                    s.mul(
                        Interval::point(self.vertices[1][axis])
                            .sub(Interval::point(self.vertices[0][axis])),
                    ),
                )
        });
        finite(dot(delta, direction.map(Interval::point)).div(self.denominator)?)
    }
    pub(crate) fn parameter_derivative(
        &self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Interval, GeometryError> {
        work.charge(24, cancel)?;
        let Curve::Line { direction, .. } = self.curve else {
            unreachable!("sealed LINE operand");
        };
        let delta = std::array::from_fn(|axis| {
            Interval::point(self.vertices[1][axis]).sub(Interval::point(self.vertices[0][axis]))
        });
        finite(dot(delta, direction.map(Interval::point)).div(self.denominator)?)
    }
    /// Enclose C(s), preserving the source's exact line restriction. Exact
    /// collinearity licenses the shared-endpoint cancellation, not epsilon snaps.
    pub(crate) fn jet(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<CurveThirdEnclosure, GeometryError> {
        work.charge(48, cancel)?;
        let s = Interval::new(range[0], range[1])?;
        if range[0] < 0.0 || range[1] > 1.0 {
            return Err(failure(
                GeometryErrorCode::InvalidGeometry,
                "source LINE query exceeds its normalized captured edge",
            ));
        }
        let delta: [Interval; 3] = std::array::from_fn(|axis| {
            Interval::point(self.vertices[1][axis]).sub(Interval::point(self.vertices[0][axis]))
        });
        let (position, derivative) = if self.exact_endpoints {
            (
                std::array::from_fn(|axis| {
                    Interval::point(self.vertices[0][axis]).add(s.mul(delta[axis]))
                }),
                delta,
            )
        } else {
            let Curve::Line {
                origin, direction, ..
            } = self.curve
            else {
                unreachable!("sealed LINE operand");
            };
            let d = direction.map(Interval::point);
            let start = std::array::from_fn(|axis| {
                Interval::point(self.vertices[0][axis]).sub(Interval::point(origin[axis]))
            });
            let anchor = dot(start, d).div(self.direction_norm_squared)?;
            let slope = dot(delta, d).div(self.direction_norm_squared)?;
            (
                std::array::from_fn(|axis| {
                    Interval::point(origin[axis]).add(d[axis].mul(anchor.add(s.mul(slope))))
                }),
                d.map(|component| component.mul(slope)),
            )
        };
        for value in position.into_iter().chain(derivative) {
            finite(value)?;
        }
        Ok(CurveThirdEnclosure {
            jet: CurveEnclosure {
                position,
                derivative,
                second_derivative: [Interval::point(0.0); 3],
            },
            third_derivative: [Interval::point(0.0); 3],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn line() -> Curve {
        Curve::Line {
            origin: [0.0; 3],
            direction: [1.0, 0.0, 0.0],
            magnitude: 3.0,
        }
    }

    #[test]
    fn finite_carrier_line_proposal_cannot_become_closed_endpoint_authority() {
        use solidcraft_step::geom as s;
        use spiling_contracts::geometry::StepEntityId;
        use std::sync::Arc;

        let placement = s::Placement {
            origin: [0.0; 3],
            axis: [0.0, 0.0, 1.0],
            ref_direction: [1.0, 0.0, 0.0],
        };
        let carrier = Arc::new(s::Surface::RectangularTrimmed {
            basis: Arc::new(s::Surface::Cylinder {
                position: placement,
                radius: 2.0,
            }),
            u1: 0.0,
            u2: 1.0,
            v1: 0.0,
            v2: 1.0,
            u_sense: true,
            v_sense: true,
        });
        let xy = [2.0 * 0.5_f64.cos(), 2.0 * 0.5_f64.sin()];
        let source = OriginalCoedgeSource {
            source_face: StepEntityId(1),
            bound_id: StepEntityId(2),
            loop_id: StepEntityId(3),
            oriented_edge_id: StepEntityId(4),
            edge_curve_id: StepEntityId(5),
            curve_id: StepEntityId(6),
            vertex_ids: [StepEntityId(7), StepEntityId(8)],
            vertex_points_mm: [[xy[0], xy[1], 0.2], [xy[0], xy[1], 1.0 + 1.0e-8]],
            forward: true,
            same_sense: true,
            spatial_curve: Arc::new(Curve::SurfaceCurve {
                kind: s::SurfaceCurveKind::Surface,
                curve_3d: Arc::new(Curve::Line {
                    origin: [xy[0], xy[1], 0.0],
                    direction: [0.0, 0.0, 1.0],
                    magnitude: 1.0,
                }),
                associated: vec![s::Associated::Pcurve(s::Pcurve {
                    surface: carrier,
                    curve: Arc::new(Curve::Line {
                        origin: [0.5, 0.0, 0.0],
                        direction: [0.0, 1.0, 0.0],
                        magnitude: 1.0,
                    }),
                })],
                master: s::PreferredRep::PcurveS1,
            }),
            pcurves: Vec::new(),
        };
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(10_000_000);
        let bytes = 2 * 1024 * 1024;
        let selected = selection::select(&source, 1.0e-7, bytes, &mut work, &cancel).unwrap();
        assert_eq!(selected.active, selected.search);
        assert!(selected.periodic.is_none());
        assert!(selected.closed_domain.is_none());
        assert!(selected.active[0] > 0.0 && selected.active[1] < 1.0);
        let curve = PreparedOriginalCurve::new(
            &source.spatial_curve,
            selected.active,
            bytes,
            &mut work,
            &cancel,
        )
        .unwrap();
        // These numerical premises all hold at the inward carrier proposal.
        // They do not prove it is a genuine closed parameter incidence.
        assert!(
            roots::closed_fence_endpoint(
                &curve,
                source.vertex_points_mm[1],
                selected.active,
                1.0e-7,
                bytes,
                &mut work,
                &cancel,
            )
            .unwrap()
            .is_some()
        );
        assert!(
            endpoint_root(
                &curve,
                source.vertex_points_mm[1],
                selected.search,
                selected.periodic,
                selected.closed_domain,
                1.0e-7,
                bytes,
                &mut work,
                &cancel,
            )
            .is_err()
        );
        assert!(
            SourceSpatialRestriction::new(&source, 1.0e-7, bytes, &mut work, &cancel,).is_err()
        );
    }

    #[test]
    fn original_bracket_spatial_endpoints_preserve_the_captured_source_allowance() {
        const BYTES: usize = 2 * 1024 * 1024;
        let definition = crate::from_step_bytes(include_bytes!(
            "../../../../fixtures/geometry/independent-exporters/occt-ribbed-bracket-mm.step"
        ))
        .unwrap();
        let topology = definition.original_source();
        let allowance = definition.provenance().uncertainty_mm.unwrap();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(20_000_000);
        for (index, source) in topology.coedges.iter().enumerate() {
            work.charge(index as u64 + 1, &cancel).unwrap();
            if topology.coedges[..index]
                .iter()
                .any(|previous| previous.edge_curve_id == source.edge_curve_id)
            {
                continue;
            }
            let operator =
                SourceSpatialRestriction::new(source, allowance, BYTES, &mut work, &cancel)
                    .unwrap_or_else(|error| {
                        panic!("original edge {:?}: {error}", source.edge_curve_id)
                    });
            assert!(operator.source_error_mm() <= allowance);
            for (end, vertex) in source.vertex_points_mm.iter().enumerate() {
                let position = operator
                    .tensor([end as f64; 2], 0, &mut work, &cancel)
                    .unwrap()[0];
                let residual = position
                    .iter()
                    .zip(vertex)
                    .fold(Interval::point(0.0), |sum, (p, v)| {
                        sum.add(p.sub(Interval::point(*v)).square())
                    })
                    .sqrt()
                    .unwrap();
                assert!(residual.hi <= allowance);
                if operator.exact_source_endpoints() {
                    assert!(position.iter().zip(vertex).all(|(p, v)| p.contains(*v)));
                }
            }
        }
    }

    #[test]
    fn original_bracket_preferred_pcurve_closed_fence_endpoint_witness() {
        use solidcraft_step::geom::{PreferredRep, Surface};
        use spiling_contracts::geometry::StepEntityId;

        const BYTES: usize = 2 * 1024 * 1024;
        let definition = crate::from_step_bytes(include_bytes!(
            "../../../../fixtures/geometry/independent-exporters/occt-ribbed-bracket-mm.step"
        ))
        .unwrap();
        let topology = definition.original_source();
        let face = topology
            .faces
            .iter()
            .find(|face| face.source_face == StepEntityId(17))
            .unwrap();
        let wire = &face.wires[0];
        let local = 4;
        let source = topology.coedge(wire.coedges[local]).unwrap();
        let Curve::SurfaceCurve { master, .. } = source.spatial_curve.as_ref() else {
            panic!("original bracket operand lost its declared preferred master");
        };
        assert!(matches!(master, PreferredRep::PcurveS1));
        let association = source
            .pcurves
            .iter()
            .find(|association| association.association_index == 0)
            .unwrap();
        let preferred =
            crate::kernel::source_operand::preferred_source_pcurve(&source.spatial_curve)
                .unwrap()
                .unwrap();
        let Curve::BSpline(spline) = preferred.curve.as_ref() else {
            panic!("selected original master is not its finite spline");
        };
        let Surface::Plane { .. } = preferred.surface.as_ref() else {
            panic!("selected original master lost its plane carrier");
        };
        let allowance = definition.provenance().uncertainty_mm.unwrap();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(20_000_000);
        let selection = selection::select(source, allowance, BYTES, &mut work, &cancel).unwrap();
        let operand = PreparedOriginalCurve::new(
            &source.spatial_curve,
            selection.active,
            BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let endpoint = 1;
        let vertex = source.vertex_points_mm[endpoint];
        let parameter = selection.active[1];
        let jet = operand
            .tensor([parameter; 2], 1, &mut work, &cancel)
            .unwrap();
        let delta = std::array::from_fn(|axis| jet[0][axis].sub(Interval::point(vertex[axis])));
        let g = dot(delta, jet[1]);
        let residual = endpoint_error(
            &operand,
            Interval::point(parameter),
            vertex,
            &mut work,
            &cancel,
        )
        .unwrap();
        let whole = operand
            .tensor(selection.search, 2, &mut work, &cancel)
            .unwrap();
        let whole_delta =
            std::array::from_fn(|axis| whole[0][axis].sub(Interval::point(vertex[axis])));
        let derivative = dot(whole[1], whole[1]).add(dot(whole_delta, whole[2]));
        let stationary = roots::isolate_endpoint(
            &operand,
            vertex,
            selection.search,
            allowance,
            BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let exact = endpoint_identity(&operand, parameter, vertex, &mut work, &cancel).unwrap();

        assert_eq!(source.oriented_edge_id, StepEntityId(139));
        assert_eq!(source.edge_curve_id, StepEntityId(140));
        assert_eq!(source.curve_id, StepEntityId(143));
        assert_eq!(source.vertex_ids[endpoint], StepEntityId(141));
        assert_eq!(association.source_pcurve, StepEntityId(148));
        assert_eq!(association.carrier_id, StepEntityId(32));
        assert_eq!(spline.degree, 1);
        assert_eq!(selection.active, [-19.7537037037, 6.646296296296]);
        assert_eq!(selection.search, selection.active);
        assert_eq!(
            selection.closed_domain.map(|domain| domain.range()),
            Some(selection.active)
        );
        assert!(selection.periodic.is_none());
        assert_eq!(allowance, 1.0e-7);
        assert_eq!(vertex, [1.8, 1.8, 7.6]);
        assert!(
            stationary.is_empty(),
            "diagnosis falsified: there is a stationary root"
        );
        assert!(!exact);
        assert!(
            g.hi < 0.0,
            "diagnosis falsified: outward upper-fence sign is unresolved"
        );
        assert!(derivative.lo > 0.0);
        assert!(residual > 0.0 && residual <= allowance);
        let endpoint = endpoint_root(
            &operand,
            vertex,
            selection.search,
            None,
            selection.closed_domain,
            allowance,
            BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let SourceEndpoint::ClosedFence(fence) = endpoint else {
            panic!("a constrained endpoint was mislabeled as stationary-root authority");
        };
        assert_eq!(fence.parameter(), parameter);
        assert!(fence.source_error_mm() >= residual);
        assert!(
            endpoint_root(
                &operand,
                vertex,
                selection.search,
                None,
                selection.closed_domain,
                residual / 2.0,
                BYTES,
                &mut work,
                &cancel,
            )
            .is_err()
        );
        // A periodic search seam must not gain the same closed-fence authority.
        assert!(
            endpoint_root(
                &operand,
                vertex,
                selection.search,
                Some(Interval::point(1.0)),
                selection.closed_domain,
                allowance,
                BYTES,
                &mut work,
                &cancel,
            )
            .is_err()
        );

        let operator =
            SourceSpatialRestriction::new(source, allowance, BYTES, &mut work, &cancel).unwrap();
        assert!(operator.source_error_mm() >= residual);
        assert!(operator.source_error_mm() <= allowance);
        assert!(!operator.exact_source_endpoints());
        assert_eq!(
            operator.endpoint_parameters().unwrap()[1],
            Interval::point(parameter)
        );
        assert!(
            operator
                .parameter_derivative(&mut work, &cancel)
                .unwrap()
                .lo
                > 0.0
        );
        let end = operator.tensor([1.0; 2], 0, &mut work, &cancel).unwrap()[0];
        for axis in 0..3 {
            assert!(end[axis].intersection(jet[0][axis]).is_some());
        }
    }
    #[test]
    fn closed_fence_does_not_replace_stationary_or_ambiguous_source_endpoints() {
        use solidcraft_step::geom::Placement;
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(10_000_000);
        let bytes = 2 * 1024 * 1024;
        let source = line();
        let curve =
            PreparedOriginalCurve::new(&source, [0.0, 1.0], bytes, &mut work, &cancel).unwrap();
        let endpoint = endpoint_root(
            &curve,
            [1.5, 1.0e-8, 0.0],
            [0.0, 1.0],
            None,
            None,
            1.0e-7,
            bytes,
            &mut work,
            &cancel,
        )
        .unwrap();
        let SourceEndpoint::Root { root, .. } = endpoint else {
            panic!("an existing stationary endpoint changed authority");
        };
        assert!(root.source_error_mm() >= 1.0e-8);

        let source = Curve::Circle {
            position: Placement {
                origin: [0.0; 3],
                axis: [0.0, 0.0, 1.0],
                ref_direction: [1.0, 0.0, 0.0],
            },
            radius: 2.0,
        };
        let range = [-0.4, 2.0 * std::f64::consts::TAU - 0.4];
        let curve = PreparedOriginalCurve::new(&source, range, bytes, &mut work, &cancel).unwrap();
        // Two distinct source parameters own equally near stationary points;
        // allowing residual does not authorize selecting either or a fence.
        assert!(
            endpoint_root(
                &curve,
                [2.0, 1.0, 0.0],
                range,
                None,
                None,
                0.3,
                bytes,
                &mut work,
                &cancel,
            )
            .is_err()
        );
    }

    #[test]
    fn selected_nonperiodic_pcurve_line_keeps_its_finite_source_restriction() {
        let definition = crate::from_step_bytes(include_bytes!(
            "../../../../fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step"
        ))
        .unwrap();
        let source = definition
            .original_source()
            .coedges
            .iter()
            .find(|source| {
                let Some(pcurve) =
                    crate::kernel::source_operand::preferred_source_pcurve(&source.spatial_curve)
                        .unwrap()
                else {
                    return false;
                };
                matches!(
                    pcurve.surface.as_ref(),
                    solidcraft_step::geom::Surface::Cylinder { .. }
                ) && matches!(pcurve.curve.as_ref(), Curve::Line { direction, .. }
                    if direction[0] == 0.0 && direction[1] != 0.0)
            })
            .unwrap();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(2_000_000);
        let operator =
            SourceSpatialRestriction::new(source, 1.0e-7, 2 * 1024 * 1024, &mut work, &cancel)
                .unwrap();
        for parameter in [0.0, 0.5, 1.0] {
            let jet = operator
                .tensor([parameter; 2], 1, &mut work, &cancel)
                .unwrap();
            for axis in 0..3 {
                let expected = source.vertex_points_mm[0][axis] * (1.0 - parameter)
                    + source.vertex_points_mm[1][axis] * parameter;
                let error = jet[0][axis].sub(Interval::point(expected));
                assert!(error.lo.abs().max(error.hi.abs()) < 1.0e-7);
                let expected_velocity =
                    source.vertex_points_mm[1][axis] - source.vertex_points_mm[0][axis];
                let error = jet[1][axis].sub(Interval::point(expected_velocity));
                assert!(error.lo.abs().max(error.hi.abs()) < 1.0e-7);
            }
        }
    }
    #[test]
    fn selected_finite_spline_master_keeps_its_coefficient_fence_endpoint() {
        let definition = crate::from_step_bytes(include_bytes!(
            "../../../../fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step"
        ))
        .unwrap();
        let source = definition
            .original_source()
            .coedges
            .iter()
            .find(|source| source.edge_curve_id == spiling_contracts::geometry::StepEntityId(279))
            .unwrap();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(2_000_000);
        let operator =
            SourceSpatialRestriction::new(source, 1.0e-7, 2 * 1024 * 1024, &mut work, &cancel)
                .unwrap();
        for endpoint in 0..2 {
            let position = operator
                .tensor([endpoint as f64; 2], 0, &mut work, &cancel)
                .unwrap()[0];
            for axis in 0..3 {
                let residual =
                    position[axis].sub(Interval::point(source.vertex_points_mm[endpoint][axis]));
                assert!(residual.lo.abs().max(residual.hi.abs()) < 1.0e-7);
            }
        }
        let middle = operator.tensor([0.5; 2], 0, &mut work, &cancel).unwrap()[0];
        for axis in [0, 2] {
            assert!(middle[axis].lo > 0.58 && middle[axis].hi < 0.59);
        }
    }
    #[test]
    fn independent_circular_pcurve_rejects_the_long_complementary_branch() {
        use spiling_contracts::geometry::StepEntityId;
        use std::sync::Arc;
        let definition = crate::from_step_bytes(include_bytes!(
            "../../../../fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step"
        ))
        .unwrap();
        let face = definition
            .original_source()
            .faces
            .iter()
            .find(|face| face.source_face == StepEntityId(461))
            .unwrap();
        let source = definition
            .original_source()
            .coedges
            .iter()
            .find(|source| {
                source.source_face == face.source_face && source.edge_curve_id == StepEntityId(142)
            })
            .unwrap();
        let pcurve = source
            .pcurves
            .iter()
            .find(|pcurve| pcurve.carrier_id == face.carrier_id)
            .unwrap();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(20_000_000);
        let spatial = Arc::new(
            SourceSpatialRestriction::new(source, 1.0e-7, 2 * 1024 * 1024, &mut work, &cancel)
                .unwrap(),
        );
        let curve = pcurve::SourcePcurveRestriction::new(
            face,
            source,
            pcurve,
            &spatial,
            1.0e-7,
            2 * 1024 * 1024,
            &mut work,
            &cancel,
        )
        .unwrap();
        let middle = curve.tensor([0.5; 2], 0, &mut work, &cancel).unwrap()[0];
        assert!(middle[0].lo > 0.78 && middle[0].hi < 0.79);
        assert!(middle[1].lo.abs().max(middle[1].hi.abs()) < 1.0e-7);
    }
    #[test]
    fn preferred_opposite_pcurve_gauge_keeps_the_declared_3d_half_circle() {
        use solidcraft_step::geom as s;
        use std::sync::Arc;
        let record = crate::analysis::test_source::circular_prism([0.0, 0.0], 4.0, 3.0);
        let placement = |z| s::Placement {
            origin: [0.0, 0.0, z],
            axis: [0.0, 0.0, 1.0],
            ref_direction: [1.0, 0.0, 0.0],
        };
        let carrier = Arc::new(s::Surface::Cylinder {
            position: placement(0.0),
            radius: 4.0,
        });
        let edge_curves = record
            .edges
            .iter()
            .map(|edge| {
                let solidcraft_geom::brep::Curve::Circle { .. } = &edge.curve else {
                    return None;
                };
                let z = record.vertices[edge.vertices[0] as usize].point[2];
                Some(s::Curve::SurfaceCurve {
                    kind: s::SurfaceCurveKind::Surface,
                    curve_3d: Arc::new(s::Curve::Circle {
                        position: placement(z),
                        radius: 4.0,
                    }),
                    associated: vec![s::Associated::Pcurve(s::Pcurve {
                        surface: carrier.clone(),
                        curve: Arc::new(s::Curve::Line {
                            origin: [0.0, z, 0.0],
                            direction: [-1.0, 0.0, 0.0],
                            magnitude: 1.0,
                        }),
                    })],
                    master: s::PreferredRep::PcurveS1,
                })
            })
            .collect();
        // Actual STEP writer/importer retains this deliberately independent,
        // oppositely directed source gauge. It is not a fake native chart.
        let definition = crate::analysis::test_source::original_definition(
            crate::analysis::test_source::SourceBrep {
                record,
                edge_curves,
                face_surfaces: vec![],
            },
        );
        let source = definition
            .original_source()
            .coedges
            .iter()
            .find(|source| {
                source.vertex_points_mm[0] == [4.0, 0.0, 0.0]
                    && source.vertex_points_mm[1] == [-4.0, 0.0, 0.0]
            })
            .unwrap();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(2_000_000);
        let operator =
            SourceSpatialRestriction::new(source, 1.0e-6, 2 * 1024 * 1024, &mut work, &cancel)
                .unwrap();
        assert!(
            operator
                .parameter_derivative(&mut work, &cancel)
                .unwrap()
                .hi
                < 0.0
        );
        assert!(operator.original_master_association().is_some());
        let middle = operator.tensor([0.5; 2], 1, &mut work, &cancel).unwrap();
        assert!(
            middle[0][1].lo > 3.99,
            "the complementary lower arc borrowed the master raw sense"
        );
    }
    #[test]
    fn captured_box_spatial_operators_keep_authored_endpoints_and_shared_identity() {
        let definition = crate::analysis::test_source::original_definition(
            crate::analysis::test_source::box_source(8.0, 5.0, 3.0),
        );
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(2_000_000);
        let coedges = &definition.original_source().coedges;
        assert!(!coedges.is_empty());
        for source in coedges {
            let operator =
                SourceSpatialRestriction::new(source, 1.0e-7, 2 * 1024 * 1024, &mut work, &cancel)
                    .unwrap();
            assert!(operator.exact_source_endpoints());
            assert_eq!(operator.source_error_mm(), 0.0);
            for endpoint in 0..2 {
                for axis in 0..3 {
                    assert!(
                        crate::parametric::curve_endpoint_coordinate_equal(
                            &operator,
                            endpoint as f64,
                            axis,
                            source.vertex_points_mm[endpoint][axis],
                            &mut work,
                            &cancel
                        )
                        .unwrap()
                    );
                }
            }
            let paired = coedges
                .iter()
                .find(|other| {
                    other.edge_curve_id == source.edge_curve_id
                        && other.oriented_edge_id != source.oriented_edge_id
                })
                .unwrap();
            let other =
                SourceSpatialRestriction::new(paired, 1.0e-7, 2 * 1024 * 1024, &mut work, &cancel)
                    .unwrap();
            assert!(operator.same_operator(&other, &mut work, &cancel).unwrap());
        }
    }
    #[test]
    fn rational_source_knot_keeps_its_normalized_cover_and_original_position() {
        use spiling_contracts::geometry::StepEntityId;
        use std::sync::Arc;
        let record = crate::analysis::test_source::nurbs_cylinder();
        let edge = &record.edges[0];
        // Restrict the genuine rational edge, independently of whole-surface
        // seam admission. No literal CIRCLE or fitted analytic replacement.
        let source = OriginalCoedgeSource {
            source_face: StepEntityId(1),
            bound_id: StepEntityId(2),
            loop_id: StepEntityId(3),
            oriented_edge_id: StepEntityId(4),
            edge_curve_id: StepEntityId(5),
            curve_id: StepEntityId(6),
            vertex_ids: [StepEntityId(7), StepEntityId(8)],
            vertex_points_mm: edge
                .vertices
                .map(|index| record.vertices[index as usize].point),
            forward: true,
            same_sense: true,
            spatial_curve: Arc::new(solidcraft_step::write::topology::curve(&edge.curve).unwrap()),
            pcurves: Vec::new(),
        };
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(4_000_000);
        let operator =
            SourceSpatialRestriction::new(&source, 1.0e-7, 2 * 1024 * 1024, &mut work, &cancel)
                .unwrap();
        let index = operator
            .original_knots()
            .iter()
            .position(|&knot| knot == 1.0)
            .unwrap();
        let band = operator.knot_bands()[index];
        assert!(band.contains(0.5));
        assert!(
            operator
                .source_parameter_image([band.lo, band.hi], &mut work, &cancel)
                .unwrap()
                .unwrap()
                .contains(1.0)
        );
        let jet = operator
            .original_knot_jet(index, 0, &mut work, &cancel)
            .unwrap();
        for (coordinate, expected) in jet[0].iter().zip([0.0, 5.0, 0.0]) {
            let residual = coordinate.sub(Interval::point(expected));
            assert!(residual.lo.abs().max(residual.hi.abs()) < 1.0e-7);
        }
    }
    #[test]
    fn original_line_restriction_retains_nonrepresentable_source_parameters() {
        let curve = line();
        let vertices = [[1.0, 0.0, 0.0], [2.0, 0.0, 0.0]];
        let mut work = GeometryWorkBudget::new(10_000);
        let cancel = AtomicBool::new(false);
        let restriction =
            SourceLineRestriction::new(&curve, &vertices, true, 0.0, &mut work, &cancel)
                .unwrap()
                .unwrap();
        assert_eq!(restriction.source_error_mm(), 0.0);
        assert!(
            restriction
                .endpoint_parameters()
                .iter()
                .all(|parameter| parameter.width() > 0.0)
        );
        let jet = restriction.jet([0.0, 1.0], &mut work, &cancel).unwrap();
        assert!(jet.jet.position[0].contains(1.0) && jet.jet.position[0].contains(2.0));
        assert!(jet.jet.derivative[0].contains(1.0));
        assert!(
            restriction
                .parameter_derivative(&mut work, &cancel)
                .unwrap()
                .contains(1.0 / 3.0)
        );
    }
    #[test]
    fn source_line_restriction_preserves_declared_reversal() {
        let curve = line();
        let vertices = [[2.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
        let mut work = GeometryWorkBudget::new(10_000);
        let cancel = AtomicBool::new(false);
        let restriction =
            SourceLineRestriction::new(&curve, &vertices, false, 0.0, &mut work, &cancel)
                .unwrap()
                .unwrap();
        assert!(
            restriction
                .parameter_derivative(&mut work, &cancel)
                .unwrap()
                .hi
                < 0.0
        );
        assert!(
            restriction
                .jet([0.0, 1.0], &mut work, &cancel)
                .unwrap()
                .jet
                .derivative[0]
                .hi
                < 0.0
        );
        assert!(
            SourceLineRestriction::new(&curve, &vertices, true, 0.0, &mut work, &cancel).is_err()
        );
    }
    #[test]
    fn source_endpoint_error_is_checked_not_borrowed_from_native_tolerance() {
        let curve = line();
        let vertices = [[1.0, 1.0e-6, 0.0], [2.0, 1.0e-6, 0.0]];
        let mut work = GeometryWorkBudget::new(10_000);
        let error = SourceLineRestriction::new(
            &curve,
            &vertices,
            true,
            1.0e-7,
            &mut work,
            &AtomicBool::new(false),
        )
        .err()
        .unwrap();
        assert_eq!(
            error.code,
            GeometryErrorCode::ParameterCorrespondenceUnavailable
        );
    }
}
