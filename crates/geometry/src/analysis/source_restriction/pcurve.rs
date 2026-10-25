// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Selected original UV operators. The preferred-master case shares the spatial
//! restriction's immutable parameter expression; independently gauged PCURVEs
//! are aligned only by calibration of their actual lifted world geometry.
use super::{
    SourceSpatialRestriction, admit_vec,
    roots::{self, EndpointRoot},
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

#[derive(Clone, Copy)]
enum RawEndpoint {
    Literal { value: f64, turns: i32 },
    Root(EndpointRoot),
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
            RawEndpoint::Root(root) => Ok(root.enclosure()),
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

/// The Arc is the already admitted spatial payload, not a new root recipe or a
/// second allocation. Its bytes are charged once by the owning trim cache.
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
    cuts: Vec<f64>,
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
        AxisSpans::from_cover_cuts(&self.cuts, range, work, cancel)
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
                spatial,
                allowance,
                available(remaining, fixed)?,
                work,
                cancel,
            )?;
            let lifted = LiftedPcurve::new(
                &pcurve.operands,
                &selected,
                available(remaining, fixed)?,
                work,
                cancel,
            )?;
            let live = fixed
                .checked_add(lifted.retained_bytes())
                .ok_or_else(capacity)?;
            let mut raw = None;
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
                    if a <= allowance
                        && b <= allowance
                        && directed(candidate, &lifted, spatial, work, cancel)?
                    {
                        if raw.is_some() {
                            return Err(unavailable(
                                "declared original PCurve endpoints admit more than one directed world branch",
                            ));
                        }
                        raw = Some(candidate);
                    }
                }
            } else if coedge.vertex_ids[0] == coedge.vertex_ids[1]
                && coedge.vertex_points_mm[0] == coedge.vertex_points_mm[1]
                && angular_uv_period(&pcurve.operands.curve, work, cancel)?.is_some()
            {
                // The vertex ownership selects the closed source branch; only
                // the actual UV coefficient period licenses this shared-root
                // shift. World periodicity alone would not license it.
                let period = angular_uv_period(&pcurve.operands.curve, work, cancel)?;
                let selection =
                    super::selection::select_curve(&pcurve.operands.curve, true, work, cancel)?;
                let endpoint = super::endpoint_root(
                    &lifted,
                    coedge.vertex_points_mm[0],
                    selection.search,
                    period,
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
                        endpoints: [RawEndpoint::Root(root), RawEndpoint::FirstShift { turns }],
                        period,
                    };
                    let image = candidate.image([0.0, 1.0])?;
                    if image.lo < selected.raw[0] || image.hi > selected.raw[1] {
                        continue;
                    }
                    if directed(candidate, &lifted, spatial, work, cancel)? {
                        if raw.is_some() {
                            return Err(unavailable(
                                "closed original PCurve has ambiguous actual directed period ownership",
                            ));
                        }
                        raw = Some(candidate);
                    }
                }
            } else {
                let first = roots::isolate_endpoint(
                    &lifted,
                    coedge.vertex_points_mm[0],
                    selected.raw,
                    allowance,
                    available(remaining, live)?,
                    work,
                    cancel,
                )?;
                let first_bytes = first
                    .capacity()
                    .checked_mul(std::mem::size_of::<EndpointRoot>())
                    .ok_or_else(capacity)?;
                let second = roots::isolate_endpoint(
                    &lifted,
                    coedge.vertex_points_mm[1],
                    selected.raw,
                    allowance,
                    available(
                        remaining,
                        live.checked_add(first_bytes).ok_or_else(capacity)?,
                    )?,
                    work,
                    cancel,
                )?;
                for &a in &first {
                    for &b in &second {
                        work.charge(1, cancel)?;
                        let candidate = RawRestriction {
                            endpoints: [RawEndpoint::Root(a), RawEndpoint::Root(b)],
                            period: None,
                        };
                        if directed(candidate, &lifted, spatial, work, cancel)? {
                            if raw.is_some() {
                                return Err(unavailable(
                                    "original PCurve world vertices admit multiple directed endpoint-root branches",
                                ));
                            }
                            raw = Some(candidate);
                        }
                    }
                }
            }
            let raw = raw.ok_or_else(|| {
                unavailable(
                    "bounded original PCurve has no unique directed world endpoint restriction",
                )
            })?;
            let live = live
                .checked_add(std::mem::size_of::<NormalizedLift<'_, 'd>>())
                .ok_or_else(capacity)?;
            let mut normalized = NormalizedLift {
                lifted: &lifted,
                raw,
                cuts: Vec::new(),
            };
            let derivative = raw.derivative()?;
            let a = raw.endpoint(0)?;
            let (knots, _) = lifted.chart().parameter_axis(work, cancel)?;
            let knot_count = knots.len();
            admit_vec(&mut normalized.cuts, 2, live, remaining)?;
            normalized.cuts.extend([0.0, 1.0]);
            for index in 0..knot_count {
                work.charge(1, cancel)?;
                let knot = lifted.chart().parameter_axis(work, cancel)?.0[index];
                let band = Interval::point(knot).sub(a).div(derivative)?;
                if let Some(band) = band.intersection(Interval::new(0.0, 1.0)?) {
                    let metadata = normalized
                        .cuts
                        .capacity()
                        .checked_mul(std::mem::size_of::<f64>())
                        .ok_or_else(capacity)?;
                    admit_vec(
                        &mut normalized.cuts,
                        2,
                        live.checked_add(metadata).ok_or_else(capacity)?,
                        remaining,
                    )?;
                    normalized.cuts.extend([band.lo, band.hi]);
                }
            }
            work.charge(
                (normalized.cuts.len() as u64).saturating_mul(usize::BITS as u64),
                cancel,
            )?;
            normalized.cuts.sort_unstable_by(f64::total_cmp);
            normalized.cuts.dedup();
            let metadata = normalized
                .cuts
                .capacity()
                .checked_mul(std::mem::size_of::<f64>())
                .ok_or_else(capacity)?;
            let calibration = calibrate_source_curves(
                spatial.as_ref(),
                &normalized,
                [0.0, 1.0],
                allowance,
                available(remaining, live.checked_add(metadata).ok_or_else(capacity)?)?,
                work,
                cancel,
            )?;
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
            drop(normalized);
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
    pub(crate) fn source_error_mm(&self) -> f64 {
        self.calibration()
            .map_or(0.0, SourceCurveCalibration::maximum_position_error_mm)
    }
    pub(crate) fn relative_derivative_error_bound(&self) -> f64 {
        self.calibration()
            .map_or(0.0, SourceCurveCalibration::relative_derivative_error_bound)
    }
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
                        {
                            if a == b && at != bt {
                                return Ok(true);
                            }
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
                    {
                        if a == b && at != bt {
                            return Ok(true);
                        }
                    }
                }
                let exact = |endpoint| match endpoint {
                    RawEndpoint::Literal { value, turns: 0 } => Some(value),
                    RawEndpoint::Root(root) => root.exact_parameter(),
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
    pub(crate) fn original_knot_jet(
        &self,
        index: usize,
        order: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[[Interval; 3]; 4], GeometryError> {
        let &knot = self.original_knots.get(index).ok_or_else(|| {
            unavailable("original PCurve knot index is outside its retained raw census")
        })?;
        let a = self.parameter_image([0.0; 2], work, cancel)?;
        let b = self.parameter_image([1.0; 2], work, cancel)?;
        let active = if a.hi < b.lo {
            a.hi <= knot && knot <= b.lo
        } else if b.hi < a.lo {
            b.hi <= knot && knot <= a.lo
        } else {
            false
        };
        if !active {
            return Err(unavailable(
                "raw PCurve knot inverse band alone does not prove an active original knot",
            ));
        }
        self.chart.evaluate_order([knot; 2], order, work, cancel)
    }
    pub(crate) fn same_operator(
        &self,
        other: &Self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        work.charge(1, cancel)?;
        if std::ptr::eq(self, other) {
            return Ok(true);
        }
        if !std::ptr::eq(self.face, other.face)
            || !std::ptr::eq(self.coedge, other.coedge)
            || !Arc::ptr_eq(&self.source.operands.curve, &other.source.operands.curve)
            || !Arc::ptr_eq(
                &self.source.operands.surface,
                &other.source.operands.surface,
            )
        {
            return Ok(false);
        }
        match (&self.recipe, &other.recipe) {
            (ParameterRecipe::Shared(a), ParameterRecipe::Shared(b)) => Ok(Arc::ptr_eq(a, b)),
            // Independent cubic calibrations are not expression aliases merely
            // because their endpoint ranges or residual tubes overlap.
            _ => Ok(false),
        }
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
