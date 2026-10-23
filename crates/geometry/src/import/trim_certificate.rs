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

mod operand_binding;

const MAX_WORK: u64 = 16_000_000;
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

/// One terminal declined predicate; never a trace of adaptive cells.
#[derive(Default)]
struct CorrespondenceDecline {
    predicate: &'static str,
    stop: Option<&'static str>,
    range: Option<[f64; 2]>,
    depth: Option<usize>,
    endpoint_norms: Option<[[f64; 2]; 2]>,
    position_upper: Option<f64>,
    midpoint_norm: Option<[f64; 2]>,
    tangent: Option<TangentAgreement>,
    cause: Option<GeometryErrorCode>,
}

impl std::fmt::Debug for CorrespondenceDecline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CorrespondenceDecline")
            .field("predicate", &self.predicate)
            .field("stop", &self.stop)
            .field("range", &self.range)
            .field("depth", &self.depth)
            .field("endpoint_norms", &self.endpoint_norms)
            .field("position_upper", &self.position_upper)
            .field("midpoint_norm", &self.midpoint_norm)
            .field("tangent", &self.tangent)
            .field("cause", &self.cause)
            .finish()
    }
}

/// One current completed joint, never a history of proposals or proof cells.
#[derive(Debug, Default)]
pub(super) struct RetainedJointDiagnostic {
    pub phase: &'static str,
    pub phase_started_work: u64,
    pub allowed_native: f64,
    pub coefficient_margin_uv: [f64; 2],
    pub initial_achieved_native: Option<f64>,
    pub inverse_gain: Option<[f64; 2]>,
    pub metric: super::implicit::RefinementDiagnostic,
    pub refined_achieved_native: Option<f64>,
    pub final_margin_uv: Option<[f64; 2]>,
}

impl std::fmt::Display for RetainedJointDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "phase={} allowed_native={:e} coeff_uv={:?} base={:?} inverse={:?} B={:?} G={:?} D={:?} goal={:?} refined={:?} final_uv={:?}",
            self.phase,
            self.allowed_native,
            self.coefficient_margin_uv,
            self.initial_achieved_native,
            self.inverse_gain,
            self.metric.coefficient_radius,
            self.metric.residual_gain,
            self.metric.point_diameter_budget_native,
            self.metric.derived_limit,
            self.refined_achieved_native,
            self.final_margin_uv,
        )
    }
}

#[derive(Clone, Copy, Default)]
struct EvaluationCount {
    calls: u64,
    work: u64,
}
impl EvaluationCount {
    fn record(&mut self, work: u64) {
        self.calls += 1;
        self.work += work;
    }
}

/// Fixed failure-only counters for the current verify traversal, not a trace.
#[derive(Clone, Copy, Default)]
pub(super) struct EvaluationCost {
    source_endpoint: EvaluationCount,
    source_c1: EvaluationCount,
    source_jet: EvaluationCount,
    target_endpoint: EvaluationCount,
    target_cell: EvaluationCount,
    tangent_comparisons: u64,
    total_work: u64,
}
impl std::fmt::Debug for EvaluationCost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let accounted = self.source_endpoint.work + self.source_c1.work + self.source_jet.work
            + self.target_endpoint.work + self.target_cell.work;
        write!(f, "se={}/{},sc1={}/{},sj={}/{},te={}/{},tc={}/{},cmp={},other={:?}",
            self.source_endpoint.calls, self.source_endpoint.work,
            self.source_c1.calls, self.source_c1.work,
            self.source_jet.calls, self.source_jet.work,
            self.target_endpoint.calls, self.target_endpoint.work,
            self.target_cell.calls, self.target_cell.work,
            self.tangent_comparisons, self.total_work.checked_sub(accounted))
    }
}

fn counted_query<T>(
    counter: Option<&mut EvaluationCount>,
    work: &mut GeometryWorkBudget,
    query: impl FnOnce(&mut GeometryWorkBudget) -> T,
) -> T {
    if let Some(counter) = counter {
        let started = work.consumed();
        let result = query(work);
        counter.record(work.consumed() - started);
        result
    } else {
        query(work)
    }
}

#[derive(Clone, Copy)]
pub(super) struct FailedSpanDiagnostic {
    range: [f64; 2],
    depth: usize,
    goal: f64,
    endpoint_norms: [[f64; 2]; 2],
    map: ([f64; 2], [f64; 2], [f64; 2]),
    parent_m: Option<([f64; 2], f64, f64)>,
    child_bounds: [Option<f64>; 2],
    stage: &'static str,
    coedge: bool,
    position_bound: bool,
    oracle: &'static str,
}
impl std::fmt::Debug for FailedSpanDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "r={:?},d={},T={:e},ep_hi~=[{:.6e},{:.6e}],",
            self.range, self.depth, self.goal,
            self.endpoint_norms[0][1], self.endpoint_norms[1][1])?;
        if let Some((_, second, bound)) = self.parent_m {
            write!(f, "M~=[{second:.6e},{bound:.6e}],")?;
        } else {
            f.write_str("M=-,")?;
        }
        write!(f, "sub={:?},at={},coedge={},pos={},src={},map={:?}",
            self.child_bounds, self.stage, self.coedge, self.position_bound, self.oracle, self.map)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum ChartBasisDiagnostic {
    Line([[f64; 2]; 2]),
    Ellipse([f64; 2], bool, Matrix3),
    Other(&'static str),
}
#[derive(Clone, Copy)]
pub(super) struct ChartOperatorDiagnostic {
    outer: Option<([f64; 2], bool, bool)>,
    second: Option<([f64; 2], bool, bool)>,
    depth: usize,
    reflection: Option<[f64; 2]>,
    transforms: usize,
    basis: ChartBasisDiagnostic,
}
impl ChartOperatorDiagnostic {
    pub(super) fn same_basis(&self, other: &Self) -> Option<bool> {
        match (self.basis, other.basis) {
            (ChartBasisDiagnostic::Other(_), _) | (_, ChartBasisDiagnostic::Other(_)) => None,
            _ => Some(self.basis == other.basis),
        }
    }
}
impl std::fmt::Debug for ChartOperatorDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let basis = match self.basis {
            ChartBasisDiagnostic::Line(_) => ("line", None),
            ChartBasisDiagnostic::Ellipse(range, forward, _) => ("ellipse", Some((range, forward))),
            ChartBasisDiagnostic::Other(kind) => (kind, None),
        };
        write!(f, "w0={:?},w1={:?},d={},xf={},base={basis:?}",
            self.outer, self.second, self.depth, self.transforms)?;
        if self.depth > 2 {
            write!(f, ",first-rev={:?}", self.reflection)?;
        }
        Ok(())
    }
}
pub(super) fn chart_operators(mut chart: &model::Curve2D) -> ChartOperatorDiagnostic {
    let mut facts = ChartOperatorDiagnostic {
        outer: None, second: None, depth: 0, reflection: None, transforms: 0,
        basis: ChartBasisDiagnostic::Other("depth"),
    };
    while let model::Curve2D::TrimmedCurve(wrapper) = chart {
        if facts.depth == MAX_DEPTH { return facts }
        let (a, b) = wrapper.entity().range_tuple();
        let identity = *wrapper.transform() == Matrix3::identity();
        if facts.depth == 0 {
            facts.outer = Some(([a, b], wrapper.orientation(), identity));
        } else if facts.depth == 1 {
            facts.second = Some(([a, b], wrapper.orientation(), identity));
        }
        if !wrapper.orientation() && facts.reflection.is_none() {
            facts.reflection = Some([a, b]);
        }
        facts.transforms += usize::from(!identity);
        facts.depth += 1;
        chart = wrapper.entity().curve().as_ref();
    }
    facts.basis = match chart {
        model::Curve2D::Line(line) => ChartBasisDiagnostic::Line([
            [line.0.x, line.0.y], [line.1.x, line.1.y],
        ]),
        model::Curve2D::Conic(model::Conic2D::Ellipse(ellipse)) => {
            let (a, b) = ellipse.range_tuple();
            ChartBasisDiagnostic::Ellipse([a, b], ellipse.orientation(), *ellipse.transform())
        }
        model::Curve2D::BsplineCurve(_) => ChartBasisDiagnostic::Other("spline"),
        model::Curve2D::NurbsCurve(_) => ChartBasisDiagnostic::Other("nurbs"),
        model::Curve2D::Polyline(_) => ChartBasisDiagnostic::Other("polyline"),
        _ => ChartBasisDiagnostic::Other("other"),
    };
    facts
}

/// Retain only the last and largest fallbacks, even when they eventually prove
/// correspondence. A later cumulative-budget failure must not lose its prior
/// actual work consumer just because the current harmonic entry cannot start.
#[derive(Default)]
pub(super) struct CorrespondenceDiagnostic {
    pub active_source: Option<CorrespondenceSource>,
    pub retained_joint: Option<RetainedJointDiagnostic>,
    pub evaluation: Option<EvaluationCost>,
    pub failed_span: Option<FailedSpanDiagnostic>,
    last: Option<FallbackDiagnostic>,
    largest: Option<FallbackDiagnostic>,
    decline: Option<CorrespondenceDecline>,
}

impl CorrespondenceDiagnostic {
    pub(super) fn begin_retained_joint_phase(&mut self, phase: &'static str, consumed: u64) {
        self.decline = None;
        self.evaluation = None;
        self.failed_span = None;
        if let Some(joint) = &mut self.retained_joint {
            joint.phase = phase;
            joint.phase_started_work = consumed;
        }
    }

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
        if let Some(cost) = &self.evaluation {
            write!(f, "eval[{cost:?}]; ")?;
        }
        if let Some(joint) = &self.retained_joint {
            write!(f, "joint[{joint}]; ")?;
        }
        if let Some(decline) = &self.decline {
            write!(f, "decline={decline:?}; ")?;
        }
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

/// Completed whole-expression proof. No inverse qualification can construct it.
/// Span inventories refer to the unchanged expressions selected by the owning
/// coedge; they do not copy adaptive proof cells.
#[derive(Clone, Copy)]
pub(crate) struct OriginalCoedgeCorrespondence {
    map: AffineMap,
    position_error: f64,
    relative_derivative_error: f64,
    margin_uv: [f64; 2],
    provenance: OriginalCorrespondenceProvenance,
    inventory: OriginalSpanInventory,
    binding: Option<crate::kernel::OriginalCoedgeBinding>,
    // Transient source-admission seal; normalized output always drops it.
    retained_operands: Option<operand_binding::Binding>,
}

/// A borrowed, function-bound original selected image on this child's domain.
/// Its nominal conic window supplies no range authority; the completed
/// original master's identity map supplies the explicit containing domain.
#[derive(Clone, Copy)]
struct SelectedMasterImage<'a> {
    input: Correspondence<'a>,
    prepared: &'a model::PreparedValidatedCurve<'static>,
    map: AffineMap,
    authorized_target: [f64; 2],
}

/// Import-only authority for the original selected master. Function identity
/// may restrict its completed nominal proof; it never grants a face binding.
/// Keeping this beside, not inside, a coedge receipt avoids published growth.
#[derive(Clone, Copy)]
pub(super) struct SelectedMasterReceipt {
    whole: OriginalCoedgeCorrespondence,
    functions: Option<operand_binding::FunctionBinding>,
    pub diagnostic_chart: ChartOperatorDiagnostic,
}
impl SelectedMasterReceipt {
    pub(super) fn whole(&self) -> &OriginalCoedgeCorrespondence {
        &self.whole
    }

    fn restrict_base(
        &self,
        input: Correspondence<'_>,
        prepared: &model::PreparedValidatedCurve<'static>,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<OriginalCoedgeCorrespondence>, GeometryError> {
        check_cancel(cancel)?;
        let Some(functions) = self.functions else { return Ok(None) };
        let whole = &self.whole;
        if whole.provenance != OriginalCorrespondenceProvenance::Forward
            || whole.binding.is_some() || whole.retained_operands.is_none()
            || !whole.map.identity || whole.map.slope.lo != 1.0 || whole.map.slope.hi != 1.0
            || !whole.position_error.is_finite() || whole.position_error < 0.0
            || !input.maximum_error_mm.is_finite() || input.maximum_error_mm <= 0.0
            || whole.position_error > input.maximum_error_mm
            || !whole.relative_derivative_error.is_finite()
            || !(0.0..=0.25).contains(&whole.relative_derivative_error)
            || (0..2).any(|axis| !input.margin_uv[axis].is_finite()
                || input.margin_uv[axis] < 0.0
                || input.margin_uv[axis] > whole.margin_uv[axis])
        {
            return Ok(None);
        }
        let Some((a, b)) = input.curve.try_range_tuple() else { return Ok(None) };
        let Some((c, d)) = input.chart.try_range_tuple() else { return Ok(None) };
        if ![a, b, c, d].iter().all(|value| value.is_finite()) {
            return Ok(None);
        }
        let active = prepared.range();
        let source = [a.max(active[0]), b.min(active[1])];
        let target = [c, d];
        if source[0] >= source[1] || target[0] >= target[1]
            || source[0] < whole.map.source[0] || source[1] > whole.map.source[1]
            || target[0] < whole.map.target[0] || target[1] > whole.map.target[1]
        {
            return Ok(None);
        }
        let map = AffineMap::new(source, target)?;
        if operand_binding::capture_functions(input, (prepared, 1.0), map, work, cancel)?
            != Some(functions)
        {
            return Ok(None);
        }
        // The complete parent uses native t. The actual child affine map and
        // finite chart restriction/reversal chain also prove literal +t+0,
        // including every intermediate domain and oriented endpoint image.
        // Remaining operands/branch/frame are identical, so both completed
        // nominal obligations restrict without a unit/tangent change.
        let mut child = OriginalCoedgeCorrespondence::completed(
            input, map, whole.position_error, Some(prepared), work, cancel,
        )?;
        child.relative_derivative_error = whole.relative_derivative_error;
        Ok(Some(child))
    }

    fn selected_image<'a>(
        &self,
        input: Correspondence<'a>,
        prepared: &'a model::PreparedValidatedCurve<'static>,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<SelectedMasterImage<'a>>, GeometryError> {
        check_cancel(cancel)?;
        let Some(functions) = self.functions else { return Ok(None) };
        let whole = &self.whole;
        if whole.provenance != OriginalCorrespondenceProvenance::Forward
            || whole.binding.is_some() || whole.retained_operands.is_none()
            || !whole.map.identity || whole.map.slope.lo != 1.0 || whole.map.slope.hi != 1.0
            || prepared.mapped_basis().is_some()
        {
            return Ok(None);
        }
        let Some((a, b)) = input.curve.try_range_tuple() else { return Ok(None) };
        let active = prepared.range();
        let source = [a.max(active[0]), b.min(active[1])];
        if !source.iter().all(|value| value.is_finite()) || source[0] >= source[1]
            || source[0] < whole.map.source[0] || source[1] > whole.map.source[1]
            || source[0] < whole.map.target[0] || source[1] > whole.map.target[1]
        {
            return Ok(None);
        }
        let Some(model::Curve::IntersectionCurve(root)) = prepared.native_curve()
        else { return Ok(None) };
        let map = AffineMap::new(source, source)?;
        for boundary in [root.boundary0(), root.boundary1()].into_iter().flatten() {
            work.charge(1, cancel)?;
            let selected = Correspondence {
                curve: input.curve,
                surface: boundary.surface().as_ref(),
                chart: boundary.curve(),
                margin_uv: [0.0; 2],
                ..input
            };
            if operand_binding::capture_functions(
                selected, (prepared, 1.0), map, work, cancel,
            )? == Some(functions) {
                return Ok(Some(SelectedMasterImage {
                    input: selected, prepared, map, authorized_target: whole.map.target,
                }));
            }
        }
        Ok(None)
    }

    fn factor_position<'a>(
        &self,
        input: Correspondence<'a>,
        prepared: &'a model::PreparedValidatedCurve<'static>,
        nodes: Option<&mut usize>,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
        diagnostic: Option<&mut CorrespondenceDiagnostic>,
    ) -> Result<Option<RetainedPosition<'a>>, GeometryError> {
        let Some(image) = self.selected_image(input, prepared, work, cancel)?
        else { return Ok(None) };
        let model::Surface::Plane(plane) = image.input.surface else { return Ok(None) };
        let Some((c, d)) = input.chart.try_range_tuple() else { return Ok(None) };
        let mut harmonic = super::harmonic::HarmonicDiagnostic::new(input.maximum_error_mm);
        let Some(delta) = resolution(super::harmonic::certify(
            super::harmonic::HarmonicComparison {
                source: super::harmonic::HarmonicSource::PlaneImage(plane, image.input.chart),
                surface: input.surface, chart: input.chart,
                range: image.map.source, target: [c, d], margin: input.margin_uv,
                tolerance: input.maximum_error_mm, require_tangent: false,
            },
            work, cancel, &mut harmonic,
        ))? else { return Ok(None) };
        let goal = Interval::point(input.maximum_error_mm).sub(Interval::point(delta)).lo;
        if !goal.is_finite() || goal <= 0.0 { return Ok(None) }
        // The function-bound whole receipt already covers this identical
        // selected image on the child range. Reuse its POSITION only when its
        // achieved bound fits the actual remaining allowance; otherwise the
        // unchanged finer original-root proof remains mandatory.
        let root_error = if self.whole.position_error.is_finite()
            && self.whole.position_error >= 0.0
            && self.whole.position_error <= goal
            && self.whole.margin_uv.iter().all(|value| value.is_finite() && *value >= 0.0)
        {
            self.whole.position_error
        } else {
            let mut local = GeometryWorkBudget::new(work.remaining().min(MAX_WORK));
            let result = certify_scoped(
                Correspondence { maximum_error_mm: goal, ..image.input },
                ProofPolicy::ForwardPosition, Some((prepared, 1.0)), None, Some(&image),
                nodes, &mut local, cancel, diagnostic,
            );
            work.charge(local.consumed(), &AtomicBool::new(false))?;
            check_cancel(cancel)?;
            let Some(proof) = resolution(result)? else { return Ok(None) };
            proof.position_error
        };
        let total = Interval::point(root_error).add(Interval::point(delta)).hi;
        if !total.is_finite() || total > input.maximum_error_mm { return Ok(None) }
        Ok(Some(RetainedPosition {
            input, prepared, frame_scale: 1.0,
            proof: RetainedPositionProof::Image {
                selected: image, root_error, image_error: delta,
            },
        }))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OriginalCorrespondenceProvenance {
    ExactCoefficients,
    Forward,
    CompletedPoleGraph,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OriginalSpanInventory {
    pub(crate) source_knot_boundaries: usize,
    pub(crate) uv_knot_boundaries: usize,
    pub(crate) continuation_spans: usize,
}

impl OriginalCoedgeCorrespondence {
    fn completed(
        input: Correspondence<'_>,
        map: AffineMap,
        bound: f64,
        retained: Option<&model::PreparedValidatedCurve<'static>>,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        let mut inventory = OriginalSpanInventory {
            source_knot_boundaries: 0,
            uv_knot_boundaries: 0,
            continuation_spans: 0,
        };
        visit_curve_cuts(input.curve, &mut |_| {
            work.charge(1, cancel)?;
            inventory.source_knot_boundaries += 1;
            Ok(())
        })?;
        visit_chart_cuts(input.chart, &mut |_| {
            work.charge(1, cancel)?;
            inventory.uv_knot_boundaries += 1;
            Ok(())
        })?;
        if let Some(prepared) = retained {
            for _ in prepared.pieces() {
                work.charge(1, cancel)?;
                inventory.continuation_spans += 1;
            }
        }
        Ok(Self {
            map,
            position_error: bound,
            // Both successful whole-source lanes prove the strict 1/4 bound.
            // This retained conservative bound is not a measured sample ratio.
            relative_derivative_error: 0.25,
            margin_uv: input.margin_uv,
            provenance: OriginalCorrespondenceProvenance::Forward,
            inventory,
            binding: None,
            retained_operands: None,
        })
    }

    /// Only the caller's completed whole-coefficient theorem may use this
    /// constructor. It is distinct from a zero residual numerical proof.
    pub(super) fn exact_coefficients(
        curve: &model::Curve,
        chart: &model::Curve2D,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        let (a, b) = curve.range_tuple();
        let (c, d) = chart.range_tuple();
        let map = AffineMap::new([a, b], [c, d])?;
        // No carrier is needed: this path has already proved coefficient
        // identity and only captures the two original expression inventories.
        let mut source_knot_boundaries = 0;
        let mut uv_knot_boundaries = 0;
        visit_curve_cuts(curve, &mut |_| {
            work.charge(1, cancel)?;
            source_knot_boundaries += 1;
            Ok(())
        })?;
        visit_chart_cuts(chart, &mut |_| {
            work.charge(1, cancel)?;
            uv_knot_boundaries += 1;
            Ok(())
        })?;
        Ok(Self {
            map,
            position_error: 0.0,
            relative_derivative_error: 0.0,
            margin_uv: [0.0; 2],
            provenance: OriginalCorrespondenceProvenance::ExactCoefficients,
            inventory: OriginalSpanInventory {
                source_knot_boundaries,
                uv_knot_boundaries,
                continuation_spans: 0,
            },
            binding: None,
            retained_operands: None,
        })
    }

    fn seal_retained(
        mut self,
        input: Correspondence<'_>,
        prepared: (&model::PreparedValidatedCurve<'static>, f64),
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        self.retained_operands = operand_binding::capture(input, prepared, work, cancel)?;
        Ok(self)
    }

    pub(crate) fn native_range(&self) -> [f64; 2] {
        self.map.source
    }
    /// Endpoint pairing retains orientation, not merely sorted UV bounds.
    pub(crate) fn uv_range(&self) -> [f64; 2] {
        self.map.target
    }
    /// Identity of the COMPLETE sealed affine native-to-original-UV parameter
    /// law, including its orientation and normalized derivative enclosure.
    /// This does not compare curve geometry or supply source/use ownership.
    pub(crate) fn same_affine_native_uv_map(&self, other: &Self) -> bool {
        self.provenance != OriginalCorrespondenceProvenance::CompletedPoleGraph
            && other.provenance != OriginalCorrespondenceProvenance::CompletedPoleGraph
            && self.map.source == other.map.source
            && self.map.target == other.map.target
            && self.map.identity == other.map.identity
            && self.map.slope.lo == other.map.slope.lo
            && self.map.slope.hi == other.map.slope.hi
    }
    pub(crate) fn uv_parameter_image(&self, range: [f64; 2]) -> Result<Interval, GeometryError> {
        if range[0] < self.map.source[0] || range[1] > self.map.source[1] || range[0] > range[1] {
            return Err(GeometryError::new(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "original coedge map query lies outside its complete certified range",
            ));
        }
        self.map.image(range)
    }
    /// Enclose the native parameters of a qualified original-UV parameter
    /// interval under this COMPLETE sealed affine law. This is parameter
    /// algebra, not an inverse surface query or source/use ownership proof.
    /// Completed pole graphs do not supply this affine-map authority.
    pub(crate) fn affine_native_parameter_preimage(
        &self,
        range: [f64; 2],
    ) -> Result<Option<Interval>, GeometryError> {
        if self.provenance == OriginalCorrespondenceProvenance::CompletedPoleGraph {
            return Ok(None);
        }
        if !range[0].is_finite()
            || !range[1].is_finite()
            || range[0] > range[1]
            || range[0] < self.map.target[0].min(self.map.target[1])
            || range[1] > self.map.target[0].max(self.map.target[1])
        {
            return Err(GeometryError::new(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "original UV preimage query leaves its complete certified affine range",
            ));
        }
        Ok(Some(
            self.map
                .preimage(range[0])?
                .hull(self.map.preimage(range[1])?),
        ))
    }
    pub(crate) fn uv_parameter_derivative(&self) -> Interval {
        self.map.slope
    }
    pub(crate) fn position_error_mm(&self) -> f64 {
        self.position_error
    }
    pub(crate) fn relative_derivative_error_bound(&self) -> f64 {
        self.relative_derivative_error
    }
    pub(crate) fn margin_uv(&self) -> [f64; 2] {
        self.margin_uv
    }
    pub(crate) fn provenance(&self) -> OriginalCorrespondenceProvenance {
        self.provenance
    }
    pub(crate) fn span_inventory(&self) -> OriginalSpanInventory {
        self.inventory
    }
    /// Borrow the complete original coefficient/continuation inventories.
    /// Boundaries are emitted in native-edge coordinates as outward intervals;
    /// repeated or overlapping knot boundaries are intentional, not gaps.
    pub(crate) fn visit_span_boundaries(
        &self,
        curve: &model::Curve,
        chart: &model::Curve2D,
        continuation: Option<&model::PreparedValidatedCurve<'static>>,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
        visit: &mut impl FnMut(Interval) -> Result<(), GeometryError>,
    ) -> Result<(), GeometryError> {
        check_cancel(cancel)?;
        let mut emit = |boundary: Interval| {
            work.charge(1, cancel)?;
            let low = boundary.lo.max(self.map.source[0]);
            let high = boundary.hi.min(self.map.source[1]);
            if low <= high {
                visit(Interval::new(low, high)?)?;
            }
            Ok(())
        };
        emit(Interval::point(self.map.source[0]))?;
        emit(Interval::point(self.map.source[1]))?;
        visit_curve_cuts(curve, &mut emit)?;
        if let Some(prepared) = continuation {
            for piece in prepared.pieces() {
                emit(Interval::point(piece.parameter[0]))?;
                emit(Interval::point(piece.parameter[1]))?;
            }
        }
        visit_chart_cuts(chart, &mut |boundary| {
            let low = boundary.lo.max(self.map.target[0].min(self.map.target[1]));
            let high = boundary.hi.min(self.map.target[0].max(self.map.target[1]));
            if low <= high {
                emit(self.map.preimage(low)?.hull(self.map.preimage(high)?))?;
            }
            Ok(())
        })
    }
    pub(crate) fn binding(&self) -> Option<&crate::kernel::OriginalCoedgeBinding> {
        self.binding.as_ref()
    }
    pub(super) fn bind(mut self, binding: crate::kernel::OriginalCoedgeBinding) -> Self {
        self.binding = Some(binding);
        // The seal is valid only during import while its original owner lives.
        self.retained_operands = None;
        self
    }
    pub(super) fn complete_pole_graph(&mut self) {
        self.provenance = OriginalCorrespondenceProvenance::CompletedPoleGraph;
    }
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
    Ok(certify_with_policy(
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
    )?
    .map(|proof| proof.position_error))
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
) -> Result<Option<OriginalCoedgeCorrespondence>, GeometryError> {
    Ok(certify_with_policy(
        input,
        ProofPolicy::DeclaredCoedge(input.margin_uv),
        work,
        cancel,
        diagnostic,
    )?
    .and_then(|proof| proof.coedge))
}

#[derive(Clone, Copy, PartialEq)]
enum ProofPolicy {
    EitherPosition,
    ForwardPosition,
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

/// Position-only calls never construct a material correspondence or traverse
/// its retained inventory. Only the whole-source tangent policy can do that.
struct PolicyProof {
    position_error: f64,
    coedge: Option<OriginalCoedgeCorrespondence>,
}
impl PolicyProof {
    fn completed(
        input: Correspondence<'_>,
        map: AffineMap,
        bound: f64,
        retained: Option<&model::PreparedValidatedCurve<'static>>,
        policy: ProofPolicy,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        Ok(Self {
            position_error: bound,
            coedge: if matches!(policy, ProofPolicy::DeclaredCoedge(_)) {
                Some(OriginalCoedgeCorrespondence::completed(
                    input, map, bound, retained, work, cancel,
                )?)
            } else {
                None
            },
        })
    }
}

fn certify_with_policy(
    input: Correspondence<'_>,
    policy: ProofPolicy,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    mut diagnostic: Option<&mut CorrespondenceDiagnostic>,
) -> Result<Option<PolicyProof>, GeometryError> {
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
    let result = certify(input, policy, None, None, &mut local, cancel, diagnostic.as_deref_mut());
    if let (Some(diagnostic), Err(cause)) = (diagnostic, &result)
        && diagnostic.decline.is_none()
    {
        diagnostic.decline = Some(CorrespondenceDecline {
            predicate: "correspondence-evaluation-error",
            cause: Some(cause.code),
            ..Default::default()
        });
    }
    // Cancellation stops geometry work, not the debit for work already done.
    work.charge(local.consumed(), &AtomicBool::new(false))?;
    check_cancel(cancel)?;
    resolution(result)
}

/// The original preferred master is checked for every native child, but its
/// completed WHOLE-original forward receipt need not be solved again when
/// actual operands, branch, map, frame and uncertainty are identical.
pub(super) fn certify_selected_master_correspondence(
    input: Correspondence<'_>,
    prepared: (&model::PreparedValidatedCurve<'static>, f64),
    previous: Option<&SelectedMasterReceipt>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<SelectedMasterReceipt>, GeometryError> {
    check_cancel(cancel)?;
    if let Some(previous) = previous {
        let previous_proof = &previous.whole;
        if previous_proof.provenance == OriginalCorrespondenceProvenance::Forward
            && previous_proof.binding.is_none()
            && previous_proof.retained_operands.is_some()
            && previous_proof.position_error.is_finite()
            && previous_proof.position_error >= 0.0
            && previous_proof.position_error <= input.maximum_error_mm
            && previous_proof.relative_derivative_error.is_finite()
            && previous_proof.relative_derivative_error <= 0.25
            && previous_proof.margin_uv == input.margin_uv
            && operand_binding::capture(input, prepared, work, cancel)? == previous_proof.retained_operands
        {
            return Ok(Some(*previous));
        }
    }
    let Some(proof) = certify_retained_coedge_correspondence(
        input, prepared, work, cancel, None,
    )? else {
        return Ok(None);
    };
    let functions = if proof.map.identity {
        operand_binding::capture_functions(input, prepared, proof.map, work, cancel)?
    } else {
        None
    };
    Ok(Some(SelectedMasterReceipt {
        whole: proof.seal_retained(input, prepared, work, cancel)?,
        functions,
        diagnostic_chart: chart_operators(input.chart),
    }))
}

pub(super) fn certify_retained_coedge_correspondence(
    input: Correspondence<'_>,
    prepared: (&model::PreparedValidatedCurve<'static>, f64),
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    diagnostic: Option<&mut CorrespondenceDiagnostic>,
) -> Result<Option<OriginalCoedgeCorrespondence>, GeometryError> {
    certify_retained(input, prepared, None, work, cancel, diagnostic)
}

/// A completed base receipt and metric stay coupled to their exact borrowed
/// operands. No public scalar can replace a whole-position certificate.
struct RetainedPosition<'a> {
    input: Correspondence<'a>,
    prepared: &'a model::PreparedValidatedCurve<'static>,
    frame_scale: f64,
    proof: RetainedPositionProof<'a>,
}

enum RetainedPositionProof<'a> {
    Expanded {
        coefficient_margin: [f64; 2],
        base: OriginalCoedgeCorrespondence,
        metric: super::implicit::RetainedRefinement,
    },
    Identical {
        source: OriginalCoedgeCorrespondence,
        actual: operand_binding::Binding,
    },
    Image {
        selected: SelectedMasterImage<'a>,
        root_error: f64,
        image_error: f64,
    },
}

impl RetainedPosition<'_> {
    fn bound(
        &self,
        input: Correspondence<'_>,
        prepared: (&model::PreparedValidatedCurve<'static>, f64),
    ) -> Result<Option<f64>, GeometryError> {
        let active = prepared.0.range();
        let native = input.curve.range_tuple();
        let chart = input.chart.range_tuple();
        let base = match &self.proof {
            RetainedPositionProof::Expanded { base, .. } => Some(base),
            RetainedPositionProof::Identical { source, .. } => Some(source),
            RetainedPositionProof::Image { .. } => None,
        };
        if !std::ptr::eq(self.input.curve, input.curve)
            || !std::ptr::eq(self.input.surface, input.surface)
            || !std::ptr::eq(self.input.chart, input.chart)
            || !std::ptr::eq(self.prepared, prepared.0)
            || prepared.1 != self.frame_scale
            || input.margin_uv != self.input.margin_uv
            || input.maximum_error_mm != self.input.maximum_error_mm
            || base.is_some_and(|base|
                base.native_range() != [native.0.max(active[0]), native.1.min(active[1])]
                    || base.uv_range() != [chart.0, chart.1])
        {
            return Ok(None);
        }
        let bound = match &self.proof {
            RetainedPositionProof::Expanded { coefficient_margin, base, metric } => {
                if base.margin_uv != *coefficient_margin { return Ok(None); }
                metric.completed_position(base.position_error_mm())?
            }
            RetainedPositionProof::Identical { source, actual } => {
                if source.retained_operands != Some(*actual) || source.margin_uv != input.margin_uv {
                    return Ok(None);
                }
                source.position_error_mm()
            }
            RetainedPositionProof::Image { selected, root_error, image_error } => {
                if !std::ptr::eq(selected.prepared, prepared.0)
                    || !std::ptr::eq(selected.input.curve, input.curve)
                    || prepared.1 != 1.0 || !selected.map.identity
                    || selected.map.source != [native.0.max(active[0]), native.1.min(active[1])]
                    || !root_error.is_finite() || *root_error < 0.0
                    || !image_error.is_finite() || *image_error < 0.0
                {
                    return Ok(None);
                }
                Interval::point(*root_error).add(Interval::point(*image_error)).hi
            }
        };
        Ok((bound.is_finite() && bound <= input.maximum_error_mm).then_some(bound))
    }
}

/// Reuses only a completed whole-forward source receipt with identical actual
/// operands/branch/map/tube/frame, then freshly proves full relative tangents.
/// Any ordinary binding decline recomputes normalized base/tube position.
pub(super) fn certify_normalized_retained_correspondence(
    input: Correspondence<'_>,
    prepared: (&model::PreparedValidatedCurve<'static>, f64),
    previous: Option<&OriginalCoedgeCorrespondence>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    diagnostic: Option<&mut CorrespondenceDiagnostic>,
) -> Result<Option<OriginalCoedgeCorrespondence>, GeometryError> {
    check_cancel(cancel)?;
    let mut reuse_state = "absent";
    if let Some(source) = previous
        && source.retained_operands.is_some()
        && source.position_error.is_finite()
        && source.position_error <= input.maximum_error_mm
        && source.margin_uv == input.margin_uv
    {
        match operand_binding::capture(input, prepared, work, cancel)? {
            Some(actual) if source.retained_operands == Some(actual) => {
                let position = RetainedPosition {
                    input,
                    prepared: prepared.0,
                    frame_scale: prepared.1,
                    proof: RetainedPositionProof::Identical { source: *source, actual },
                };
                return certify_retained(input, prepared, Some(&position), work, cancel, diagnostic)
                    .map_err(|cause| GeometryError::new(cause.code, format!(
                        "normalized-retained identical-operands relative-tangent proof: {}", cause.message,
                    )));
            }
            Some(_) => reuse_state = "changed",
            None => reuse_state = "unsupported",
        }
    }
    if input.margin_uv == [0.0; 2] {
        return certify_retained_coedge_correspondence(input, prepared, work, cancel, diagnostic);
    }
    let active = prepared.0.range();
    let native = input.curve.range_tuple();
    let range = [native.0.max(active[0]), native.1.min(active[1])];
    let mut metric_diagnostic = super::implicit::RefinementDiagnostic::default();
    let metric_result = super::implicit::normalized_tube_refinement(
        input, range, prepared.1, work, cancel, &mut metric_diagnostic,
    );
    let annotate = |cause: GeometryError, phase: &str, base: Option<&CorrespondenceDiagnostic>| {
        GeometryError::new(cause.code, format!(
            "normalized-tube reuse={reuse_state} phase={phase} scale={} margin_uv={:?} R={:?} goal={:?} D={:?} base_decline={:?}: {}",
            prepared.1, input.margin_uv, metric_diagnostic.coefficient_radius,
            metric_diagnostic.derived_limit, metric_diagnostic.point_diameter_budget_native,
            base.and_then(|value| value.decline.as_ref()), cause.message,
        ))
    };
    let Some(metric) = metric_result.map_err(|cause| annotate(cause, "metric", None))? else {
        // A conservative first-jet metric need not fit every certifiable
        // rectangle. The existing independent whole-expression proof remains
        // available; no receipt from other operands is substituted.
        return certify_retained_coedge_correspondence(input, prepared, work, cancel, diagnostic)
            .map_err(|cause| annotate(cause, "metric-declined/direct", None));
    };
    let mut base_diagnostic = CorrespondenceDiagnostic::default();
    let Some(base) = certify_retained_coedge_correspondence(
        Correspondence {
            maximum_error_mm: metric.base_limit(),
            margin_uv: [0.0; 2],
            ..input
        },
        prepared,
        work,
        cancel,
        Some(&mut base_diagnostic),
    ).map_err(|cause| annotate(cause, "base", Some(&base_diagnostic)))? else {
        // E+R is sufficient, not necessary: independent nominal/tube boxes
        // can lose correlation that the direct whole-tube proof preserves.
        // Only an explicit decline reaches this path; errors propagate above.
        return certify_retained_coedge_correspondence(input, prepared, work, cancel, diagnostic)
            .map_err(|cause| annotate(cause, "base-declined/direct", Some(&base_diagnostic)));
    };
    let position = RetainedPosition {
        input,
        prepared: prepared.0,
        frame_scale: prepared.1,
        proof: RetainedPositionProof::Expanded {
            coefficient_margin: [0.0; 2], base, metric,
        },
    };
    certify_retained(input, prepared, Some(&position), work, cancel, diagnostic)
        .map_err(|cause| annotate(cause, "composed-tangent", Some(&base_diagnostic)))
}

fn certify_retained(
    input: Correspondence<'_>,
    prepared: (&model::PreparedValidatedCurve<'static>, f64),
    position: Option<&RetainedPosition<'_>>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    diagnostic: Option<&mut CorrespondenceDiagnostic>,
) -> Result<Option<OriginalCoedgeCorrespondence>, GeometryError> {
    certify_retained_with_nodes(input, prepared, position, None, work, cancel, diagnostic)
}

fn certify_retained_with_nodes(
    input: Correspondence<'_>,
    prepared: (&model::PreparedValidatedCurve<'static>, f64),
    position: Option<&RetainedPosition<'_>>,
    nodes: Option<&mut usize>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    mut diagnostic: Option<&mut CorrespondenceDiagnostic>,
) -> Result<Option<OriginalCoedgeCorrespondence>, GeometryError> {
    let Correspondence {
        maximum_error_mm: maximum_error,
        margin_uv,
        ..
    } = input;
    check_cancel(cancel)?;
    if !prepared.1.is_finite() || prepared.1 <= 0.0 {
        return Err(GeometryError::new(
            GeometryErrorCode::ConditioningFailure,
            "retained original correspondence needs a finite positive frame scale",
        ));
    }
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
    let position_bound = match position {
        Some(position) => match position.bound(input, prepared)? {
            Some(bound) => Some(bound),
            None => return Ok(None),
        },
        None => None,
    };
    let mut local = GeometryWorkBudget::new(work.remaining().min(MAX_WORK));
    let result = certify_scoped(
        input,
        ProofPolicy::DeclaredCoedge(margin_uv),
        Some(prepared),
        position_bound,
        None,
        nodes,
        &mut local,
        cancel,
        diagnostic.as_deref_mut(),
    );
    if let (Some(diagnostic), Err(cause)) = (diagnostic, &result)
        && diagnostic.decline.is_none()
    {
        diagnostic.decline = Some(CorrespondenceDecline {
            predicate: "retained-correspondence-evaluation-error",
            cause: Some(cause.code),
            ..Default::default()
        });
    }
    work.charge(local.consumed(), &AtomicBool::new(false))?;
    check_cancel(cancel)?;
    Ok(resolution(result)?.and_then(|proof| proof.coedge))
}

/// A comparison image never supplies current UV authority. Its freshly proved
/// root error and coefficient-proved image discrepancy only finish position;
/// the actual current full-tube tangent proof still constructs the coedge.
fn certify_retained_base(
    input: Correspondence<'_>,
    prepared: &model::PreparedValidatedCurve<'static>,
    master: Option<&SelectedMasterReceipt>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    mut diagnostic: Option<&mut CorrespondenceDiagnostic>,
) -> Result<Option<OriginalCoedgeCorrespondence>, GeometryError> {
    let Some(master) = master.filter(|_| input.maximum_error_mm.is_finite()
        && input.maximum_error_mm > 0.0
        && input.margin_uv.iter().all(|value| value.is_finite() && *value >= 0.0))
    else {
        return certify_retained_coedge_correspondence(
            input, (prepared, 1.0), work, cancel, diagnostic,
        );
    };
    // Factorization, actual tangents and any ordinary fallback share the
    // original per-proof work allowance as well as the import's cumulative cap.
    let mut local = GeometryWorkBudget::new(work.remaining().min(MAX_WORK));
    let mut nodes = 0;
    let result = (|| {
        if let Some(position) = master.factor_position(
            input, prepared, Some(&mut nodes), &mut local, cancel, diagnostic.as_deref_mut(),
        )?
            && let Some(proof) = certify_retained_with_nodes(
                input, (prepared, 1.0), Some(&position), Some(&mut nodes),
                &mut local, cancel, diagnostic.as_deref_mut(),
            )?
        {
            return Ok(Some(proof));
        }
        // Sufficient factorization nonproof is not a refusal of the old proof.
        certify_retained_with_nodes(
            input, (prepared, 1.0), None, Some(&mut nodes), &mut local, cancel, diagnostic,
        )
    })();
    work.charge(local.consumed(), &AtomicBool::new(false))?;
    check_cancel(cancel)?;
    result
}

/// Completes the original-root inverse and whole forward tube position proof,
/// then independently proves relative tangents on that same expanded tube.
/// All operands/ranges/orientations stay immutable in the native source frame.
/// A final tube's achieved error is never reused as the base-trace residual.
pub(super) fn complete_retained_correspondence(
    input: Correspondence<'_>,
    prepared: &monstertruck_modeling::PreparedValidatedCurve<'static>,
    source_scale: f64,
    master: Option<&SelectedMasterReceipt>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    mut diagnostic: Option<&mut CorrespondenceDiagnostic>,
) -> Result<
    Option<([f64; 2], OriginalCoedgeCorrespondence)>,
    GeometryError,
> {
    if let Some(diagnostic) = diagnostic.as_deref_mut() {
        diagnostic.retained_joint = Some(RetainedJointDiagnostic {
            allowed_native: input.maximum_error_mm,
            coefficient_margin_uv: input.margin_uv,
            ..Default::default()
        });
        diagnostic.begin_retained_joint_phase("base", work.consumed());
    }
    if let Some(master) = master
        && let Some(base) = master.restrict_base(input, prepared, work, cancel)?
    {
        if let Some(completed) = complete_retained_from_base(
            input, prepared, source_scale, base, false, None, work, cancel, diagnostic.as_deref_mut(),
        )? {
            return Ok(Some(completed));
        }
        // A whole-master error can be looser than the child's achieved error.
        // Its sufficient metric nonproof must restart the unchanged child
        // algorithm, never refuse using the coarse hint's inverse tube.
        if let Some(diagnostic) = diagnostic.as_deref_mut() {
            diagnostic.retained_joint = Some(RetainedJointDiagnostic {
                allowed_native: input.maximum_error_mm,
                coefficient_margin_uv: input.margin_uv,
                ..Default::default()
            });
            diagnostic.begin_retained_joint_phase("base", work.consumed());
        }
    }
    let Some(base) = certify_retained_base(
        input, prepared, master,
        work,
        cancel,
        diagnostic.as_deref_mut(),
    ).map_err(|cause| {
        retained_joint_error(cause, "base", input.maximum_error_mm, input.margin_uv)
    })? else {
        return Ok(None);
    };
    complete_retained_from_base(
        input, prepared, source_scale, base, true, master, work, cancel, diagnostic,
    )
}

fn retained_joint_error(
    cause: GeometryError, phase: &str, allowed: f64, margin: [f64; 2],
) -> GeometryError {
    GeometryError::new(cause.code, format!(
        "retained-joint phase={phase} allowed_native={allowed:e} margin_uv={margin:?}: {}",
        cause.message,
    ))
}

fn complete_retained_from_base(
    input: Correspondence<'_>,
    prepared: &model::PreparedValidatedCurve<'static>,
    source_scale: f64,
    base: OriginalCoedgeCorrespondence,
    direct_backstop: bool,
    master: Option<&SelectedMasterReceipt>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    mut diagnostic: Option<&mut CorrespondenceDiagnostic>,
) -> Result<Option<([f64; 2], OriginalCoedgeCorrespondence)>, GeometryError> {
    if let Some(diagnostic) = diagnostic.as_deref_mut() {
        diagnostic.begin_retained_joint_phase("source-inverse", work.consumed());
        if let Some(joint) = &mut diagnostic.retained_joint {
            joint.initial_achieved_native = Some(base.position_error_mm());
        }
    }
    let inverse = super::implicit::certify_uv_margin(
        super::implicit::UvMarginInput {
            curve: input.curve,
            prepared,
            surface: input.surface,
            chart: input.chart,
            physical_residual: base.position_error_mm(),
            coefficient_margin: input.margin_uv,
        },
        work,
        cancel,
    )?;
    let direct_margin_uv = inverse.margin_uv();
    // Preserve the original proved base's tube for the direct backstop.
    // Try its sufficient metric/coupled proof before repeating full position
    // subdivision on that tube; a base already meeting the derived goal is
    // reused, otherwise only the existing one derived refinement is attempted.
    if let Some(diagnostic) = diagnostic.as_deref_mut() {
        diagnostic.begin_retained_joint_phase("refinement-metric", work.consumed());
        if let Some(joint) = &mut diagnostic.retained_joint {
            joint.inverse_gain = Some(inverse.inverse_gain());
            joint.final_margin_uv = Some(direct_margin_uv);
        }
    }
    let metric = diagnostic.as_deref_mut()
        .and_then(|value| value.retained_joint.as_mut())
        .map(|joint| &mut joint.metric);
    let point_tolerance_native = super::implicit::original_point_tolerance_native(source_scale);
    let Some(metric) = inverse.refinement_limit(
        input.maximum_error_mm, point_tolerance_native, work, cancel, metric,
    )? else {
        if !direct_backstop { return Ok(None) }
        return complete_retained_direct(
            input, prepared, direct_margin_uv, work, cancel, diagnostic,
        );
    };
    let limit = metric.base_limit();
    let refined = if base.position_error_mm() <= limit {
        base
    } else {
        // One source-metric-derived precision on the SAME branch, chart and
        // coefficient margin. This does not change the final allowed error.
        if let Some(diagnostic) = diagnostic.as_deref_mut() {
            diagnostic.begin_retained_joint_phase("refined-base", work.consumed());
        }
        let Some(refined) = certify_retained_base(
            Correspondence { maximum_error_mm: limit, ..input }, prepared, master,
            work,
            cancel,
            diagnostic.as_deref_mut(),
        ).map_err(|cause| {
            retained_joint_error(cause, "refined-base", limit, input.margin_uv)
        })? else {
            if !direct_backstop { return Ok(None) }
            return complete_retained_direct(
                input, prepared, direct_margin_uv, work, cancel, diagnostic,
            );
        };
        refined
    };
    let margin_uv = inverse.refined_margin(refined.position_error_mm())?;
    if let Some(diagnostic) = diagnostic.as_deref_mut() {
        diagnostic.begin_retained_joint_phase("last-final", work.consumed());
        if let Some(joint) = &mut diagnostic.retained_joint {
            joint.refined_achieved_native = Some(refined.position_error_mm());
            joint.final_margin_uv = Some(margin_uv);
        }
    }
    // The refined proof, inverse metric and final tangent proof borrow the
    // identical operands. The sealed receipt refuses any changed normalized
    // curve/chart, active map, frame scale or tube rather than reusing E.
    let final_input = Correspondence { margin_uv, ..input };
    let position = RetainedPosition {
        input: final_input,
        prepared,
        frame_scale: 1.0,
        proof: RetainedPositionProof::Expanded {
            coefficient_margin: input.margin_uv, base: refined, metric,
        },
    };
    let bound = certify_retained(
        final_input,
        (prepared, 1.0),
        Some(&position),
        work,
        cancel,
        diagnostic.as_deref_mut(),
    ).map_err(|cause| {
        retained_joint_error(cause, "last-final", input.maximum_error_mm, margin_uv)
    })?;
    let Some(bound) = bound else {
        if !direct_backstop { return Ok(None) }
        return complete_retained_direct(
            input, prepared, direct_margin_uv, work, cancel, diagnostic,
        );
    };
    let bound = bound.seal_retained(final_input, (prepared, 1.0), work, cancel)?;
    Ok(Some((margin_uv, bound)))
}

/// Sufficient metric/refinement nonproof does not deny an otherwise valid
/// direct certificate. Use ONLY the original completed loose-base inverse
/// tube here, never the state of an unproved/refined proposal.
fn complete_retained_direct(
    input: Correspondence<'_>,
    prepared: &model::PreparedValidatedCurve<'static>,
    margin_uv: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    mut diagnostic: Option<&mut CorrespondenceDiagnostic>,
) -> Result<Option<([f64; 2], OriginalCoedgeCorrespondence)>, GeometryError> {
    if let Some(diagnostic) = diagnostic.as_deref_mut() {
        diagnostic.begin_retained_joint_phase("direct-final", work.consumed());
        if let Some(joint) = &mut diagnostic.retained_joint {
            joint.final_margin_uv = Some(margin_uv);
        }
    }
    let final_input = Correspondence { margin_uv, ..input };
    let Some(bound) = certify_retained_coedge_correspondence(
        final_input, (prepared, 1.0), work, cancel, diagnostic,
    ).map_err(|cause| GeometryError::new(cause.code, format!(
        "retained-joint phase=direct-final allowed_native={:e} margin_uv={margin_uv:?}: {}",
        input.maximum_error_mm, cause.message,
    )))? else {
        return Ok(None);
    };
    let bound = bound.seal_retained(final_input, (prepared, 1.0), work, cancel)?;
    Ok(Some((margin_uv, bound)))
}

fn terminal(error: &GeometryError) -> bool {
    matches!(
        error.code,
        GeometryErrorCode::Cancelled | GeometryErrorCode::ResourceLimit
    )
}
fn resolution<T>(result: Result<Option<T>, GeometryError>) -> Result<Option<T>, GeometryError> {
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
    fn retained_first(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<[Interval; 3]>, GeometryError> {
        match &self.implicit {
            Some(curve) => curve.retained_first_enclosure((range[0], range[1]), work, cancel),
            None => Ok(None),
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
enum SourceJetQualification {
    C1,
    TangentOnly,
}

/// Local to one immutable verify call. C1 is minted only from cell.smooth;
/// tangent-only traversal must never turn a piecewise jet into Taylor authority.
#[derive(Clone, Copy)]
struct SourceJetReceipt {
    range: [f64; 2],
    jet: CurveEnclosure,
    qualification: SourceJetQualification,
}
impl SourceJetReceipt {
    fn check_child(&self, range: [f64; 2]) -> Result<(), GeometryError> {
        if !(self.range[0] <= range[0] && range[0] <= range[1] && range[1] <= self.range[1]) {
            return Err(GeometryError::new(
                GeometryErrorCode::InvalidGeometry,
                "inherited correspondence jet does not contain its child interval",
            ));
        }
        Ok(())
    }
}


#[derive(Clone, Copy)]
struct Span {
    range: [f64; 2],
    endpoints: [[Interval; 3]; 2],
    depth: usize,
    source_jet: Option<SourceJetReceipt>,
    // Only a qualified C1 parent with complete relative tangents supplies this.
    // The enclosing verify call fixes the original operands, tube and frame.
    smooth_remainder: Option<([f64; 2], f64)>,
}
const EMPTY_SPAN: Span = Span {
    range: [0.0; 2],
    endpoints: [[Interval { lo: 0.0, hi: 0.0 }; 3]; 2],
    depth: 0,
    source_jet: None,
    smooth_remainder: None,
};
const STACK_BYTES: usize = size_of::<[Span; MAX_DEPTH + 1]>();

fn certify(
    input: Correspondence<'_>,
    policy: ProofPolicy,
    retained: Option<(&model::PreparedValidatedCurve<'static>, f64)>,
    position_bound: Option<f64>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    diagnostic: Option<&mut CorrespondenceDiagnostic>,
) -> Result<Option<PolicyProof>, GeometryError> {
    certify_scoped(input, policy, retained, position_bound, None, None, work, cancel, diagnostic)
}

fn certify_scoped(
    input: Correspondence<'_>,
    policy: ProofPolicy,
    retained: Option<(&model::PreparedValidatedCurve<'static>, f64)>,
    position_bound: Option<f64>,
    selected: Option<&SelectedMasterImage<'_>>,
    nodes: Option<&mut usize>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    mut diagnostic: Option<&mut CorrespondenceDiagnostic>,
) -> Result<Option<PolicyProof>, GeometryError> {
    let Correspondence {
        curve,
        surface,
        chart,
        maximum_error_mm: tolerance,
        ..
    } = input;
    if let Some(diagnostic) = diagnostic.as_deref_mut() {
        diagnostic.decline = None;
    }
    work.charge(1, cancel)?;
    let Some((a, b)) = curve.try_range_tuple() else {
        if let Some(diagnostic) = diagnostic.as_deref_mut() {
            diagnostic.decline = Some(CorrespondenceDecline {
                predicate: "native-curve-range-unavailable",
                ..Default::default()
            });
        }
        return Ok(None);
    };
    // A mapped original basis may have a larger natural domain than its retained
    // EDGE_CURVE restriction; conversely a periodic child is narrower than its
    // shared whole-original receipt. Both independent authorities must hold.
    let (a, b) = retained.map_or((a, b), |(prepared, _)| {
        let range = prepared.range();
        (a.max(range[0]), b.min(range[1]))
    });
    let Some((c, d)) = chart.try_range_tuple() else {
        if let Some(diagnostic) = diagnostic.as_deref_mut() {
            diagnostic.decline = Some(CorrespondenceDecline {
                predicate: "chart-range-unavailable",
                range: Some([a, b]),
                ..Default::default()
            });
        }
        return Ok(None);
    };
    let (a, b, c, d) = if let Some(selected) = selected {
        // Only the function-bound original selected image can supply this
        // explicit child domain. Never try the reversed EitherPosition branch.
        if policy != ProofPolicy::ForwardPosition || position_bound.is_some()
            || retained.is_none_or(|(owner, scale)|
                !std::ptr::eq(owner, selected.prepared) || scale != 1.0)
            || !std::ptr::eq(input.curve, selected.input.curve)
            || !std::ptr::eq(input.surface, selected.input.surface)
            || !std::ptr::eq(input.chart, selected.input.chart)
            || input.margin_uv != [0.0; 2] || !selected.map.identity
            || selected.map.source[0] < a || selected.map.source[1] > b
            || selected.map.target[0] < selected.authorized_target[0]
            || selected.map.target[1] > selected.authorized_target[1]
        {
            return Ok(None);
        }
        (selected.map.source[0], selected.map.source[1],
            selected.map.target[0], selected.map.target[1])
    } else {
        (a, b, c, d)
    };
    Interval::new(a, b)?;
    Interval::new(c, d)?;
    if a == b || c == d {
        if let Some(diagnostic) = diagnostic.as_deref_mut() {
            diagnostic.decline = Some(CorrespondenceDecline {
                predicate: "zero-native-or-chart-range",
                range: Some([a, b]),
                ..Default::default()
            });
        }
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
        implicit: if let Some((prepared, scale)) = retained {
            Some(PreparedCurveEnclosure::borrowed_restricted(
                prepared,
                scale,
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
    let original_retained = retained.map(|(prepared, _)| prepared);
    let retained = source
        .implicit
        .as_ref()
        .map_or(0, PreparedCurveEnclosure::retained_bytes);
    let available = MAX_SCRATCH_BYTES
        .checked_sub(SCRATCH_OVERHEAD_BYTES)
        .and_then(|bytes| bytes.checked_sub(retained))
        .ok_or_else(|| resource("native pcurve source capture exhausts scratch capacity"))?;
    let mut cuts = Vec::new();
    let mut local_nodes = 0;
    let nodes = nodes.unwrap_or(&mut local_nodes);
    let targets = [[c, d], [d, c]];
    let target_count = if policy == ProofPolicy::EitherPosition {
        2
    } else {
        1
    };
    for &target in &targets[..target_count] {
        if let Some(diagnostic) = diagnostic.as_deref_mut() {
            diagnostic.decline = None;
        }
        let map = AffineMap::new([a, b], target)?;
        let mut harmonic_diagnostic = super::harmonic::HarmonicDiagnostic::new(tolerance);
        let harmonic_started = work.consumed();
        if source.implicit.is_none()
            && let Some(bound) = resolution(super::harmonic::certify(
                super::harmonic::HarmonicComparison {
                    source: super::harmonic::HarmonicSource::Curve(source.original),
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
            return Ok(Some(PolicyProof::completed(
                input, map, bound, original_retained, policy, work, cancel,
            )?));
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
                position_bound,
                nodes,
                work,
                cancel,
                diagnostic.as_deref_mut(),
            )
        })();
        let fallback_work = work.consumed() - fallback_started;
        if let Some(diagnostic) = diagnostic.as_deref_mut() {
            diagnostic.record(FallbackDiagnostic {
                source: diagnostic.active_source,
                harmonic: harmonic_diagnostic,
                harmonic_work,
                fallback_work,
                nodes: *nodes,
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
            return Ok(Some(PolicyProof::completed(
                input,
                map,
                bound,
                original_retained,
                policy,
                work,
                cancel,
            )?));
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
type SplitMidpoint = (f64, [Interval; 3], Interval);

// A curve wrapper may enter its parameter-curve chart with the same fixed map.
const SCRATCH_OVERHEAD_BYTES: usize = STACK_BYTES
    + 2 * size_of::<CutMap>()
    + super::harmonic::SCRATCH_BYTES
    + operand_binding::SCRATCH_BYTES
    + size_of::<super::harmonic::HarmonicDiagnostic>()
    + 2 * size_of::<CorrespondenceDiagnostic>()
    + size_of::<Option<EvaluationCost>>()
    + size_of::<Option<SplitMidpoint>>()
    + size_of::<Option<[Interval; 3]>>()
    + 2 * size_of::<[Interval; 3]>()
    + size_of::<Option<FailedSpanDiagnostic>>()
    + size_of::<Option<&'static str>>()
    + size_of::<super::implicit::RefinementDiagnostic>()
    + size_of::<RetainedPosition<'static>>()
    + 2 * size_of::<SelectedMasterImage<'static>>()
    + size_of::<GeometryWorkBudget>()
    + 3 * size_of::<usize>()
    + size_of::<CorrespondenceDecline>();

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
    // Correspondence compares complete one-sided bounds. C1 is qualified
    // independently before a Taylor residual is used below.
    let result = model::validated_curve2d_piecewise_jet(
        chart, [range.lo, range.hi], &mut native, cancel,
    );
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
    parametric::surface_piecewise_enclosure(surface, expanded_chart_domain(uv, margin_uv), work, cancel)
}

fn expanded_chart_domain(
    uv: &ChartCurveEnclosure,
    margin_uv: [f64; 2],
) -> [[f64; 2]; 2] {
    std::array::from_fn(|axis| {
        let enclosure = uv.position[axis].add(Interval {
            lo: -margin_uv[axis],
            hi: margin_uv[axis],
        });
        [enclosure.lo, enclosure.hi]
    })
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
    mut cost: Option<&mut EvaluationCost>,
) -> Result<[Interval; 3], GeometryError> {
    let MappedCorrespondence {
        source,
        surface,
        chart,
        map,
        policy,
    } = *input;
    let margin_uv = policy.margin_uv();
    let c = counted_query(
        cost.as_deref_mut().map(|cost| &mut cost.source_endpoint), work,
        |work| source.endpoint(t, work, cancel),
    )?;
    let s = counted_query(
        cost.as_deref_mut().map(|cost| &mut cost.target_endpoint), work, |work| {
            let uv = chart_jet(chart, map.image([t, t])?, work, cancel)?;
            parametric::surface_position_enclosure(
                surface, expanded_chart_domain(&uv, margin_uv), work, cancel,
            )
        },
    )?;
    Ok(std::array::from_fn(|axis| c[axis].sub(s[axis])))
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
    second: [Interval; 3],
    tangent: Option<[Interval; 3]>,
    position: [Interval; 3],
    smooth: bool,
}

fn cell_enclosure(
    input: &MappedCorrespondence<'_, '_>,
    range: [f64; 2],
    position_required: bool,
    parent_source: Option<&SourceJetReceipt>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    mut cost: Option<&mut EvaluationCost>,
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
    let chart_range = map.image(range)?;
    let uv = chart_jet(chart, chart_range, work, cancel)?;
    let s = composed_surface(surface, &uv, margin_uv, work, cancel)?;
    let [du, dv] = uv.derivative;
    let tangent = require_tangent.then(|| {
        std::array::from_fn(|axis| s.du[axis].mul(du).add(s.dv[axis].mul(dv)).mul(map.slope))
    });
    if !position_required {
        return Ok(CellEnclosure {
            second: [Interval::point(0.0); 3],
            tangent,
            position: s.position,
            smooth: false,
        });
    }
    let [ddu, ddv] = uv.second_derivative;
    let slope_squared = map.slope.square();
    let second = std::array::from_fn(|axis| {
        s.duu[axis]
            .mul(du.square())
            .add(s.duv[axis].mul(du).mul(dv).mul(Interval::point(2.0)))
            .add(s.dvv[axis].mul(dv.square()))
            .add(s.du[axis].mul(ddu))
            .add(s.dv[axis].mul(ddv))
            .mul(slope_squared)
    });
    let source_smooth = if let Some(parent) = parent_source
        && matches!(parent.qualification, SourceJetQualification::C1)
    {
        parent.check_child(range)?;
        true
    } else {
        counted_query(cost.as_deref_mut().map(|cost| &mut cost.source_c1), work,
            |work| source_span_smooth(source, range, work, cancel))?
    };
    // Only source C1 is inherited. Keep this actual child chart and surface
    // box qualified independently before using their piecewise Hessians.
    let smooth = source_smooth
        && chart_span_smooth(chart, chart_range, work, cancel)?
        && surface_span_smooth(surface, &uv, margin_uv, work, cancel)?;
    Ok(CellEnclosure {
        second,
        tangent,
        position: s.position,
        smooth,
    })
}

fn compare_cell(
    cell: &CellEnclosure,
    source: CurveEnclosure,
    range: [f64; 2],
    endpoints: [Interval; 2],
    position_bound: Option<f64>,
) -> Result<(f64, TangentAgreement, Option<f64>), GeometryError> {
    let tangent = match cell.tangent {
        Some(chart) => relative_tangent_agreement(source.derivative, chart)?,
        None => TangentAgreement::Proved,
    };
    let mut remainder = None;
    let bound = if let Some(bound) = position_bound {
        bound
    } else if cell.smooth {
        // Preserve vector-component cancellation before taking the norm.
        let second_residual = std::array::from_fn(|axis| {
            source.second_derivative[axis].sub(cell.second[axis])
        });
        let second_upper = norm(second_residual)?.hi;
        remainder = Some(second_upper);
        residual_taylor_upper(range, endpoints, second_upper)
    } else {
        // Piecewise Hessian hulls do not prove a Taylor bound across C0 joints.
        norm(std::array::from_fn(|axis| {
            source.position[axis].sub(cell.position[axis])
        }))?.hi
    };
    Ok((bound, tangent, remainder))
}

fn residual_taylor_upper(
    range: [f64; 2],
    endpoints: [Interval; 2],
    second_upper: f64,
) -> f64 {
    let width = Interval::point(range[1]).sub(Interval::point(range[0]));
    Interval::point(endpoints[0].hi.max(endpoints[1].hi))
        .add(Interval::point(second_upper)
            .mul(width.square()).mul(Interval::point(0.125)))
        .hi
}

fn push_split(
    pending: &mut [Span],
    length: &mut usize,
    parent: &Span,
    middle: f64,
    midpoint: &[Interval; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    if *length + 1 >= pending.len() {
        return Err(resource("native pcurve interval proof fixed stack exhausted"));
    }
    work.charge(2, cancel)?;
    pending[*length] = Span {
        range: [middle, parent.range[1]],
        endpoints: [*midpoint, parent.endpoints[1]],
        depth: parent.depth + 1,
        source_jet: parent.source_jet,
        smooth_remainder: parent.smooth_remainder,
    };
    pending[*length + 1] = Span {
        range: [parent.range[0], middle],
        endpoints: [parent.endpoints[0], *midpoint],
        depth: parent.depth + 1,
        source_jet: parent.source_jet,
        smooth_remainder: parent.smooth_remainder,
    };
    *length += 2;
    Ok(())
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    position_bound: Option<f64>,
    nodes: &mut usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    mut diagnostic: Option<&mut CorrespondenceDiagnostic>,
) -> Result<Option<f64>, GeometryError> {
    let mut cost = diagnostic.as_ref().map(|_| EvaluationCost::default());
    let started = work.consumed();
    let result = verify_cells(
        input, cuts, tolerance, position_bound, nodes, work, cancel,
        diagnostic.as_deref_mut(), cost.as_mut(),
    );
    if let (Some(diagnostic), Some(mut cost)) = (diagnostic, cost) {
        cost.total_work = work.consumed() - started;
        diagnostic.evaluation = Some(cost);
    }
    result
}

fn verify_cells(
    input: &MappedCorrespondence<'_, '_>,
    cuts: &[f64],
    tolerance: f64,
    position_bound: Option<f64>,
    nodes: &mut usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    mut diagnostic: Option<&mut CorrespondenceDiagnostic>,
    mut cost: Option<&mut EvaluationCost>,
) -> Result<Option<f64>, GeometryError> {
    let mut pending = [EMPTY_SPAN; MAX_DEPTH + 1];
    let oracle = diagnostic.as_ref().map(|_| {
        if input.source.implicit.is_none() { return "static" }
        let mut curve = input.source.original;
        for _ in 0..MAX_WRAPPER_DEPTH {
            let model::Curve::TrimmedCurve(wrapper) = curve else { break };
            curve = wrapper.entity().curve().as_ref();
        }
        if matches!(curve, model::Curve::IntersectionCurve(_)) {
            "implicit-prepared"
        } else {
            "explicit-prepared"
        }
    });
    let mut achieved: f64 = 0.0;
    let endpoint = |t, work: &mut GeometryWorkBudget, cost: Option<&mut EvaluationCost>| {
        if position_bound.is_some() {
            Ok([Interval::point(0.0); 3])
        } else {
            residual(input, t, work, cancel, cost)
        }
    };
    let mut front = endpoint(cuts[0], work, cost.as_deref_mut())?;
    for pair in cuts.windows(2) {
        let back = endpoint(pair[1], work, cost.as_deref_mut())?;
        pending[0] = Span {
            range: [pair[0], pair[1]],
            endpoints: [front, back],
            depth: 0,
            source_jet: None,
            smooth_remainder: None,
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
            let mut span = pending[length];
            let endpoints = [norm(span.endpoints[0])?, norm(span.endpoints[1])?];
            let mut declined = CorrespondenceDecline {
                range: Some(span.range),
                depth: Some(span.depth),
                endpoint_norms: Some(endpoints.map(|v| [v.lo, v.hi])),
                ..Default::default()
            };
            if endpoints.iter().any(|bound| bound.lo > tolerance) {
                declined.predicate = "endpoint-position-lower-exceeds-allowed";
                if let Some(diagnostic) = diagnostic.as_deref_mut() {
                    diagnostic.decline = Some(declined);
                }
                return Ok(None);
            }
            let mut failure = oracle.map(|oracle| FailedSpanDiagnostic {
                range: span.range, depth: span.depth, goal: tolerance,
                map: (input.map.source, input.map.target, [input.map.slope.lo, input.map.slope.hi]),
                endpoint_norms: endpoints.map(|bound| [bound.lo, bound.hi]),
                parent_m: None, child_bounds: [None; 2], stage: "cell",
                coedge: matches!(input.policy, ProofPolicy::DeclaredCoedge(_)),
                position_bound: position_bound.is_some(), oracle,
            });
            let mut midpoint_cache: Option<SplitMidpoint> = None;
            // The parent's C1 and relative-tangent proof restrict to every
            // contained child. Fresh endpoint residuals plus its complete
            // residual-Hessian norm can finish the SAME Taylor theorem
            // without reevaluating either original expression.
            if let Some((parent, second_upper)) = span.smooth_remainder {
                if !(parent[0] <= span.range[0] && span.range[0] <= span.range[1]
                    && span.range[1] <= parent[1])
                {
                    return Err(GeometryError::new(
                        GeometryErrorCode::InvalidGeometry,
                        "inherited correspondence remainder does not contain its child interval",
                    ));
                }
                let bound = residual_taylor_upper(span.range, endpoints, second_upper);
                if let Some(failure) = &mut failure {
                    failure.parent_m = Some((parent, second_upper, bound));
                }
                if bound <= tolerance {
                    achieved = achieved.max(bound);
                    continue;
                }
                // One ordinary split can finish both children using this SAME
                // completed C1/full-residual-Hessian receipt. Coedge policy
                // also retains its parent tangent proof; position-only policy
                // grants no tangent or UV authority.
                if position_bound.is_none()
                    && matches!(input.policy, ProofPolicy::DeclaredCoedge(_) | ProofPolicy::ForwardPosition)
                    && span.depth < MAX_DEPTH
                    && length + 1 < pending.len()
                    && *nodes <= MAX_NODES - 2
                {
                    let middle = span.range[0] * 0.5 + span.range[1] * 0.5;
                    if span.range[0] < middle && middle < span.range[1] {
                        if let Some(failure) = &mut failure { failure.stage = "lookahead-point"; }
                        let midpoint = endpoint(middle, work, cost.as_deref_mut()).inspect_err(|_| {
                            if let Some(diagnostic) = diagnostic.as_deref_mut() {
                                diagnostic.failed_span = failure;
                            }
                        })?;
                        let midpoint_norm = norm(midpoint)?;
                        let left_bound = residual_taylor_upper(
                            [span.range[0], middle], [endpoints[0], midpoint_norm], second_upper,
                        );
                        if let Some(failure) = &mut failure {
                            failure.child_bounds[0] = Some(left_bound);
                        }
                        if left_bound <= tolerance {
                            let right_bound = residual_taylor_upper(
                                [middle, span.range[1]], [midpoint_norm, endpoints[1]], second_upper,
                            );
                            if let Some(failure) = &mut failure {
                                failure.child_bounds[1] = Some(right_bound);
                            }
                            if right_bound <= tolerance {
                                // Same proof, push/pop and node/work admission.
                                push_split(&mut pending, &mut length, &span, middle, &midpoint, work, cancel)?;
                                continue;
                            }
                        }
                        // Nonproof retains the ordinary fresh-jet route. Its
                        // later split must not evaluate this midpoint twice.
                        midpoint_cache = Some((middle, midpoint, midpoint_norm));
                    }
                }
            }
            let mut source_jet = span.source_jet;
            let mut smooth_remainder = span.smooth_remainder;
            let attempt = (|| {
                if let Some(parent) = &source_jet {
                    parent.check_child(span.range)?;
                }
                let cell_started = cost.as_ref().map(|cost| (work.consumed(), cost.source_c1.work));
                if let Some(failure) = &mut failure { failure.stage = "target-cell"; }
                let evaluated = cell_enclosure(
                    input, span.range, position_bound.is_none(),
                    if position_bound.is_none() { source_jet.as_ref() } else { None },
                    work, cancel, cost.as_deref_mut(),
                );
                if let (Some(cost), Some((started, source_c1))) = (cost.as_deref_mut(), cell_started) {
                    let source_c1_work = cost.source_c1.work - source_c1;
                    cost.target_cell.record(work.consumed() - started - source_c1_work);
                }
                let cell = evaluated?;
                if let Some(parent) = source_jet {
                    // Inherited bounds only grant a COMPLETE positive. A
                    // decline (including overflow of this coarser arithmetic)
                    // requests the same tighter original-branch jet as before.
                    if cell.tangent.is_some()
                        && let Some(cost) = cost.as_deref_mut()
                    {
                        cost.tangent_comparisons += 1;
                    }
                    if let Some(failure) = &mut failure { failure.stage = "inherited-jet"; }
                    match compare_cell(&cell, parent.jet, span.range, endpoints, position_bound) {
                        Ok((bound, tangent, _))
                            if bound <= tolerance && tangent == TangentAgreement::Proved =>
                        {
                            return Ok((bound, tangent));
                        }
                        Ok(_) => {}
                        Err(cause) if cause.code == GeometryErrorCode::ConditioningFailure => {}
                        Err(cause) => return Err(cause),
                    }
                }
                // A complete position receipt leaves only the actual full-tube
                // tangent obligation. The same retained root's piecewise first
                // jet supplies no position, Hessian or smoothness authority.
                if let Some(bound) = position_bound
                    && bound <= tolerance
                    && let Some(target) = cell.tangent
                {
                    if let Some(failure) = &mut failure { failure.stage = "retained-tangent"; }
                    let retained = counted_query(
                        cost.as_deref_mut().map(|cost| &mut cost.source_jet), work,
                        |work| input.source.retained_first(span.range, work, cancel),
                    )?;
                    if let Some(first) = retained {
                        if let Some(cost) = cost.as_deref_mut() {
                            cost.tangent_comparisons += 1;
                        }
                        match relative_tangent_agreement(first, target) {
                            Ok(TangentAgreement::Proved) =>
                                return Ok((bound, TangentAgreement::Proved)),
                            Ok(_) => {}
                            Err(cause) if cause.code == GeometryErrorCode::ConditioningFailure => {}
                            Err(cause) => return Err(cause),
                        }
                    }
                    // Broad-box nonproof cannot reject or replace the fresh
                    // original-branch query below.
                }
                if let Some(failure) = &mut failure { failure.stage = "fresh-source-jet"; }
                let refined = counted_query(
                    cost.as_deref_mut().map(|cost| &mut cost.source_jet), work,
                    |work| input.source.jet(span.range, work, cancel),
                )?;
                // Never carry a piecewise Hessian across an unqualified
                // parent. Tangent-only cells do not use second derivatives.
                source_jet = (cell.smooth || position_bound.is_some()).then_some(SourceJetReceipt {
                    range: span.range,
                    jet: refined,
                    qualification: if cell.smooth {
                        SourceJetQualification::C1
                    } else {
                        SourceJetQualification::TangentOnly
                    },
                });
                if cell.tangent.is_some()
                    && let Some(cost) = cost.as_deref_mut()
                {
                    cost.tangent_comparisons += 1;
                }
                if let Some(failure) = &mut failure { failure.stage = "fresh-comparison"; }
                let (bound, tangent, remainder) =
                    compare_cell(&cell, refined, span.range, endpoints, position_bound)?;
                smooth_remainder = if tangent == TangentAgreement::Proved {
                    remainder.map(|second_upper| (span.range, second_upper))
                } else {
                    None
                };
                Ok((bound, tangent))
            })();
            match attempt {
                Ok((bound, tangent)) => {
                    declined.position_upper = Some(bound);
                    declined.tangent = Some(tangent);
                    declined.predicate = if tangent == TangentAgreement::Unresolved {
                        "relative-tangent-unresolved"
                    } else {
                        "whole-position-upper-exceeds-allowed"
                    };
                    if tangent == TangentAgreement::Disproved {
                        declined.predicate = "relative-tangent-disproved";
                        if let Some(diagnostic) = diagnostic.as_deref_mut() {
                            diagnostic.decline = Some(declined);
                        }
                        return Ok(None);
                    }
                    if bound <= tolerance && tangent == TangentAgreement::Proved {
                        achieved = achieved.max(bound);
                        continue;
                    }
                }
                Err(error) if terminal(&error) => {
                    declined.cause = Some(error.code);
                    if let Some(snapshot) = failure {
                        declined.predicate = snapshot.stage;
                    }
                    if let Some(diagnostic) = diagnostic.as_deref_mut() {
                        diagnostic.failed_span = failure;
                        diagnostic.decline = Some(declined);
                    }
                    return Err(error);
                }
                Err(error)
                    if !matches!(
                        error.code,
                        GeometryErrorCode::ConditioningFailure
                            | GeometryErrorCode::AnalysisIndeterminate
                            | GeometryErrorCode::SingularGeometry
                    ) =>
                {
                    declined.predicate = "cell-enclosure-nonterminal-error";
                    declined.cause = Some(error.code);
                    if let Some(diagnostic) = diagnostic.as_deref_mut() {
                        diagnostic.decline = Some(declined);
                    }
                    return Ok(None);
                }
                Err(error) => {
                    declined.predicate = "cell-enclosure-conditioning-unresolved";
                    declined.cause = Some(error.code);
                }
            }
            let middle = midpoint_cache.as_ref().map_or_else(
                || span.range[0] * 0.5 + span.range[1] * 0.5,
                |(middle, _, _)| *middle,
            );
            if span.depth == MAX_DEPTH || middle <= span.range[0] || middle >= span.range[1] {
                declined.stop = Some(if span.depth == MAX_DEPTH {
                    "subdivision-depth"
                } else {
                    "unrepresentable-midpoint"
                });
                if let Some(diagnostic) = diagnostic.as_deref_mut() {
                    diagnostic.decline = Some(declined);
                }
                return Ok(None);
            }
            let (midpoint, midpoint_norm) = match midpoint_cache {
                Some((_, midpoint, midpoint_norm)) => (midpoint, midpoint_norm),
                None => {
                    if let Some(failure) = &mut failure { failure.stage = "split-point"; }
                    let midpoint = endpoint(middle, work, cost.as_deref_mut()).inspect_err(|_| {
                        if let Some(diagnostic) = diagnostic.as_deref_mut() {
                            diagnostic.failed_span = failure;
                        }
                    })?;
                    (midpoint, norm(midpoint)?)
                }
            };
            if midpoint_norm.lo > tolerance {
                declined.predicate = "midpoint-position-lower-exceeds-allowed";
                declined.midpoint_norm = Some([midpoint_norm.lo, midpoint_norm.hi]);
                if let Some(diagnostic) = diagnostic.as_deref_mut() {
                    diagnostic.decline = Some(declined);
                }
                return Ok(None);
            }
            // Same depth-first stack admission for ordinary and proved splits.
            span.source_jet = source_jet;
            span.smooth_remainder = smooth_remainder;
            push_split(&mut pending, &mut length, &span, middle, &midpoint, work, cancel)?;
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
        assert!((1.0e-9..=1.0e-7).contains(&bound.position_error_mm()));
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
        let cancel = AtomicBool::new(false);
        let prepared = model::prepared_validated_curve_owned(
            curve, [0.2, 0.8], &model::ValidatedCurveOptions::default(),
            &mut model::ValidatedGeometryBudget::new(MAX_WORK as usize), &cancel,
        ).unwrap();
        let view = PreparedCurveEnclosure::borrowed(&prepared, 1.0);
        let mut work = GeometryWorkBudget::new(MAX_WORK);
        // Optional preparation layout is not this trim/pose contract. Any
        // retained answer must enclose the actual reversed derivative.
        if let Some(retained) = view.retained_first_enclosure((0.3, 0.4), &mut work, &cancel).unwrap() {
            assert!(retained[0].lo <= -1.0 && retained[0].hi >= -1.0);
        }
        let fresh = view.jet((0.3, 0.4), &mut work, &cancel).unwrap();
        assert!(fresh.derivative[0].lo <= -1.0 && fresh.derivative[0].hi >= -1.0);
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
        .map(|proof| proof.position_error_mm())
    }

    #[test]
    fn sealed_affine_native_uv_map_rejects_changed_qualified_gauges() {
        let surface = plane();
        let curve = |a, b| {
            model::Curve::BsplineCurve(model::BsplineCurve::new(
                knots(a, b, 1),
                vec![Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0)],
            ))
        };
        let native_a = curve(10.0, 12.0);
        let native_b = curve(20.0, 22.0);
        let chart_a =
            model::Curve2D::Line(model::Line(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0)));
        let chart_b = model::Curve2D::BsplineCurve(model::BsplineCurve::new(
            knots(2.0, 4.0, 1),
            vec![Point2::new(0.0, 0.0), Point2::new(1.0, 0.0)],
        ));
        let prove = |curve, chart| {
            certify_declared_coedge_correspondence(
                Correspondence {
                    curve,
                    surface: &surface,
                    chart,
                    maximum_error_mm: 1e-7,
                    margin_uv: [0.0; 2],
                },
                &mut GeometryWorkBudget::new(MAX_WORK),
                &AtomicBool::new(false),
                None,
            )
            .unwrap()
            .expect("actual whole-source affine correspondence")
        };
        let first = prove(&native_a, &chart_a);
        let independent = prove(&native_a, &chart_a);
        let native_gauge = prove(&native_b, &chart_a);
        let uv_gauge = prove(&native_a, &chart_b);
        assert!(first.same_affine_native_uv_map(&independent));
        assert!(!first.same_affine_native_uv_map(&native_gauge));
        assert!(!first.same_affine_native_uv_map(&uv_gauge));
        let check_preimage = |proof: OriginalCoedgeCorrespondence, uv, expected| {
            let image = proof
                .affine_native_parameter_preimage(uv)
                .unwrap()
                .expect("completed original affine correspondence");
            assert!(image.lo <= expected && expected <= image.hi);
        };
        check_preimage(first, [0.0, 0.0], 10.0);
        check_preimage(first, [0.5, 0.5], 11.0);
        check_preimage(first, [1.0, 1.0], 12.0);
        check_preimage(native_gauge, [0.5, 0.5], 21.0);
        check_preimage(uv_gauge, [3.0, 3.0], 11.0);
        assert_eq!(
            first
                .affine_native_parameter_preimage([2.0, 4.0])
                .unwrap_err()
                .code,
            GeometryErrorCode::ParameterCorrespondenceUnavailable,
        );
        let mut completed_pole = first;
        completed_pole.complete_pole_graph();
        assert!(
            completed_pole
                .affine_native_parameter_preimage([0.0, 1.0])
                .unwrap()
                .is_none(),
            "a completed graph must not inherit an unproved affine inverse",
        );
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
            assert!((carrier.subs(0.0, angle) - expected).magnitude() <= error.position_error_mm());
            assert!((original.subs(angle) - expected).magnitude() <= error.position_error_mm());
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
    fn retained_physical_image_factor_keeps_root_and_current_operand_authority() {
        let range = (0.0, 0.125);
        let selected_chart = model::Curve2D::Conic(model::Conic2D::Ellipse(
            model::Processor::<_, Matrix3>::new(model::TrimmedCurve::new(
                model::UnitCircle::<Point2>::new(), range,
            )),
        ));
        let selected_surface = plane();
        let guide_surface = model::Surface::Plane(model::Plane::from_origin_vectors(
            Point3::new(0.0, 0.0, 0.02), Vector3::unit_x(), Vector3::unit_y(),
        ));
        let mut cylinder = model::Processor::<_, Matrix4>::new(
            model::RevolutionSurface::by_revolution(
                model::Curve::Line(model::Line(
                    Point3::new(1.0, 0.0, -1.0), Point3::new(1.0, 0.0, 0.0),
                )),
                Point3::new(0.0, 0.0, -1.0), Vector3::unit_z(),
            ),
        );
        cylinder.invert();
        let current_surface = model::Surface::RevolutionSurface(cylinder);
        let line = |v| model::Curve2D::TrimmedCurve(model::Processor::<_, Matrix3>::new(
            model::TrimmedCurve::new(Box::new(model::Curve2D::Line(model::Line(
                Point2::new(0.0, v), Point2::new(1.0, v),
            ))), range),
        ));
        let current_chart = line(1.0);
        let curve = model::Curve::IntersectionCurve(model::SurfaceCurve::with_boundaries(
            Box::new(selected_surface.clone()), Box::new(current_surface.clone()),
            Box::new(model::Curve::ParameterCurve(model::ParameterCurve::new(
                selected_chart.clone(), Box::new(guide_surface),
            ))),
            Some(model::ParameterCurve::new(
                selected_chart.clone(), Box::new(selected_surface.clone()),
            )),
            Some(model::ParameterCurve::new(
                current_chart.clone(), Box::new(current_surface.clone()),
            )),
        ));
        let cancel = AtomicBool::new(false);
        let prepare = || model::prepared_validated_curve_owned(
            curve.clone(), [range.0, range.1], &model::ValidatedCurveOptions::default(),
            &mut model::ValidatedGeometryBudget::new(MAX_WORK as usize), &cancel,
        ).unwrap();
        let prepared = prepare();
        let mut work = GeometryWorkBudget::new(MAX_WORK);
        let selected = Correspondence {
            curve: &curve, surface: &selected_surface, chart: &selected_chart,
            maximum_error_mm: 1.0e-7, margin_uv: [0.0; 2],
        };
        let master = certify_selected_master_correspondence(
            selected, (&prepared, 1.0), None, &mut work, &cancel,
        ).unwrap().unwrap();
        let current = Correspondence {
            surface: &current_surface, chart: &current_chart, ..selected
        };
        let position = master.factor_position(
            current, &prepared, None, &mut work, &cancel, None,
        ).unwrap().unwrap();
        let bound = position.bound(current, (&prepared, 1.0)).unwrap().unwrap();
        assert!(bound <= current.maximum_error_mm);
        assert!(position.bound(current, (&prepared, 25.4)).unwrap().is_none());
        assert!(certify_retained(
            current, (&prepared, 1.0), Some(&position), &mut work, &cancel, None,
        ).unwrap().is_some());
        let (_, completed) = complete_retained_correspondence(
            current, &prepared, 1.0, Some(&master), &mut work, &cancel, None,
        ).unwrap().unwrap();
        assert!(completed.position_error_mm() <= current.maximum_error_mm);
        // The comparison is a declared plane image, not the displaced guide,
        // and a different owner cannot inherit its selected-image binding.
        let foreign = prepare();
        assert!(master.factor_position(
            current, &foreign, None, &mut work, &cancel, None,
        ).unwrap().is_none());
        let displaced = line(1.001);
        let displaced_input = Correspondence { chart: &displaced, ..current };
        assert!(position.bound(displaced_input, (&prepared, 1.0)).unwrap().is_none());
        assert!(master.factor_position(
            displaced_input, &prepared, None, &mut work, &cancel, None,
        ).unwrap().is_none());
        let mut reversed = current_chart.clone();
        reversed.invert();
        assert!(master.factor_position(
            Correspondence { chart: &reversed, ..current },
            &prepared, None, &mut work, &cancel, None,
        ).unwrap().is_none());
        assert!(master.factor_position(
            Correspondence { maximum_error_mm: 1.0e-14, ..current },
            &prepared, None, &mut work, &cancel, None,
        ).unwrap().is_none());
        assert_eq!(master.factor_position(
            current, &prepared, None, &mut GeometryWorkBudget::new(0), &cancel, None,
        ).err().unwrap().code, GeometryErrorCode::ResourceLimit);
        assert_eq!(master.factor_position(
            current, &prepared, None, &mut work, &AtomicBool::new(true), None,
        ).err().unwrap().code, GeometryErrorCode::Cancelled);
    }

    #[test]
    fn retained_tangent_nonproof_keeps_fresh_derivatives_and_position_binding() {
        let curve = model::Curve::BsplineCurve(model::BsplineCurve::new(
            knots(0.0, 1.0, 2),
            vec![Point3::new(0.0, 0.0, 0.0), Point3::new(0.5, 0.0, 0.0),
                Point3::new(1.0, 1.0, 0.0)],
        ));
        let chart = model::Curve2D::BsplineCurve(model::BsplineCurve::new(
            knots(0.0, 1.0, 2),
            vec![Point2::new(0.0, 0.0), Point2::new(0.5, 0.0), Point2::new(1.0, 1.0)],
        ));
        let surface = plane();
        let cancel = AtomicBool::new(false);
        let prepared = model::prepared_validated_curve_owned(
            curve.clone(), [0.0, 1.0], &model::ValidatedCurveOptions::default(),
            &mut model::ValidatedGeometryBudget::new(MAX_WORK as usize), &cancel,
        ).unwrap();
        let source = Source {
            original: &curve,
            implicit: Some(PreparedCurveEnclosure::borrowed(&prepared, 1.0)),
        };
        let mut work = GeometryWorkBudget::new(MAX_WORK);
        let mapped = MappedCorrespondence {
            source: &source, surface: &surface, chart: &chart,
            map: AffineMap::new([0.0, 1.0], [0.0, 1.0]).unwrap(),
            policy: ProofPolicy::DeclaredCoedge([0.0; 2]),
        };
        let range = [0.0, 0.01];
        let target = cell_enclosure(
            &mapped, range, false, None, &mut work, &cancel, None,
        ).unwrap().tangent.unwrap();
        let coarse = source.retained_first(range, &mut work, &cancel).unwrap().unwrap();
        assert_ne!(
            relative_tangent_agreement(coarse, target).unwrap(), TangentAgreement::Proved,
            "whole-piece uncertainty cannot prove this child tangent",
        );
        let fresh = source.jet(range, &mut work, &cancel).unwrap();
        assert_eq!(
            relative_tangent_agreement(fresh.derivative, target).unwrap(),
            TangentAgreement::Proved,
        );
        let input = Correspondence {
            curve: &curve, surface: &surface, chart: &chart,
            maximum_error_mm: 1.0e-7, margin_uv: [0.0; 2],
        };
        let proof = certify_retained_coedge_correspondence(
            input, (&prepared, 1.0), &mut work, &cancel, None,
        ).unwrap().unwrap()
            .seal_retained(input, (&prepared, 1.0), &mut work, &cancel).unwrap();
        assert!(certify_normalized_retained_correspondence(
            input, (&prepared, 1.0), Some(&proof), &mut work, &cancel, None,
        ).unwrap().is_some());
        // Identical derivatives do not authorize a displaced position.
        let displaced = model::Curve2D::BsplineCurve(model::BsplineCurve::new(
            knots(0.0, 1.0, 2),
            vec![Point2::new(0.001, 0.0), Point2::new(0.501, 0.0),
                Point2::new(1.001, 1.0)],
        ));
        assert!(certify_normalized_retained_correspondence(
            Correspondence { chart: &displaced, ..input }, (&prepared, 1.0),
            Some(&proof), &mut work, &cancel, None,
        ).unwrap().is_none());
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
        let mut processor = model::Processor::with_transform(
            model::TrimmedCurve::new(Box::new(curve.clone()), (0.0, 1.0)),
            Matrix4::from_translation(Vector3::new(3.0, 4.0, 5.0)),
        );
        processor.invert();
        let mapped = model::prepared_validated_curve_owned(
            model::Curve::TrimmedCurve(processor), [0.0, 1.0],
            &model::ValidatedCurveOptions::default(),
            &mut model::ValidatedGeometryBudget::new(MAX_WORK as usize), &cancel,
        ).unwrap();
        let mapped_view = PreparedCurveEnclosure::borrowed(&mapped, 1.0);
        assert!(mapped_view.retained_first_enclosure((0.2, 0.3), &mut work, &cancel)
            .unwrap().is_none());
        let fresh_mapped = mapped_view.jet((0.2, 0.3), &mut work, &cancel).unwrap();
        assert!(fresh_mapped.derivative[1].lo <= -1.0 && fresh_mapped.derivative[1].hi >= -1.0);
        assert!(fresh_mapped.position[0].lo <= 3.0 && fresh_mapped.position[0].hi >= 3.0);
        assert!(
            certify_retained_coedge_correspondence(
                Correspondence {
                    curve: &curve,
                    surface: &xy,
                    chart: &chart(0.0),
                    maximum_error_mm: 1.0e-7,
                    margin_uv: [0.0; 2],
                },
                (&prepared, 1.0),
                &mut work,
                &cancel,
                None,
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
                (&prepared, 1.0),
                &mut work,
                &cancel,
                None,
            )
            .unwrap()
            .is_none()
        );
        let selected_chart = chart(0.0);
        let input = Correspondence {
            curve: &curve,
            surface: &xy,
            chart: &selected_chart,
            maximum_error_mm: 1.0e-7,
            margin_uv: [0.0; 2],
        };
        let base = certify_retained_coedge_correspondence(
            input, (&prepared, 1.0), &mut work, &cancel, None,
        ).unwrap().unwrap();
        let inverse = super::super::implicit::certify_uv_margin(
            super::super::implicit::UvMarginInput {
                curve: &curve,
                prepared: &prepared,
                surface: &xy,
                chart: &selected_chart,
                physical_residual: base.position_error_mm(),
                coefficient_margin: input.margin_uv,
            },
            &mut work,
            &cancel,
        ).unwrap();
        let metric = inverse.refinement_limit(
            input.maximum_error_mm,
            super::super::implicit::original_point_tolerance_native(1.0),
            &mut work,
            &cancel,
            None,
        ).unwrap().unwrap();
        let final_input = Correspondence {
            margin_uv: inverse.margin_uv(),
            ..input
        };
        let position = RetainedPosition {
            input: final_input,
            prepared: &prepared,
            frame_scale: 1.0,
            proof: RetainedPositionProof::Expanded {
                coefficient_margin: input.margin_uv, base, metric,
            },
        };
        let proof = certify_retained(
            final_input, (&prepared, 1.0), Some(&position), &mut work, &cancel, None,
        ).unwrap().unwrap();
        assert!(proof.position_error_mm() <= input.maximum_error_mm);
        let sealed = proof.seal_retained(final_input, (&prepared, 1.0), &mut work, &cancel).unwrap();
        let same_curve = curve.clone();
        let same_surface = xy.clone();
        let same_chart = chart(-0.0);
        let same_input = Correspondence {
            curve: &same_curve,
            surface: &same_surface,
            chart: &same_chart,
            ..final_input
        };
        assert!(sealed.retained_operands.is_some());
        assert!(operand_binding::capture(same_input, (&prepared, 1.0), &mut work, &cancel)
            .unwrap() == sealed.retained_operands);
        let rebound = certify_normalized_retained_correspondence(
            same_input, (&prepared, 1.0), Some(&sealed), &mut work, &cancel, None,
        ).unwrap().unwrap();
        assert!(rebound.position_error_mm() <= input.maximum_error_mm);
        assert!(rebound.retained_operands.is_none());
        assert!(operand_binding::capture(
            Correspondence { margin_uv: [1.0e-8; 2], ..same_input },
            (&prepared, 1.0), &mut work, &cancel,
        ).unwrap() != sealed.retained_operands);
        assert!(operand_binding::capture(
            same_input, (&prepared, 25.4), &mut work, &cancel,
        ).unwrap() != sealed.retained_operands);
        let other_owner = model::prepared_validated_curve_owned(
            curve.clone(), [0.0, 1.0], &model::ValidatedCurveOptions::default(),
            &mut model::ValidatedGeometryBudget::new(MAX_WORK as usize), &cancel,
        ).unwrap();
        assert!(operand_binding::capture(
            same_input, (&other_owner, 1.0), &mut work, &cancel,
        ).unwrap() != sealed.retained_operands);
        let nonfinite_curve = model::Curve::Line(model::Line(
            Point3::new(f64::NAN, 0.0, 0.0), Point3::new(0.0, 1.0, 0.0),
        ));
        assert!(operand_binding::capture(
            Correspondence { curve: &nonfinite_curve, ..same_input },
            (&prepared, 1.0), &mut work, &cancel,
        ).unwrap().is_none());
        // A normalized replacement, changed chart or frame must obtain its
        // own completed forward receipt, even when ranges happen to match.
        let replacement = curve.clone();
        assert!(position.bound(
            Correspondence { curve: &replacement, ..final_input }, (&prepared, 1.0),
        ).unwrap().is_none());
        let displaced_chart = chart(0.05);
        assert!(operand_binding::capture(
            Correspondence { chart: &displaced_chart, ..same_input },
            (&prepared, 1.0), &mut work, &cancel,
        ).unwrap() != sealed.retained_operands);
        assert!(position.bound(
            Correspondence { chart: &displaced_chart, ..final_input }, (&prepared, 1.0),
        ).unwrap().is_none());
        assert!(position.bound(final_input, (&prepared, 2.0)).unwrap().is_none());
        // Final normalization changes the spatial frame and may replace the
        // UV parameter range. Recompute against those actual operands instead
        // of carrying the source-frame receipt across the conversion.
        let scale = 25.4;
        let mut normalized_curve = curve.clone();
        normalized_curve.transform_by(Matrix4::from_scale(scale));
        let mut normalized_surface = xy.clone();
        normalized_surface.transform_by(Matrix4::from_scale(scale));
        let normalized_chart = model::Curve2D::Line(model::Line(
            Point2::new(0.0, 0.0), Point2::new(0.0, 1.0),
        ));
        let normalized = certify_normalized_retained_correspondence(
            Correspondence {
                curve: &normalized_curve,
                surface: &normalized_surface,
                chart: &normalized_chart,
                maximum_error_mm: 1.0e-7,
                margin_uv: [1.0e-10; 2],
            },
            (&prepared, scale),
            Some(&sealed),
            &mut work,
            &cancel,
            None,
        ).unwrap().unwrap();
        assert!(normalized.position_error_mm() <= 1.0e-7);
        assert_eq!(normalized.native_range(), [0.0, 1.0]);
        assert_eq!(normalized.uv_range(), [0.0, 1.0]);

        // A selected-master proof can restrict to a genuinely identical child
        // function, but neither an equally sloped shifted map nor a hidden
        // narrower trim supplies authority for the whole child.
        let nominal = Correspondence {
            curve: &curve, surface: &xy, chart: &normalized_chart,
            maximum_error_mm: 1.0e-7, margin_uv: [0.0; 2],
        };
        let selected = certify_selected_master_correspondence(
            nominal, (&prepared, 1.0), None, &mut work, &cancel,
        ).unwrap().unwrap();
        let child_curve = model::Curve::TrimmedCurve(model::Processor::<_, Matrix4>::new(
            model::TrimmedCurve::new(Box::new(curve.clone()), (0.25, 0.75)),
        ));
        let clipped_chart = |basis, range| model::Curve2D::TrimmedCurve(
            model::Processor::<_, Matrix3>::new(model::TrimmedCurve::new(Box::new(basis), range)),
        );
        let child_chart = clipped_chart(normalized_chart.clone(), (0.25, 0.75));
        let child_input = Correspondence { curve: &child_curve, chart: &child_chart, ..nominal };
        let restricted = selected.restrict_base(child_input, &prepared, &mut work, &cancel)
            .unwrap().unwrap();
        assert_eq!(restricted.native_range(), [0.25, 0.75]);
        assert_eq!(restricted.uv_range(), [0.25, 0.75]);
        let (_, completed_child) = complete_retained_correspondence(
            child_input, &prepared, 1.0, Some(&selected), &mut work, &cancel, None,
        ).unwrap().unwrap();
        assert!(completed_child.position_error_mm() <= nominal.maximum_error_mm);
        let shifted_chart = clipped_chart(normalized_chart.clone(), (0.5, 1.0));
        assert!(selected.restrict_base(
            Correspondence { chart: &shifted_chart, ..child_input },
            &prepared, &mut work, &cancel,
        ).unwrap().is_none());
        let narrower = clipped_chart(normalized_chart.clone(), (0.25, 0.5));
        let invalid_child_chart = clipped_chart(narrower, (0.25, 0.75));
        assert!(selected.restrict_base(
            Correspondence { chart: &invalid_child_chart, ..child_input },
            &prepared, &mut work, &cancel,
        ).unwrap().is_none());
        let wrong_function = clipped_chart(model::Curve2D::Line(model::Line(
            Point2::new(0.01, 0.0), Point2::new(0.01, 1.0),
        )), (0.25, 0.75));
        assert!(selected.restrict_base(
            Correspondence { chart: &wrong_function, ..child_input },
            &prepared, &mut work, &cancel,
        ).unwrap().is_none());

        // Exact finite reversals cancel the child's translated parameter law:
        // 1 - (1/2 - (t - 1/2)) = t. No periodic function is involved.
        let reversed_chart = |basis, range| {
            let mut wrapper = model::Processor::<_, Matrix3>::new(
                model::TrimmedCurve::new(Box::new(basis), range),
            );
            wrapper.invert();
            model::Curve2D::TrimmedCurve(wrapper)
        };
        let reversed_native = model::Curve::TrimmedCurve(model::Processor::<_, Matrix4>::new(
            model::TrimmedCurve::new(Box::new(curve.clone()), (0.5, 1.0)),
        ));
        let twice_reversed = reversed_chart(
            reversed_chart(normalized_chart.clone(), (0.0, 1.0)), (0.0, 0.5),
        );
        let reversed_input = Correspondence {
            curve: &reversed_native, chart: &twice_reversed, ..nominal
        };
        let restricted = selected.restrict_base(reversed_input, &prepared, &mut work, &cancel)
            .unwrap().unwrap();
        assert_eq!(restricted.native_range(), [0.5, 1.0]);
        assert_eq!(restricted.uv_range(), [0.0, 0.5]);
        let (_, completed_reversed) = complete_retained_correspondence(
            reversed_input, &prepared, 1.0, Some(&selected), &mut work, &cancel, None,
        ).unwrap().unwrap();
        assert!(completed_reversed.position_error_mm() <= nominal.maximum_error_mm);
        // Matching derivative direction is not matching the affine constant.
        let wrong_anchor = reversed_chart(
            reversed_chart(normalized_chart.clone(), (0.0, 1.0)), (0.125, 0.625),
        );
        assert!(selected.restrict_base(
            Correspondence { chart: &wrong_anchor, ..reversed_input },
            &prepared, &mut work, &cancel,
        ).unwrap().is_none());
        // The same reflection sum does not authorize leaving an inner trim.
        let wrong_domain = reversed_chart(
            reversed_chart(normalized_chart.clone(), (0.375, 0.625)), (0.0, 0.5),
        );
        assert!(selected.restrict_base(
            Correspondence { chart: &wrong_domain, ..reversed_input },
            &prepared, &mut work, &cancel,
        ).unwrap().is_none());
        assert_eq!(selected.restrict_base(
            reversed_input, &prepared, &mut GeometryWorkBudget::new(0), &cancel,
        ).err().unwrap().code, GeometryErrorCode::ResourceLimit);
        assert_eq!(selected.restrict_base(
            reversed_input, &prepared, &mut work, &AtomicBool::new(true),
        ).err().unwrap().code, GeometryErrorCode::Cancelled);
        // An unrepresentable intermediate reflection sum is ordinary binding
        // nonproof, not permission to refuse the original geometric positive.
        let rounded_native = model::Curve::TrimmedCurve(model::Processor::<_, Matrix4>::new(
            model::TrimmedCurve::new(Box::new(curve.clone()), (0.1, 0.9)),
        ));
        let rounded_chart = reversed_chart(
            reversed_chart(normalized_chart.clone(), (0.1, 0.9)), (0.1, 0.9),
        );
        let (_, rounded_joint) = complete_retained_correspondence(
            Correspondence { curve: &rounded_native, chart: &rounded_chart, ..nominal },
            &prepared, 1.0, Some(&selected), &mut work, &cancel, None,
        ).unwrap().unwrap();
        assert!(rounded_joint.position_error_mm() <= nominal.maximum_error_mm);

        // A valid fixed tube need not satisfy the stronger E+R decomposition.
        // The actual root is (a,t,0), the nominal chart is (0,t), and the
        // complete tube's farthest displacement is sqrt((2a)^2+a^2) < T.
        let tolerance = 1.0e-7;
        let a = 0.44 * tolerance;
        assert!((2.0_f64 * a).hypot(a) < tolerance);
        let offset_plane = model::Surface::Plane(model::Plane::new(
            Point3::new(a, 0.0, 0.0),
            Point3::new(a, 1.0, 0.0),
            Point3::new(a, 0.0, 1.0),
        ));
        let offset_root = model::Curve::IntersectionCurve(model::SurfaceCurve::with_boundaries(
            Box::new(xy.clone()),
            Box::new(offset_plane),
            Box::new(model::Curve::Line(model::Line(
                Point3::new(a, 0.0, 0.0), Point3::new(a, 1.0, 0.0),
            ))),
            None,
            None,
        ));
        let offset_prepared = model::prepared_validated_curve_owned(
            offset_root.clone(),
            [0.0, 1.0],
            &model::ValidatedCurveOptions::default(),
            &mut model::ValidatedGeometryBudget::new(MAX_WORK as usize),
            &cancel,
        ).unwrap();
        let offset_input = Correspondence {
            curve: &offset_root,
            surface: &xy,
            chart: &normalized_chart,
            maximum_error_mm: tolerance,
            margin_uv: [a; 2],
        };
        let offset_proof = certify_normalized_retained_correspondence(
            offset_input, (&offset_prepared, 1.0), None, &mut work, &cancel, None,
        ).unwrap().unwrap();
        assert!(offset_proof.position_error_mm() <= tolerance);
        // Source completion's stricter sufficient metric must preserve the
        // same direct whole-tube positive, using its original proved inverse.
        let (_, source_joint) = complete_retained_correspondence(
            Correspondence { margin_uv: [0.0; 2], ..offset_input },
            &offset_prepared,
            1.0,
            None,
            &mut work,
            &cancel,
            None,
        ).unwrap().unwrap();
        assert!(source_joint.position_error_mm() <= tolerance);
        let nominal_offset = Correspondence { margin_uv: [0.0; 2], ..offset_input };
        let offset_master = certify_selected_master_correspondence(
            nominal_offset, (&offset_prepared, 1.0), None, &mut work, &cancel,
        ).unwrap().unwrap();
        let (_, hinted_joint) = complete_retained_correspondence(
            nominal_offset, &offset_prepared, 1.0, Some(&offset_master),
            &mut work, &cancel, None,
        ).unwrap().unwrap();
        assert!(hinted_joint.position_error_mm() <= tolerance);
        assert!(certify_normalized_retained_correspondence(
            Correspondence { margin_uv: [2.0 * a; 2], ..offset_input },
            (&offset_prepared, 1.0),
            None,
            &mut work,
            &cancel,
            None,
        ).unwrap().is_none());
    }

    #[test]
    fn retained_receipt_keeps_independent_endpoint_recipe_and_nonzero_bounds() {
        let curve = model::Curve::BsplineCurve(model::BsplineCurve::new(
            knots(10.0, 12.0, 1),
            vec![Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0)],
        ));
        let chart = model::Curve2D::Line(model::Line(
            Point2::new(0.0, 1.0e-9),
            Point2::new(1.0, 1.0e-9),
        ));
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(MAX_WORK);
        let proof = certify_declared_coedge_correspondence(
            Correspondence {
                curve: &curve,
                surface: &plane(),
                chart: &chart,
                maximum_error_mm: 1.0e-7,
                margin_uv: [0.0; 2],
            },
            &mut work,
            &cancel,
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            proof.provenance(),
            OriginalCorrespondenceProvenance::Forward
        );
        assert_eq!(proof.native_range(), [10.0, 12.0]);
        assert_eq!(proof.uv_range(), [0.0, 1.0]);
        assert_eq!(proof.uv_parameter_image([10.0, 10.0]).unwrap().lo, 0.0);
        assert_eq!(proof.uv_parameter_image([12.0, 12.0]).unwrap().hi, 1.0);
        let slope = proof.uv_parameter_derivative();
        assert!(slope.lo <= 0.5 && 0.5 <= slope.hi);
        assert!(proof.position_error_mm() >= 1.0e-9);
        assert_eq!(proof.relative_derivative_error_bound(), 0.25);
        assert!(proof.span_inventory().source_knot_boundaries >= 4);
        assert!(proof.binding().is_none());
    }

    #[test]
    fn retained_exact_coefficients_and_completed_graph_have_distinct_provenance() {
        let curve = model::Curve::Line(model::Line(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
        ));
        let chart = model::Curve2D::Line(model::Line(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0)));
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(MAX_WORK);
        let exact =
            OriginalCoedgeCorrespondence::exact_coefficients(&curve, &chart, &mut work, &cancel)
                .unwrap();
        assert_eq!(
            exact.provenance(),
            OriginalCorrespondenceProvenance::ExactCoefficients
        );
        assert_eq!(exact.position_error_mm(), 0.0);
        assert_eq!(exact.relative_derivative_error_bound(), 0.0);
        let mut forward = certify_declared_coedge_correspondence(
            Correspondence {
                curve: &curve,
                surface: &plane(),
                chart: &chart,
                maximum_error_mm: 1.0e-7,
                margin_uv: [0.0; 2],
            },
            &mut work,
            &cancel,
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            forward.provenance(),
            OriginalCorrespondenceProvenance::Forward
        );
        forward.complete_pole_graph();
        assert_eq!(
            forward.provenance(),
            OriginalCorrespondenceProvenance::CompletedPoleGraph
        );
    }
}
