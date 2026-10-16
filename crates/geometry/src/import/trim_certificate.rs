// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Positional correspondence of independently parameterized native curves.
//! Source identity, chart invertibility and trim topology are separate proofs.

use crate::{
    GeometryError, GeometryWorkBudget, check_cancel,
    parametric::{self, ChartCurveEnclosure, CurveEnclosure, Interval, PreparedCurveEnclosure},
};
use monstertruck_modeling::{self as model, base::*};
use spiling_contracts::geometry::GeometryErrorCode;
use std::{mem::size_of, sync::atomic::AtomicBool};

const MAX_WORK: u64 = 8_000_000;
const MAX_SCRATCH_BYTES: usize = 8 * 1024 * 1024;
const MAX_CUTS: usize = 65_536;
const MAX_DEPTH: usize = 32;
const MAX_NODES: usize = 131_072;

#[derive(Clone, Copy, PartialEq)]
pub(super) struct CorrespondenceSource {
    pub face: usize,
    pub wire: usize,
    pub coedge: usize,
    pub source_edge: Option<u64>,
    pub native_edge: usize,
    pub shift: [f64; 2],
    pub native_outer: Option<((f64, f64), bool)>,
}

impl std::fmt::Display for CorrespondenceSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "f={} w={} c={} e={}",
            self.face, self.wire, self.coedge, self.native_edge
        )?;
        if let Some(source) = self.source_edge {
            write!(f, " src={source}")?;
        }
        if f.alternate() {
            write!(f, " shift={:?}", self.shift)?;
            if let Some(outer) = self.native_outer {
                write!(f, " outer={outer:?}")?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct FallbackDiagnostic {
    source: Option<CorrespondenceSource>,
    harmonic: super::harmonic::HarmonicDiagnostic,
    harmonic_work: u64,
    fallback_work: u64,
    nodes: usize,
    source_range: [f64; 2],
    target_range: [f64; 2],
    outcome: &'static str,
}

impl std::fmt::Display for FallbackDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(source) = self.source {
            write!(f, "{source} ")?;
        }
        write!(
            f,
            "work={} harmonic={} n={} {} ({})",
            self.fallback_work, self.harmonic_work, self.nodes, self.outcome, self.harmonic
        )?;
        if let Some(source) = self.source
            && let Some(outer) = source.native_outer
        {
            write!(f, " outer={outer:?}")?;
        }
        if f.alternate() {
            write!(
                f,
                " source_range={:?} target_range={:?}",
                self.source_range, self.target_range
            )?;
        }
        Ok(())
    }
}

/// Retain only the last and largest fallbacks, even when they eventually prove
/// correspondence. A later cumulative-budget failure must not lose its prior
/// actual work consumer just because the current harmonic entry cannot start.
#[derive(Default)]
pub(super) struct CorrespondenceDiagnostic {
    pub active_source: Option<CorrespondenceSource>,
    last: Option<FallbackDiagnostic>,
    largest: Option<FallbackDiagnostic>,
}

impl CorrespondenceDiagnostic {
    fn record(&mut self, receipt: FallbackDiagnostic) {
        self.last = Some(receipt);
        if self
            .largest
            .is_none_or(|previous| receipt.fallback_work > previous.fallback_work)
        {
            self.largest = Some(receipt);
        }
    }
}

impl std::fmt::Display for CorrespondenceDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(largest) = self.largest {
            write!(f, "maxFB({largest})")?;
            if f.alternate()
                && let Some(last) = self.last
                && last.source != largest.source
            {
                write!(f, "; lastFB({last})")?;
            }
            Ok(())
        } else {
            f.write_str("noFB")
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Correspondence<'a> {
    pub curve: &'a model::Curve,
    pub surface: &'a model::Surface,
    pub chart: &'a model::Curve2D,
    pub maximum_error_mm: f64,
    pub margin_uv: [f64; 2],
}

/// Prove a whole-original-range physical residual under one captured monotone
/// affine correspondence. The native curve range and the pcurve's *own* range
/// are independent. Both endpoint-pairing orientations are proposals only;
/// neither is accepted without bounded verification of every interior position.
///
/// `Some(error_mm)` bounds `|C(t) - S(U(s(t)))|` everywhere, with `s` the
/// normalized affine map between those complete ranges. It does NOT prove an
/// exact conversion, a UV inverse, source-geometry validity, trim coverage or
/// topology, and cannot by itself authorize any of those operations. `None`
/// means nonmatching or unresolved correspondence, including an unresolved
/// nonsmooth chart crossing. Cancellation and exhausted work/storage remain
/// typed errors. Native work is debited on unsuccessful attempts as well.
pub(super) fn certify_curve_on_surface(
    curve: &model::Curve,
    surface: &model::Surface,
    chart: &model::Curve2D,
    maximum_error_mm: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<f64>, GeometryError> {
    certify_with_policy(
        Correspondence {
            curve,
            surface,
            chart,
            maximum_error_mm,
            margin_uv: [0.0; 2],
        },
        ProofPolicy::EitherPosition,
        work,
        cancel,
        None,
    )
}

/// Verify a retained declared coedge's forward affine parameter proposal using
/// both whole-interval physical position and relative tangent correspondence.
/// Every accepted smooth cell proves `|C' - (S o U o q)'| < min|C'| / 4`.
/// Tangents are scaled before subtraction and squared norms, so a microscopic
/// periodic carrier cannot authorize a wrong winding merely because its entire
/// diameter lies below the absolute physical tolerance.
///
/// This requires the caller's original source association/use identity. It does
/// not independently supply a UV inverse, UV winding/arrangement certificate,
/// trim coverage or native use incidence. Those remain separate obligations.
pub(super) fn certify_declared_coedge_correspondence(
    input: Correspondence<'_>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    diagnostic: Option<&mut CorrespondenceDiagnostic>,
) -> Result<Option<f64>, GeometryError> {
    certify_with_policy(
        input,
        ProofPolicy::DeclaredCoedge(input.margin_uv),
        work,
        cancel,
        diagnostic,
    )
}

#[derive(Clone, Copy, PartialEq)]
enum ProofPolicy {
    EitherPosition,
    DeclaredCoedge([f64; 2]),
}
impl ProofPolicy {
    fn margin_uv(self) -> [f64; 2] {
        match self {
            Self::DeclaredCoedge(margin) => margin,
            _ => [0.0; 2],
        }
    }
}

fn certify_with_policy(
    input: Correspondence<'_>,
    policy: ProofPolicy,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    diagnostic: Option<&mut CorrespondenceDiagnostic>,
) -> Result<Option<f64>, GeometryError> {
    let maximum_error_mm = input.maximum_error_mm;
    check_cancel(cancel)?;
    if !maximum_error_mm.is_finite() || maximum_error_mm <= 0.0 {
        return Err(GeometryError::new(
            GeometryErrorCode::InvalidGeometry,
            "native pcurve correspondence needs a finite positive physical tolerance",
        ));
    }
    if policy
        .margin_uv()
        .into_iter()
        .any(|x| !x.is_finite() || x < 0.0)
    {
        return Err(GeometryError::new(
            GeometryErrorCode::InvalidGeometry,
            "native declared pcurve uncertainty must be finite and nonnegative",
        ));
    }
    // The per-certificate frozen cap narrows, never resets, the caller's job cap.
    let mut local = GeometryWorkBudget::new(work.remaining().min(MAX_WORK));
    let result = certify(input, policy, None, &mut local, cancel, diagnostic);
    // Cancellation stops geometry work, not the debit for work already done.
    work.charge(local.consumed(), &AtomicBool::new(false))?;
    check_cancel(cancel)?;
    resolution(result)
}

pub(super) fn certify_retained_coedge_correspondence(
    input: Correspondence<'_>,
    prepared: &model::PreparedValidatedCurve<'static>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<f64>, GeometryError> {
    let Correspondence {
        maximum_error_mm: maximum_error,
        margin_uv,
        ..
    } = input;
    check_cancel(cancel)?;
    if !maximum_error.is_finite()
        || maximum_error <= 0.0
        || margin_uv
            .into_iter()
            .any(|value| !value.is_finite() || value < 0.0)
    {
        return Err(GeometryError::new(
            GeometryErrorCode::InvalidGeometry,
            "invalid retained original implicit correspondence tolerance",
        ));
    }
    let mut local = GeometryWorkBudget::new(work.remaining().min(MAX_WORK));
    let result = certify(
        input,
        ProofPolicy::DeclaredCoedge(margin_uv),
        Some(prepared),
        &mut local,
        cancel,
        None,
    );
    work.charge(local.consumed(), &AtomicBool::new(false))?;
    check_cancel(cancel)?;
    resolution(result)
}

fn terminal(error: &GeometryError) -> bool {
    matches!(
        error.code,
        GeometryErrorCode::Cancelled | GeometryErrorCode::ResourceLimit
    )
}
fn resolution(result: Result<Option<f64>, GeometryError>) -> Result<Option<f64>, GeometryError> {
    match result {
        Err(error) if !terminal(&error) => Ok(None),
        other => other,
    }
}
fn resource(message: &str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::ResourceLimit, message)
}

/// Explicit curves need only one immutable borrowed source capture. In
/// particular, do not prepare a separate implicit branch for each child span.
/// Explicit C0 splines are evaluated only after their native knots are split.
struct Source<'a> {
    original: &'a model::Curve,
    implicit: Option<PreparedCurveEnclosure<'a>>,
}
impl Source<'_> {
    fn jet(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<CurveEnclosure, GeometryError> {
        if let Some(curve) = &self.implicit {
            curve.jet((range[0], range[1]), work, cancel)
        } else {
            parametric::curve_enclosure(self.original, (range[0], range[1]), work, cancel)
        }
    }
    fn endpoint(
        &self,
        t: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[Interval; 3], GeometryError> {
        if let Some(curve) = &self.implicit {
            // Native evaluate retains continuation-joint endpoint enclosures.
            curve.endpoint_position(t, work, cancel)
        } else {
            Ok(self.jet([t, t], work, cancel)?.position)
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct AffineMap {
    source: [f64; 2],
    target: [f64; 2],
    slope: Interval,
    identity: bool,
}
impl AffineMap {
    pub(super) fn new(source: [f64; 2], target: [f64; 2]) -> Result<Self, GeometryError> {
        let identity = source == target;
        let slope = if identity {
            Interval::point(1.0)
        } else {
            Interval::point(target[1])
                .sub(Interval::point(target[0]))
                .div(Interval::point(source[1]).sub(Interval::point(source[0])))?
        };
        Interval::new(slope.lo, slope.hi)?;
        if slope.contains_zero() {
            return Err(GeometryError::new(
                GeometryErrorCode::ConditioningFailure,
                "affine native/pcurve parameter correspondence is not strictly monotone",
            ));
        }
        Ok(Self {
            source,
            target,
            slope,
            identity,
        })
    }
    pub(super) fn image(self, range: [f64; 2]) -> Result<Interval, GeometryError> {
        if self.identity {
            return Interval::new(range[0], range[1]);
        }
        let point = |t: f64| -> Result<Interval, GeometryError> {
            if t == self.source[0] {
                return Ok(Interval::point(self.target[0]));
            }
            if t == self.source[1] {
                return Ok(Interval::point(self.target[1]));
            }
            let normalized = Interval::point(t)
                .sub(Interval::point(self.source[0]))
                .div(Interval::point(self.source[1]).sub(Interval::point(self.source[0])))?;
            Ok(Interval::point(self.target[0]).add(
                normalized
                    .mul(Interval::point(self.target[1]).sub(Interval::point(self.target[0]))),
            ))
        };
        let image = point(range[0])?.hull(point(range[1])?);
        // Exact monotonicity proves the full image lies in the target range.
        // Intersecting this independently proved bound also keeps exact source
        // endpoints inside a bounded native spline's original parameter domain.
        Interval::new(
            image.lo.max(self.target[0].min(self.target[1])),
            image.hi.min(self.target[0].max(self.target[1])),
        )
    }
    fn preimage(self, value: f64) -> Result<Interval, GeometryError> {
        if self.identity {
            return Ok(Interval::point(value));
        }
        let normalized = Interval::point(value)
            .sub(Interval::point(self.target[0]))
            .div(Interval::point(self.target[1]).sub(Interval::point(self.target[0])))?;
        let value = Interval::point(self.source[0]).add(
            normalized.mul(Interval::point(self.source[1]).sub(Interval::point(self.source[0]))),
        );
        Interval::new(value.lo.max(self.source[0]), value.hi.min(self.source[1]))
    }
}

#[derive(Clone, Copy)]
struct Span {
    range: [f64; 2],
    endpoints: [[Interval; 3]; 2],
    depth: usize,
}
const EMPTY_SPAN: Span = Span {
    range: [0.0; 2],
    endpoints: [[Interval { lo: 0.0, hi: 0.0 }; 3]; 2],
    depth: 0,
};
const STACK_BYTES: usize = size_of::<[Span; MAX_DEPTH + 1]>();

fn certify(
    input: Correspondence<'_>,
    policy: ProofPolicy,
    retained: Option<&model::PreparedValidatedCurve<'static>>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    mut diagnostic: Option<&mut CorrespondenceDiagnostic>,
) -> Result<Option<f64>, GeometryError> {
    let Correspondence {
        curve,
        surface,
        chart,
        maximum_error_mm: tolerance,
        ..
    } = input;
    work.charge(1, cancel)?;
    let Some((a, b)) = curve.try_range_tuple() else {
        return Ok(None);
    };
    // A mapped original basis may have a larger natural domain than its retained
    // EDGE_CURVE restriction; conversely a periodic child is narrower than its
    // shared whole-original receipt. Both independent authorities must hold.
    let (a, b) = retained.map_or((a, b), |prepared| {
        let range = prepared.range();
        (a.max(range[0]), b.min(range[1]))
    });
    let Some((c, d)) = chart.try_range_tuple() else {
        return Ok(None);
    };
    Interval::new(a, b)?;
    Interval::new(c, d)?;
    if a == b || c == d {
        return Ok(None);
    }
    let mut basis = curve;
    for depth in 0..=MAX_WRAPPER_DEPTH {
        work.charge(1, cancel)?;
        if let model::Curve::TrimmedCurve(wrapped) = basis {
            if depth == MAX_WRAPPER_DEPTH {
                return Err(resource(
                    "native correspondence basis nesting exceeds source limit",
                ));
            }
            basis = wrapped.entity().curve().as_ref();
        } else {
            break;
        }
    }
    let source = Source {
        original: curve,
        implicit: if let Some(prepared) = retained {
            Some(PreparedCurveEnclosure::borrowed_restricted(
                prepared,
                1.0,
                (a, b),
            ))
        } else if matches!(basis, model::Curve::IntersectionCurve(_)) {
            Some(parametric::prepare_curve_enclosure(
                curve,
                (a, b),
                MAX_SCRATCH_BYTES - SCRATCH_OVERHEAD_BYTES,
                work,
                cancel,
            )?)
        } else {
            None
        },
    };
    let retained = source
        .implicit
        .as_ref()
        .map_or(0, PreparedCurveEnclosure::retained_bytes);
    let available = MAX_SCRATCH_BYTES
        .checked_sub(SCRATCH_OVERHEAD_BYTES)
        .and_then(|bytes| bytes.checked_sub(retained))
        .ok_or_else(|| resource("native pcurve source capture exhausts scratch capacity"))?;
    let mut cuts = Vec::new();
    let mut nodes = 0;
    let targets = [[c, d], [d, c]];
    let target_count = if policy == ProofPolicy::EitherPosition {
        2
    } else {
        1
    };
    for &target in &targets[..target_count] {
        let map = AffineMap::new([a, b], target)?;
        let mut harmonic_diagnostic = super::harmonic::HarmonicDiagnostic::new(tolerance);
        let harmonic_started = work.consumed();
        if source.implicit.is_none()
            && let Some(bound) = resolution(super::harmonic::certify(
                super::harmonic::HarmonicComparison {
                    curve: source.original,
                    surface,
                    chart,
                    range: [a, b],
                    target,
                    margin: policy.margin_uv(),
                    tolerance,
                    require_tangent: matches!(policy, ProofPolicy::DeclaredCoedge(_)),
                },
                work,
                cancel,
                &mut harmonic_diagnostic,
            )
            .map_err(|cause| {
                if !terminal(&cause) {
                    return cause;
                }
                GeometryError::new(
                    cause.code,
                    format!(
                        "whole-correspondence harmonic source=[{a:e},{b:e}] target={target:?} work={}: {harmonic_diagnostic}: {}",
                        work.consumed() - harmonic_started, cause.message,
                    ),
                )
            }))?
        {
            return Ok(Some(bound));
        }
        let harmonic_work = work.consumed() - harmonic_started;
        let fallback_started = work.consumed();
        cuts.clear();
        let result = (|| {
            add_cut(&mut cuts, a, available, work, cancel)?;
            add_cut(&mut cuts, b, available, work, cancel)?;
            if let Some(prepared) = &source.implicit {
                for (low, high) in prepared.parameter_intervals() {
                    add_cut(&mut cuts, low, available, work, cancel)?;
                    add_cut(&mut cuts, high, available, work, cancel)?;
                }
            } else {
                let curve = source.original;
                visit_curve_cuts(curve, &mut |value| {
                    work.charge(1, cancel)?;
                    let low = value.lo.max(a);
                    let high = value.hi.min(b);
                    if low <= high {
                        add_cut(&mut cuts, low, available, work, cancel)?;
                        add_cut(&mut cuts, high, available, work, cancel)?;
                    }
                    Ok(())
                })?;
            }
            visit_chart_cuts(chart, &mut |value| {
                work.charge(1, cancel)?;
                let low = value.lo.max(c);
                let high = value.hi.min(d);
                if low <= high {
                    // Nonrepresentable inverse knot images remain an interval.
                    // Both ends are split, never rounded into a guessed exact
                    // joint. A nonsmooth band must still pass native jets or
                    // remain unresolved; no interpolation crosses a hidden kink.
                    let joint = map.preimage(low)?.hull(map.preimage(high)?);
                    add_cut(&mut cuts, joint.lo, available, work, cancel)?;
                    add_cut(&mut cuts, joint.hi, available, work, cancel)?;
                }
                Ok(())
            })?;
            // Bound sort work as well as native evaluations before doing it.
            let levels = usize::BITS - cuts.len().leading_zeros();
            work.charge((cuts.len() as u64) * u64::from(levels), cancel)?;
            cuts.sort_unstable_by(f64::total_cmp);
            cuts.dedup();
            verify(
                &MappedCorrespondence {
                    source: &source,
                    surface,
                    chart,
                    map,
                    policy,
                },
                &cuts,
                tolerance,
                &mut nodes,
                work,
                cancel,
            )
        })();
        let fallback_work = work.consumed() - fallback_started;
        if let Some(diagnostic) = diagnostic.as_deref_mut() {
            diagnostic.record(FallbackDiagnostic {
                source: diagnostic.active_source,
                harmonic: harmonic_diagnostic,
                harmonic_work,
                fallback_work,
                nodes,
                source_range: [a, b],
                target_range: target,
                outcome: match &result {
                    Ok(Some(_)) => "proved",
                    Ok(None) => "unresolved",
                    Err(_) => "error",
                },
            });
        }
        let result = result.map_err(|cause: GeometryError| {
            if !terminal(&cause) {
                return cause;
            }
            GeometryError::new(
                cause.code,
                format!(
                    "whole-correspondence interval fallback source=[{a:e},{b:e}] target={target:?} harmonic_work={harmonic_work} ({harmonic_diagnostic}) fallback_work={} nodes={nodes}: {}",
                    work.consumed() - fallback_started, cause.message,
                ),
            )
        });
        if let Some(bound) = resolution(result)? {
            return Ok(Some(bound));
        }
    }
    Ok(None)
}

fn add_cut(
    cuts: &mut Vec<f64>,
    value: f64,
    bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    work.charge(1, cancel)?;
    if !value.is_finite() {
        return Err(GeometryError::new(
            GeometryErrorCode::ConditioningFailure,
            "nonfinite native smooth-span boundary",
        ));
    }
    if cuts.len() >= MAX_CUTS {
        return Err(resource("native pcurve smooth-span count exceeds cap"));
    }
    if cuts.len() == cuts.capacity() {
        let capacity = cuts.capacity().max(8).saturating_mul(2).min(MAX_CUTS);
        if capacity > bytes / size_of::<f64>() {
            return Err(resource(
                "native pcurve smooth-span storage exceeds scratch capacity",
            ));
        }
        work.charge((capacity - cuts.capacity()) as u64, cancel)?;
        cuts.try_reserve_exact(capacity - cuts.len())
            .map_err(|_| resource("native pcurve smooth-span storage allocation refused"))?;
    }
    cuts.push(value);
    Ok(())
}

const MAX_WRAPPER_DEPTH: usize = 32;

/// An original knot is transported through retained wrapper senses, never by
/// rounding a reflected knot into an asserted exact smooth-piece boundary.
/// Poses do not change parameter knots; the native jets still evaluate their
/// actual affine matrices and complete borrowed bases.
struct CutMap {
    reflected_ranges: [[f64; 2]; MAX_WRAPPER_DEPTH],
    reflections: usize,
    depth: usize,
}
impl CutMap {
    const IDENTITY: Self = Self {
        reflected_ranges: [[0.0; 2]; MAX_WRAPPER_DEPTH],
        reflections: 0,
        depth: 0,
    };
    fn image(&self, mut value: Interval) -> Result<Interval, GeometryError> {
        Interval::new(value.lo, value.hi)?;
        for range in self.reflected_ranges[..self.reflections].iter().rev() {
            value = Interval::point(range[0])
                .add(Interval::point(range[1]))
                .sub(value);
        }
        Interval::new(value.lo, value.hi)
    }
    fn enter(&mut self, range: (f64, f64), orientation: bool) -> Result<(), GeometryError> {
        Interval::new(range.0, range.1)?;
        if self.depth == MAX_WRAPPER_DEPTH {
            return Err(resource("native pcurve basis-wrapper nesting exceeds cap"));
        }
        self.depth += 1;
        if !orientation {
            self.reflected_ranges[self.reflections] = [range.0, range.1];
            self.reflections += 1;
        }
        Ok(())
    }
}
// A curve wrapper may enter its parameter-curve chart with the same fixed map.
const SCRATCH_OVERHEAD_BYTES: usize = STACK_BYTES
    + 2 * size_of::<CutMap>()
    + super::harmonic::SCRATCH_BYTES
    + size_of::<super::harmonic::HarmonicDiagnostic>()
    + size_of::<CorrespondenceDiagnostic>();

fn visit_chart_cuts(
    chart: &model::Curve2D,
    visit: &mut impl FnMut(Interval) -> Result<(), GeometryError>,
) -> Result<(), GeometryError> {
    visit_chart_cuts_mapped(chart, CutMap::IDENTITY, visit)
}
fn visit_chart_cuts_mapped(
    mut chart: &model::Curve2D,
    mut map: CutMap,
    visit: &mut impl FnMut(Interval) -> Result<(), GeometryError>,
) -> Result<(), GeometryError> {
    loop {
        match chart {
            model::Curve2D::BsplineCurve(c) => {
                for &value in c.knot_vector().iter() {
                    visit(map.image(Interval::point(value))?)?;
                }
            }
            model::Curve2D::NurbsCurve(c) => {
                for &value in c.knot_vector().iter() {
                    visit(map.image(Interval::point(value))?)?;
                }
            }
            model::Curve2D::Polyline(c) => {
                for index in 0..c.len() {
                    visit(map.image(Interval::point(index as f64))?)?;
                }
            }
            model::Curve2D::TrimmedCurve(c) => {
                let range = c.try_range_tuple().ok_or_else(|| {
                    GeometryError::new(
                        GeometryErrorCode::ConditioningFailure,
                        "unbounded native pcurve basis wrapper",
                    )
                })?;
                visit(map.image(Interval::point(range.0))?)?;
                visit(map.image(Interval::point(range.1))?)?;
                map.enter(range, c.orientation())?;
                chart = c.entity().curve().as_ref();
                continue;
            }
            model::Curve2D::Line(_) | model::Curve2D::Conic(_) => {}
        }
        return Ok(());
    }
}
fn visit_curve_cuts(
    mut curve: &model::Curve,
    visit: &mut impl FnMut(Interval) -> Result<(), GeometryError>,
) -> Result<(), GeometryError> {
    let mut map = CutMap::IDENTITY;
    loop {
        match curve {
            model::Curve::BsplineCurve(c) => {
                for &value in c.knot_vector().iter() {
                    visit(map.image(Interval::point(value))?)?;
                }
            }
            model::Curve::NurbsCurve(c) => {
                for &value in c.knot_vector().iter() {
                    visit(map.image(Interval::point(value))?)?;
                }
            }
            model::Curve::ParameterCurve(c) => {
                return visit_chart_cuts_mapped(c.curve(), map, visit);
            }
            model::Curve::TrimmedCurve(c) => {
                let range = c.try_range_tuple().ok_or_else(|| {
                    GeometryError::new(
                        GeometryErrorCode::ConditioningFailure,
                        "unbounded native spatial-curve basis wrapper",
                    )
                })?;
                visit(map.image(Interval::point(range.0))?)?;
                visit(map.image(Interval::point(range.1))?)?;
                map.enter(range, c.orientation())?;
                curve = c.entity().curve().as_ref();
                continue;
            }
            model::Curve::Line(_) | model::Curve::IntersectionCurve(_) => {}
        }
        return Ok(());
    }
}

fn chart_jet(
    chart: &model::Curve2D,
    range: Interval,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<ChartCurveEnclosure, GeometryError> {
    check_cancel(cancel)?;
    let mut native = model::ValidatedGeometryBudget::new(
        usize::try_from(work.remaining()).unwrap_or(usize::MAX),
    );
    let result = model::validated_curve2d_jet(chart, [range.lo, range.hi], &mut native, cancel);
    work.charge(native.consumed() as u64, &AtomicBool::new(false))?;
    check_cancel(cancel)?;
    let jet = result.map_err(parametric::native_error)?;
    let adapt = |v: model::ValidatedInterval| Interval {
        lo: v.lower,
        hi: v.upper,
    };
    Ok(ChartCurveEnclosure {
        position: jet.position.map(adapt),
        derivative: jet.first.map(adapt),
        second_derivative: jet.second.map(adapt),
    })
}
fn composed_surface(
    surface: &model::Surface,
    uv: &ChartCurveEnclosure,
    margin_uv: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<parametric::SurfaceEnclosure, GeometryError> {
    let domain = std::array::from_fn(|axis| {
        let enclosure = uv.position[axis].add(Interval {
            lo: -margin_uv[axis],
            hi: margin_uv[axis],
        });
        [enclosure.lo, enclosure.hi]
    });
    parametric::surface_enclosure(surface, domain, work, cancel)
}
struct MappedCorrespondence<'a, 'source> {
    source: &'a Source<'source>,
    surface: &'a model::Surface,
    chart: &'a model::Curve2D,
    map: AffineMap,
    policy: ProofPolicy,
}

fn residual(
    input: &MappedCorrespondence<'_, '_>,
    t: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[Interval; 3], GeometryError> {
    let MappedCorrespondence {
        source,
        surface,
        chart,
        map,
        policy,
    } = *input;
    let margin_uv = policy.margin_uv();
    let c = source.endpoint(t, work, cancel)?;
    let uv = chart_jet(chart, map.image([t, t])?, work, cancel)?;
    let s = composed_surface(surface, &uv, margin_uv, work, cancel)?;
    Ok(std::array::from_fn(|axis| c[axis].sub(s.position[axis])))
}

fn chart_span_smooth(
    chart: &model::Curve2D,
    range: Interval,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let mut smooth = true;
    visit_chart_cuts(chart, &mut |cut| {
        work.charge(1, cancel)?;
        if cut.lo < range.hi && range.lo < cut.hi {
            smooth = false;
        }
        Ok(())
    })?;
    Ok(smooth)
}
fn surface_span_smooth(
    surface: &model::Surface,
    uv: &ChartCurveEnclosure,
    margin_uv: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let domain = std::array::from_fn(|axis| {
        let coordinate = uv.position[axis].add(Interval {
            lo: -margin_uv[axis],
            hi: margin_uv[axis],
        });
        // A genuinely constant coordinate does not cross knots while the other
        // coordinate varies. This includes a fixed uncertain raw chart offset.
        if uv.derivative[axis].lo == 0.0
            && uv.derivative[axis].hi == 0.0
            && uv.second_derivative[axis].lo == 0.0
            && uv.second_derivative[axis].hi == 0.0
        {
            [coordinate.mid(); 2]
        } else {
            [coordinate.lo, coordinate.hi]
        }
    });
    parametric::surface_parameter_span_smooth(surface, domain, work, cancel)
}
fn source_span_smooth(
    source: &Source<'_>,
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    if let Some(prepared) = &source.implicit {
        return prepared.is_smooth_on((range[0], range[1]), work, cancel);
    }
    let original = source.original;
    let mut smooth = true;
    visit_curve_cuts(original, &mut |cut| {
        work.charge(1, cancel)?;
        if cut.lo < range[1] && range[0] < cut.hi {
            smooth = false;
        }
        Ok(())
    })?;
    if !smooth {
        return Ok(false);
    }
    let mut curve = original;
    let mut parameter = Interval::new(range[0], range[1])?;
    for depth in 0..=MAX_WRAPPER_DEPTH {
        work.charge(1, cancel)?;
        match curve {
            model::Curve::TrimmedCurve(wrapped) => {
                if depth == MAX_WRAPPER_DEPTH {
                    return Err(resource("native smooth-span nesting exceeds source limit"));
                }
                if !wrapped.orientation() {
                    let (a, b) = wrapped.range_tuple();
                    parameter = Interval::point(a).add(Interval::point(b)).sub(parameter);
                }
                curve = wrapped.entity().curve().as_ref();
            }
            model::Curve::ParameterCurve(composed) => {
                if !chart_span_smooth(composed.curve(), parameter, work, cancel)? {
                    return Ok(false);
                }
                let uv = chart_jet(composed.curve(), parameter, work, cancel)?;
                return surface_span_smooth(
                    composed.surface().as_ref(),
                    &uv,
                    [0.0; 2],
                    work,
                    cancel,
                );
            }
            model::Curve::IntersectionCurve(_) => return Ok(false),
            _ => return Ok(true),
        }
    }
    Ok(false)
}
struct CellEnclosure {
    second_residual: [Interval; 3],
    tangents: Option<([Interval; 3], [Interval; 3])>,
    position_residual: [Interval; 3],
    smooth: bool,
}

fn cell_enclosure(
    input: &MappedCorrespondence<'_, '_>,
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<CellEnclosure, GeometryError> {
    let MappedCorrespondence {
        source,
        surface,
        chart,
        map,
        policy,
    } = *input;
    let margin_uv = policy.margin_uv();
    let require_tangent = matches!(policy, ProofPolicy::DeclaredCoedge(_));
    let c = source.jet(range, work, cancel)?;
    let chart_range = map.image(range)?;
    let uv = chart_jet(chart, chart_range, work, cancel)?;
    let s = composed_surface(surface, &uv, margin_uv, work, cancel)?;
    let [du, dv] = uv.derivative;
    let [ddu, ddv] = uv.second_derivative;
    let slope_squared = map.slope.square();
    let second_residual = std::array::from_fn(|axis| {
        let composed = s.duu[axis]
            .mul(du.square())
            .add(s.duv[axis].mul(du).mul(dv).mul(Interval::point(2.0)))
            .add(s.dvv[axis].mul(dv.square()))
            .add(s.du[axis].mul(ddu))
            .add(s.dv[axis].mul(ddv))
            .mul(slope_squared);
        // Fold the vector difference before taking a norm. Constant/common
        // derivative components therefore cancel instead of adding their norms.
        c.second_derivative[axis].sub(composed)
    });
    let tangents = require_tangent.then(|| {
        let chart_tangent =
            std::array::from_fn(|axis| s.du[axis].mul(du).add(s.dv[axis].mul(dv)).mul(map.slope));
        (c.derivative, chart_tangent)
    });
    let smooth = source_span_smooth(source, range, work, cancel)?
        && chart_span_smooth(chart, chart_range, work, cancel)?
        && surface_span_smooth(surface, &uv, margin_uv, work, cancel)?;
    Ok(CellEnclosure {
        second_residual,
        tangents,
        position_residual: std::array::from_fn(|axis| c.position[axis].sub(s.position[axis])),
        smooth,
    })
}
fn norm(vector: [Interval; 3]) -> Result<Interval, GeometryError> {
    let squared = vector
        .into_iter()
        .fold(Interval::point(0.0), |sum, component| {
            sum.add(component.square())
        });
    Interval {
        lo: squared.lo.max(0.0),
        hi: squared.hi,
    }
    .sqrt()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TangentAgreement {
    Proved,
    Disproved,
    Unresolved,
}

fn relative_tangent_agreement(
    source: [Interval; 3],
    chart: [Interval; 3],
) -> Result<TangentAgreement, GeometryError> {
    let scale = source
        .into_iter()
        .chain(chart)
        .map(Interval::abs_upper)
        .fold(0.0, f64::max);
    if !scale.is_finite() {
        return Err(GeometryError::new(
            GeometryErrorCode::ConditioningFailure,
            "native coedge tangent enclosure is not finite",
        ));
    }
    if scale == 0.0 {
        return Ok(TangentAgreement::Unresolved);
    }
    // Direct outward division is essential: forming 1/scale would overflow on
    // subnormal speeds even when the normalized coordinates are well bounded.
    let normalize = |value: Interval| Interval {
        lo: parametric::down(value.lo / scale),
        hi: parametric::up(value.hi / scale),
    };
    let source = source.map(normalize);
    let chart = chart.map(normalize);
    let difference = std::array::from_fn(|axis| source[axis].sub(chart[axis]));
    let speed = norm(source)?;
    let error = norm(difference)?;
    let required = Interval::point(speed.lo).mul(Interval::point(0.25)).lo;
    if speed.lo > 0.0 && error.hi < required {
        return Ok(TangentAgreement::Proved);
    }
    let impossible = Interval::point(speed.hi).mul(Interval::point(0.25)).hi;
    if error.lo >= impossible {
        return Ok(TangentAgreement::Disproved);
    }
    Ok(TangentAgreement::Unresolved)
}

fn verify(
    input: &MappedCorrespondence<'_, '_>,
    cuts: &[f64],
    tolerance: f64,
    nodes: &mut usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<f64>, GeometryError> {
    let mut pending = [EMPTY_SPAN; MAX_DEPTH + 1];
    let mut achieved: f64 = 0.0;
    let mut front = residual(input, cuts[0], work, cancel)?;
    for pair in cuts.windows(2) {
        let back = residual(input, pair[1], work, cancel)?;
        pending[0] = Span {
            range: [pair[0], pair[1]],
            endpoints: [front, back],
            depth: 0,
        };
        front = back;
        let mut length = 1;
        while length != 0 {
            work.charge(1, cancel)?;
            if *nodes >= MAX_NODES {
                return Err(resource("native pcurve interval proof node cap exhausted"));
            }
            *nodes += 1;
            length -= 1;
            let span = pending[length];
            let endpoints = [norm(span.endpoints[0])?, norm(span.endpoints[1])?];
            if endpoints.iter().any(|bound| bound.lo > tolerance) {
                return Ok(None);
            }
            let attempt = cell_enclosure(input, span.range, work, cancel).and_then(|cell| {
                let tangent = match cell.tangents {
                    Some((source, chart)) => relative_tangent_agreement(source, chart)?,
                    None => TangentAgreement::Proved,
                };
                let bound = if cell.smooth {
                    let width = Interval::point(span.range[1]).sub(Interval::point(span.range[0]));
                    Interval::point(endpoints[0].hi.max(endpoints[1].hi))
                        .add(
                            Interval::point(norm(cell.second_residual)?.hi)
                                .mul(width.square())
                                .mul(Interval::point(0.125)),
                        )
                        .hi
                } else {
                    // A hull of piecewise second derivatives is NOT a
                    // Taylor bound across a C0 knot or uncertain knot band.
                    // This complete pointwise box is valid even for a jump.
                    norm(cell.position_residual)?.hi
                };
                Ok((bound, tangent))
            });
            match attempt {
                Ok((bound, tangent)) => {
                    if tangent == TangentAgreement::Disproved {
                        return Ok(None);
                    }
                    if bound <= tolerance && tangent == TangentAgreement::Proved {
                        achieved = achieved.max(bound);
                        continue;
                    }
                }
                Err(error) if terminal(&error) => return Err(error),
                Err(error)
                    if !matches!(
                        error.code,
                        GeometryErrorCode::ConditioningFailure
                            | GeometryErrorCode::AnalysisIndeterminate
                            | GeometryErrorCode::SingularGeometry
                    ) =>
                {
                    return Ok(None);
                }
                Err(_) => {}
            }
            let middle = span.range[0] * 0.5 + span.range[1] * 0.5;
            if span.depth == MAX_DEPTH || middle <= span.range[0] || middle >= span.range[1] {
                return Ok(None);
            }
            let midpoint = residual(input, middle, work, cancel)?;
            if norm(midpoint)?.lo > tolerance {
                return Ok(None);
            }
            work.charge(2, cancel)?;
            // Depth-first traversal needs at most depth+1 slots; storage is
            // fixed before the first split, independent of input complexity.
            pending[length] = Span {
                range: [middle, span.range[1]],
                endpoints: [midpoint, span.endpoints[1]],
                depth: span.depth + 1,
            };
            pending[length + 1] = Span {
                range: [span.range[0], middle],
                endpoints: [span.endpoints[0], midpoint],
                depth: span.depth + 1,
            };
            length += 2;
        }
    }
    Ok(Some(achieved))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane() -> model::Surface {
        model::Surface::Plane(model::Plane::new(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
        ))
    }
    fn prove(curve: &model::Curve, chart: &model::Curve2D) -> Option<f64> {
        certify_curve_on_surface(
            curve,
            &plane(),
            chart,
            1.0e-7,
            &mut GeometryWorkBudget::new(MAX_WORK),
            &AtomicBool::new(false),
        )
        .unwrap()
    }
    fn knots(a: f64, b: f64, degree: usize) -> model::KnotVector {
        model::KnotVector::from(
            std::iter::repeat_n(a, degree + 1)
                .chain(std::iter::repeat_n(b, degree + 1))
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn line_independent_ranges_and_reversed_pairing_are_verified() {
        let curve = model::Curve::BsplineCurve(model::BsplineCurve::new(
            knots(10.0, 12.0, 1),
            vec![Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0)],
        ));
        let chart = model::Curve2D::Line(model::Line(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0)));
        assert!(prove(&curve, &chart).is_some_and(|bound| bound <= 1.0e-7));
        let reversed =
            model::Curve2D::Line(model::Line(Point2::new(1.0, 0.0), Point2::new(0.0, 0.0)));
        assert!(prove(&curve, &reversed).is_some());
        assert!(declared(&curve, &plane(), &chart, 1.0e-7).is_some());
        assert!(declared(&curve, &plane(), &reversed, 1.0e-7).is_none());
        assert_eq!(curve.range_tuple(), (10.0, 12.0));
        assert_eq!(chart.range_tuple(), (0.0, 1.0));
    }

    #[test]
    fn analytic_conic_has_whole_interval_correspondence() {
        let curve = model::Curve::BsplineCurve(model::BsplineCurve::new(
            knots(0.0, 1.0, 2),
            vec![
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(0.0, 1.0, 0.0),
                Point3::new(1.0, 2.0, 0.0),
            ],
        ));
        let chart = model::Curve2D::Conic(model::Conic2D::Parabola(model::Processor::new(
            model::TrimmedCurve::new(model::UnitParabola::<Point2>::new(), (0.0, 1.0)),
        )));
        assert!(prove(&curve, &chart).is_some());
    }

    #[test]
    fn native_rational_pcurve_is_not_replaced_by_a_sampled_alias() {
        let controls = [[1.0, 0.0, 1.0], [0.5, 0.5, 0.5], [0.0, 1.0, 1.0]];
        let curve = model::Curve::NurbsCurve(model::NurbsCurve::new(model::BsplineCurve::new(
            knots(0.0, 1.0, 2),
            controls
                .iter()
                .map(|p| Vector4::new(p[0], p[1], 0.0, p[2]))
                .collect(),
        )));
        let chart = model::Curve2D::NurbsCurve(model::NurbsCurve::new(model::BsplineCurve::new(
            knots(0.0, 1.0, 2),
            controls
                .iter()
                .map(|p| Vector3::new(p[0], p[1], p[2]))
                .collect(),
        )));
        assert!(prove(&curve, &chart).is_some());
        let wrong = model::Curve2D::Line(model::Line(Point2::new(1.0, 0.0), Point2::new(0.0, 1.0)));
        assert!(prove(&curve, &wrong).is_none());
    }

    #[test]
    fn endpoint_and_midpoint_alias_does_not_certify_an_independent_parameter() {
        let curve = model::Curve::Line(model::Line(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
        ));
        let chart = model::Curve2D::BsplineCurve(model::BsplineCurve::new(
            knots(4.0, 6.0, 3),
            vec![
                Point2::new(0.0, 0.0),
                Point2::new(0.75, 0.0),
                Point2::new(0.25, 0.0),
                Point2::new(1.0, 0.0),
            ],
        ));
        // The normalized endpoints and midpoint match, but the actual affine
        // correspondence differs in the interior. No nonlinear inverse guessed.
        assert!(prove(&curve, &chart).is_none());
    }

    #[test]
    fn c0_native_knots_split_before_second_order_interpolation() {
        let knot = model::KnotVector::from(vec![0.0, 0.0, 0.37, 1.0, 1.0]);
        let curve = model::Curve::BsplineCurve(model::BsplineCurve::new(
            knot.clone(),
            vec![
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(1.0, 1.0, 0.0),
                Point3::new(2.0, 0.0, 0.0),
            ],
        ));
        let chart = model::Curve2D::BsplineCurve(model::BsplineCurve::new(
            knot,
            vec![
                Point2::new(0.0, 0.0),
                Point2::new(1.0, 1.0),
                Point2::new(2.0, 0.0),
            ],
        ));
        assert!(prove(&curve, &chart).is_some());
    }

    #[test]
    fn c0_carrier_bumps_cannot_use_a_hull_of_piecewise_zero_hessians() {
        let values = [0.0, 0.0, 0.01, 0.0, 0.0, 0.0, -0.01, 0.0, 0.0];
        let mut uknots = vec![0.0, 0.0];
        uknots.extend((1..8).map(|i| i as f64 / 8.0));
        uknots.extend([1.0, 1.0]);
        let surface = model::Surface::BsplineSurface(model::BsplineSurface::new(
            (model::KnotVector::from(uknots), knots(0.0, 1.0, 1)),
            values
                .into_iter()
                .enumerate()
                .map(|(i, y)| {
                    vec![
                        Point3::new(i as f64 / 8.0, y, 0.0),
                        Point3::new(i as f64 / 8.0, y + 1.0, 0.0),
                    ]
                })
                .collect(),
        ));
        let curve = model::Curve::Line(model::Line(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
        ));
        let chart = model::Curve2D::Line(model::Line(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0)));
        // Both endpoints AND the midpoint agree. Every open piece has zero
        // Hessian and tangent discrepancy only .08 < 1/4, but quarter points
        // are physically wrong by .01 mm. Piecewise derivative hulls do not
        // establish the missing Taylor continuity premise.
        assert!(declared(&curve, &surface, &chart, 1.0e-7).is_none());
    }

    #[test]
    fn declared_raw_chart_offset_uncertainty_is_in_the_physical_bound() {
        let curve = model::Curve::Line(model::Line(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
        ));
        let chart = model::Curve2D::Line(model::Line(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0)));
        let certify = |margin| {
            certify_declared_coedge_correspondence(
                Correspondence {
                    curve: &curve,
                    surface: &plane(),
                    chart: &chart,
                    maximum_error_mm: 1.0e-7,
                    margin_uv: [0.0, margin],
                },
                &mut GeometryWorkBudget::new(MAX_WORK),
                &AtomicBool::new(false),
                None,
            )
            .unwrap()
        };
        let bound = certify(1.0e-9).unwrap();
        assert!((1.0e-9..=1.0e-7).contains(&bound));
        assert!(certify(1.0e-3).is_none());
    }

    #[test]
    fn cancellation_and_exhausted_work_stay_typed_and_failed_work_is_debited() {
        let curve = model::Curve::Line(model::Line(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
        ));
        let chart = model::Curve2D::Line(model::Line(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0)));
        let mut work = GeometryWorkBudget::new(MAX_WORK);
        assert_eq!(
            certify_curve_on_surface(
                &curve,
                &plane(),
                &chart,
                1.0e-7,
                &mut work,
                &AtomicBool::new(true)
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::Cancelled
        );
        let mut work = GeometryWorkBudget::new(8);
        assert_eq!(
            certify_curve_on_surface(
                &curve,
                &plane(),
                &chart,
                1.0e-7,
                &mut work,
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::ResourceLimit
        );
        assert!(work.consumed() > 0);
    }

    #[test]
    fn scratch_growth_is_refused_before_allocation() {
        let mut cuts = Vec::new();
        assert_eq!(
            add_cut(
                &mut cuts,
                0.0,
                0,
                &mut GeometryWorkBudget::new(100),
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::ResourceLimit
        );
        assert_eq!(cuts.capacity(), 0);
    }

    #[test]
    fn retained_full_basis_trim_sense_and_pose_are_evaluated_natively() {
        let basis3 = model::Curve::BsplineCurve(model::BsplineCurve::new(
            knots(0.0, 1.0, 2),
            vec![
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(0.5, 1.0, 0.0),
                Point3::new(1.0, 0.0, 0.0),
            ],
        ));
        let basis2 = model::Curve2D::BsplineCurve(model::BsplineCurve::new(
            knots(0.0, 1.0, 2),
            vec![
                Point2::new(0.0, 0.0),
                Point2::new(0.5, 1.0),
                Point2::new(1.0, 0.0),
            ],
        ));
        let mut spatial = model::Processor::with_transform(
            model::TrimmedCurve::new(Box::new(basis3), (0.2, 0.8)),
            Matrix4::from_translation(Vector3::new(3.0, 4.0, 0.0)),
        );
        let mut planar = model::Processor::with_transform(
            model::TrimmedCurve::new(Box::new(basis2), (0.2, 0.8)),
            Matrix3::from_translation(Vector2::new(3.0, 4.0)),
        );
        spatial.invert();
        planar.invert();
        let curve = model::Curve::TrimmedCurve(spatial);
        let chart = model::Curve2D::TrimmedCurve(planar);
        assert_eq!(curve.range_tuple(), (0.2, 0.8));
        assert_eq!(chart.range_tuple(), (0.2, 0.8));
        assert!(prove(&curve, &chart).is_some());
    }

    #[test]
    fn reflected_inner_c0_knots_retain_original_joint_coverage() {
        let basis = model::Curve2D::BsplineCurve(model::BsplineCurve::new(
            model::KnotVector::from(vec![0.0, 0.0, 0.25, 1.0, 1.0]),
            vec![
                Point2::new(0.0, 0.0),
                Point2::new(1.0, 1.0),
                Point2::new(2.0, 0.0),
            ],
        ));
        let mut wrapper = model::Processor::<_, Matrix3>::new(model::TrimmedCurve::new(
            Box::new(basis),
            (0.125, 0.875),
        ));
        wrapper.invert();
        let mut found = false;
        visit_chart_cuts(&model::Curve2D::TrimmedCurve(wrapper), &mut |cut| {
            if cut.lo <= 0.75 && cut.hi >= 0.75 {
                found = true;
            }
            Ok(())
        })
        .unwrap();
        assert!(found);
    }

    fn declared(
        curve: &model::Curve,
        surface: &model::Surface,
        chart: &model::Curve2D,
        tolerance: f64,
    ) -> Option<f64> {
        certify_declared_coedge_correspondence(
            Correspondence {
                curve,
                surface,
                chart,
                maximum_error_mm: tolerance,
                margin_uv: [0.0; 2],
            },
            &mut GeometryWorkBudget::new(MAX_WORK),
            &AtomicBool::new(false),
            None,
        )
        .unwrap()
    }

    #[test]
    fn declared_independent_ranges_need_the_actual_forward_affine_derivative() {
        let curve = model::Curve::BsplineCurve(model::BsplineCurve::new(
            knots(10.0, 12.0, 1),
            vec![Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0)],
        ));
        let correct =
            model::Curve2D::Line(model::Line(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0)));
        assert!(declared(&curve, &plane(), &correct, 1.0e-7).is_some());
        let doubled =
            model::Curve2D::Line(model::Line(Point2::new(0.0, 0.0), Point2::new(2.0, 0.0)));
        // A permissive absolute error must not authorize the doubled tangent.
        assert!(
            certify_curve_on_surface(
                &curve,
                &plane(),
                &doubled,
                10.0,
                &mut GeometryWorkBudget::new(MAX_WORK),
                &AtomicBool::new(false)
            )
            .unwrap()
            .is_some()
        );
        assert!(declared(&curve, &plane(), &doubled, 10.0).is_none());
    }

    fn circular_source(radius: f64) -> (model::Curve, model::Surface) {
        let raw_plane = model::Surface::Plane(model::Plane::new(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(radius, 0.0, 0.0),
            Point3::new(0.0, radius, 0.0),
        ));
        let circle = model::Curve2D::Conic(model::Conic2D::Ellipse(
            model::Processor::<_, Matrix3>::new(model::TrimmedCurve::new(
                model::UnitCircle::<Point2>::new(),
                (0.0, std::f64::consts::TAU),
            )),
        ));
        let curve =
            model::Curve::ParameterCurve(model::ParameterCurve::new(circle, Box::new(raw_plane)));
        let profile = model::Curve::Line(model::Line(
            Point3::new(radius, 0.0, 0.0),
            Point3::new(radius, 0.0, radius),
        ));
        let surface = model::Surface::RevolutionSurface(model::Processor::new(
            model::RevolutionSurface::by_revolution(
                profile,
                Point3::new(0.0, 0.0, 0.0),
                Vector3::unit_z(),
            ),
        ));
        (curve, surface)
    }
    fn angular_line(slope: f64, range: (f64, f64)) -> model::Curve2D {
        model::Curve2D::TrimmedCurve(model::Processor::<_, Matrix3>::new(
            model::TrimmedCurve::new(
                Box::new(model::Curve2D::Line(model::Line(
                    Point2::new(0.0, 0.0),
                    Point2::new(0.0, slope),
                ))),
                range,
            ),
        ))
    }

    #[test]
    fn whole_original_harmonics_prove_physical_correspondence_with_bounded_work() {
        for radius in [6.0, 1.0e-200] {
            let (curve, mut surface) = circular_source(radius);
            let chart = angular_line(1.0, (0.0, std::f64::consts::TAU));
            let mut work = GeometryWorkBudget::new(50_000);
            assert!(
                certify_declared_coedge_correspondence(
                    Correspondence {
                        curve: &curve,
                        surface: &surface,
                        chart: &chart,
                        maximum_error_mm: 1e-7,
                        margin_uv: [0.0; 2],
                    },
                    &mut work,
                    &AtomicBool::new(false),
                    None,
                )
                .unwrap()
                .is_some()
            );
            // Same harmonic parameters on a different physical carrier do not
            // certify the original world curve, irrespective of its radius.
            surface.transform_by(Matrix4::from_translation(Vector3::unit_z()));
            let mut work = GeometryWorkBudget::new(50_000);
            assert!(
                certify_declared_coedge_correspondence(
                    Correspondence {
                        curve: &curve,
                        surface: &surface,
                        chart: &chart,
                        maximum_error_mm: 1e-7,
                        margin_uv: [0.0; 2],
                    },
                    &mut work,
                    &AtomicBool::new(false),
                    None,
                )
                .unwrap()
                .is_none()
            );
        }
    }

    #[test]
    fn posed_three_four_five_circle_encloses_original_native_evaluation_roundoff() {
        let tau = std::f64::consts::TAU;
        let center = Point3::new(10.0, -5.0, 3.0);
        let original = model::Curve::ParameterCurve(model::ParameterCurve::new(
            model::Curve2D::Conic(model::Conic2D::Ellipse(
                model::Processor::<_, Matrix3>::new(model::TrimmedCurve::new(
                    model::UnitCircle::<Point2>::new(),
                    (0.0, tau),
                )),
            )),
            Box::new(model::Surface::Plane(model::Plane::from_origin_vectors(
                center,
                Vector3::new(0.0, 5.0, 0.0),
                Vector3::new(-4.0, 0.0, 3.0),
            ))),
        ));
        let carrier = model::Surface::RevolutionSurface(model::Processor::new(
            model::RevolutionSurface::by_revolution(
                model::Curve::Line(model::Line(
                    center + Vector3::new(0.0, 5.0, 0.0),
                    center + Vector3::new(3.0, 5.0, 4.0),
                )),
                center,
                Vector3::new(3.0, 0.0, 4.0),
            ),
        ));
        let chart = angular_line(1.0, (0.0, tau));
        let error = certify_declared_coedge_correspondence(
            Correspondence {
                curve: &original,
                surface: &carrier,
                chart: &chart,
                maximum_error_mm: 1e-7,
                margin_uv: [0.0; 2],
            },
            &mut GeometryWorkBudget::new(50_000),
            &AtomicBool::new(false),
            None,
        )
        .unwrap()
        .unwrap();
        for index in 0..=32 {
            let angle = tau * f64::from(index) / 32.0;
            let expected = Point3::new(
                10.0 - 4.0 * angle.sin(),
                -5.0 + 5.0 * angle.cos(),
                3.0 + 3.0 * angle.sin(),
            );
            assert!((carrier.subs(0.0, angle) - expected).magnitude() <= error);
            assert!((original.subs(angle) - expected).magnitude() <= error);
        }
    }

    #[test]
    fn tiny_periodic_winding_alias_cannot_hide_inside_absolute_position_error() {
        for radius in [1.0e-10, 1.0e-200] {
            let (curve, surface) = circular_source(radius);
            let wrong = angular_line(2.0, (0.0, std::f64::consts::TAU));
            assert!(
                certify_curve_on_surface(
                    &curve,
                    &surface,
                    &wrong,
                    1.0e-7,
                    &mut GeometryWorkBudget::new(MAX_WORK),
                    &AtomicBool::new(false)
                )
                .unwrap()
                .is_some()
            );
            assert!(declared(&curve, &surface, &wrong, 1.0e-7).is_none());
            let correct = angular_line(1.0, (0.0, std::f64::consts::TAU));
            assert!(declared(&curve, &surface, &correct, 1.0e-7).is_some());
        }
    }

    #[test]
    fn independent_angular_range_accepts_only_its_verified_affine_scale() {
        let (curve, surface) = circular_source(1.0);
        let chart = angular_line(2.0, (0.0, std::f64::consts::PI));
        // q(t)=t/2 is proposed from the independent ranges, then both the
        // physical position and the complete composed tangent prove it.
        assert!(declared(&curve, &surface, &chart, 1.0e-7).is_some());
    }

    #[test]
    fn tangent_normalization_precedes_squaring_and_refuses_unproved_regularity() {
        let tiny = f64::from_bits(16);
        let exact = [
            Interval::point(tiny),
            Interval::point(0.0),
            Interval::point(0.0),
        ];
        assert!(relative_tangent_agreement(exact, exact).unwrap() == TangentAgreement::Proved);
        let uncertain = [
            Interval::new(-tiny, tiny).unwrap(),
            Interval::point(0.0),
            Interval::point(0.0),
        ];
        assert!(
            relative_tangent_agreement(uncertain, uncertain).unwrap()
                == TangentAgreement::Unresolved
        );
        let curve = model::Curve::Line(model::Line(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0e-300, 0.0, 0.0),
        ));
        let chart = model::Curve2D::Line(model::Line(
            Point2::new(0.0, 0.0),
            Point2::new(1.0e-300, 0.0),
        ));
        assert!(declared(&curve, &plane(), &chart, 1.0e-7).is_some());
    }

    #[test]
    fn retained_original_root_not_displaced_guide_owns_independent_uv_correspondence() {
        let xy = plane();
        let yz = model::Surface::Plane(model::Plane::new(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(0.0, 0.0, 1.0),
        ));
        let guide = model::Curve::Line(model::Line(
            Point3::new(0.05, 0.0, 0.02),
            Point3::new(0.05, 1.0, 0.02),
        ));
        let curve = model::Curve::IntersectionCurve(model::SurfaceCurve::with_boundaries(
            Box::new(xy.clone()),
            Box::new(yz),
            Box::new(guide),
            None,
            None,
        ));
        let cancel = AtomicBool::new(false);
        let prepared = model::prepared_validated_curve_owned(
            curve.clone(),
            [0.0, 1.0],
            &model::ValidatedCurveOptions::default(),
            &mut model::ValidatedGeometryBudget::new(MAX_WORK as usize),
            &cancel,
        )
        .unwrap();
        let chart = |x| {
            model::Curve2D::BsplineCurve(model::BsplineCurve::new(
                model::KnotVector::from(vec![10.0, 10.0, 20.0, 20.0]),
                vec![Point2::new(x, 0.0), Point2::new(x, 1.0)],
            ))
        };
        let mut work = GeometryWorkBudget::new(MAX_WORK);
        assert!(
            certify_retained_coedge_correspondence(
                Correspondence {
                    curve: &curve,
                    surface: &xy,
                    chart: &chart(0.0),
                    maximum_error_mm: 1.0e-7,
                    margin_uv: [0.0; 2],
                },
                &prepared,
                &mut work,
                &cancel
            )
            .unwrap()
            .is_some()
        );
        assert!(
            certify_retained_coedge_correspondence(
                Correspondence {
                    curve: &curve,
                    surface: &xy,
                    chart: &chart(0.05),
                    maximum_error_mm: 1.0e-7,
                    margin_uv: [0.0; 2],
                },
                &prepared,
                &mut work,
                &cancel
            )
            .unwrap()
            .is_none()
        );
    }
}
