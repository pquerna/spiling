// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Admission of the *original selected* SURFACE_CURVE master, independently of
//! the current coedge's carrier. A preferred pcurve is not a conversion receipt:
//! its complete forward physical correspondence must be proved separately.

use super::{conversion_error, error, source_trim, trim_certificate};
use super::trim_certificate::SelectedMasterReceipt;
use crate::{GeometryWorkBudget, parametric::Interval};
use monstertruck_io::step::{
    SurfaceParameterChart,
    load::step_geometry::{
        Curve2D as StepCurve2D, Curve3D, SourceParameterGauge, StepParameterCurve,
        Surface as StepSurface, SurfaceCurveAssociatedGeometry, SurfaceCurveKind,
        SurfaceCurveRepresentation, SurfaceParameterMap, SweepSurface,
    },
};
use monstertruck_modeling::{self as model, base::*};
use spiling_contracts::geometry::{GeometryError, GeometryErrorCode};
use std::sync::atomic::AtomicBool;

const MAX_DEPTH: usize = 32;
// Includes simultaneous original-UV copies and native conversion storage. The
// correspondence routine has its own, separately bounded native scratch arena.
const MAX_COPY_BYTES: usize = 8 * 1024 * 1024;

fn unresolved(message: &str) -> GeometryError {
    error(
        GeometryErrorCode::ParameterCorrespondenceUnavailable,
        message,
    )
}
fn resource() -> GeometryError {
    error(
        GeometryErrorCode::ResourceLimit,
        "selected source master exceeds bounded nesting or conversion storage",
    )
}
fn finite_range(range: (f64, f64)) -> Result<(f64, f64), GeometryError> {
    if !range.0.is_finite() || !range.1.is_finite() || range.0 >= range.1 {
        Err(unresolved(
            "selected source master has no finite noncollapsed active interval",
        ))
    } else {
        Ok(range)
    }
}

#[derive(Clone, Copy)]
struct MasterAssociation<'a> {
    active: Option<(f64, f64)>,
    whole: Option<(f64, f64)>,
    retained: Option<(&'a model::Curve, &'a model::PreparedValidatedCurve<'static>)>,
    previous: Option<&'a SelectedMasterReceipt>,
    scale: f64,
}

/// Checks every original preferred association, including one below periodic
/// split/restriction wrappers. No current-face lookup can substitute another
/// association. CURVE_3D adds no preferred-pcurve obligation; genuine implicit
/// continuation is admitted independently by its retained native branch gate.
///
/// `original_range`, when present, is bound by the importer to the WHOLE
/// original EDGE_CURVE and its native coedge uses, never a periodic child's
/// restricted interval. The STEP reader clips bare spatial LINEs to original
/// BREP vertices; their finite authority lives in that source lineage, not in
/// LINE's nominal native parameter-range trait.
///
/// `retained` borrows the already-owned whole native root and its immutable
/// continuation in the source carrier frame. It is required for a preferred
/// PCURVE on a genuine INTERSECTION_CURVE; its explicit guide is not authority.
/// `completed` is an import-local receipt slot for this stable original owner,
/// not a validation flag. Every call still resolves the actual selected
/// association/domain; only exact bound operand equality can reuse its proof.
pub(super) fn validate_selected_master(
    source: &Curve3D,
    original_range: Option<(f64, f64)>,
    retained: Option<(&model::Curve, &model::PreparedValidatedCurve<'static>)>,
    completed: &mut Option<SelectedMasterReceipt>,
    scale: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    work.charge(1, cancel)?;
    if !scale.is_finite() || scale <= 0.0 {
        return Err(unresolved(
            "selected source master needs finite positive source units",
        ));
    }
    let original_range = original_range.map(finite_range).transpose()?;
    let checked = visit(
        source,
        MasterAssociation {
            active: None,
            whole: original_range,
            retained,
            previous: completed.as_ref(),
            scale,
        },
        0,
        work,
        cancel,
    )?;
    *completed = checked;
    Ok(())
}

fn visit(
    source: &Curve3D,
    association: MasterAssociation<'_>,
    depth: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<SelectedMasterReceipt>, GeometryError> {
    let MasterAssociation {
        active,
        whole,
        retained,
        previous,
        scale,
    } = association;
    work.charge(1, cancel)?;
    if depth > MAX_DEPTH {
        return Err(resource());
    }
    match source {
        Curve3D::TrimmedCurve(trim) => {
            let range = finite_range(trim.entity().range_tuple())?;
            if let Some(active) = active
                && (active.0 < range.0 || active.1 > range.1)
            {
                return Err(unresolved(
                    "nested selected-master restriction leaves its original active interval",
                ));
            }
            let gauge = trim
                .raw_parameter_gauge()
                .map_err(|cause| conversion_error("selected source trim gauge refused", &cause))?;
            // Prove the unsplit basis, at a tolerance that remains valid under
            // the ACTUAL affine operator. Translation cancels in the residual;
            // the outward Frobenius norm bounds its linear amplification. Both
            // expressions undergo the same parameter reflection when inverted.
            let scale = transported_scale(scale, *trim.transform())?;
            let (whole, retained) =
                if let Some((curve, prepared)) = retained {
                    // The native owner excludes a verified generated periodic
                    // restriction from its WHOLE original preparation. Every actual
                    // original wrapper, including inversion/nonuniform transforms,
                    // instead descends through the SAME immutable mapped receipt.
                    let generated_child = matches!(gauge, SourceParameterGauge::Internal)
                        && *trim.transform() == Matrix4::identity()
                        && trim.orientation()
                        && whole.is_some_and(|whole| {
                            range != whole && trim.entity().curve().range_tuple() == whole
                        });
                    if generated_child {
                        (whole, Some((curve, prepared)))
                    } else {
                        let basis = prepared.mapped_basis().ok_or_else(||unresolved(
                        "original implicit trim is missing its immutable mapped basis receipt"))?;
                        let range = basis.range();
                        (
                            Some(finite_range((range[0], range[1]))?),
                            Some((basis.native_curve().unwrap_or(curve), basis)),
                        )
                    }
                } else {
                    (whole, None)
                };
            visit(
                trim.entity().curve(),
                MasterAssociation {
                    active: Some(range),
                    whole,
                    retained,
                    previous,
                    scale,
                },
                depth + 1,
                work,
                cancel,
            )
        }
        Curve3D::SurfaceCurve(surface_curve) => {
            let index = match surface_curve.master_representation() {
                SurfaceCurveRepresentation::Curve3D => None,
                SurfaceCurveRepresentation::ParameterCurve0 => Some(0),
                SurfaceCurveRepresentation::ParameterCurve1 => Some(1),
            };
            if let Some(index) = index {
                let Some(SurfaceCurveAssociatedGeometry::ParameterCurve(selected)) =
                    surface_curve.associated_geometry().get(index)
                else {
                    return Err(unresolved(
                        "preferred source master does not name its exact declared PCURVE association",
                    ));
                };
                if surface_curve.kind() == SurfaceCurveKind::IntersectionCurve {
                    return prove_retained(source, selected, association, work, cancel);
                } else {
                    prove(
                        surface_curve.leader(),
                        selected,
                        whole.or(active),
                        scale,
                        work,
                        cancel,
                    )?;
                }
            }
            if surface_curve.kind() == SurfaceCurveKind::IntersectionCurve {
                return Ok(None);
            }
            visit(surface_curve.leader(), association, depth + 1, work, cancel)
        }
        // The intersection's seed is not its exact world curve, nor a preferred
        // master. Its continuation receipt belongs to the implicit admission gate.
        Curve3D::IntersectionCurve(_) => Ok(None),
        _ => Ok(None),
    }
}

fn prove_retained(
    source: &Curve3D,
    selected: &StepParameterCurve,
    association: MasterAssociation<'_>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<SelectedMasterReceipt>, GeometryError> {
    let MasterAssociation {
        whole,
        retained,
        previous,
        scale,
        ..
    } = association;
    let (curve, prepared) = retained.ok_or_else(|| unresolved(
        "selected implicit master needs its already-owned whole root in the source carrier frame",
    ))?;
    let curve = prepared.native_curve().unwrap_or(curve);
    let prepared_range = prepared.range();
    let range = finite_range((prepared_range[0], prepared_range[1]))?;
    if whole.is_some_and(|whole| whole != range) {
        return Err(unresolved(
            "selected implicit master continuation does not cover its whole original edge interval",
        ));
    }
    let mut copies = CopyBound {
        bytes: 0,
        work,
        cancel,
    };
    copies.uv(selected.curve(), 0)?;
    copies.surface(selected.surface(), 0)?;
    let live_copy_bytes = copies.bytes;
    let (surface, conversion) = selected
        .surface()
        .to_modeling_with_parameter_map()
        .map_err(|cause| {
            conversion_error(
                "selected implicit carrier conversion refused",
                cause.as_ref(),
            )
        })?;
    let (chart, margin) = retained_chart(
        source, selected, curve, &surface, conversion, range, work, cancel,
    )?;
    // Only the original selected UV expression enters this proof. Native chart
    // extraction and the explicit guide above supply proposals, never receipts.
    // This certificate borrows the SAME whole branch as every later consumer.
    if let Some(proof) = prove_reusable_retained_master(
        trim_certificate::Correspondence {
            curve,
            surface: &surface,
            chart: &chart,
            maximum_error_mm: 1.0e-7 / scale,
            margin_uv: margin,
        },
        (prepared, 1.0),
        previous,
        source,
        live_copy_bytes,
        scale,
        work,
        cancel,
    )? {
        return Ok(Some(proof));
    }
    let natural = finite_range(curve.range_tuple())?;
    if range != natural && !matches!(selected.curve(), StepCurve2D::Line(_)) {
        // A bounded association may declare the whole basis chart rather than
        // an edge-active chart. Transport the original expression's proposed
        // affine correspondence onto the actual retained original restriction.
        // Never extend the owned branch to the unused natural basis interval.
        let chart = restrict_chart_proposal(chart, range, natural)?;
        if let Some(proof) = prove_reusable_retained_master(
            trim_certificate::Correspondence {
                curve,
                surface: &surface,
                chart: &chart,
                maximum_error_mm: 1.0e-7 / scale,
                margin_uv: margin,
            },
            (prepared, 1.0),
            previous,
            source,
            live_copy_bytes,
            scale,
            work,
            cancel,
        )? {
            return Ok(Some(proof));
        }
    }
    Err(unresolved(
        "selected original implicit PCURVE lacks whole retained-root forward correspondence",
    ))
}

fn prove_reusable_retained_master(
    input: trim_certificate::Correspondence<'_>,
    prepared: (&model::PreparedValidatedCurve<'static>, f64),
    previous: Option<&SelectedMasterReceipt>,
    source: &Curve3D,
    live_copy_bytes: usize,
    scale: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<SelectedMasterReceipt>, GeometryError> {
    if previous.is_some() || !matches!(input.surface, model::Surface::Plane(_)) {
        return trim_certificate::certify_selected_master_correspondence(
            input, prepared, previous, work, cancel,
        );
    }
    let prepared_range = prepared.0.range();
    let native_range = input.curve.range_tuple();
    let active = [
        prepared_range[0].max(native_range.0),
        prepared_range[1].min(native_range.1),
    ];
    // PlaneImage consumes literal native t. Preserve independent chart maps on
    // the ordinary admission path rather than inventing a periodic alias.
    let identity = input.chart.try_range_tuple()
        .is_some_and(|uv| active == [uv.0, uv.1]);
    if identity {
        // R is a conditional quality premise, not an achieved source bound.
        // The inverse traversal only proposes a target; the independent whole
        // native proof below must discharge A <= q <= R before receipt reuse.
        let proposed = optional_precision(whole_owner_precision(
            source, input, prepared.0, input.maximum_error_mm,
            live_copy_bytes, scale, work, cancel,
        ))?;
        if let Some(proposed) = proposed {
            // Reserve search headroom for the actual periodic/current image.
            // This policy does not certify its discrepancy or change its goal.
            let goal = Interval::point(proposed)
                .mul(Interval::point(0.5)).lo.min(input.maximum_error_mm);
            if goal.is_finite() && goal > 0.0 {
                if let Some(proof) = trim_certificate::certify_selected_master_correspondence(
                    trim_certificate::Correspondence { maximum_error_mm: goal, ..input },
                    prepared, None, work, cancel,
                )? {
                    let achieved = proof.whole().position_error_mm();
                    if achieved.is_finite() && achieved >= 0.0 && achieved <= goal {
                        return Ok(Some(proof));
                    }
                }
            }
        }
    }
    // One optional quality attempt, then the unchanged ordinary proof. No
    // inverse proposal grants authority; errors and cumulative work propagate.
    trim_certificate::certify_selected_master_correspondence(
        input, prepared, previous, work, cancel,
    )
}

/// The selected declaration and optional original associations use exactly the
/// same chart conversion and finite LINE branch proposal. Neither a guide nor
/// a declaration becomes residual authority here.
fn retained_chart(
    source: &Curve3D,
    declared: &StepParameterCurve,
    curve: &model::Curve,
    surface: &model::Surface,
    conversion: SurfaceParameterMap,
    range: (f64, f64),
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(model::Curve2D, [f64; 2]), GeometryError> {
    let (map, margin) = input_map(conversion, declared.input_parameter_chart())?;
    let chart = if let StepCurve2D::Line(raw) = declared.curve() {
        let domain = source_trim::intrinsic_domain(declared.surface(), surface, conversion);
        let candidate = match source_trim::native_chart(
            curve, surface, None, domain, work, cancel,
        )? {
            Some(candidate) => Some(candidate),
            None => source_trim::implicit_guide_chart(
                source, declared.surface(), surface, conversion, work, cancel,
            )?,
        }.ok_or_else(|| {
            unresolved("selected implicit UV LINE has no bounded active-branch proposal")
        })?;
        let candidate =
            restrict_chart_proposal(candidate, range, finite_range(curve.range_tuple())?)?;
        source_trim::line_branch_proposal(
            *raw,
            map.affine_transform()
                .ok_or_else(|| unresolved("selected implicit UV LINE chart is not affine"))?,
            &candidate,
        ).ok_or_else(|| {
            unresolved("selected implicit UV LINE active-branch proposal is not finite")
        })?
    } else {
        let chart = model::Curve2D::try_from(declared.curve()).map_err(|cause| {
            conversion_error("original selected implicit UV expression refused", cause.as_ref())
        })?;
        let range = finite_range(chart.range_tuple())?;
        if matches!(map, SurfaceParameterMap::Identity) {
            chart
        } else {
            model::Curve2D::TrimmedCurve(model::Processor::with_transform(
                model::TrimmedCurve::new(Box::new(chart), range),
                map.affine_transform()
                    .ok_or_else(|| unresolved("selected implicit chart is not affine"))?,
            ))
        }
    };
    Ok((chart, margin))
}

/// Optional producer quality is not another admission gate for an unused
/// association. Preserve actual invalid-source, resource and cancellation
/// errors rather than treating every failure as a declined proposal.
fn optional_precision<T>(
    result: Result<Option<T>, GeometryError>,
) -> Result<Option<T>, GeometryError> {
    match result {
        Err(cause) if matches!(
            cause.code,
            GeometryErrorCode::ParameterCorrespondenceUnavailable
                | GeometryErrorCode::UnsupportedConversion
                | GeometryErrorCode::ConditioningFailure
                | GeometryErrorCode::SingularGeometry
                | GeometryErrorCode::AnalysisIndeterminate
        ) => Ok(None),
        other => other,
    }
}

fn inverse_base_precision(
    input: trim_certificate::Correspondence<'_>,
    prepared: &model::PreparedValidatedCurve<'static>,
    residual: f64,
    scale: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<f64>, GeometryError> {
    let inverse = super::implicit::certify_uv_margin(
        super::implicit::UvMarginInput {
            curve: input.curve,
            prepared,
            surface: input.surface,
            chart: input.chart,
            physical_residual: residual,
            coefficient_margin: input.margin_uv,
        },
        work, cancel,
    )?;
    Ok(inverse.refinement_limit(
        input.maximum_error_mm, super::implicit::original_point_tolerance_native(scale),
        work, cancel, None,
    )?.map(|metric| metric.base_limit()))
}

/// For each usable ORIGINAL carrier i, first prove the whole fixed-forward
/// selected-plane/current-image discrepancy delta_i. Only then can
/// E_i = up(R + delta_i) seed that carrier's original-root inverse tube.
/// R may be hypothetical: no resulting metric supplies completed authority.
/// Its complete metric gives T_i = down((allowed-B_i-D)/(1+G_i)); the independent
/// selected proof must discharge A <= its chosen quality target <= R.
fn whole_owner_precision(
    source: &Curve3D,
    input: trim_certificate::Correspondence<'_>,
    prepared: &model::PreparedValidatedCurve<'static>,
    residual_envelope: f64,
    live_copy_bytes: usize,
    scale: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<f64>, GeometryError> {
    let model::Surface::Plane(plane) = input.surface else { return Ok(None) };
    let mut goal = optional_precision(inverse_base_precision(
        input, prepared, residual_envelope, scale, work, cancel,
    ))?;
    let Curve3D::SurfaceCurve(original) = source else { return Ok(goal) };
    let selected = match original.master_representation() {
        SurfaceCurveRepresentation::ParameterCurve0 => 0,
        SurfaceCurveRepresentation::ParameterCurve1 => 1,
        SurfaceCurveRepresentation::Curve3D => return Ok(goal),
    };
    let active = prepared.range();
    let range = finite_range((active[0], active[1]))?;
    for (index, association) in original.associated_geometry().iter().enumerate() {
        work.charge(1, cancel)?;
        if index == selected { continue }
        let SurfaceCurveAssociatedGeometry::ParameterCurve(declared) = association
        else { continue };
        let target = optional_precision((|| {
            // The selected converted surface/chart remain live while this
            // single candidate and its conversion coexist. No BREP/root copy.
            let mut copies = CopyBound { bytes: live_copy_bytes, work, cancel };
            copies.uv(declared.curve(), 0)?;
            copies.surface(declared.surface(), 0)?;
            let (surface, conversion) = declared.surface()
                .to_modeling_with_parameter_map()
                .map_err(|cause| conversion_error(
                    "original associated implicit carrier conversion refused", cause.as_ref(),
                ))?;
            let (mut chart, margin) = retained_chart(
                source, declared, input.curve, &surface, conversion, range, work, cancel,
            )?;
            let discrepancy = |chart: &model::Curve2D, work: &mut GeometryWorkBudget| {
                let target = finite_range(chart.range_tuple())?;
                super::harmonic::certify(
                    super::harmonic::HarmonicComparison {
                        source: super::harmonic::HarmonicSource::PlaneImage(plane, input.chart),
                        surface: &surface, chart, range: active,
                        target: [target.0, target.1], margin,
                        tolerance: input.maximum_error_mm, require_tangent: false,
                    },
                    work, cancel,
                    &mut super::harmonic::HarmonicDiagnostic::new(input.maximum_error_mm),
                )
            };
            let mut delta = optional_precision(discrepancy(&chart, work))?;
            let natural = finite_range(input.curve.range_tuple())?;
            if delta.is_none() && range != natural
                && !matches!(declared.curve(), StepCurve2D::Line(_))
            {
                chart = restrict_chart_proposal(chart, range, natural)?;
                delta = optional_precision(discrepancy(&chart, work))?;
            }
            let Some(delta) = delta else { return Ok(None) };
            let residual = Interval::point(residual_envelope).add(Interval::point(delta)).hi;
            let Some(limit) = inverse_base_precision(
                trim_certificate::Correspondence {
                    surface: &surface, chart: &chart, margin_uv: margin, ..input
                },
                prepared, residual, scale, work, cancel,
            )? else { return Ok(None) };
            let remaining = Interval::point(limit).sub(Interval::point(delta)).lo;
            Ok((remaining.is_finite() && remaining > 0.0).then_some(remaining))
        })())?;
        if let Some(target) = target {
            goal = Some(goal.map_or(target, |current| current.min(target)));
        }
    }
    Ok(goal)
}

fn restrict_chart_proposal(
    chart: model::Curve2D,
    active: (f64, f64),
    natural: (f64, f64),
) -> Result<model::Curve2D, GeometryError> {
    if active == natural {
        return Ok(chart);
    }
    let own = finite_range(chart.range_tuple())?;
    // Endpoint arithmetic is search data only. The complete retained-root
    // forward proof independently encloses the actual stored restriction.
    let parameter = |t| own.0 + (own.1 - own.0) * ((t - natural.0) / (natural.1 - natural.0));
    let proposed = finite_range((parameter(active.0), parameter(active.1)))?;
    Ok(model::Curve2D::TrimmedCurve(
        model::Processor::<_, Matrix3>::new(model::TrimmedCurve::new(Box::new(chart), proposed)),
    ))
}

fn transported_scale(scale: f64, matrix: Matrix4) -> Result<f64, GeometryError> {
    if matrix == Matrix4::identity() {
        return Ok(scale);
    }
    if matrix.x.w != 0.0
        || matrix.y.w != 0.0
        || matrix.z.w != 0.0
        || matrix.w.w != 1.0
        || (0..4).any(|i| (0..4).any(|j| !matrix[i][j].is_finite()))
    {
        return Err(unresolved(
            "selected source master wrapper lacks a finite affine error transport",
        ));
    }
    let mut square = Interval::point(0.0);
    for column in 0..3 {
        for row in 0..3 {
            square = square.add(Interval::point(matrix[column][row]).square());
        }
    }
    let norm = Interval {
        lo: square.lo.max(0.0),
        hi: square.hi,
    }
    .sqrt()?;
    let bound = Interval::point(scale).mul(norm).hi;
    if !bound.is_finite() || bound <= 0.0 {
        return Err(unresolved(
            "selected source master affine amplification cannot be enclosed",
        ));
    }
    Ok(bound)
}

fn prove(
    leader: &Curve3D,
    selected: &StepParameterCurve,
    active: Option<(f64, f64)>,
    scale: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    // Preflight every recursive source expansion BEFORE a native conversion or
    // original UV clone. Charge failed attempts too; do not reset caller work.
    let mut copies = CopyBound {
        bytes: 0,
        work,
        cancel,
    };
    copies.curve(leader, 0)?;
    copies.uv(selected.curve(), 0)?;
    copies.surface(selected.surface(), 0)?;
    let mut curve = explicit_curve(leader, 0, work, cancel)?;
    // Bare spatial LINE authority comes from the bound original EDGE_CURVE
    // interval (or an actual enclosing source trim), never its convenience 0..1.
    if unbounded_line(leader, 0)? {
        let range = active.ok_or_else(|| {
            unresolved("unbounded selected spatial LINE needs its original finite restriction")
        })?;
        curve = model::Curve::TrimmedCurve(model::Processor::<_, Matrix4>::new(
            model::TrimmedCurve::new(Box::new(curve), range),
        ));
    }
    finite_range(curve.range_tuple())?;
    let (surface, map) = selected
        .surface()
        .to_modeling_with_parameter_map()
        .map_err(|cause| {
            conversion_error("selected source carrier conversion refused", cause.as_ref())
        })?;
    // Input UV may be raw STEP, decoded, swapped or reflected. Compose exactly
    // once with the DECODED-to-native operator, retaining coefficient error.
    let (map, margin) = input_map(map, selected.input_parameter_chart())?;
    let retained = source_trim::retain(
        source_trim::TrimInput {
            source_curve: leader,
            native_curve: &curve,
            surface: source_trim::SurfaceInput {
                source_surface: selected.surface(),
                native_surface: &surface,
                map,
                scale,
            },
            declared: Some(selected.curve()),
        },
        margin,
        work,
        cancel,
    )?;
    let chart = retained
        .curve
        .as_ref()
        .ok_or_else(|| unresolved("selected original PCURVE has no retained active chart"))?;
    // A bare UV LINE must have received an actual finite active branch from
    // retain's complete physical proof, never its default native unit interval.
    if matches!(selected.curve(), StepCurve2D::Line(_))
        && (!retained.certificate.is_certified()
            || !matches!(chart, model::Curve2D::TrimmedCurve(_)))
    {
        return Err(unresolved(
            "unbounded selected UV LINE has no proved finite active branch",
        ));
    }
    finite_range(chart.range_tuple())?;
    let margin = std::array::from_fn(|i| margin[i].max(retained.margin_uv[i]));
    // Retention may already have completed this exact whole-curve forward
    // position AND relative-tangent proof. Reuse only that typed receipt with
    // at least the same UV uncertainty; coefficient projections, inverse tubes
    // and current-face associations never grant selected-master authority.
    if !retained.proves_forward(1.0e-7 / scale, margin)
        && trim_certificate::certify_declared_coedge_correspondence(
            trim_certificate::Correspondence {
                curve: &curve,
                surface: &surface,
                chart,
                maximum_error_mm: 1.0e-7 / scale,
                margin_uv: margin,
            },
            work,
            cancel,
            None,
        )?
        .is_none()
    {
        return Err(unresolved(
            "selected original PCURVE lacks complete forward physical correspondence to its original world curve",
        ));
    }
    Ok(())
}

fn unbounded_line(source: &Curve3D, depth: usize) -> Result<bool, GeometryError> {
    if depth > MAX_DEPTH {
        return Err(resource());
    }
    match source {
        Curve3D::SurfaceCurve(c) => unbounded_line(c.leader(), depth + 1),
        Curve3D::Line(_) => Ok(true),
        _ => Ok(false),
    }
}

/// Unlike the generic projected SurfaceCurve conversion, this deliberately
/// retains the explicit original leader expression. No association or implicit
/// intersection seed is substituted as an exact curve.
fn explicit_curve(
    source: &Curve3D,
    depth: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<model::Curve, GeometryError> {
    work.charge(1, cancel)?;
    if depth > MAX_DEPTH {
        return Err(resource());
    }
    match source {
        Curve3D::SurfaceCurve(c) => explicit_curve(c.leader(), depth + 1, work, cancel),
        Curve3D::TrimmedCurve(c) => {
            c.raw_parameter_gauge().map_err(|cause| {
                conversion_error("selected master leader trim gauge refused", &cause)
            })?;
            let basis = explicit_curve(c.entity().curve(), depth + 1, work, cancel)?;
            let mut trim = model::Processor::with_transform(
                model::TrimmedCurve::new(Box::new(basis), finite_range(c.entity().range_tuple())?),
                *c.transform(),
            );
            if !c.orientation() {
                trim.invert();
            }
            Ok(model::Curve::TrimmedCurve(trim))
        }
        Curve3D::ParameterCurve(pcurve) => {
            let (surface, conversion) =
                pcurve
                    .surface()
                    .to_modeling_with_parameter_map()
                    .map_err(|cause| {
                        conversion_error(
                            "selected master world PCURVE carrier refused",
                            cause.as_ref(),
                        )
                    })?;
            let (map, margin) = input_map(conversion, pcurve.input_parameter_chart())?;
            if margin.into_iter().any(|x| x != 0.0) {
                return Err(unresolved(
                    "selected master world PCURVE needs an enclosed source-side chart receipt",
                ));
            }
            let uv = model::Curve2D::try_from(pcurve.curve()).map_err(|cause| {
                conversion_error(
                    "selected master world PCURVE expression refused",
                    cause.as_ref(),
                )
            })?;
            let range = finite_range(uv.range_tuple())?;
            let uv = if matches!(map, SurfaceParameterMap::Identity) {
                uv
            } else {
                model::Curve2D::TrimmedCurve(model::Processor::with_transform(
                    model::TrimmedCurve::new(Box::new(uv), range),
                    map.affine_transform().ok_or_else(|| {
                        unresolved("selected master world PCURVE map is unavailable")
                    })?,
                ))
            };
            Ok(model::Curve::ParameterCurve(model::ParameterCurve::new(
                uv,
                Box::new(surface),
            )))
        }
        Curve3D::IntersectionCurve(_) => Err(unresolved(
            "selected preferred PCURVE on a true implicit curve needs its retained continuation proof",
        )),
        _ => source.to_parameter_preserving_modeling().map_err(|cause| {
            conversion_error(
                "original selected-master world expression refused",
                cause.as_ref(),
            )
        }),
    }
}

/// SurfaceParameterChart has an exact signed-permutation linear part and an
/// enclosed translation. Keeping that structure avoids multiplying/rounding a
/// general matrix and then mislabelling its nominal coefficients as exact.
fn input_map(
    map: SurfaceParameterMap,
    input: SurfaceParameterChart,
) -> Result<(SurfaceParameterMap, [f64; 2]), GeometryError> {
    let decoded = map.internal_affine_transform().ok_or_else(|| {
        unresolved("selected source carrier has no established decoded-to-native affine chart")
    })?;
    if decoded.x.z != 0.0
        || decoded.y.z != 0.0
        || decoded.z.z != 1.0
        || (0..3).any(|i| (0..3).any(|j| !decoded[i][j].is_finite()))
    {
        return Err(unresolved(
            "selected source carrier chart is not finite affine",
        ));
    }
    let mut offsets = [Interval::point(0.0); 2];
    for (offset, axis) in offsets.iter_mut().zip(input.axes) {
        if axis.scale != 1.0 && axis.scale != -1.0 {
            return Err(unresolved(
                "selected PCURVE input chart lacks an exact oriented axis",
            ));
        }
        *offset = Interval::new(axis.offset_enclosure.lower, axis.offset_enclosure.upper)?;
        if !axis.offset.is_finite() || axis.offset < offset.lo || axis.offset > offset.hi {
            return Err(unresolved(
                "selected PCURVE input chart translation is not enclosed",
            ));
        }
    }
    let mut matrix = Matrix3::identity();
    for column in 0..2 {
        let axis = if input.swapped { 1 - column } else { column };
        matrix[column] = decoded[axis] * input.axes[axis].scale;
    }
    let mut margin = [0.0; 2];
    for row in 0..2 {
        let nominal = decoded.z[row]
            + decoded.x[row] * input.axes[0].offset
            + decoded.y[row] * input.axes[1].offset;
        if !nominal.is_finite() {
            return Err(unresolved("selected PCURVE chart composition overflowed"));
        }
        matrix.z[row] = nominal;
        // Skip exact zero coefficients; they introduce no arithmetic error.
        let mut enclosed = Interval::point(decoded.z[row]);
        for axis in 0..2 {
            if decoded[axis][row] != 0.0 && (offsets[axis].lo != 0.0 || offsets[axis].hi != 0.0) {
                enclosed = enclosed.add(Interval::point(decoded[axis][row]).mul(offsets[axis]));
            }
        }
        margin[row] = if enclosed.lo == nominal && enclosed.hi == nominal {
            0.0
        } else {
            enclosed.sub(Interval::point(nominal)).abs_upper()
        };
    }
    Ok((
        if matrix == Matrix3::identity() {
            SurfaceParameterMap::Identity
        } else {
            SurfaceParameterMap::Affine(matrix)
        },
        margin,
    ))
}

/// Conservative scalar storage envelope for source clones and their native
/// conversions. The same cap covers nested pcurve carriers and swept profiles.
struct CopyBound<'a> {
    bytes: usize,
    work: &'a mut GeometryWorkBudget,
    cancel: &'a AtomicBool,
}
impl CopyBound<'_> {
    fn add(&mut self, scalars: usize, depth: usize) -> Result<(), GeometryError> {
        self.work
            .charge(scalars.saturating_add(1) as u64, self.cancel)?;
        self.bytes = self
            .bytes
            .checked_add(scalars.checked_mul(32).ok_or_else(resource)?)
            .ok_or_else(resource)?;
        if depth > MAX_DEPTH || self.bytes > MAX_COPY_BYTES {
            return Err(resource());
        }
        Ok(())
    }
    fn uv(&mut self, curve: &StepCurve2D, depth: usize) -> Result<(), GeometryError> {
        self.add(32, depth)?;
        match curve {
            StepCurve2D::Polyline(c) => self.add(c.len().saturating_mul(3), depth),
            StepCurve2D::BsplineCurve(c) => self.add(
                c.knot_vector()
                    .len()
                    .saturating_add(c.control_points().len().saturating_mul(3)),
                depth,
            ),
            StepCurve2D::NurbsCurve(c) => self.add(
                c.knot_vector()
                    .len()
                    .saturating_add(c.control_points().len().saturating_mul(4)),
                depth,
            ),
            StepCurve2D::TrimmedCurve(c) => {
                finite_range(c.entity().range_tuple())?;
                c.raw_parameter_gauge().map_err(|cause| {
                    conversion_error("selected original UV trim gauge refused", &cause)
                })?;
                self.uv(c.entity().curve(), depth + 1)
            }
            _ => Ok(()),
        }
    }
    fn curve(&mut self, curve: &Curve3D, depth: usize) -> Result<(), GeometryError> {
        self.add(64, depth)?;
        match curve {
            Curve3D::Polyline(c) => self.add(c.len().saturating_mul(4), depth),
            Curve3D::BsplineCurve(c) => self.add(
                c.knot_vector()
                    .len()
                    .saturating_add(c.control_points().len().saturating_mul(4)),
                depth,
            ),
            Curve3D::NurbsCurve(c) => self.add(
                c.knot_vector()
                    .len()
                    .saturating_add(c.control_points().len().saturating_mul(5)),
                depth,
            ),
            Curve3D::TrimmedCurve(c) => {
                finite_range(c.entity().range_tuple())?;
                c.raw_parameter_gauge().map_err(|cause| {
                    conversion_error("selected original spatial trim gauge refused", &cause)
                })?;
                self.curve(c.entity().curve(), depth + 1)
            }
            Curve3D::SurfaceCurve(c) => self.curve(c.leader(), depth + 1),
            Curve3D::ParameterCurve(c) => {
                self.uv(c.curve(), depth + 1)?;
                self.surface(c.surface(), depth + 1)
            }
            Curve3D::IntersectionCurve(_) => Err(unresolved(
                "selected preferred master requires actual implicit continuation",
            )),
            _ => Ok(()),
        }
    }
    fn surface(&mut self, surface: &StepSurface, depth: usize) -> Result<(), GeometryError> {
        self.add(256, depth)?;
        match surface {
            StepSurface::BsplineSurface(c) => {
                self.add(
                    c.knot_vector_u()
                        .len()
                        .saturating_add(c.knot_vector_v().len()),
                    depth,
                )?;
                for row in c.control_points() {
                    self.add(row.len().saturating_mul(4), depth)?;
                }
                Ok(())
            }
            StepSurface::NurbsSurface(c) => {
                self.add(
                    c.knot_vector_u()
                        .len()
                        .saturating_add(c.knot_vector_v().len()),
                    depth,
                )?;
                for row in c.control_points() {
                    self.add(row.len().saturating_mul(5), depth)?;
                }
                Ok(())
            }
            StepSurface::SweepSurface(SweepSurface::ExtrusionSurface(c)) => {
                self.curve(c.entity_curve(), depth + 1)
            }
            StepSurface::SweepSurface(SweepSurface::RevolutionSurface(c)) => {
                self.curve(c.entity().entity_curve(), depth + 1)
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use monstertruck_io::step::load::step_geometry::{
        Conic2D as StepConic2D, Conic3D, ElementarySurface, SourceTrimmedCurve, SurfaceCurve3D,
        SurfaceCurveKind,
    };

    fn plane(z: f64) -> StepSurface {
        StepSurface::ElementarySurface(ElementarySurface::Plane(model::Plane::from_origin_vectors(
            Point3::new(0.0, 0.0, z),
            Vector3::unit_x(),
            Vector3::unit_y(),
        )))
    }
    fn leader() -> Curve3D {
        Curve3D::BsplineCurve(model::BsplineCurve::new(
            model::KnotVector::from(vec![2.0, 2.0, 5.0, 5.0]),
            vec![Point3::origin(), Point3::new(20.0, 0.0, 0.0)],
        ))
    }
    fn pcurve(surface: StepSurface, range: (f64, f64), end: Point2) -> StepParameterCurve {
        StepParameterCurve::new(
            Box::new(StepCurve2D::BsplineCurve(model::BsplineCurve::new(
                model::KnotVector::from(vec![range.0, range.0, range.1, range.1]),
                vec![Point2::origin(), end],
            ))),
            Box::new(surface),
        )
    }
    fn associated(curve: StepParameterCurve) -> SurfaceCurveAssociatedGeometry {
        SurfaceCurveAssociatedGeometry::ParameterCurve(curve)
    }
    fn source(
        leader: Curve3D,
        associations: Vec<SurfaceCurveAssociatedGeometry>,
        master: SurfaceCurveRepresentation,
    ) -> Curve3D {
        Curve3D::SurfaceCurve(SurfaceCurve3D::new(
            SurfaceCurveKind::SurfaceCurve,
            Box::new(leader),
            associations,
            master,
        ))
    }
    fn validate(source: &Curve3D) -> Result<(), GeometryError> {
        validate_selected_master(
            source, None, None, &mut None, 1.0,
            &mut GeometryWorkBudget::new(8_000_000), &AtomicBool::new(false),
        )
    }
    fn refused(source: &Curve3D) {
        assert_eq!(
            validate(source).unwrap_err().code,
            GeometryErrorCode::ParameterCorrespondenceUnavailable
        );
    }

    #[test]
    fn original_raw_chart_circle_master_keeps_world_carrier_and_whole_domain() {
        let tau = std::f64::consts::TAU;
        let circle = |shift| {
            StepCurve2D::Conic(StepConic2D::Ellipse(model::Processor::with_transform(
                model::TrimmedCurve::new(model::UnitCircle::<Point2>::new(), (0.0, tau)),
                Matrix3::from_translation(Vector2::new(shift, 0.0)) * Matrix3::from_scale(2.0),
            )))
        };
        let mut input = SurfaceParameterChart::identity();
        input.axes[0].offset = 5.0;
        input.axes[0].offset_enclosure =
            monstertruck_io::step::ParameterEnclosure::new(5.0, 5.0).unwrap();
        let original = Curve3D::ParameterCurve(
            StepParameterCurve::try_from_parts(Box::new(circle(-5.0)), Box::new(plane(0.0)), input)
                .unwrap(),
        );
        let selected = StepParameterCurve::new(Box::new(circle(0.0)), Box::new(plane(0.0)));
        let matching = source(
            original.clone(),
            vec![associated(selected)],
            SurfaceCurveRepresentation::ParameterCurve0,
        );
        validate_selected_master(
            &matching, None, None, &mut None, 1.0,
            &mut GeometryWorkBudget::new(50_000), &AtomicBool::new(false),
        )
        .unwrap();
        let wrong_carrier = StepParameterCurve::new(Box::new(circle(0.0)), Box::new(plane(1.0)));
        refused(&source(
            original.clone(),
            vec![associated(wrong_carrier)],
            SurfaceCurveRepresentation::ParameterCurve0,
        ));
        let half = StepCurve2D::Conic(StepConic2D::Ellipse(model::Processor::with_transform(
            model::TrimmedCurve::new(model::UnitCircle::<Point2>::new(), (0.0, tau * 0.5)),
            Matrix3::from_scale(2.0),
        )));
        let wrong_domain = StepParameterCurve::new(Box::new(half), Box::new(plane(0.0)));
        refused(&source(
            original,
            vec![associated(wrong_domain)],
            SurfaceCurveRepresentation::ParameterCurve0,
        ));
    }

    fn retained_planar_root() -> (
        Curve3D,
        model::Curve,
        model::PreparedValidatedCurve<'static>,
    ) {
        let delta = 1.0 / 1024.0;
        let line = model::Line(Point3::new(0.0, delta, 0.0), Point3::new(1.0, delta, 0.0));
        let curve = model::Curve::IntersectionCurve(model::SurfaceCurve::with_boundaries(
            Box::new(model::Surface::Plane(model::Plane::from_origin_vectors(
                Point3::origin(),
                Vector3::unit_x(),
                Vector3::unit_y(),
            ))),
            Box::new(model::Surface::Plane(model::Plane::from_origin_vectors(
                Point3::origin(),
                Vector3::unit_x(),
                Vector3::unit_z(),
            ))),
            Box::new(model::Curve::Line(line)),
            None,
            None,
        ));
        let prepared = model::prepared_validated_curve_owned(
            curve.clone(),
            [0.0, 1.0],
            &model::ValidatedCurveOptions::default(),
            &mut model::ValidatedGeometryBudget::new(1_000_000),
            &AtomicBool::new(false),
        )
        .unwrap();
        (Curve3D::Line(line), curve, prepared)
    }
    fn implicit_source(guide: Curve3D, selected: StepParameterCurve) -> Curve3D {
        Curve3D::SurfaceCurve(SurfaceCurve3D::new(
            SurfaceCurveKind::IntersectionCurve,
            Box::new(guide),
            vec![
                associated(selected),
                SurfaceCurveAssociatedGeometry::Surface(Box::new(StepSurface::ElementarySurface(
                    ElementarySurface::Plane(model::Plane::from_origin_vectors(
                        Point3::origin(),
                        Vector3::unit_x(),
                        Vector3::unit_z(),
                    )),
                ))),
            ],
            SurfaceCurveRepresentation::ParameterCurve0,
        ))
    }

    #[test]
    fn genuine_selected_master_uses_retained_root_not_offset_explicit_guide() {
        let (guide, root, prepared) = retained_planar_root();
        let original = implicit_source(
            guide,
            pcurve(plane(0.0), (7.0, 19.0), Point2::new(1.0, 0.0)),
        );
        let mut completed = None;
        validate_selected_master(
            &original, Some((0.0, 1.0)), Some((&root, &prepared)), &mut completed, 1.0,
            &mut GeometryWorkBudget::new(8_000_000), &AtomicBool::new(false),
        ).unwrap();
        let proof = completed.as_ref().expect("selected whole-original master receipt").whole();
        assert_eq!(proof.native_range(), [0.0, 1.0]);
        assert_eq!(proof.uv_range(), [7.0, 19.0]);
        assert!(proof.position_error_mm() <= 1.0e-7);
        validate_selected_master(
            &original.clone(), Some((0.0, 1.0)), Some((&root, &prepared)), &mut completed, 1.0,
            &mut GeometryWorkBudget::new(8_000_000), &AtomicBool::new(false),
        ).unwrap();
        // A genuine intersection cannot fall back to its displaced guide when
        // its already-owned branch is absent.
        refused(&original);
    }

    #[test]
    fn selected_plane_precision_reserves_original_cylinder_image_before_its_inverse() {
        let range = (0.0, 0.5);
        let selected = StepParameterCurve::new(
            Box::new(StepCurve2D::Conic(StepConic2D::Ellipse(
                model::Processor::with_transform(
                    model::TrimmedCurve::new(model::UnitCircle::<Point2>::new(), range),
                    Matrix3::from_scale(2.0),
                ),
            ))),
            Box::new(plane(0.0)),
        );
        let mut cylinder = model::Processor::new(model::RevolutionSurface::by_revolution(
            model::Line(Point3::new(2.0, 0.0, -1.0), Point3::new(2.0, 0.0, 0.0)),
            Point3::new(0.0, 0.0, -1.0), Vector3::unit_z(),
        ));
        cylinder.invert();
        let associated = StepParameterCurve::new(
            Box::new(StepCurve2D::Line(model::Line(
                Point2::new(0.0, 1.0), Point2::new(1.0, 1.0),
            ))),
            Box::new(StepSurface::ElementarySurface(
                ElementarySurface::CylindricalSurface(cylinder),
            )),
        );
        let (selected_surface, selected_map) =
            selected.surface().to_modeling_with_parameter_map().unwrap();
        let (current_surface, current_map) =
            associated.surface().to_modeling_with_parameter_map().unwrap();
        let selected_chart = model::Curve2D::try_from(selected.curve()).unwrap();
        let current_chart = model::Curve2D::TrimmedCurve(model::Processor::<_, Matrix3>::new(
            model::TrimmedCurve::new(
                Box::new(model::Curve2D::Line(model::Line(
                    Point2::new(0.0, 1.0), Point2::new(1.0, 1.0),
                ))),
                range,
            ),
        ));
        let guide = model::Curve::ParameterCurve(model::ParameterCurve::new(
            selected_chart.clone(),
            Box::new(model::Surface::Plane(model::Plane::from_origin_vectors(
                Point3::new(0.0, 0.0, 0.02), Vector3::unit_x(), Vector3::unit_y(),
            ))),
        ));
        let curve = model::Curve::IntersectionCurve(model::SurfaceCurve::with_boundaries(
            Box::new(selected_surface.clone()), Box::new(current_surface.clone()),
            Box::new(guide),
            Some(model::ParameterCurve::new(
                selected_chart.clone(), Box::new(selected_surface.clone()),
            )),
            Some(model::ParameterCurve::new(
                current_chart, Box::new(current_surface.clone()),
            )),
        ));
        // The original and native guide are the same displaced circle. Its
        // angular branch only proposes the unbounded LINE's finite active
        // interval; the retained root, not this offset guide, proves position.
        let source_guide = Curve3D::Conic(Conic3D::Ellipse(model::Processor::with_transform(
            model::TrimmedCurve::new(model::UnitCircle::<Point3>::new(), range),
            Matrix4::from_translation(Vector3::new(0.0, 0.0, 0.02))
                * Matrix4::from_scale(2.0),
        )));
        let original = Curve3D::SurfaceCurve(SurfaceCurve3D::new(
            SurfaceCurveKind::IntersectionCurve, Box::new(source_guide.clone()),
            vec![self::associated(selected.clone()), self::associated(associated.clone())],
            SurfaceCurveRepresentation::ParameterCurve0,
        ));
        let cancel = AtomicBool::new(false);
        let prepared = model::prepared_validated_curve_owned(
            curve.clone(), [range.0, range.1], &model::ValidatedCurveOptions::default(),
            &mut model::ValidatedGeometryBudget::new(8_000_000), &cancel,
        ).unwrap();
        let mut work = GeometryWorkBudget::new(128_000_000);
        let (selected_chart, selected_margin) = retained_chart(
            &original, &selected, &curve, &selected_surface, selected_map,
            range, &mut work, &cancel,
        ).unwrap();
        let (current_chart, current_margin) = retained_chart(
            &original, &associated, &curve, &current_surface, current_map,
            range, &mut work, &cancel,
        ).unwrap();
        let selected_input = trim_certificate::Correspondence {
            curve: &curve, surface: &selected_surface, chart: &selected_chart,
            maximum_error_mm: 1.0e-7, margin_uv: selected_margin,
        };
        let coarse = trim_certificate::certify_selected_master_correspondence(
            selected_input, (&prepared, 1.0), None, &mut work, &cancel,
        ).unwrap().unwrap();
        let model::Surface::Plane(plane) = &selected_surface else { unreachable!() };
        let current_range = current_chart.range_tuple();
        let delta = super::super::harmonic::certify(
            super::super::harmonic::HarmonicComparison {
                source: super::super::harmonic::HarmonicSource::PlaneImage(
                    plane, &selected_chart,
                ),
                surface: &current_surface, chart: &current_chart,
                range: [range.0, range.1], target: [current_range.0, current_range.1],
                margin: current_margin, tolerance: selected_input.maximum_error_mm,
                require_tangent: false,
            },
            &mut work, &cancel,
            &mut super::super::harmonic::HarmonicDiagnostic::new(
                selected_input.maximum_error_mm,
            ),
        ).unwrap().unwrap();
        let current_input = trim_certificate::Correspondence {
            surface: &current_surface, chart: &current_chart,
            margin_uv: current_margin, ..selected_input
        };
        let residual = Interval::point(coarse.whole().position_error_mm())
            .add(Interval::point(delta)).hi;
        let current_limit = inverse_base_precision(
            current_input, &prepared, residual, 1.0, &mut work, &cancel,
        ).unwrap().unwrap();
        let selected_limit = inverse_base_precision(
            selected_input, &prepared, coarse.whole().position_error_mm(),
            1.0, &mut work, &cancel,
        ).unwrap().unwrap();
        assert!(Interval::point(current_limit).sub(Interval::point(delta)).lo < selected_limit);
        let mut master = None;
        validate_selected_master(
            &original, Some(range), Some((&curve, &prepared)), &mut master,
            1.0, &mut work, &cancel,
        ).unwrap();
        let master = master.unwrap();
        assert!(Interval::point(master.whole().position_error_mm())
            .add(Interval::point(delta)).hi <= current_limit);
        let (_, final_proof) = trim_certificate::complete_retained_correspondence(
            current_input, &prepared, 1.0, Some(&master), &mut work, &cancel, None,
        ).unwrap().unwrap();
        assert!(final_proof.position_error_mm() <= current_input.maximum_error_mm);

        // A declared but unrelated unused carrier cannot seed an inverse with
        // A alone, nor become a new mandatory import capability.
        let unused = StepParameterCurve::new(
            Box::new(selected.curve().clone()), Box::new(self::plane(0.5)),
        );
        let unrelated = Curve3D::SurfaceCurve(SurfaceCurve3D::new(
            SurfaceCurveKind::IntersectionCurve, Box::new(source_guide),
            vec![self::associated(selected), self::associated(unused)],
            SurfaceCurveRepresentation::ParameterCurve0,
        ));
        validate_selected_master(
            &unrelated, Some(range), Some((&curve, &prepared)), &mut None,
            1.0, &mut work, &cancel,
        ).unwrap();
    }

    #[test]
    fn genuine_master_matching_only_guide_is_refused_against_retained_root() {
        let (guide, root, prepared) = retained_planar_root();
        let matching = implicit_source(
            guide.clone(),
            pcurve(plane(0.0), (7.0, 19.0), Point2::new(1.0, 0.0)),
        );
        let mut completed = None;
        validate_selected_master(
            &matching, Some((0.0, 1.0)), Some((&root, &prepared)), &mut completed, 1.0,
            &mut GeometryWorkBudget::new(8_000_000), &AtomicBool::new(false),
        ).unwrap();
        assert!(completed.is_some());
        let delta = 1.0 / 1024.0;
        let selected = StepParameterCurve::new(
            Box::new(StepCurve2D::BsplineCurve(model::BsplineCurve::new(
                model::KnotVector::from(vec![7.0, 7.0, 19.0, 19.0]),
                vec![Point2::new(0.0, delta), Point2::new(1.0, delta)],
            ))),
            Box::new(plane(0.0)),
        );
        let original = implicit_source(guide, selected);
        assert_eq!(
            validate_selected_master(
                &original,
                Some((0.0, 1.0)),
                Some((&root, &prepared)),
                &mut completed,
                1.0,
                &mut GeometryWorkBudget::new(8_000_000),
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::ParameterCorrespondenceUnavailable
        );
    }

    #[test]
    fn genuine_selected_unbounded_line_proposal_requires_same_retained_root_proof() {
        let (guide, root, prepared) = retained_planar_root();
        let selected = StepParameterCurve::new(
            Box::new(StepCurve2D::Line(model::Line(
                Point2::origin(),
                Point2::new(0.25, 0.0),
            ))),
            Box::new(plane(0.0)),
        );
        let original = implicit_source(guide, selected);
        validate_selected_master(
            &original, Some((0.0, 1.0)), Some((&root, &prepared)), &mut None, 1.0,
            &mut GeometryWorkBudget::new(8_000_000), &AtomicBool::new(false),
        )
        .unwrap();
    }

    #[test]
    fn mapped_original_implicit_trim_borrows_same_active_basis_receipt() {
        for variant in 0..3 {
            let (guide, root, _) = retained_planar_root();
            let uv = if variant != 0 {
                StepCurve2D::BsplineCurve(model::BsplineCurve::new(
                    model::KnotVector::from(vec![7.0, 7.0, 19.0, 19.0]),
                    if variant == 1 {
                        vec![Point2::new(0.25, 0.0), Point2::new(0.75, 0.0)]
                    } else {
                        vec![Point2::origin(), Point2::new(1.0, 0.0)]
                    },
                ))
            } else {
                StepCurve2D::Line(model::Line(Point2::origin(), Point2::new(0.25, 0.0)))
            };
            let selected = StepParameterCurve::new(Box::new(uv), Box::new(plane(0.0)));
            let original = implicit_source(guide, selected);
            let pose = Matrix4::from_nonuniform_scale(2.0, 3.0, 4.0);
            let mut source_trim = model::Processor::with_transform(
                model::TrimmedCurve::new(Box::new(original), (0.25, 0.75)),
                pose,
            );
            source_trim.invert();
            let source = Curve3D::TrimmedCurve(SourceTrimmedCurve::internal(source_trim));
            let mut native_trim = model::Processor::with_transform(
                model::TrimmedCurve::new(Box::new(root), (0.25, 0.75)),
                pose,
            );
            native_trim.invert();
            let native = model::Curve::TrimmedCurve(native_trim);
            let prepared = model::prepared_validated_curve_owned(
                native.clone(),
                [0.25, 0.75],
                &model::ValidatedCurveOptions::default(),
                &mut model::ValidatedGeometryBudget::new(1_000_000),
                &AtomicBool::new(false),
            )
            .unwrap();
            validate_selected_master(
                &source, Some((0.25, 0.75)), Some((&native, &prepared)), &mut None, 1.0,
                &mut GeometryWorkBudget::new(8_000_000), &AtomicBool::new(false),
            )
            .unwrap();
        }
    }

    #[test]
    fn selected_master_has_independent_uv_parameter_scale() {
        let selected = pcurve(plane(0.0), (7.0, 19.0), Point2::new(20.0, 0.0));
        validate(&source(
            leader(),
            vec![associated(selected)],
            SurfaceCurveRepresentation::ParameterCurve0,
        ))
        .unwrap();
    }

    #[test]
    fn clipped_original_spatial_line_uses_bound_edge_interval_authority() {
        let line = Curve3D::Line(model::Line(Point3::origin(), Point3::new(20.0, 0.0, 0.0)));
        let selected = StepParameterCurve::new(
            Box::new(StepCurve2D::Line(model::Line(
                Point2::origin(),
                Point2::new(1.0, 0.0),
            ))),
            Box::new(plane(0.0)),
        );
        let original = source(
            line,
            vec![associated(selected)],
            SurfaceCurveRepresentation::ParameterCurve0,
        );
        // Without original BREP lineage the generic entrypoint cannot invent a
        // finite domain from a bare LINE's parameter-range convenience.
        refused(&original);
        validate_selected_master(
            &original, Some((0.0, 1.0)), None, &mut None, 1.0,
            &mut GeometryWorkBudget::new(8_000_000), &AtomicBool::new(false),
        )
        .unwrap();
    }

    #[test]
    fn selected_plane_line_with_normal_roundoff_keeps_whole_active_domain() {
        // This is the first independent rounded-housing association's frame:
        // plane normal +X, U +Z, V -Y; the original LINE has a nonzero normal
        // residual. An exact coefficient projection cannot grant its receipt.
        let carrier = StepSurface::ElementarySurface(ElementarySurface::Plane(
            model::Plane::from_origin_vectors(
                Point3::origin(),
                Vector3::unit_z(),
                -Vector3::unit_y(),
            ),
        ));
        for normal_residual in [-4.440892098501e-16, -1.0e-3] {
            for bounded_leader in [false, true] {
                let line = Curve3D::Line(model::Line(
                    Point3::new(normal_residual, 2.0, 0.0),
                    Point3::new(normal_residual, 2.0, 1.0),
                ));
                let leader = if bounded_leader {
                    Curve3D::TrimmedCurve(SourceTrimmedCurve::internal(model::Processor::<
                        _,
                        Matrix4,
                    >::new(
                        model::TrimmedCurve::new(Box::new(line), (2.0, 8.0)),
                    )))
                } else {
                    line
                };
                let selected = StepParameterCurve::new(
                    Box::new(StepCurve2D::Line(model::Line(
                        Point2::new(0.0, -2.0),
                        Point2::new(1.0, -2.0),
                    ))),
                    Box::new(carrier.clone()),
                );
                let original = source(
                    leader,
                    vec![associated(selected)],
                    SurfaceCurveRepresentation::ParameterCurve0,
                );
                let result = validate_selected_master(
                    &original, Some((2.0, 8.0)), None, &mut None, 1.0,
                    &mut GeometryWorkBudget::new(8_000_000), &AtomicBool::new(false),
                );
                if normal_residual.abs() < 1.0e-7 {
                    result.unwrap();
                } else {
                    assert_eq!(
                        result.unwrap_err().code,
                        GeometryErrorCode::ParameterCorrespondenceUnavailable
                    );
                }
            }
        }
    }

    #[test]
    fn selected_cylinder_generator_uses_original_finite_spatial_line_span() {
        let mut cylinder = model::Processor::new(model::RevolutionSurface::by_revolution(
            model::Line(Point3::new(2.0, 0.0, 0.0), Point3::new(2.0, 0.0, 1.0)),
            Point3::origin(),
            Vector3::unit_z(),
        ));
        // STEP cylinder UV is angle/axial, unlike its revolution basis.
        cylinder.invert();
        let carrier =
            StepSurface::ElementarySurface(ElementarySurface::CylindricalSurface(cylinder));
        for direction in [0.25, -0.25] {
            for angle in [0.0, std::f64::consts::TAU] {
                let selected = StepParameterCurve::new(
                    Box::new(StepCurve2D::Line(model::Line(
                        Point2::new(angle, 0.0),
                        Point2::new(angle, direction),
                    ))),
                    Box::new(carrier.clone()),
                );
                let original = source(
                    Curve3D::Line(model::Line(
                        Point3::new(2.0, 0.0, 0.0),
                        Point3::new(2.0, 0.0, 1.0),
                    )),
                    vec![associated(selected)],
                    SurfaceCurveRepresentation::ParameterCurve0,
                );
                validate_selected_master(
                    &original, Some((2.0, 8.0)), None, &mut None, 1.0,
                    &mut GeometryWorkBudget::new(8_000_000), &AtomicBool::new(false),
                )
                .unwrap();
            }
        }
        // A finite chart proposal must not authorize a different interior
        // expression even when its axial endpoint coordinates match.
        let selected = StepParameterCurve::new(
            Box::new(StepCurve2D::Line(model::Line(
                Point2::origin(),
                Point2::new(0.125, 0.25),
            ))),
            Box::new(carrier),
        );
        let original = source(
            Curve3D::Line(model::Line(
                Point3::new(2.0, 0.0, 0.0),
                Point3::new(2.0, 0.0, 1.0),
            )),
            vec![associated(selected)],
            SurfaceCurveRepresentation::ParameterCurve0,
        );
        assert_eq!(
            validate_selected_master(
                &original,
                Some((2.0, 8.0)),
                None,
                &mut None,
                1.0,
                &mut GeometryWorkBudget::new(8_000_000),
                &AtomicBool::new(false),
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::ParameterCorrespondenceUnavailable
        );
    }

    #[test]
    fn clipped_line_split_does_not_narrow_original_master_authority() {
        let line = Curve3D::Line(model::Line(Point3::origin(), Point3::new(20.0, 0.0, 0.0)));
        let selected = StepParameterCurve::new(
            Box::new(StepCurve2D::BsplineCurve(model::BsplineCurve::new(
                model::KnotVector::from(vec![7.0, 7.0, 13.0, 19.0, 19.0]),
                vec![
                    Point2::origin(),
                    Point2::new(10.0, 0.0),
                    Point2::new(20.0, 2.0),
                ],
            ))),
            Box::new(plane(0.0)),
        );
        let original = source(
            line,
            vec![associated(selected)],
            SurfaceCurveRepresentation::ParameterCurve0,
        );
        let prefix = Curve3D::TrimmedCurve(SourceTrimmedCurve::internal(model::Processor::<
            _,
            Matrix4,
        >::new(
            model::TrimmedCurve::new(Box::new(original), (0.0, 0.5)),
        )));
        assert_eq!(
            validate_selected_master(
                &prefix,
                Some((0.0, 1.0)),
                None,
                &mut None,
                1.0,
                &mut GeometryWorkBudget::new(8_000_000),
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::ParameterCorrespondenceUnavailable
        );
    }

    #[test]
    fn selected_stray_carrier_cannot_be_replaced_by_valid_incident_association() {
        let good = pcurve(plane(0.0), (7.0, 19.0), Point2::new(20.0, 0.0));
        let bad = pcurve(plane(3.0), (7.0, 19.0), Point2::new(20.0, 0.0));
        refused(&source(
            leader(),
            vec![associated(good.clone()), associated(bad.clone())],
            SurfaceCurveRepresentation::ParameterCurve1,
        ));
        refused(&source(
            leader(),
            vec![associated(bad), associated(good)],
            SurfaceCurveRepresentation::ParameterCurve0,
        ));
    }

    #[test]
    fn selected_index_must_be_a_pcurve_not_the_next_available_association() {
        let good = pcurve(plane(0.0), (7.0, 19.0), Point2::new(20.0, 0.0));
        refused(&source(
            leader(),
            vec![
                SurfaceCurveAssociatedGeometry::Surface(Box::new(plane(0.0))),
                associated(good),
            ],
            SurfaceCurveRepresentation::ParameterCurve0,
        ));
    }

    #[test]
    fn curve3d_master_has_no_extra_preferred_pcurve_authority() {
        let bad = pcurve(plane(3.0), (7.0, 19.0), Point2::new(20.0, 0.0));
        validate(&source(
            leader(),
            vec![associated(bad)],
            SurfaceCurveRepresentation::Curve3D,
        ))
        .unwrap();
    }

    #[test]
    fn original_unbounded_uv_line_uses_complete_active_span() {
        let selected = StepParameterCurve::new(
            Box::new(StepCurve2D::Line(model::Line(
                Point2::origin(),
                Point2::new(1.0, 0.0),
            ))),
            Box::new(plane(0.0)),
        );
        validate(&source(
            leader(),
            vec![associated(selected)],
            SurfaceCurveRepresentation::ParameterCurve0,
        ))
        .unwrap();
    }

    #[test]
    fn periodic_split_cannot_hide_wrong_original_master_winding() {
        let tau = std::f64::consts::TAU;
        let radius = 1.0e-10;
        let profile = model::Line(Point3::new(radius, 0.0, 0.0), Point3::new(radius, 0.0, 1.0));
        let carrier = StepSurface::ElementarySurface(ElementarySurface::CylindricalSurface(
            model::Processor::new(model::RevolutionSurface::by_revolution(
                profile,
                Point3::origin(),
                Vector3::unit_z(),
            )),
        ));
        let circle = Curve3D::Conic(Conic3D::Ellipse(model::Processor::with_transform(
            model::TrimmedCurve::new(model::UnitCircle::<Point3>::new(), (0.0, tau)),
            Matrix4::from_scale(radius),
        )));
        let selected = pcurve(carrier, (7.0, 19.0), Point2::new(0.0, 2.0 * tau));
        let original = source(
            circle,
            vec![associated(selected)],
            SurfaceCurveRepresentation::ParameterCurve0,
        );
        let split = Curve3D::TrimmedCurve(SourceTrimmedCurve::internal(model::Processor::<
            _,
            Matrix4,
        >::new(
            model::TrimmedCurve::new(Box::new(original), (0.0, tau / 2.0)),
        )));
        refused(&split);
    }

    #[test]
    fn nested_original_master_is_not_hidden_by_curve3d_wrapper() {
        let bad = source(
            leader(),
            vec![associated(pcurve(
                plane(3.0),
                (7.0, 19.0),
                Point2::new(20.0, 0.0),
            ))],
            SurfaceCurveRepresentation::ParameterCurve0,
        );
        refused(&source(bad, vec![], SurfaceCurveRepresentation::Curve3D));
    }

    #[test]
    fn input_chart_translation_error_is_not_an_exact_identity() {
        let mut chart = SurfaceParameterChart::identity();
        chart.axes[0].offset_enclosure =
            monstertruck_io::step::ParameterEnclosure::new(-1.0, 1.0).unwrap();
        let selected = StepParameterCurve::try_from_parts(
            Box::new(StepCurve2D::BsplineCurve(model::BsplineCurve::new(
                model::KnotVector::from(vec![7.0, 7.0, 19.0, 19.0]),
                vec![Point2::origin(), Point2::new(20.0, 0.0)],
            ))),
            Box::new(plane(0.0)),
            chart,
        )
        .unwrap();
        refused(&source(
            leader(),
            vec![associated(selected)],
            SurfaceCurveRepresentation::ParameterCurve0,
        ));
    }

    #[test]
    fn split_restriction_cannot_validate_only_a_matching_prefix() {
        let selected = StepParameterCurve::new(
            Box::new(StepCurve2D::BsplineCurve(model::BsplineCurve::new(
                model::KnotVector::from(vec![7.0, 7.0, 13.0, 19.0, 19.0]),
                vec![
                    Point2::origin(),
                    Point2::new(10.0, 0.0),
                    Point2::new(20.0, 2.0),
                ],
            ))),
            Box::new(plane(0.0)),
        );
        let original = source(
            leader(),
            vec![associated(selected)],
            SurfaceCurveRepresentation::ParameterCurve0,
        );
        let prefix = Curve3D::TrimmedCurve(SourceTrimmedCurve::internal(model::Processor::<
            _,
            Matrix4,
        >::new(
            model::TrimmedCurve::new(Box::new(original), (2.0, 3.0)),
        )));
        refused(&prefix);
    }

    #[test]
    fn outer_actual_affine_amplification_is_not_ignored() {
        let selected = pcurve(plane(1.0e-10), (7.0, 19.0), Point2::new(20.0, 0.0));
        let original = source(
            leader(),
            vec![associated(selected)],
            SurfaceCurveRepresentation::ParameterCurve0,
        );
        // Local residual is below the physical tolerance, but the actual outer
        // operator amplifies it to a unit-sized world discrepancy.
        validate(&original).unwrap();
        let scaled = Curve3D::TrimmedCurve(SourceTrimmedCurve::internal(
            model::Processor::with_transform(
                model::TrimmedCurve::new(Box::new(original), (2.0, 5.0)),
                Matrix4::from_scale(1.0e10),
            ),
        ));
        refused(&scaled);
    }

    #[test]
    fn raw_and_decoded_input_charts_are_applied_once() {
        use monstertruck_io::step::load::{SurfaceOfLinearExtrusionHolder, Table};
        use std::str::FromStr;
        use step_p21::{ast::DataSection, tables::EntityTable};
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
        let carrier = StepSurface::SweepSurface(SweepSurface::ExtrusionSurface(
            monstertruck_io::step::load::step_geometry::StepExtrusionSurface::try_from(&entity)
                .unwrap(),
        ));
        let world = Curve3D::BsplineCurve(model::BsplineCurve::new(
            model::KnotVector::from(vec![2.0, 2.0, 5.0, 5.0]),
            vec![Point3::new(14.0, 0.0, 0.0), Point3::new(10.0, 0.0, 1.0)],
        ));
        let raw_uv = StepCurve2D::BsplineCurve(model::BsplineCurve::new(
            model::KnotVector::from(vec![7.0, 7.0, 19.0, 19.0]),
            vec![Point2::new(-7.0, 0.0), Point2::new(-5.0, 1.0)],
        ));
        let raw =
            StepParameterCurve::try_from_raw(Box::new(raw_uv), Box::new(carrier.clone())).unwrap();
        assert_ne!(
            raw.input_parameter_chart(),
            SurfaceParameterChart::identity()
        );
        validate(&source(
            world.clone(),
            vec![associated(raw)],
            SurfaceCurveRepresentation::ParameterCurve0,
        ))
        .unwrap();
        let decoded_uv = StepCurve2D::BsplineCurve(model::BsplineCurve::new(
            model::KnotVector::from(vec![7.0, 7.0, 19.0, 19.0]),
            vec![Point2::new(5.0, 0.0), Point2::new(7.0, 1.0)],
        ));
        let decoded = StepParameterCurve::new(Box::new(decoded_uv), Box::new(carrier));
        validate(&source(
            world,
            vec![associated(decoded)],
            SurfaceCurveRepresentation::ParameterCurve0,
        ))
        .unwrap();
    }

    #[test]
    fn cancellation_and_nested_work_remain_typed() {
        let cancel = AtomicBool::new(true);
        assert_eq!(
            validate_selected_master(
                &leader(),
                None,
                None,
                &mut None,
                1.0,
                &mut GeometryWorkBudget::new(1),
                &cancel
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::Cancelled
        );
        let mut nested = leader();
        for _ in 0..=MAX_DEPTH {
            nested = source(nested, vec![], SurfaceCurveRepresentation::Curve3D);
        }
        assert_eq!(
            validate(&nested).unwrap_err().code,
            GeometryErrorCode::ResourceLimit
        );
        assert_eq!(
            validate_selected_master(
                &source(leader(), vec![], SurfaceCurveRepresentation::Curve3D),
                None,
                None,
                &mut None,
                1.0,
                &mut GeometryWorkBudget::new(1),
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::ResourceLimit
        );
    }
}
