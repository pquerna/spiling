// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Selected original UV operators. The preferred-master case shares the spatial
//! restriction's immutable parameter expression; independently gauged PCURVEs
//! are aligned only by calibration of their actual lifted world geometry.
use super::{
    SourceSpatialRestriction, admit_vec,
    roots::{self, ClosedFenceEndpoint, EndpointRoot},
};
use crate::kernel::source::{OriginalCoedgeSource, OriginalFaceSource, OriginalPcurveSource};
use crate::kernel::source_operand::{PreparedOriginalCurve, preferred_source_pcurve};
use crate::kernel::trim::{SourceCurveCalibration, calibrate_source_curves};
use crate::parametric::{AxisSpans, CurveEvaluator, Interval};
use crate::{GeometryError, GeometryWorkBudget};
use solidcraft_step::geom::Curve;
use spiling_contracts::geometry::GeometryErrorCode;
use std::sync::{Arc, atomic::AtomicBool};

mod lift;
mod window;
use lift::LiftedPcurve;

fn unavailable(message: &'static str) -> GeometryError {
    GeometryError::new(
        GeometryErrorCode::ParameterCorrespondenceUnavailable,
        message,
    )
}
fn capacity() -> GeometryError {
    GeometryError::new(
        GeometryErrorCode::ResourceLimit,
        "selected original PCurve exceeds its admitted live byte allowance",
    )
}
fn available(limit: usize, live: usize) -> Result<usize, GeometryError> {
    limit.checked_sub(live).ok_or_else(capacity)
}

/// A search cover in the declared master's own gauge, not endpoint incidence
/// or permission to reuse the spatial reference's parameter values.
pub(super) fn selected_master_window(
    source: &OriginalCoedgeSource,
    pcurve: &solidcraft_step::geom::Pcurve,
    same_sense: bool,
    allowance: f64,
    remaining: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<super::selection::SourceSelection, GeometryError> {
    let window = window::select_operand(source, pcurve, allowance, remaining, work, cancel)?;
    Ok(super::selection::SourceSelection {
        active: window.raw,
        search: window.search,
        // Preserve only selector-owned provenance. Carrier-derived LINE
        // proposals carry None even when active == search.
        closed_domain: window.closed_domain,
        declared_endpoints: window.declared_endpoints,
        endpoint_targets: None,
        declared_turns: window.declared_turns,
        periodic: window.period,
        increasing: same_sense,
    })
}

#[derive(Clone, Copy)]
enum RawEndpoint {
    Literal { value: f64, turns: i32 },
    Root { root: EndpointRoot, turns: i32 },
    ClosedFence(ClosedFenceEndpoint),
    FirstShift { turns: i32 },
}
#[derive(Clone, Copy)]
struct RawRestriction {
    endpoints: [RawEndpoint; 2],
    period: Option<Interval>,
}
impl RawRestriction {
    fn endpoint(&self, index: usize) -> Result<Interval, GeometryError> {
        match self.endpoints[index] {
            RawEndpoint::Root { root, turns: 0 } => Ok(root.enclosure()),
            RawEndpoint::Root { root, turns } => Ok(root.enclosure().add(
                self.period
                    .ok_or_else(|| {
                        unavailable("original PCurve root turn has no actual UV period")
                    })?
                    .mul(Interval::point(f64::from(turns))),
            )),
            RawEndpoint::ClosedFence(fence) => Ok(Interval::point(fence.parameter())),
            RawEndpoint::FirstShift { turns } => Ok(self.endpoint(0)?.add(
                self.period
                    .ok_or_else(|| {
                        unavailable("original PCurve shared-root shift has no actual UV period")
                    })?
                    .mul(Interval::point(f64::from(turns))),
            )),
            RawEndpoint::Literal { value, turns: 0 } => Ok(Interval::point(value)),
            RawEndpoint::Literal { value, turns } => Ok(Interval::point(value).add(
                self.period
                    .ok_or_else(|| {
                        unavailable("original PCurve endpoint turn has no actual period expression")
                    })?
                    .mul(Interval::point(f64::from(turns))),
            )),
        }
    }
    fn derivative(&self) -> Result<Interval, GeometryError> {
        if let RawEndpoint::FirstShift { turns } = self.endpoints[1] {
            return Ok(self
                .period
                .ok_or_else(|| {
                    unavailable("original PCurve shared-root derivative has no actual UV period")
                })?
                .mul(Interval::point(f64::from(turns))));
        }
        Ok(self.endpoint(1)?.sub(self.endpoint(0)?))
    }
    fn image(&self, range: [f64; 2]) -> Result<Interval, GeometryError> {
        normalized(range)?;
        let [a, b] = [self.endpoint(0)?, self.endpoint(1)?];
        if matches!(self.endpoints[1], RawEndpoint::FirstShift { .. }) {
            let slope = self.derivative()?;
            return Ok(a
                .add(slope.mul(Interval::point(range[0])))
                .hull(a.add(slope.mul(Interval::point(range[1])))));
        }
        let at = |s: f64| {
            if s == 0.0 {
                a
            } else if s == 1.0 {
                b
            } else {
                a.mul(Interval::point(1.0).sub(Interval::point(s)))
                    .add(b.mul(Interval::point(s)))
            }
        };
        let image = at(range[0]).hull(at(range[1]));
        image.intersect(a.hull(b))
    }
    fn jet(&self, range: [f64; 2]) -> Result<[Interval; 4], GeometryError> {
        Ok([
            self.image(range)?,
            self.derivative()?,
            Interval::point(0.0),
            Interval::point(0.0),
        ])
    }
}

/// A genuine closed-domain minimum excludes every stationary root by the same
/// complete-domain theorem used by the spatial owner. Otherwise retain the
/// unchanged COMPLETE stationary inventory, including all competing branches.
enum RawEndpointCandidates {
    ClosedFence(ClosedFenceEndpoint),
    Stationary(Vec<EndpointRoot>),
}
impl RawEndpointCandidates {
    fn iter(&self) -> impl Iterator<Item = RawEndpoint> + '_ {
        let (fence, roots) = match self {
            Self::ClosedFence(fence) => (Some(RawEndpoint::ClosedFence(*fence)), &[][..]),
            Self::Stationary(roots) => (None, roots.as_slice()),
        };
        fence.into_iter().chain(
            roots
                .iter()
                .copied()
                .map(|root| RawEndpoint::Root { root, turns: 0 }),
        )
    }
    fn retained_bytes(&self) -> Result<usize, GeometryError> {
        let root_capacity = match self {
            Self::ClosedFence(_) => 0,
            Self::Stationary(roots) => roots.capacity(),
        };
        root_capacity
            .checked_mul(std::mem::size_of::<EndpointRoot>())
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Self>()))
            .ok_or_else(capacity)
    }
}

fn endpoint_candidates(
    lifted: &LiftedPcurve<'_>,
    vertex: [f64; 3],
    selected: &window::PcurveWindow,
    allowance: f64,
    remaining: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<RawEndpointCandidates, GeometryError> {
    let remaining = available(remaining, std::mem::size_of::<RawEndpointCandidates>())?;
    if selected.period.is_none()
        && let Some(domain) = selected
            .closed_domain
            .filter(|domain| domain.range() == selected.search)
        && let Some(fence) = roots::closed_fence_endpoint(
            lifted,
            vertex,
            domain.range(),
            allowance,
            remaining,
            work,
            cancel,
        )?
    {
        return Ok(RawEndpointCandidates::ClosedFence(fence));
    }
    roots::isolate_endpoint(
        lifted,
        vertex,
        selected.search,
        allowance,
        remaining,
        work,
        cancel,
    )
    .map(RawEndpointCandidates::Stationary)
}

fn endpoint_turns(
    endpoint: RawEndpoint,
    period: Option<Interval>,
    active: [f64; 2],
) -> Result<std::ops::RangeInclusive<i32>, GeometryError> {
    let Some(period) = period else {
        return Ok(0..=0);
    };
    let RawEndpoint::Root { root, turns: 0 } = endpoint else {
        return Err(unavailable(
            "periodic PCurve turn inventory needs an established base root",
        ));
    };
    // Necessary integer cover only. Each complete shifted endpoint/path still
    // must lie in the unchanged actual selected coefficient domain below.
    let turns = Interval::new(active[0], active[1])?
        .sub(root.enclosure())
        .div(period)?;
    let lower = turns.lo.ceil();
    let upper = turns.hi.floor();
    if lower < f64::from(i32::MIN) || upper > f64::from(i32::MAX) {
        return Err(capacity());
    }
    Ok(lower as i32..=upper as i32)
}

fn with_turns(endpoint: RawEndpoint, turns: i32) -> Result<RawEndpoint, GeometryError> {
    match endpoint {
        RawEndpoint::Root { root, turns: 0 } => Ok(RawEndpoint::Root { root, turns }),
        endpoint if turns == 0 => Ok(endpoint),
        _ => Err(unavailable(
            "PCurve coefficient turn lacks a retained base-root expression",
        )),
    }
}

/// A common integer turn of the ACTUAL UV curve preserves its complete
/// normalized image, not merely the lifted world endpoints. Each transported
/// stationary endpoint must additionally equal the candidate's established
/// root by whole-hull uniqueness. Different winding is never quotiented away.
fn same_periodic_branch(
    first: RawRestriction,
    second: RawRestriction,
    uv_period: Option<Interval>,
    lifted: &LiftedPcurve<'_>,
    vertices: [[f64; 3]; 2],
    remaining: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let Some(period) = uv_period else {
        // A LINE on a periodic carrier may repeat in world space but not UV.
        return Ok(false);
    };
    if !first
        .endpoints
        .iter()
        .chain(&second.endpoints)
        .all(|endpoint| matches!(endpoint, RawEndpoint::Root { .. }))
    {
        return Ok(false);
    }
    let turns = second
        .endpoint(0)?
        .sub(first.endpoint(0)?)
        .div(period)?
        .mid()
        .round();
    if !turns.is_finite() || turns.abs() > f64::from(i32::MAX) {
        return Ok(false);
    }
    let shift = period.mul(Interval::point(turns));
    for (index, vertex) in vertices.into_iter().enumerate() {
        if !roots::same_stationary_root(
            lifted,
            vertex,
            first.endpoint(index)?.add(shift),
            second.endpoint(index)?,
            remaining,
            work,
            cancel,
        )? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The Arc is the already admitted spatial payload, not a new root recipe or a
/// second allocation. Its bytes are charged once by the owning trim cache.
#[expect(
    clippy::large_enum_variant,
    reason = "Calibrated PCURVE data stays inline in the admitted restriction; shared data already uses its existing Arc owner"
)]
enum ParameterRecipe<'d> {
    Shared(Arc<SourceSpatialRestriction<'d>>),
    Calibrated {
        raw: RawRestriction,
        calibration: SourceCurveCalibration,
    },
}
pub(crate) struct SourcePcurveRestriction<'d> {
    face: &'d OriginalFaceSource,
    coedge: &'d OriginalCoedgeSource,
    source: &'d OriginalPcurveSource,
    chart: PreparedOriginalCurve<'d>,
    recipe: ParameterRecipe<'d>,
    original_knots: Vec<f64>,
    knot_bands: Vec<Interval>,
    cover_cuts: Vec<f64>,
    exact_endpoints: bool,
}

/// The same complete oriented UV path in its raw endpoint-expression gauge.
/// This view grants no native/spatial parameter correspondence: interior query
/// parameters differ from `SourcePcurveRestriction`'s calibrated gauge.
#[derive(Clone, Copy)]
pub(crate) struct SourcePcurveGeometry<'a, 'd> {
    owner: &'a SourcePcurveRestriction<'d>,
    raw: &'a RawRestriction,
}

impl CurveEvaluator for SourcePcurveGeometry<'_, '_> {
    fn active_range(&self) -> [f64; 2] {
        [0.0, 1.0]
    }
    fn exact_affine_coefficients(
        &self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<crate::parametric::ExactAffineCurve>, GeometryError> {
        use crate::exact_coefficient::ExactCoefficient as E;
        work.charge(1, cancel)?;
        // Root enclosures and periodic turn bounds are not exact parameters.
        // Literal coefficient fences are exact parameters, not vertex identities.
        let exact = |endpoint| match endpoint {
            RawEndpoint::Literal { value, turns: 0 } => Some(value),
            RawEndpoint::Root { root, turns: 0 } => root.exact_parameter(),
            RawEndpoint::ClosedFence(fence) => Some(fence.parameter()),
            _ => None,
        };
        let [Some(a), Some(b)] = self.raw.endpoints.map(exact) else {
            return Ok(None);
        };
        let Some(mut affine) = self.owner.chart.exact_affine_coefficients(work, cancel)? else {
            return Ok(None);
        };
        let mut slope = E::point(b);
        if slope.push(-a).is_none() {
            return Ok(None);
        }
        for axis in 0..3 {
            let Some(offset) = affine.derivative[axis].product(&E::point(a), work, cancel)? else {
                return Ok(None);
            };
            for term in &offset.terms[..offset.len] {
                if affine.position[axis].push(*term).is_none() {
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
    fn source_parameter_image(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Interval>, GeometryError> {
        work.charge(1, cancel)?;
        self.raw.image(range).map(Some)
    }
    fn tensor(
        &self,
        range: [f64; 2],
        order: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[[Interval; 3]; 4], GeometryError> {
        work.charge(1, cancel)?;
        if order > 3 {
            return Err(unavailable("geometric PCurve jet order exceeds three"));
        }
        let map = self.raw.jet(range)?;
        Ok(compose(
            self.owner
                .chart
                .evaluate_order([map[0].lo, map[0].hi], order, work, cancel)?,
            map,
            order,
        ))
    }
    fn continuous(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let p = self.raw.image(range)?;
        self.owner
            .chart
            .span_is_continuous([p.lo, p.hi], work, cancel)
    }
    fn smooth(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let p = self.raw.image(range)?;
        self.owner.chart.span_is_smooth([p.lo, p.hi], work, cancel)
    }
    fn parameter_spans(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<AxisSpans<'_>, GeometryError> {
        work.charge(1, cancel)?;
        normalized(range)?;
        // A complete evaluation cover, not a new knot inventory. The borrowed
        // original chart retains every knot and proves continuity independently;
        // calibrated-gauge cover cuts must not be reused in this different gauge.
        AxisSpans::single(range)
    }
    fn original_curve(&self) -> Option<&Curve> {
        Some(self.owner.chart.source())
    }
}

fn normalized(range: [f64; 2]) -> Result<(), GeometryError> {
    Interval::new(range[0], range[1])?;
    if range[0] < 0.0 || range[1] > 1.0 {
        return Err(unavailable(
            "original PCurve query exceeds its normalized source restriction",
        ));
    }
    Ok(())
}
fn compose(raw: [[Interval; 3]; 4], map: [Interval; 4], order: usize) -> [[Interval; 3]; 4] {
    let mut result = raw;
    for axis in 0..3 {
        if order >= 1 {
            result[1][axis] = raw[1][axis].mul(map[1]);
        }
        if order >= 2 {
            result[2][axis] = raw[2][axis]
                .mul(map[1].square())
                .add(raw[1][axis].mul(map[2]));
        }
        if order >= 3 {
            result[3][axis] = raw[3][axis]
                .mul(map[1].square().mul(map[1]))
                .add(
                    raw[2][axis]
                        .mul(map[1])
                        .mul(map[2])
                        .mul(Interval::point(3.0)),
                )
                .add(raw[1][axis].mul(map[3]));
        }
    }
    result
}

/// Temporary actual S_original(gamma_raw(t)) under the endpoint root expression.
/// It never manufactures a source coedge or treats a UV point as a world vertex.
struct NormalizedLift<'a, 'd> {
    lifted: &'a LiftedPcurve<'d>,
    raw: RawRestriction,
    cuts: &'a [f64],
}
impl CurveEvaluator for NormalizedLift<'_, '_> {
    fn active_range(&self) -> [f64; 2] {
        [0.0, 1.0]
    }
    fn tensor(
        &self,
        range: [f64; 2],
        order: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[[Interval; 3]; 4], GeometryError> {
        if order > 3 {
            return Err(unavailable("original PCurve lift jet order exceeds three"));
        }
        let map = self.raw.jet(range)?;
        Ok(compose(
            self.lifted
                .tensor([map[0].lo, map[0].hi], order, work, cancel)?,
            map,
            order,
        ))
    }
    fn continuous(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let p = self.raw.image(range)?;
        self.lifted.continuous([p.lo, p.hi], work, cancel)
    }
    fn smooth(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let p = self.raw.image(range)?;
        self.lifted.smooth([p.lo, p.hi], work, cancel)
    }
    fn parameter_spans(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<AxisSpans<'_>, GeometryError> {
        normalized(range)?;
        AxisSpans::from_cover_cuts(self.cuts, range, work, cancel)
    }
}

fn preferred_identity(
    face: &OriginalFaceSource,
    pcurve: &OriginalPcurveSource,
    spatial: &SourceSpatialRestriction<'_>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    if !Arc::ptr_eq(&face.carrier, &pcurve.operands.surface) {
        return Ok(false);
    }
    let Some(mut curve) = spatial.original_curve() else {
        return Ok(false);
    };
    for _ in 0..64 {
        work.charge(1, cancel)?;
        match curve {
            Curve::Trimmed { basis, .. } => curve = basis,
            Curve::SurfaceCurve { curve_3d, .. } => {
                if let Some(preferred) = preferred_source_pcurve(curve)? {
                    return Ok(Arc::ptr_eq(&preferred.curve, &pcurve.operands.curve)
                        && Arc::ptr_eq(&preferred.surface, &pcurve.operands.surface));
                }
                curve = curve_3d;
            }
            _ => return Ok(false),
        }
    }
    Err(unavailable(
        "original PCurve preferred-master nesting exceeds its finite inventory",
    ))
}

fn endpoint_error(
    curve: &dyn CurveEvaluator,
    parameter: Interval,
    vertex: [f64; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(f64, bool), GeometryError> {
    let p = curve.tensor([parameter.lo, parameter.hi], 0, work, cancel)?[0];
    let delta: [Interval; 3] = std::array::from_fn(|i| p[i].sub(Interval::point(vertex[i])));
    let error = delta
        .into_iter()
        .fold(Interval::point(0.0), |a, b| a.add(b.square()))
        .sqrt()?
        .hi;
    let mut exact = parameter.lo == parameter.hi;
    if exact {
        for axis in 0..3 {
            exact &= crate::parametric::curve_endpoint_coordinate_equal(
                curve,
                parameter.lo,
                axis,
                vertex[axis],
                work,
                cancel,
            )?;
        }
    }
    Ok((if exact { 0.0 } else { error }, exact))
}
fn directed(
    raw: RawRestriction,
    lifted: &LiftedPcurve<'_>,
    spatial: &SourceSpatialRestriction<'_>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let slope = raw.derivative()?;
    if slope.contains_zero() {
        return Ok(false);
    }
    let p = raw.image([0.5; 2])?;
    let a = spatial.tensor([0.5; 2], 1, work, cancel)?[1];
    let b = lifted.tensor([p.lo, p.hi], 1, work, cancel)?[1];
    let product = (0..3).fold(Interval::point(0.0), |v, i| {
        v.add(a[i].mul(b[i]).mul(slope))
    });
    Ok(product.lo > 0.0)
}

fn calibrate_raw_branch(
    spatial: &SourceSpatialRestriction<'_>,
    lifted: &LiftedPcurve<'_>,
    raw: RawRestriction,
    cuts: &mut Vec<f64>,
    allowance: f64,
    remaining: usize,
    live: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SourceCurveCalibration, GeometryError> {
    let live = live
        .checked_add(std::mem::size_of::<NormalizedLift<'_, '_>>())
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Vec<f64>>()))
        .ok_or_else(capacity)?;
    let derivative = raw.derivative()?;
    let start = raw.endpoint(0)?;
    let knot_count = lifted.chart().parameter_axis(work, cancel)?.0.len();
    cuts.clear();
    let metadata = cuts
        .capacity()
        .checked_mul(std::mem::size_of::<f64>())
        .ok_or_else(capacity)?;
    admit_vec(
        cuts,
        2,
        live.checked_add(metadata).ok_or_else(capacity)?,
        remaining,
    )?;
    cuts.extend([0.0, 1.0]);
    for index in 0..knot_count {
        work.charge(1, cancel)?;
        let knot = lifted.chart().parameter_axis(work, cancel)?.0[index];
        let band = Interval::point(knot).sub(start).div(derivative)?;
        if let Some(band) = band.intersection(Interval::new(0.0, 1.0)?) {
            let metadata = cuts
                .capacity()
                .checked_mul(std::mem::size_of::<f64>())
                .ok_or_else(capacity)?;
            admit_vec(
                cuts,
                2,
                live.checked_add(metadata).ok_or_else(capacity)?,
                remaining,
            )?;
            cuts.extend([band.lo, band.hi]);
        }
    }
    work.charge(
        (cuts.len() as u64).saturating_mul(usize::BITS as u64),
        cancel,
    )?;
    cuts.sort_unstable_by(f64::total_cmp);
    cuts.dedup();
    let metadata = cuts
        .capacity()
        .checked_mul(std::mem::size_of::<f64>())
        .ok_or_else(capacity)?;
    let normalized = NormalizedLift { lifted, raw, cuts };
    calibrate_source_curves(
        spatial,
        &normalized,
        [0.0, 1.0],
        allowance,
        32,
        available(remaining, live.checked_add(metadata).ok_or_else(capacity)?)?,
        work,
        cancel,
    )
}

fn angular_uv_period(
    mut curve: &Curve,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Interval>, GeometryError> {
    for _ in 0..64 {
        work.charge(1, cancel)?;
        match curve {
            Curve::Trimmed { basis, .. } => curve = basis,
            Curve::Circle { .. } | Curve::Ellipse { .. } => {
                return crate::kernel::source_operand::original_curve_parameter_period(
                    curve, work, cancel,
                );
            }
            _ => return Ok(None),
        }
    }
    Err(unavailable(
        "original UV angular-period expression exceeds its finite inventory",
    ))
}

impl<'d> SourcePcurveRestriction<'d> {
    pub(crate) fn new(
        face: &'d OriginalFaceSource,
        coedge: &'d OriginalCoedgeSource,
        pcurve: &'d OriginalPcurveSource,
        spatial: &Arc<SourceSpatialRestriction<'d>>,
        allowance: f64,
        remaining: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        work.charge(1, cancel)?;
        if !allowance.is_finite() || allowance < 0.0 {
            return Err(unavailable(
                "original PCurve needs a finite nonnegative physical calibration ceiling",
            ));
        }
        if !Arc::ptr_eq(&face.carrier, &pcurve.operands.surface) {
            return Err(unavailable(
                "selected original PCurve does not use this actual original face carrier",
            ));
        }
        let spatial_source = spatial.source();
        if !Arc::ptr_eq(&spatial_source.spatial_curve, &coedge.spatial_curve)
            || spatial_source.edge_curve_id != coedge.edge_curve_id
            || spatial_source.curve_id != coedge.curve_id
            || spatial_source.vertex_ids != coedge.vertex_ids
            || spatial_source.vertex_points_mm != coedge.vertex_points_mm
            || spatial_source.same_sense != coedge.same_sense
        {
            return Err(unavailable(
                "original PCurve and shared spatial restriction have different source edge genealogy",
            ));
        }
        let fixed = std::mem::size_of::<Self>();
        let (chart, recipe, exact_endpoints) = if preferred_identity(
            face, pcurve, spatial, work, cancel,
        )? {
            let endpoints = spatial.endpoint_parameters()?;
            let raw = endpoints[0].hull(endpoints[1]);
            let chart = PreparedOriginalCurve::new(
                &pcurve.operands.curve,
                [raw.lo, raw.hi],
                available(remaining, fixed)?,
                work,
                cancel,
            )?;
            (
                chart,
                ParameterRecipe::Shared(Arc::clone(spatial)),
                spatial.exact_source_endpoints(),
            )
        } else {
            let selected = window::select(
                face,
                coedge,
                pcurve,
                allowance,
                available(remaining, fixed)?,
                work,
                cancel,
            )?;
            let branch_fixed = fixed
                .checked_add(std::mem::size_of::<window::PcurveWindow>())
                .ok_or_else(capacity)?;
            let lifted = LiftedPcurve::new(
                &pcurve.operands,
                &selected,
                available(remaining, branch_fixed)?,
                work,
                cancel,
            )?;
            let live = branch_fixed
                .checked_add(lifted.retained_bytes())
                .ok_or_else(capacity)?;
            // Only the actual UV expression licenses root turns and UV branch
            // equivalence. A carrier's world period is insufficient.
            let uv_period = angular_uv_period(&pcurve.operands.curve, work, cancel)?;
            let mut raw: Option<(RawRestriction, SourceCurveCalibration)> = None;
            let mut first_rejection = None;
            let mut candidate_cuts = Vec::new();
            // Endpoint and midpoint-germ tests only propose branches. Count
            // ambiguity after complete world/relative-derivative qualification.
            let mut consider = |candidate: RawRestriction,
                                root_bytes: usize,
                                work: &mut GeometryWorkBudget|
             -> Result<bool, GeometryError> {
                let retained = raw.as_ref().map_or(0, |(_, proof)| proof.retained_bytes());
                let branch_live = live
                    .checked_add(root_bytes)
                    .and_then(|bytes| bytes.checked_add(retained))
                    .ok_or_else(capacity)?;
                if let Some((previous, _)) = &raw {
                    let cut_bytes = candidate_cuts
                        .capacity()
                        .checked_mul(std::mem::size_of::<f64>())
                        .ok_or_else(capacity)?;
                    if same_periodic_branch(
                        *previous,
                        candidate,
                        uv_period,
                        &lifted,
                        coedge.vertex_points_mm,
                        available(
                            remaining,
                            branch_live.checked_add(cut_bytes).ok_or_else(capacity)?,
                        )?,
                        work,
                        cancel,
                    )? {
                        // The already qualified complete UV expression and
                        // its calibration are unchanged, not a second branch.
                        return Ok(true);
                    }
                }
                if !directed(candidate, &lifted, spatial, work, cancel)? {
                    return Ok(false);
                }
                match calibrate_raw_branch(
                    spatial,
                    &lifted,
                    candidate,
                    &mut candidate_cuts,
                    allowance,
                    remaining,
                    branch_live,
                    work,
                    cancel,
                ) {
                    Ok(calibration) => {
                        if raw.is_some() {
                            return Err(unavailable(
                                "original PCurve admits multiple whole-correspondence-qualified parameter branches",
                            ));
                        }
                        raw = Some((candidate, calibration));
                        Ok(true)
                    }
                    Err(error)
                        if matches!(
                            error.code,
                            GeometryErrorCode::Cancelled | GeometryErrorCode::ResourceLimit
                        ) =>
                    {
                        Err(error)
                    }
                    Err(error) => {
                        first_rejection.get_or_insert(error);
                        Ok(false)
                    }
                }
            };
            if let Some(values) = selected.declared_endpoints {
                let candidate = RawRestriction {
                    endpoints: std::array::from_fn(|i| RawEndpoint::Literal {
                        value: values[i],
                        turns: selected.declared_turns[i],
                    }),
                    period: selected.period,
                };
                for candidate in [
                    candidate,
                    RawRestriction {
                        endpoints: [candidate.endpoints[1], candidate.endpoints[0]],
                        period: candidate.period,
                    },
                ] {
                    let a = endpoint_error(
                        &lifted,
                        candidate.endpoint(0)?,
                        coedge.vertex_points_mm[0],
                        work,
                        cancel,
                    )?
                    .0;
                    let b = endpoint_error(
                        &lifted,
                        candidate.endpoint(1)?,
                        coedge.vertex_points_mm[1],
                        work,
                        cancel,
                    )?
                    .0;
                    if a <= allowance && b <= allowance {
                        consider(candidate, 0, work)?;
                    }
                }
            } else if coedge.vertex_ids[0] == coedge.vertex_ids[1]
                && coedge.vertex_points_mm[0] == coedge.vertex_points_mm[1]
                && uv_period.is_some()
            {
                // The vertex ownership selects the closed source branch; only
                // the actual UV coefficient period licenses this shared-root
                // shift. World periodicity alone would not license it.
                let period = uv_period;
                let endpoint = super::endpoint_root(
                    &lifted,
                    coedge.vertex_points_mm[0],
                    selected.search,
                    period,
                    None,
                    allowance,
                    available(remaining, live)?,
                    work,
                    cancel,
                )?;
                let super::SourceEndpoint::Root { root, turns: 0, .. } = endpoint else {
                    return Err(unavailable(
                        "original closed UV branch lacks its actual first-root recipe",
                    ));
                };
                for turns in [-1, 1] {
                    let candidate = RawRestriction {
                        endpoints: [
                            RawEndpoint::Root { root, turns: 0 },
                            RawEndpoint::FirstShift { turns },
                        ],
                        period,
                    };
                    let image = candidate.image([0.0, 1.0])?;
                    if image.lo < selected.raw[0] || image.hi > selected.raw[1] {
                        continue;
                    }
                    consider(candidate, 0, work)?;
                }
            } else {
                let first = endpoint_candidates(
                    &lifted,
                    coedge.vertex_points_mm[0],
                    &selected,
                    allowance,
                    available(remaining, live)?,
                    work,
                    cancel,
                )?;
                let first_bytes = first.retained_bytes()?;
                let second = endpoint_candidates(
                    &lifted,
                    coedge.vertex_points_mm[1],
                    &selected,
                    allowance,
                    available(
                        remaining,
                        live.checked_add(first_bytes).ok_or_else(capacity)?,
                    )?,
                    work,
                    cancel,
                )?;
                let root_bytes = second
                    .retained_bytes()?
                    .checked_add(first_bytes)
                    .ok_or_else(capacity)?;
                for a in first.iter() {
                    for b in second.iter() {
                        let a_turns = endpoint_turns(a, uv_period, selected.raw)?;
                        let b_turns = endpoint_turns(b, uv_period, selected.raw)?;
                        if a_turns.is_empty() || b_turns.is_empty() {
                            continue;
                        }
                        let first_winding = b_turns
                            .start()
                            .checked_sub(*a_turns.end())
                            .ok_or_else(capacity)?;
                        let last_winding = b_turns
                            .end()
                            .checked_sub(*a_turns.start())
                            .ok_or_else(capacity)?;
                        for winding in first_winding..=last_winding {
                            for a_turn in a_turns.clone() {
                                work.charge(1, cancel)?;
                                let b_turn = a_turn.checked_add(winding).ok_or_else(capacity)?;
                                if !b_turns.contains(&b_turn) {
                                    continue;
                                }
                                let candidate = RawRestriction {
                                    endpoints: [with_turns(a, a_turn)?, with_turns(b, b_turn)?],
                                    period: uv_period,
                                };
                                let image = candidate.image([0.0, 1.0])?;
                                if image.lo < selected.raw[0] || image.hi > selected.raw[1] {
                                    continue;
                                }
                                if consider(candidate, root_bytes, work)? {
                                    // These SAME base-root expressions with
                                    // equal winding differ only by a common
                                    // coefficient-proved UV turn. Once one is
                                    // qualified, no duplicate proof/allocation
                                    // or new ambiguity authority is needed.
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            let (raw, calibration) = raw.ok_or_else(|| first_rejection.unwrap_or_else(|| {
                unavailable(
                    "bounded original PCurve has no whole-correspondence-qualified endpoint restriction",
                )
            }))?;
            let exact = endpoint_error(
                &lifted,
                raw.endpoint(0)?,
                coedge.vertex_points_mm[0],
                work,
                cancel,
            )?
            .1 && endpoint_error(
                &lifted,
                raw.endpoint(1)?,
                coedge.vertex_points_mm[1],
                work,
                cancel,
            )?
            .1;
            (
                lifted.into_chart(),
                ParameterRecipe::Calibrated { raw, calibration },
                exact,
            )
        };
        let mut result = Self {
            face,
            coedge,
            source: pcurve,
            chart,
            recipe,
            original_knots: Vec::new(),
            knot_bands: Vec::new(),
            cover_cuts: Vec::new(),
            exact_endpoints,
        };
        if result.retained_bytes() > remaining {
            return Err(capacity());
        }
        result.retain_knots(remaining, work, cancel)?;
        Ok(result)
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self
                .chart
                .retained_bytes()
                .saturating_sub(std::mem::size_of_val(&self.chart))
            + match &self.recipe {
                ParameterRecipe::Shared(_) => 0,
                ParameterRecipe::Calibrated { calibration, .. } => calibration
                    .retained_bytes()
                    .saturating_sub(std::mem::size_of::<SourceCurveCalibration>()),
            }
            + self.original_knots.capacity() * std::mem::size_of::<f64>()
            + self.knot_bands.capacity() * std::mem::size_of::<Interval>()
            + self.cover_cuts.capacity() * std::mem::size_of::<f64>()
    }
    pub(crate) fn shared_master(&self) -> bool {
        matches!(&self.recipe, ParameterRecipe::Shared(_))
    }
    pub(crate) fn calibration(&self) -> Option<&SourceCurveCalibration> {
        match &self.recipe {
            ParameterRecipe::Shared(_) => None,
            ParameterRecipe::Calibrated { calibration, .. } => Some(calibration),
        }
    }
    /// A strictly increasing, endpoint-preserving calibration [0,1]->[0,1]
    /// changes speed only. Its sealed C0 segments share literal internal ends
    /// and cover the whole target, so removing it preserves the complete
    /// oriented path with the SAME raw roots, turns and selected chart.
    pub(crate) fn geometric_path(
        &self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<SourcePcurveGeometry<'_, 'd>>, GeometryError> {
        work.charge(1, cancel)?;
        let ParameterRecipe::Calibrated { raw, calibration } = &self.recipe else {
            return Ok(None);
        };
        if calibration.source_range() != [0.0, 1.0]
            || calibration.target_range() != [0.0, 1.0]
            || raw.derivative()?.contains_zero()
        {
            return Err(unavailable(
                "geometric PCurve view lacks a complete endpoint-preserving directed calibration",
            ));
        }
        Ok(Some(SourcePcurveGeometry { owner: self, raw }))
    }
    pub(crate) fn source_error_mm(&self) -> f64 {
        self.calibration()
            .map_or(0.0, SourceCurveCalibration::maximum_position_error_mm)
    }
    pub(crate) fn relative_derivative_error_bound(&self) -> f64 {
        self.calibration()
            .map_or(0.0, SourceCurveCalibration::relative_derivative_error_bound)
    }
    #[cfg(test)]
    pub(crate) fn exact_source_endpoints(&self) -> bool {
        self.exact_endpoints
    }
    pub(crate) fn original_knots(&self) -> &[f64] {
        &self.original_knots
    }
    pub(crate) fn knot_bands(&self) -> &[Interval] {
        &self.knot_bands
    }
    /// Literal UV coefficient identity, not source-V closure or overlap of
    /// independently enclosed endpoint positions.
    pub(crate) fn closed_uv_endpoint_identity(
        &self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        work.charge(1, cancel)?;
        let angular = angular_uv_period(self.chart.source(), work, cancel)?.is_some();
        let exact = match &self.recipe {
            ParameterRecipe::Shared(spatial) => match spatial.as_ref() {
                SourceSpatialRestriction::Endpoint(operator) => {
                    if angular {
                        if matches!(operator.endpoints[1], super::SourceEndpoint::FirstShift { turns } if turns != 0)
                        {
                            return Ok(true);
                        }
                        if let [
                            super::SourceEndpoint::Literal {
                                value: a,
                                turns: at,
                            },
                            super::SourceEndpoint::Literal {
                                value: b,
                                turns: bt,
                            },
                        ] = operator.endpoints
                            && a == b
                            && at != bt
                        {
                            return Ok(true);
                        }
                    }
                    let exact = |endpoint| match endpoint {
                        super::SourceEndpoint::Literal { value, turns: 0 } => Some(value),
                        super::SourceEndpoint::Root { root, turns: 0, .. } => {
                            root.exact_parameter()
                        }
                        _ => None,
                    };
                    [exact(operator.endpoints[0]), exact(operator.endpoints[1])]
                }
                _ => return Ok(false),
            },
            ParameterRecipe::Calibrated { raw, .. } => {
                if angular {
                    if matches!(raw.endpoints[1], RawEndpoint::FirstShift { turns } if turns != 0) {
                        return Ok(true);
                    }
                    if let [
                        RawEndpoint::Literal {
                            value: a,
                            turns: at,
                        },
                        RawEndpoint::Literal {
                            value: b,
                            turns: bt,
                        },
                    ] = raw.endpoints
                        && a == b
                        && at != bt
                    {
                        return Ok(true);
                    }
                }
                let exact = |endpoint| match endpoint {
                    RawEndpoint::Literal { value, turns: 0 } => Some(value),
                    RawEndpoint::Root { root, turns: 0 } => root.exact_parameter(),
                    RawEndpoint::ClosedFence(fence) => Some(fence.parameter()),
                    _ => None,
                };
                [exact(raw.endpoints[0]), exact(raw.endpoints[1])]
            }
        };
        let [Some(a), Some(b)] = exact else {
            return Ok(false);
        };
        if !self
            .chart
            .span_is_continuous([a.min(b), a.max(b)], work, cancel)?
        {
            return Ok(false);
        }
        let mut curve = self.chart.source();
        for _ in 0..64 {
            work.charge(1, cancel)?;
            let point = match curve {
                Curve::Trimmed { basis, .. } => {
                    curve = basis;
                    continue;
                }
                Curve::BSpline(spline) => spline.points.first(),
                Curve::Polyline(points) => points.first(),
                _ => return Ok(false),
            };
            let Some(point) = point else {
                return Ok(false);
            };
            for axis in 0..3 {
                if !crate::parametric::curve_endpoint_coordinate_equal(
                    &self.chart,
                    a,
                    axis,
                    point[axis],
                    work,
                    cancel,
                )? || !crate::parametric::curve_endpoint_coordinate_equal(
                    &self.chart,
                    b,
                    axis,
                    point[axis],
                    work,
                    cancel,
                )? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        Err(unavailable(
            "original UV endpoint identity exceeds its finite coefficient inventory",
        ))
    }
    pub(crate) fn parameter_image(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Interval, GeometryError> {
        Ok(self.parameter_jet(range, work, cancel)?[0])
    }
    pub(crate) fn parameter_jet(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[Interval; 4], GeometryError> {
        normalized(range)?;
        work.charge(1, cancel)?;
        match &self.recipe {
            ParameterRecipe::Shared(spatial) => Ok([
                spatial.source_parameter_image(range, work, cancel)?.ok_or_else(|| unavailable(
                    "shared preferred source master has no retained original raw parameter image"))?,
                spatial.parameter_derivative(work, cancel)?, Interval::point(0.0), Interval::point(0.0)]),
            ParameterRecipe::Calibrated { raw, calibration } => {
                let map = calibration.jet(range, work, cancel)?;
                let scale = raw.derivative()?;
                Ok([raw.image([map[0].lo, map[0].hi])?, map[1].mul(scale), map[2].mul(scale), map[3].mul(scale)])
            }
        }
    }
    pub(crate) fn parameter_derivative(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Interval, GeometryError> {
        Ok(self.parameter_jet(range, work, cancel)?[1])
    }

    /// Bands are cover metadata, never declarations that a raw knot is active.
    fn inverse_band(
        &self,
        knot: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Interval>, GeometryError> {
        let whole = self.parameter_image([0.0, 1.0], work, cancel)?;
        if !whole.contains(knot) {
            return Ok(None);
        }
        let derivative = self.parameter_derivative([0.0, 1.0], work, cancel)?;
        if derivative.contains_zero() {
            return Err(unavailable(
                "original PCurve knot inversion has no strict monotone parameter derivative",
            ));
        }
        let increasing = derivative.lo > 0.0;
        // Each discarded half has a strict interval inequality. Uncertain
        // evaluations stay in the band; no rounded inverse becomes a knot.
        let mut lower = [0.0, 1.0];
        let mut upper = [0.0, 1.0];
        for _ in 0..64 {
            let mid = lower[0] * 0.5 + lower[1] * 0.5;
            if mid > lower[0] && mid < lower[1] {
                let p = self.parameter_image([mid; 2], work, cancel)?;
                let before = if increasing { p.hi < knot } else { p.lo > knot };
                if before {
                    lower[0] = mid;
                } else {
                    lower[1] = mid;
                }
            }
            let mid = upper[0] * 0.5 + upper[1] * 0.5;
            if mid > upper[0] && mid < upper[1] {
                let p = self.parameter_image([mid; 2], work, cancel)?;
                let after = if increasing { p.lo > knot } else { p.hi < knot };
                if after {
                    upper[1] = mid;
                } else {
                    upper[0] = mid;
                }
            }
        }
        Ok(Some(Interval::new(lower[0], upper[1])?))
    }
    fn retain_knots(
        &mut self,
        remaining: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        let count = self.chart.parameter_axis(work, cancel)?.0.len();
        for index in 0..count {
            work.charge(1, cancel)?;
            let knot = self.chart.parameter_axis(work, cancel)?.0[index];
            if self.original_knots.last() == Some(&knot) {
                continue;
            }
            let Some(band) = self.inverse_band(knot, work, cancel)? else {
                continue;
            };
            let live = self.retained_bytes();
            admit_vec(&mut self.original_knots, 1, live, remaining)?;
            let live = self.retained_bytes();
            admit_vec(&mut self.knot_bands, 1, live, remaining)?;
            self.original_knots.push(knot);
            self.knot_bands.push(band);
        }
        let calibration_cuts = self
            .calibration()
            .map_or(0, |proof| proof.parameter_cuts().len());
        let count = self
            .knot_bands
            .len()
            .checked_mul(2)
            .and_then(|n| n.checked_add(2))
            .and_then(|n| n.checked_add(calibration_cuts))
            .ok_or_else(capacity)?;
        let live = self.retained_bytes();
        admit_vec(&mut self.cover_cuts, count, live, remaining)?;
        self.cover_cuts.extend([0.0, 1.0]);
        for band in &self.knot_bands {
            self.cover_cuts.extend([band.lo, band.hi]);
        }
        if let ParameterRecipe::Calibrated { calibration, .. } = &self.recipe {
            self.cover_cuts
                .extend_from_slice(calibration.parameter_cuts());
        }
        work.charge(
            (self.cover_cuts.len() as u64).saturating_mul(usize::BITS as u64),
            cancel,
        )?;
        self.cover_cuts.sort_unstable_by(f64::total_cmp);
        self.cover_cuts.dedup();
        Ok(())
    }
}
impl CurveEvaluator for SourcePcurveRestriction<'_> {
    fn active_range(&self) -> [f64; 2] {
        [0.0, 1.0]
    }
    fn source_parameter_map(&self) -> Option<[Interval; 2]> {
        match &self.recipe {
            ParameterRecipe::Shared(spatial) => spatial.source_parameter_map(),
            ParameterRecipe::Calibrated { .. } => None,
        }
    }
    fn source_parameter_image(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Interval>, GeometryError> {
        self.parameter_image(range, work, cancel).map(Some)
    }
    fn tensor(
        &self,
        range: [f64; 2],
        order: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[[Interval; 3]; 4], GeometryError> {
        if order > 3 {
            return Err(unavailable(
                "original PCurve derivative order exceeds three",
            ));
        }
        let map = self.parameter_jet(range, work, cancel)?;
        Ok(compose(
            self.chart
                .evaluate_order([map[0].lo, map[0].hi], order, work, cancel)?,
            map,
            order,
        ))
    }
    fn continuous(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let p = self.parameter_image(range, work, cancel)?;
        self.chart.span_is_continuous([p.lo, p.hi], work, cancel)
    }
    fn smooth(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let p = self.parameter_image(range, work, cancel)?;
        self.chart.span_is_smooth([p.lo, p.hi], work, cancel)
    }
    fn parameter_spans(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<AxisSpans<'_>, GeometryError> {
        normalized(range)?;
        AxisSpans::from_cover_cuts(&self.cover_cuts, range, work, cancel)
    }
    fn original_curve(&self) -> Option<&Curve> {
        Some(self.chart.source())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiling_contracts::geometry::StepEntityId;

    #[test]
    fn independent_pcurve_keeps_its_genuine_coefficient_fence_and_nonzero_residual() {
        const BYTES: usize = 2 * 1024 * 1024;
        let definition = crate::from_step_bytes(include_bytes!(
            "../../../../../fixtures/geometry/independent-exporters/occt-ribbed-bracket-mm.step"
        ))
        .unwrap();
        let topology = definition.original_source();
        let face = topology
            .faces
            .iter()
            .find(|face| face.carrier_id == StepEntityId(605))
            .unwrap();
        let source = topology
            .coedges
            .iter()
            .find(|source| {
                source.source_face == face.source_face && source.edge_curve_id == StepEntityId(592)
            })
            .unwrap();
        let pcurve = source
            .pcurves
            .iter()
            .find(|pcurve| pcurve.carrier_id == face.carrier_id)
            .unwrap();
        let allowance = definition.provenance().uncertainty_mm.unwrap();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(20_000_000);
        let spatial = Arc::new(
            SourceSpatialRestriction::new(source, allowance, BYTES, &mut work, &cancel).unwrap(),
        );
        assert!(!preferred_identity(face, pcurve, &spatial, &mut work, &cancel).unwrap());
        let mut selected =
            window::select(face, source, pcurve, allowance, BYTES, &mut work, &cancel).unwrap();
        assert_eq!(
            selected.closed_domain.unwrap().range(),
            [-19.7537037037, 6.646296296296]
        );
        let lifted =
            LiftedPcurve::new(&pcurve.operands, &selected, BYTES, &mut work, &cancel).unwrap();
        let candidates = endpoint_candidates(
            &lifted,
            source.vertex_points_mm[0],
            &selected,
            allowance,
            BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let RawEndpointCandidates::ClosedFence(fence) = &candidates else {
            panic!("original nonpreferred PCurve lost its typed closed coefficient fence");
        };
        let fence = *fence;
        assert_eq!(fence.parameter(), selected.raw[0]);
        assert!(fence.source_error_mm() > 0.0 && fence.source_error_mm() <= allowance);

        // Identical numeric active/search bounds without selector provenance
        // must not acquire that constrained-minimum authority.
        selected.closed_domain = None;
        match endpoint_candidates(
            &lifted,
            source.vertex_points_mm[0],
            &selected,
            allowance,
            BYTES,
            &mut work,
            &cancel,
        ) {
            Ok(candidates) => assert_eq!(candidates.iter().count(), 0),
            Err(error) => assert_eq!(
                error.code,
                GeometryErrorCode::ParameterCorrespondenceUnavailable
            ),
        }
        drop((candidates, lifted));

        let curve = SourcePcurveRestriction::new(
            face, source, pcurve, &spatial, allowance, BYTES, &mut work, &cancel,
        )
        .unwrap();
        let ParameterRecipe::Calibrated { raw, .. } = &curve.recipe else {
            panic!("independently gauged original PCurve borrowed a preferred-master recipe");
        };
        assert!(matches!(raw.endpoints[0], RawEndpoint::ClosedFence(_)));
        assert!(matches!(
            raw.endpoints[1],
            RawEndpoint::Root { turns: 0, .. }
        ));
        assert_eq!(raw.endpoint(0).unwrap(), Interval::point(fence.parameter()));
        assert!(!curve.exact_source_endpoints());
        assert!(curve.source_error_mm() > 0.0 && curve.source_error_mm() <= allowance);
        let start = curve.parameter_image([0.0; 2], &mut work, &cancel).unwrap();
        assert_eq!(start, raw.endpoint(0).unwrap());
        assert!(
            curve
                .geometric_path(&mut work, &cancel)
                .unwrap()
                .unwrap()
                .exact_affine_coefficients(&mut work, &cancel)
                .unwrap()
                .is_some()
                == raw.endpoints.iter().all(|endpoint| match endpoint {
                    RawEndpoint::ClosedFence(_) => true,
                    RawEndpoint::Root { root, turns: 0 } => root.exact_parameter().is_some(),
                    _ => false,
                })
        );
    }

    #[test]
    fn periodic_pcurve_root_cover_identifies_uv_turns_without_erasing_winding() {
        const BYTES: usize = 2 * 1024 * 1024;
        let definition = crate::from_step_bytes(include_bytes!(
            "../../../../../fixtures/geometry/independent-exporters/occt-ribbed-bracket-mm.step"
        ))
        .unwrap();
        let topology = definition.original_source();
        let allowance = definition.provenance().uncertainty_mm.unwrap();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(20_000_000);
        // Independent circular UV on an offset plane (duplicate whole-turn
        // proposals), then a seam root with nonzero translated radial residual.
        for (edge, carrier) in [(2294, 2311), (2440, 2459)] {
            let face = topology
                .faces
                .iter()
                .find(|face| face.carrier_id == StepEntityId(carrier))
                .unwrap();
            let source = topology
                .coedges
                .iter()
                .find(|source| {
                    source.source_face == face.source_face
                        && source.edge_curve_id == StepEntityId(edge)
                })
                .unwrap();
            let pcurve = source
                .pcurves
                .iter()
                .find(|pcurve| pcurve.carrier_id == face.carrier_id)
                .unwrap();
            let spatial = Arc::new(
                SourceSpatialRestriction::new(source, allowance, BYTES, &mut work, &cancel)
                    .unwrap(),
            );
            let selected =
                window::select(face, source, pcurve, allowance, BYTES, &mut work, &cancel).unwrap();
            assert!(selected.raw[0] < selected.search[0]);
            assert!(selected.search[1] < selected.raw[1]);
            assert!(selected.closed_domain.is_none());
            let curve = SourcePcurveRestriction::new(
                face, source, pcurve, &spatial, allowance, BYTES, &mut work, &cancel,
            )
            .unwrap();
            assert!(curve.source_error_mm() <= allowance);
            let ParameterRecipe::Calibrated { raw, .. } = &curve.recipe else {
                panic!("independent circular PCurve borrowed the spatial gauge");
            };
            let uv_period = angular_uv_period(&pcurve.operands.curve, &mut work, &cancel).unwrap();
            let lifted =
                LiftedPcurve::new(&pcurve.operands, &selected, BYTES, &mut work, &cancel).unwrap();
            assert!(
                same_periodic_branch(
                    *raw,
                    *raw,
                    uv_period,
                    &lifted,
                    source.vertex_points_mm,
                    BYTES,
                    &mut work,
                    &cancel,
                )
                .unwrap()
            );
            let mut shifted = None;
            for turn in [-1, 1] {
                let candidate = RawRestriction {
                    endpoints: raw.endpoints.map(|endpoint| match endpoint {
                        RawEndpoint::Root { root, turns } => RawEndpoint::Root {
                            root,
                            turns: turns + turn,
                        },
                        _ => panic!("circular arc lost its stationary endpoint recipe"),
                    }),
                    period: raw.period,
                };
                let image = candidate.image([0.0, 1.0]).unwrap();
                if selected.raw[0] <= image.lo && image.hi <= selected.raw[1] {
                    shifted = Some(candidate);
                    break;
                }
            }
            let shifted = shifted.expect("periodic source extension has no interior turn witness");
            assert!(
                same_periodic_branch(
                    *raw,
                    shifted,
                    uv_period,
                    &lifted,
                    source.vertex_points_mm,
                    BYTES,
                    &mut work,
                    &cancel,
                )
                .unwrap()
            );
            let different_winding = RawRestriction {
                endpoints: [raw.endpoints[0], shifted.endpoints[1]],
                period: raw.period,
            };
            assert!(
                !same_periodic_branch(
                    *raw,
                    different_winding,
                    uv_period,
                    &lifted,
                    source.vertex_points_mm,
                    BYTES,
                    &mut work,
                    &cancel,
                )
                .unwrap()
            );
            assert!(
                !same_periodic_branch(
                    *raw,
                    shifted,
                    None,
                    &lifted,
                    source.vertex_points_mm,
                    BYTES,
                    &mut work,
                    &cancel,
                )
                .unwrap()
            );
        }
    }
}
