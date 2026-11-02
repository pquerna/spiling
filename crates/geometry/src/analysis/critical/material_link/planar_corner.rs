// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::exact_coefficient::ExactCoefficient;
use solidcore::surface::surface::p2::SurfaceKind;

fn exact_planar_trace(
    spatial: &solidcore::curve::curve::Curve,
    chart: &solidcore::curve::curve::Curve,
    origin: [f64; 3],
    axes: [[f64; 3]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let Some(spatial) = crate::parametric::curve_prepared::native_curve_nurbs(spatial)? else {
        return Ok(false);
    };
    let Some(chart) = crate::parametric::curve_prepared::native_curve_nurbs(chart)? else {
        return Ok(false);
    };
    if spatial.degree() != chart.degree()
        || spatial.knots() != chart.knots()
        || spatial.cpts().len() != chart.cpts().len()
    {
        return Ok(false);
    }
    for (position, uv) in spatial.cpts().iter().zip(chart.cpts()) {
        work.charge(1, cancel)?;
        let weight = if chart.is_rational() { uv.w } else { 1.0 };
        let spatial_weight = if spatial.is_rational() {
            position.w
        } else {
            1.0
        };
        if weight != spatial_weight || weight <= 0.0 {
            return Ok(false);
        }
        for (axis, coordinate) in [position.x, position.y, position.z].into_iter().enumerate() {
            let mut residual = ExactCoefficient::point(coordinate);
            for (coefficient, value) in [
                (origin[axis], weight),
                (axes[0][axis], uv.x),
                (axes[1][axis], uv.y),
            ] {
                let Some(term) = ExactCoefficient::point(coefficient).product(
                    &ExactCoefficient::point(value),
                    work,
                    cancel,
                )?
                else {
                    return Ok(false);
                };
                if residual.subtract(&term).is_none() {
                    return Ok(false);
                }
            }
            if residual.sign() != 0 {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

pub(super) fn interior_ray(
    source: MaterialSourceView<'_, '_>,
    use_: CoedgeUse,
    endpoint: usize,
    remaining: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[Interval; 3]>, GeometryError> {
    work.charge(1, cancel)?;
    let domain = source.face_domain(use_.face, remaining, work, cancel)?;
    if !domain.is_native_material() || domain.loops().len() != 1 || domain.coedges().len() != 1 {
        return Ok(None);
    }
    let coedge = &domain.coedges()[0];
    if use_.coedge != 0
        || use_.wire != 0
        || use_.index != 0
        || use_.edgeuse != coedge.edgeuse
        || coedge.vertices[0] != coedge.vertices[1]
    {
        return Ok(None);
    }
    let surface = domain.surface_operand();
    let Some(mut carrier) = surface.native_surface() else {
        return Ok(None);
    };
    let mut plane = None;
    for _ in 0..32 {
        match carrier.kind() {
            SurfaceKind::CurveBounded(bound) => carrier = bound.basis(),
            SurfaceKind::Step(step) => {
                let Some(basis) = step.surface() else {
                    return Ok(None);
                };
                carrier = basis;
            }
            SurfaceKind::Plane(candidate) => {
                plane = Some(candidate);
                break;
            }
            _ => return Ok(None),
        }
    }
    let Some(plane) = plane else {
        return Ok(None);
    };
    let scale = plane.uv_scale();
    if scale.x != 1.0 || scale.y != 1.0 {
        return Ok(None);
    }
    let placement = plane.position();
    let origin = placement.origin();
    let horizontal = placement.x_axis();
    let vertical = placement.y_axis();
    let origin = [origin.x, origin.y, origin.z];
    let axes = [
        [horizontal.x, horizontal.y, horizontal.z],
        [vertical.x, vertical.y, vertical.z],
    ];
    let spatial = source
        .definition()
        .edge_curve(coedge.edge)
        .ok_or_else(resource)?;
    if coedge.native_range != coedge.uv_range
        || !exact_planar_trace(spatial, coedge.curve(), origin, axes, work, cancel)?
    {
        return Ok(None);
    }
    let strict = match PreparedFaceTrimDomain::prepare(
        source.definition(),
        domain.face(),
        remaining,
        work,
        cancel,
    ) {
        Ok(strict) => strict,
        Err(error)
            if matches!(
                error.code,
                GeometryErrorCode::ParameterCorrespondenceUnavailable
                    | GeometryErrorCode::AnalysisIndeterminate
                    | GeometryErrorCode::ConditioningFailure
            ) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    if strict.coedges().len() != 1
        || !std::ptr::eq(strict.coedges()[0].curve(), coedge.curve())
        || strict.coedges()[0].identity() != coedge.identity()
    {
        return Ok(None);
    }
    let Some(positive) = strict.exact_single_loop_orientation(work, cancel)? else {
        return Ok(None);
    };
    let mut tangents = [[Interval::point(0.0); 3]; 2];
    for (end, tangent) in tangents.iter_mut().enumerate() {
        let chart = coedge.chart_jet([coedge.uv_range[end]; 2], work, cancel)?;
        *tangent = std::array::from_fn(|axis| {
            Interval::point(axes[0][axis])
                .mul(chart.derivative[0])
                .add(Interval::point(axes[1][axis]).mul(chart.derivative[1]))
        });
        if end == 1 {
            *tangent = tangent.map(Interval::neg);
        }
    }
    if dot(tangents[0], tangents[1]).hi >= 0.0 {
        return Ok(None);
    }
    let normal = cross(axes[0].map(Interval::point), axes[1].map(Interval::point));
    if length(normal)?.lo <= 0.0 {
        return Ok(None);
    }
    let mut inward = cross(normal, tangents[endpoint]);
    if positive != (endpoint == usize::from(!coedge.forward)) {
        inward = inward.map(Interval::neg);
    }
    Ok(Some(inward))
}

#[cfg(test)]
mod tests {
    use super::*;
    use solidcore::nurbs::{b_spline_curve::BSplineCurve, crv_basic, math::CPoint};

    fn curve(points: Vec<CPoint>, dimension: u64) -> solidcore::curve::curve::Curve {
        let degree = points.len() - 1;
        BSplineCurve::from_nurb(
            crv_basic::Curve::new(
                points,
                degree,
                std::iter::repeat_n(0.0, degree + 1)
                    .chain(std::iter::repeat_n(1.0, degree + 1))
                    .collect(),
            )
            .unwrap(),
            dimension,
            true,
        )
        .into()
    }

    #[test]
    fn whole_planar_coefficients_bind_weights_gauge_and_trace_not_proximity() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let chart_points = vec![
            CPoint::new(2.0, 0.0, 0.0, 2.0),
            CPoint::new(0.0, 1.0, 0.0, 1.0),
        ];
        let spatial_points = vec![
            CPoint::new(6.0, 4.0, 16.0, 2.0),
            CPoint::new(1.0, 5.0, 8.0, 1.0),
        ];
        let origin = [1.0, 2.0, 8.0];
        let axes = [[2.0, 0.0, 0.0], [0.0, 3.0, 0.0]];
        for reverse in [false, true] {
            let mut uv = chart_points.clone();
            let mut xyz = spatial_points.clone();
            if reverse {
                uv.reverse();
                xyz.reverse();
            }
            assert!(
                exact_planar_trace(
                    &curve(xyz, 3),
                    &curve(uv, 2),
                    origin,
                    axes,
                    &mut work,
                    &cancel
                )
                .unwrap()
            );
        }
        let chart = curve(chart_points, 2);
        let mut perturbed = spatial_points.clone();
        perturbed[1].x = f64::from_bits(1.0_f64.to_bits() + 1);
        assert!(
            !exact_planar_trace(
                &curve(perturbed, 3),
                &chart,
                origin,
                axes,
                &mut work,
                &cancel
            )
            .unwrap()
        );
        let mut reversed = spatial_points;
        reversed.reverse();
        assert!(
            !exact_planar_trace(
                &curve(reversed, 3),
                &chart,
                origin,
                axes,
                &mut work,
                &cancel
            )
            .unwrap()
        );
        let same_endpoints_chart = curve(
            vec![
                CPoint::new(0.0, 0.0, 0.0, 1.0),
                CPoint::new(1.0, 0.0, 0.0, 1.0),
                CPoint::new(1.0, 1.0, 0.0, 1.0),
            ],
            2,
        );
        let changed_tangent = curve(
            vec![
                CPoint::new(1.0, 2.0, 8.0, 1.0),
                CPoint::new(3.0, f64::from_bits(2.0_f64.to_bits() + 1), 8.0, 1.0),
                CPoint::new(3.0, 5.0, 8.0, 1.0),
            ],
            3,
        );
        assert!(
            !exact_planar_trace(
                &changed_tangent,
                &same_endpoints_chart,
                origin,
                axes,
                &mut work,
                &cancel
            )
            .unwrap()
        );
    }

    #[test]
    fn native_disk_corners_bind_both_germs_and_refuse_foreign_slots_or_limits() {
        let cancel = AtomicBool::new(false);
        let definition = crate::from_step_bytes_with_cancel(
            include_bytes!(
                "../../../../../../fixtures/geometry/independent-exporters/occt-cylinder-mm.step"
            ),
            &cancel,
        )
        .unwrap();
        let mut work = GeometryWorkBudget::new(100_000_000);
        let prepared = crate::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let source = prepared.source_view().unwrap();
        let mut incidence = SourceEdgeIncidence::new(
            &definition,
            crate::MAX_PREPARED_SECTION_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        incidence
            .prepare_vertices(crate::MAX_PREPARED_SECTION_BYTES, &mut work, &cancel)
            .unwrap();
        let mut caps = std::collections::BTreeSet::new();
        for uses in incidence.values() {
            for use_ in [Some(uses.first), uses.second].into_iter().flatten() {
                let rays = [0, 1].map(|endpoint| {
                    interior_ray(
                        source,
                        use_,
                        endpoint,
                        crate::MAX_PREPARED_SECTION_BYTES,
                        &mut work,
                        &cancel,
                    )
                    .unwrap()
                });
                if let [Some(first), Some(second)] = rays {
                    assert!(first[0].hi < 0.0 && second[0].hi < 0.0);
                    assert!(dot(first, second).lo > 0.0);
                    caps.insert(use_.face);
                    let mut wrong = use_;
                    wrong.coedge = 1;
                    assert!(
                        interior_ray(
                            source,
                            wrong,
                            0,
                            crate::MAX_PREPARED_SECTION_BYTES,
                            &mut work,
                            &cancel
                        )
                        .unwrap()
                        .is_none()
                    );
                    assert_eq!(
                        interior_ray(source, use_, 0, 0, &mut work, &cancel)
                            .unwrap_err()
                            .code,
                        GeometryErrorCode::ResourceLimit
                    );
                    assert_eq!(
                        interior_ray(
                            source,
                            use_,
                            0,
                            crate::MAX_PREPARED_SECTION_BYTES,
                            &mut work,
                            &AtomicBool::new(true)
                        )
                        .unwrap_err()
                        .code,
                        GeometryErrorCode::Cancelled
                    );
                } else {
                    assert!(rays.into_iter().all(|ray| ray.is_none()));
                }
            }
        }
        assert_eq!(caps.len(), 2);
        let vertex = incidence.vertices().unwrap().next().unwrap();
        let count: usize = incidence
            .vertex_edges(vertex)
            .map(|edge| {
                incidence
                    .get(edge)
                    .unwrap()
                    .vertices
                    .iter()
                    .filter(|&&endpoint| endpoint == vertex)
                    .count()
            })
            .sum();
        let old_capacity_only =
            UV_EMBEDDING_STACK_BYTES + count * std::mem::size_of::<[Interval; 2]>();
        assert_eq!(
            vertex_fan::qualify_native_vertex(
                source,
                &incidence,
                vertex,
                [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]].map(|row| row.map(Interval::point)),
                old_capacity_only,
                &mut work,
                &cancel,
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::ResourceLimit
        );
    }
}
