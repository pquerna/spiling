// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::analysis::material_source::MaterialSourceView;
use crate::analysis::slab::HeightFunctional;

const MAX_DEPTH: u8 = 32;
pub(super) const SCRATCH_BYTES: usize = 256 * 1024
    + crate::parametric::nurbs_eval::SURFACE_EVALUATION_SCRATCH_BYTES
    + crate::parametric::PreparedValidatedCurve::scratch_bytes();

#[allow(clippy::too_many_arguments)]
pub(super) fn certify(
    source: MaterialSourceView<'_, '_>,
    pose: RigidPoseMm,
    range: [f64; 2],
    sample: [f64; 2],
    critical: [bool; 2],
    remaining: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[bool; 2], GeometryError> {
    work.charge(1, cancel)?;
    if remaining < crate::kernel::trim::NATIVE_SPATIAL_QUERY_SCRATCH_BYTES {
        return Err(error(
            GeometryErrorCode::ResourceLimit,
            "regular side native evaluator exceeds scratch allowance",
        ));
    }
    let direction = super::super::placed::rotation(pose)?[2];
    let functional = HeightFunctional {
        direction,
        offset: Interval::point(pose.translation_mm[2]),
    };
    let tolerance = source.effective_tolerance_mm();
    let uncertainty = functional.dot(
        [Interval {
            lo: -tolerance,
            hi: tolerance,
        }; 3],
    );
    let mut result = [false; 2];
    for side in 0..2 {
        work.charge(1, cancel)?;
        if critical[side] {
            continue;
        }
        let height = if side == 0 {
            Interval::new(range[0], sample[1])?
        } else {
            Interval::new(sample[0], range[1])?
        };
        let proof = native_supports(
            source,
            functional,
            height,
            uncertainty,
            remaining,
            work,
            cancel,
        );
        result[side] = match proof {
            Ok(value) => value,
            Err(failure)
                if matches!(
                    failure.code,
                    GeometryErrorCode::AnalysisIndeterminate
                        | GeometryErrorCode::ConditioningFailure
                        | GeometryErrorCode::ParameterCorrespondenceUnavailable
                ) =>
            {
                false
            }
            Err(failure) => return Err(failure),
        };
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn native_supports(
    source: MaterialSourceView<'_, '_>,
    functional: HeightFunctional,
    height: Interval,
    uncertainty: Interval,
    remaining: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let intersects = |position| {
        let support = functional.height(position).add(uncertainty);
        support.lo <= height.hi && height.lo <= support.hi
    };
    let brep = source.definition().brep();
    for &vertex in brep.vertex_list() {
        work.charge(1, cancel)?;
        let point = brep
            .vertex(vertex)
            .map_err(|_| {
                error(
                    GeometryErrorCode::InvalidGeometry,
                    "regular side lost its native vertex",
                )
            })?
            .point;
        if intersects([point.x, point.y, point.z].map(Interval::point)) {
            return Ok(false);
        }
    }
    for face in 0..source.definition().face_ids().len() {
        work.charge(1, cancel)?;
        let domain = source.face_domain(face, remaining, work, cancel)?;
        let surface = domain.surface_operand();
        for uv in
            crate::parametric::surface_parameter_boxes(&surface, domain.uv_domain(), work, cancel)?
        {
            if !face_regular(
                &surface,
                uv,
                functional,
                height,
                uncertainty,
                0,
                work,
                cancel,
            )? {
                return Ok(false);
            }
        }
        for &[start, end] in domain.loops() {
            for incoming in start..end {
                work.charge(1, cancel)?;
                let outgoing = if incoming + 1 == end {
                    start
                } else {
                    incoming + 1
                };
                let (_, joint) = domain.native_joint_cover(incoming, outgoing, work, cancel)?;
                let position = crate::parametric::surface_piecewise_enclosure(
                    &domain.surface_operand(),
                    joint.map(|value| [value.lo, value.hi]),
                    work,
                    cancel,
                )?
                .position;
                if intersects(position) {
                    return Ok(false);
                }
            }
        }
        for (index, coedge) in domain.coedges().iter().enumerate() {
            work.charge(1, cancel)?;
            if !crate::parametric::curve_parameter_span_continuous(
                coedge.curve(),
                coedge.uv_range,
                work,
                cancel,
            )? {
                return Ok(false);
            }
            if matches!(
                coedge.curve().kind(),
                solidcore::curve::curve::CurveKind::CrvOnSurf(_)
            ) {
                return Ok(false);
            }
            let mut covered = coedge.uv_range[0];
            for span in crate::parametric::curve_parameter_spans(
                coedge.curve(),
                coedge.uv_range,
                work,
                cancel,
            )? {
                work.charge(1, cancel)?;
                if span[0] > covered || span[0] < coedge.uv_range[0] || span[1] > coedge.uv_range[1]
                {
                    return Ok(false);
                }
                covered = covered.max(span[1]);
                for parameter in span {
                    let uv = coedge.chart_jet([parameter; 2], work, cancel)?.position;
                    if intersects(
                        crate::parametric::surface_piecewise_enclosure(
                            &surface,
                            uv.map(|value| [value.lo, value.hi]),
                            work,
                            cancel,
                        )?
                        .position,
                    ) && (coedge.uv_range.contains(&parameter)
                        || !chart_regular(
                            domain,
                            index,
                            [parameter; 2],
                            functional,
                            height,
                            uncertainty,
                            0,
                            work,
                            cancel,
                        )?)
                    {
                        return Ok(false);
                    }
                }
                if !chart_regular(
                    domain,
                    index,
                    span,
                    functional,
                    height,
                    uncertainty,
                    0,
                    work,
                    cancel,
                )? {
                    return Ok(false);
                }
            }
            if covered != coedge.uv_range[1] {
                return Ok(false);
            }
            let proved = domain.with_native_spatial_curve(
                index,
                remaining,
                work,
                cancel,
                |curve, work, cancel| {
                    if !crate::parametric::curve_parameter_span_continuous(
                        curve,
                        coedge.native_range,
                        work,
                        cancel,
                    )? {
                        return Ok(false);
                    }
                    if curve.native_curve().is_none_or(|native| {
                        matches!(
                            native.kind(),
                            solidcore::curve::curve::CurveKind::CrvOnSurf(_)
                        )
                    }) {
                        return Ok(false);
                    }
                    let mut covered = coedge.native_range[0];
                    for span in crate::parametric::curve_parameter_spans(
                        curve,
                        coedge.native_range,
                        work,
                        cancel,
                    )? {
                        work.charge(1, cancel)?;
                        if span[0] > covered
                            || span[0] < coedge.native_range[0]
                            || span[1] > coedge.native_range[1]
                        {
                            return Ok(false);
                        }
                        covered = covered.max(span[1]);
                        for endpoint in span {
                            if intersects(
                                crate::parametric::curve_enclosure(
                                    curve,
                                    (endpoint, endpoint),
                                    work,
                                    cancel,
                                )?
                                .position,
                            ) && (coedge.native_range.contains(&endpoint)
                                || !curve_regular(
                                    curve,
                                    [endpoint; 2],
                                    functional,
                                    height,
                                    uncertainty,
                                    0,
                                    work,
                                    cancel,
                                )?)
                            {
                                return Ok(false);
                            }
                        }
                        if !curve_regular(
                            curve,
                            span,
                            functional,
                            height,
                            uncertainty,
                            0,
                            work,
                            cancel,
                        )? {
                            return Ok(false);
                        }
                    }
                    Ok(covered == coedge.native_range[1])
                },
            )?;
            if !proved {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
fn face_regular(
    surface: &dyn crate::parametric::SurfaceEvaluator,
    uv: [[f64; 2]; 2],
    functional: HeightFunctional,
    height: Interval,
    uncertainty: Interval,
    depth: u8,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(1, cancel)?;
    let jet = crate::parametric::surface_piecewise_enclosure(surface, uv, work, cancel)?;
    let support = functional.height(jet.position).add(uncertainty);
    if support.hi < height.lo || height.hi < support.lo {
        return Ok(true);
    }
    let normal = crate::parametric::cross(jet.du, jet.dv);
    if normal
        .into_iter()
        .fold(Interval::point(0.0), |sum, value| sum.add(value.square()))
        .lo
        > 0.0
        && (!functional.dot(jet.du).contains_zero() || !functional.dot(jet.dv).contains_zero())
    {
        return Ok(true);
    }
    let axis = usize::from(depth) % 2;
    let middle = uv[axis][0] * 0.5 + uv[axis][1] * 0.5;
    if depth >= MAX_DEPTH || middle <= uv[axis][0] || uv[axis][1] <= middle {
        return Ok(false);
    }
    let mut first = uv;
    first[axis][1] = middle;
    let mut second = uv;
    second[axis][0] = middle;
    Ok(face_regular(
        surface,
        first,
        functional,
        height,
        uncertainty,
        depth + 1,
        work,
        cancel,
    )? && face_regular(
        surface,
        second,
        functional,
        height,
        uncertainty,
        depth + 1,
        work,
        cancel,
    )?)
}

#[allow(clippy::too_many_arguments)]
fn chart_regular(
    domain: &crate::kernel::PreparedFaceTrimDomain<'_>,
    index: usize,
    range: [f64; 2],
    functional: HeightFunctional,
    height: Interval,
    uncertainty: Interval,
    depth: u8,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(1, cancel)?;
    let curve = domain.coedge(index)?.chart_jet(range, work, cancel)?;
    let jet = crate::parametric::surface_piecewise_enclosure(
        &domain.surface_operand(),
        curve.position.map(|value| [value.lo, value.hi]),
        work,
        cancel,
    )?;
    let support = functional.height(jet.position).add(uncertainty);
    let tangent = std::array::from_fn(|axis| {
        jet.du[axis]
            .mul(curve.derivative[0])
            .add(jet.dv[axis].mul(curve.derivative[1]))
    });
    if support.hi < height.lo || height.hi < support.lo || !functional.dot(tangent).contains_zero()
    {
        return Ok(true);
    }
    let middle = range[0] * 0.5 + range[1] * 0.5;
    if depth >= MAX_DEPTH || middle <= range[0] || range[1] <= middle {
        return Ok(false);
    }
    Ok(chart_regular(
        domain,
        index,
        [range[0], middle],
        functional,
        height,
        uncertainty,
        depth + 1,
        work,
        cancel,
    )? && chart_regular(
        domain,
        index,
        [middle, range[1]],
        functional,
        height,
        uncertainty,
        depth + 1,
        work,
        cancel,
    )?)
}

#[allow(clippy::too_many_arguments)]
fn curve_regular(
    curve: &dyn crate::parametric::CurveEvaluator,
    range: [f64; 2],
    functional: HeightFunctional,
    height: Interval,
    uncertainty: Interval,
    depth: u8,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(1, cancel)?;
    let jet = crate::parametric::curve_enclosure(curve, (range[0], range[1]), work, cancel)?;
    let support = functional.height(jet.position).add(uncertainty);
    if support.hi < height.lo
        || height.hi < support.lo
        || !functional.dot(jet.derivative).contains_zero()
    {
        return Ok(true);
    }
    let middle = range[0] * 0.5 + range[1] * 0.5;
    if depth >= MAX_DEPTH || middle <= range[0] || range[1] <= middle {
        return Ok(false);
    }
    Ok(curve_regular(
        curve,
        [range[0], middle],
        functional,
        height,
        uncertainty,
        depth + 1,
        work,
        cancel,
    )? && curve_regular(
        curve,
        [middle, range[1]],
        functional,
        height,
        uncertainty,
        depth + 1,
        work,
        cancel,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use solidcore::nurbs::{b_spline_curve::BSplineCurve, crv_basic, math::CPoint};

    #[test]
    fn closed_knot_height_jets_keep_both_incident_sides_and_uncertainty() {
        let cancel = AtomicBool::new(false);
        let functional = HeightFunctional {
            direction: [0.0, 0.0, 1.0].map(Interval::point),
            offset: Interval::point(0.0),
        };
        for (last_height, regular) in [(3.0, true), (0.0, false)] {
            let raw = crv_basic::Curve::new(
                vec![
                    CPoint::new(0.0, 0.0, 0.0, 1.0),
                    CPoint::new(1.0, 0.0, 1.0, 1.0),
                    CPoint::new(2.0, 0.0, last_height, 1.0),
                ],
                1,
                vec![0.0, 0.0, 0.5, 1.0, 1.0],
            )
            .unwrap();
            let curve: solidcore::curve::curve::Curve =
                BSplineCurve::from_nurb(raw, 3, true).into();
            let mut work = GeometryWorkBudget::new(1_000_000);
            assert!(
                crate::parametric::curve_parameter_span_continuous(
                    &curve,
                    [0.0, 1.0],
                    &mut work,
                    &cancel
                )
                .unwrap()
            );
            assert!(
                !crate::parametric::curve_parameter_span_smooth(
                    &curve,
                    [0.0, 1.0],
                    &mut work,
                    &cancel
                )
                .unwrap()
            );
            let jet =
                crate::parametric::curve_enclosure(&curve, (0.5, 0.5), &mut work, &cancel).unwrap();
            let derivative = functional.dot(jet.derivative);
            assert!(derivative.contains(2.0));
            assert!(derivative.contains(if regular { 4.0 } else { -2.0 }));
            assert_eq!(
                curve_regular(
                    &curve,
                    [0.5; 2],
                    functional,
                    Interval::point(1.0),
                    Interval::point(0.0),
                    0,
                    &mut work,
                    &cancel
                )
                .unwrap(),
                regular
            );
            if !regular {
                let cut = Interval::point(1.0 + 0.25e-6);
                assert!(
                    curve_regular(
                        &curve,
                        [0.5; 2],
                        functional,
                        cut,
                        Interval::point(0.0),
                        0,
                        &mut work,
                        &cancel
                    )
                    .unwrap()
                );
                assert!(
                    !curve_regular(
                        &curve,
                        [0.5; 2],
                        functional,
                        cut,
                        Interval {
                            lo: -1e-6,
                            hi: 1e-6
                        },
                        0,
                        &mut work,
                        &cancel
                    )
                    .unwrap()
                );
            }
        }
    }
}
