// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Fresh coefficient proof for the comparison germs, not a modification of G,
//! the original correspondence, or its retained conservative tangent bound.

use super::*;

#[derive(Clone, Copy)]
pub(super) struct GermTransport {
    pub(super) position_error_mm: f64,
    pub(super) relative_derivative_error: f64,
}

pub(super) const STACK_BYTES: usize =
    std::mem::size_of::<[([f64; 2], u8); MAX_DEPTH as usize + 1]>()
        + std::mem::size_of::<crate::kernel::source_operand::PreparedOriginalCurve<'static>>()
        + std::mem::size_of::<Vec<f64>>();

/// Certify BOTH literal original endpoint-to-exit germs. The Native joint only
/// authorizes the common endpoint; it does not supply this whole-germ theorem.
pub(super) fn certify(
    domain: &PreparedFaceTrimDomain<'_>,
    recipe: &CornerRecipe,
    candidate: &UvJointComparison,
    allowance: f64,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[GermTransport; 2]>, GeometryError> {
    work.charge(1, cancel)?;
    if !allowance.is_finite() || allowance <= 0.0 {
        return Ok(None);
    }
    let mut result = [GermTransport {
        position_error_mm: 0.0,
        relative_derivative_error: 0.0,
    }; 2];
    for (i, end) in [recipe.outgoing, recipe.incoming].into_iter().enumerate() {
        let proof = domain.original_coedge_correspondence(end.use_.coedge)?;
        if !same_end(candidate.ends[i], end, proof)
            || !proof.position_error_mm().is_finite()
            || proof.position_error_mm() < 0.0
            || proof.position_error_mm() >= allowance
            || candidate.correction[i]
                .iter()
                .any(|v| !v.lo.is_finite() || !v.hi.is_finite() || v.lo > v.hi)
        {
            return Ok(None);
        }
        // Stream only relevant interior exact cuts before the canonical owner
        // callback: its explicit branch can hold the original edge mutex.
        // No heap allocation is needed when this germ has no interior cuts.
        let cuts = exact_interior_cuts(domain, end, remaining_bytes, work, cancel)?;
        let curve_bytes = remaining_bytes
            .checked_sub(cuts.capacity().saturating_mul(std::mem::size_of::<f64>()))
            .ok_or_else(|| refusal("comparison germ cuts exceed admitted scratch"))?;
        let transport = domain.with_original_spatial_curve(
            end.use_.coedge,
            curve_bytes,
            work,
            cancel,
            |spatial, work, cancel| {
                certify_spans(
                    domain, end, candidate, i, allowance, &cuts, spatial, work, cancel,
                )
            },
        )?;
        result[i] = match transport {
            Some(transport) => transport,
            None => return Ok(None),
        };
    }
    Ok(Some(result))
}

pub(super) fn exact_interior_cuts(
    domain: &PreparedFaceTrimDomain<'_>,
    end: End,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Vec<f64>, GeometryError> {
    let support = [end.endpoint.min(end.exit), end.endpoint.max(end.exit)];
    let relevant = |boundary: Interval| {
        boundary.lo == boundary.hi && boundary.lo > support[0] && boundary.lo < support[1]
    };
    let mut count = 0;
    domain.visit_original_coedge_span_boundaries(
        end.use_.coedge,
        work,
        cancel,
        &mut |boundary| {
            if relevant(boundary) {
                if count >= remaining_bytes / std::mem::size_of::<f64>() {
                    return Err(GeometryError::new(
                        GeometryErrorCode::ResourceLimit,
                        "comparison germ interior cuts exceed remaining scratch",
                    ));
                }
                count += 1;
            }
            Ok(())
        },
    )?;
    let mut cuts = Vec::new();
    cuts.try_reserve_exact(count).map_err(|_| {
        GeometryError::new(
            GeometryErrorCode::ResourceLimit,
            "comparison germ interior cut allocation refused",
        )
    })?;
    if cuts.capacity() > remaining_bytes / std::mem::size_of::<f64>() {
        return Err(GeometryError::new(
            GeometryErrorCode::ResourceLimit,
            "comparison germ interior cut capacity exceeds remaining scratch",
        ));
    }
    if count != 0 {
        domain.visit_original_coedge_span_boundaries(
            end.use_.coedge,
            work,
            cancel,
            &mut |boundary| {
                if relevant(boundary) {
                    // The two inventories borrow the SAME immutable operands.
                    // An unexpected inventory change must not grow storage.
                    if cuts.len() >= count {
                        return Err(refusal("comparison germ original cut inventory changed"));
                    }
                    cuts.push(boundary.lo);
                }
                Ok(())
            },
        )?;
    }
    // Use the original correspondence inventory's in-place exact-cut ordering
    // convention; precharge sorting and the full pre-dedup linear traversal.
    let levels = usize::BITS - cuts.len().leading_zeros();
    work.charge((cuts.len() as u64) * (u64::from(levels) + 1), cancel)?;
    cuts.sort_unstable_by(f64::total_cmp);
    cuts.dedup();
    work.charge(1, cancel)?;
    Ok(cuts)
}

fn certify_spans(
    domain: &PreparedFaceTrimDomain<'_>,
    end: End,
    candidate: &UvJointComparison,
    index: usize,
    allowance: f64,
    cuts: &[f64],
    spatial: &(impl crate::parametric::CurveEvaluator + ?Sized),
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<GermTransport>, GeometryError> {
    let support = [end.endpoint.min(end.exit), end.endpoint.max(end.exit)];
    if !support[0].is_finite() || !support[1].is_finite() || support[0] >= support[1] {
        return Ok(None);
    }
    let mut lower = support[0];
    let mut bound = GermTransport {
        position_error_mm: 0.0,
        relative_derivative_error: 0.0,
    };
    for upper in cuts.iter().copied().chain(std::iter::once(support[1])) {
        work.charge(1, cancel)?;
        let Some(cell) = certify_end(
            domain,
            end,
            [lower, upper],
            candidate,
            index,
            allowance,
            work,
            cancel,
            |range, work, cancel| {
                crate::parametric::curve_enclosure(spatial, (range[0], range[1]), work, cancel)
            },
        )?
        else {
            return Ok(None);
        };
        bound.position_error_mm = bound.position_error_mm.max(cell.position_error_mm);
        bound.relative_derivative_error = bound
            .relative_derivative_error
            .max(cell.relative_derivative_error);
        lower = upper;
    }
    Ok(Some(bound))
}

/// A strict *achieved* derivative bound, computed from fresh original 3DC and
/// completed surface-composition coefficients. The old receipt's requested
/// relative error (possibly exactly .25) is deliberately not an input.
pub(super) fn relative_error(source: Box3, completed: Box3) -> Result<Option<f64>, GeometryError> {
    let speed = length(source)?;
    let difference = length(std::array::from_fn(|i| source[i].sub(completed[i])))?;
    if speed.lo <= 0.0 || !speed.lo.is_finite() || !difference.hi.is_finite() {
        return Ok(None);
    }
    let relative = Interval::point(difference.hi)
        .div(Interval::point(speed.lo))?
        .hi;
    Ok((relative < 0.25).then_some(relative))
}

fn certify_end(
    domain: &PreparedFaceTrimDomain<'_>,
    end: End,
    support: [f64; 2],
    candidate: &UvJointComparison,
    index: usize,
    allowance: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    mut spatial: impl FnMut(
        [f64; 2],
        &mut GeometryWorkBudget,
        &AtomicBool,
    ) -> Result<CurveEnclosure, GeometryError>,
) -> Result<Option<GermTransport>, GeometryError> {
    let proof = domain.original_coedge_correspondence(end.use_.coedge)?;
    let spatial_range = proof.spatial_range();
    if support[0] >= support[1]
        || support[0] < spatial_range[0]
        || support[1] > spatial_range[1]
        || end.delta_t.lo <= 0.0 && end.delta_t.hi >= 0.0
    {
        return Ok(None);
    }
    let mut stack = [([0.0; 2], 0_u8); MAX_DEPTH as usize + 1];
    stack[0] = (support, 0);
    let mut pending = 1;
    let mut bound = GermTransport {
        position_error_mm: 0.0,
        relative_derivative_error: 0.0,
    };
    while pending != 0 {
        work.charge(1, cancel)?;
        pending -= 1;
        let (range, depth) = stack[pending];
        let queried = (|| {
            // t is restricted to the SAME literal spatial-provider endpoints. The
            // independently known affine image lies in [0,1]; intersection
            // only removes outward arithmetic spill, never extends a domain.
            let s = Interval {
                lo: range[0],
                hi: range[1],
            }
            .sub(Interval::point(end.endpoint))
            .div(end.delta_t)?;
            let s = intersection(s, UNIT)
                .ok_or_else(|| refusal("comparison germ left its original affine support"))?;
            let uv = chart_jet(domain, end.use_.coedge, range, work, cancel)?;
            let source = spatial(range, work, cancel)?;
            let correction = candidate.germ_correction(index, s)?;
            let shift = correction.position;
            let completed: Box2 = std::array::from_fn(|i| uv.position[i].add(shift[i]));
            let homotopy: Box2 = std::array::from_fn(|i| uv.position[i].add(UNIT.mul(shift[i])));
            let completed_s: Box2 = std::array::from_fn(|i| {
                uv.derivative[i]
                    .mul(end.delta_t)
                    .add(correction.derivative[i])
            });
            let source_s = source.derivative.map(|v| v.mul(end.delta_t));
            {
                let surface = &domain.surface_operand();
                (|| {
                    // The entire straight UV homotopy, not its endpoint samples,
                    // bounds F(completed)-F(original) by the integral first jet.
                    let homotopy_jet = surface_enclosure(surface, uv_box(homotopy), work, cancel)?;
                    let displacement = compose_first(&homotopy_jet.du, &homotopy_jet.dv, shift);
                    let position_error_mm = Interval::point(proof.position_error_mm())
                        .add(Interval::point(length(displacement)?.hi))
                        .hi;
                    if !position_error_mm.is_finite() || position_error_mm >= allowance {
                        return Ok(None);
                    }
                    let completed_jet =
                        surface_enclosure(surface, uv_box(completed), work, cancel)?;
                    let completed_spatial_s =
                        compose_first(&completed_jet.du, &completed_jet.dv, completed_s);
                    let Some(relative_derivative_error) =
                        relative_error(source_s, completed_spatial_s)?
                    else {
                        return Ok(None);
                    };
                    Ok(Some(GermTransport {
                        position_error_mm,
                        relative_derivative_error,
                    }))
                })()
            }
        })();
        match queried {
            Ok(Some(cell)) => {
                bound.position_error_mm = bound.position_error_mm.max(cell.position_error_mm);
                bound.relative_derivative_error = bound
                    .relative_derivative_error
                    .max(cell.relative_derivative_error);
                continue;
            }
            Err(error) => return Err(error),
            Ok(None) => {}
        }
        if depth >= MAX_DEPTH {
            return Ok(None);
        }
        // Every exact original span boundary has already been visited. An
        // uncertain mapped knot band remains subject to native jet admission;
        // subdivision never asserts that its midpoint is a smooth knot.
        let split = range[0] * 0.5 + range[1] * 0.5;
        if split <= range[0] || split >= range[1] {
            return Ok(None);
        }
        stack[pending] = ([split, range[1]], depth + 1);
        stack[pending + 1] = ([range[0], split], depth + 1);
        pending += 2;
    }
    Ok(Some(bound))
}

#[cfg(test)]
mod properties {
    use super::*;

    #[test]
    fn actual_fitting_coefficients_replace_requested_quarter_bound_at_zero_margin() {
        let cancel = AtomicBool::new(false);
        let definition = crate::from_step_bytes_with_cancel(include_bytes!(
            "../../../../../../../../fixtures/geometry/independent-exporters/occt-tapered-fitting-mm.step"
        ),
        &cancel,)
        .unwrap();
        let original = format!("{:?}", definition.brep());
        let mut work = GeometryWorkBudget::new(20_000_000);
        let mut incidence =
            SourceEdgeIncidence::new(&definition, MAX_SCRATCH_BYTES, &mut work, &cancel).unwrap();
        incidence
            .prepare_vertices(MAX_SCRATCH_BYTES, &mut work, &cancel)
            .unwrap();
        let prepared = crate::analysis::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let source = prepared.source_view().unwrap().with_vertex_plans(None);
        let allowance = source_allowance(&definition).unwrap();
        let mut vertices = Vec::new();
        for vertex in incidence.vertices().unwrap() {
            vertices.push(vertex);
        }
        let mut fresh = 0;
        let mut zero_margin_requested_quarter = 0;
        for vertex in vertices {
            let plan = SourceVertexTransportPlan::prepare(
                source,
                &incidence,
                vertex,
                MAX_SCRATCH_BYTES,
                &mut work,
                &cancel,
            )
            .unwrap()
            .expect("actual source-only G must retain every original NativeV");
            for recipe in &plan.recipes {
                let comparison = *recipe
                    .comparison
                    .expect("actual corner comparison")
                    .native_uv()
                    .expect("the actual fitting has native UV corner charts");
                if comparison.is_identity() {
                    continue;
                }
                let domain = source
                    .face_domain(
                        recipe.outgoing.use_.face,
                        MAX_SCRATCH_BYTES,
                        &mut work,
                        &cancel,
                    )
                    .unwrap();
                let scratch = MAX_SCRATCH_BYTES
                    .checked_sub(plan.retained_bytes() + CONICAL_STACK_BYTES)
                    .unwrap();
                let bounds = certify(
                    &domain,
                    recipe,
                    &comparison,
                    allowance,
                    scratch,
                    &mut work,
                    &cancel,
                )
                .unwrap()
                .expect("Native-authorized microscopic completion needs a fresh whole-germ proof");
                fresh += 1;
                for (i, end) in [recipe.outgoing, recipe.incoming].into_iter().enumerate() {
                    let proof = domain
                        .original_coedge_correspondence(end.use_.coedge)
                        .unwrap();
                    assert!(bounds[i].position_error_mm >= proof.position_error_mm());
                    assert!(bounds[i].position_error_mm < allowance);
                    assert!(bounds[i].relative_derivative_error < 0.25);
                    if proof.margin_uv() == [0.0; 2]
                        && proof.relative_derivative_error_bound() == 0.25
                    {
                        zero_margin_requested_quarter += 1;
                    }
                    let range = [end.endpoint.min(end.exit), end.endpoint.max(end.exit)];
                    let native = domain
                        .original_coedge_jets(end.use_.coedge, range, &mut work, &cancel)
                        .unwrap()
                        .0;
                    let source_s = native.derivative.map(|v| v.mul(end.delta_t));
                    assert!(length(source_s).unwrap().lo > 0.0);
                    // These are the actual source coefficient jets, not a
                    // line/guide fixture: an opposed or half-speed comparison
                    // cannot acquire a strict quarter-tangent certificate.
                    assert!(
                        relative_error(source_s, source_s.map(Interval::neg))
                            .unwrap()
                            .is_none()
                    );
                    assert!(
                        relative_error(source_s, source_s.map(|v| v.mul(Interval::point(0.5))),)
                            .unwrap()
                            .is_none()
                    );
                    assert!(
                        relative_error(source_s, source_s.map(|v| v.mul(Interval::point(0.75))),)
                            .unwrap()
                            .is_none(),
                        "a conservative quarter bound is not strict"
                    );
                }
                // Keep exactly the SAME admitted allowance, Native owner,
                // original coefficients and literal endpoint-to-exit support.
                // Move toward this actual source chart's interior, rather than
                // leaving a bounded carrier domain and testing an unrelated
                // parameter/arithmetic error. This is still the SAME allowance.
                let target = domain
                    .uv_domain()
                    .map(|range| Interval::point(range[0] * 0.5 + range[1] * 0.5));
                let mut excessive = comparison;
                excessive.correction = std::array::from_fn(|end| {
                    std::array::from_fn(|axis| {
                        target[axis].sub(comparison.ends[end].joint_uv[axis])
                    })
                });
                assert!(
                    certify(
                        &domain, recipe, &excessive, allowance, scratch, &mut work, &cancel,
                    )
                    .unwrap()
                    .is_none()
                );
            }
        }
        assert!(fresh > 0, "exercise a nonidentity actual Native joint");
        assert!(
            zero_margin_requested_quarter > 0,
            "the original zero-margin/.25 receipt is not an achieved derivative proof"
        );
        assert_eq!(
            format!("{:?}", definition.brep()),
            original,
            "comparison proof must never modify original curve/carrier geometry",
        );
    }
}
