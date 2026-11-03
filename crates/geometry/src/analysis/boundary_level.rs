// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{
    collar,
    material_source::MaterialSourceView,
    slab,
    source_incidence::{CoedgeUse, SourceEdgeIncidence},
};
use crate::kernel::{PreparedFaceTrimDomain, trim::NativeTrimCoedge};
use crate::parametric::{self as p, CurveEvaluator, Interval};
use crate::section::{Admission, Arc, SectionQuery};
use crate::{
    Definition, GeometryError, GeometryWorkBudget, NativeSection, PlaneFrameMm, PlaneMm,
    SectionCurve, SectionEndpoint, SectionSpan, SectionSpanAttachments,
};
use solidcore::{EdgeId, VertexId};
use spiling_contracts::geometry::{GeometryErrorCode, RigidPoseMm};
use std::sync::atomic::AtomicBool;

fn unresolved(message: &str) -> GeometryError {
    GeometryError::new(
        GeometryErrorCode::AnalysisIndeterminate,
        format!("native boundary-level section: {message}"),
    )
}
fn require(condition: bool, message: &str) -> Result<(), GeometryError> {
    if condition {
        Ok(())
    } else {
        Err(unresolved(message))
    }
}
fn norm(vector: [Interval; 3]) -> Result<f64, GeometryError> {
    Ok(vector
        .into_iter()
        .fold(Interval::point(0.0), |sum, value| sum.add(value.square()))
        .sqrt()?
        .hi)
}
fn dot(first: [Interval; 3], second: [Interval; 3]) -> Interval {
    (0..3).fold(Interval::point(0.0), |sum, axis| {
        sum.add(first[axis].mul(second[axis]))
    })
}
fn height(position: [Interval; 3], plane: PlaneMm) -> Interval {
    dot(
        std::array::from_fn(|axis| position[axis].sub(Interval::point(plane.origin_mm[axis]))),
        plane.normal.map(Interval::point),
    )
}

pub(crate) fn has_candidate(
    definition: &Definition,
    plane: PlaneMm,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let functional = slab::HeightFunctional::from_frame(PlaneFrameMm::from_plane(plane)?);
    for &edge in definition.brep().edge_list() {
        work.charge(1, cancel)?;
        if let Some(curve) = definition.edge_curve(edge)
            && slab::constant_native_curve_height(curve, functional, work, cancel)?
                == Some(Interval::point(0.0))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[derive(Clone, Copy)]
struct Chart {
    ends: [[f64; 2]; 2],
    increasing: [bool; 2],
}

fn chart(
    coedge: &NativeTrimCoedge<'_>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Chart, GeometryError> {
    let curve = coedge.curve();
    let native = p::curve_prepared::native_curve_nurbs(curve)?
        .ok_or_else(|| unresolved("boundary chart needs explicit native coefficients"))?;
    require(
        !native.is_rational(),
        "rational boundary chart is outside this supporting-coordinate cut",
    )?;
    let points = native.cpts();
    let knots = native.knots();
    let degree = native.degree();
    require(
        degree >= 1
            && points.len() > degree
            && coedge.uv_range == [knots[degree], knots[points.len()]],
        "boundary chart leaves its complete coefficient restriction",
    )?;
    require(
        knots[..=degree]
            .iter()
            .all(|value| *value == coedge.uv_range[0])
            && knots[points.len()..]
                .iter()
                .all(|value| *value == coedge.uv_range[1]),
        "boundary chart endpoints lack clamped expression identity",
    )?;
    require(
        p::curve_parameter_span_continuous(curve, coedge.uv_range, work, cancel)?,
        "boundary chart is not C0",
    )?;
    work.charge(points.len() as u64, cancel)?;
    for point in points {
        Interval::new(point.x, point.x)?;
        Interval::new(point.y, point.y)?;
    }
    let coordinate = |point: &solidcore::nurbs::math::CPoint| [point.x, point.y];
    let ends = [
        coordinate(&points[0]),
        coordinate(&points[points.len() - 1]),
    ];
    let increasing = std::array::from_fn(|axis| {
        points
            .windows(2)
            .all(|pair| coordinate(&pair[0])[axis] < coordinate(&pair[1])[axis])
    });
    Ok(Chart { ends, increasing })
}

fn supporting_sign(
    domain: &PreparedFaceTrimDomain<'_>,
    axis: usize,
    level: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<i8, GeometryError> {
    let mut sign = 0;
    for coedge in domain.coedges() {
        let native = p::curve_prepared::native_curve_nurbs(coedge.curve())?
            .ok_or_else(|| unresolved("missing supporting-coordinate coefficients"))?;
        require(
            !native.is_rational(),
            "rational supporting-coordinate law is unqualified",
        )?;
        for point in native.cpts() {
            work.charge(1, cancel)?;
            let value = [point.x, point.y][axis];
            let next = if value > level {
                1
            } else if value < level {
                -1
            } else {
                0
            };
            require(
                next == 0 || sign == 0 || next == sign,
                "face boundary crosses its proposed supporting coordinate",
            )?;
            if next != 0 {
                sign = next;
            }
        }
    }
    require(
        sign != 0,
        "whole face is coincident rather than a regular partition",
    )?;
    Ok(sign)
}

fn zero_cover(
    domain: &PreparedFaceTrimDomain<'_>,
    axis: usize,
    level: f64,
    plane: PlaneMm,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    let mut line = domain.uv_domain();
    line[axis] = [level; 2];
    let position = domain.surface_evaluator().tensor(line, 0, work, cancel)?[0][0];
    require(
        height(position, plane) == Interval::point(0.0),
        "retained supporting coordinate is not an exact carrier zero",
    )?;
    let mut pending = [([0.0; 2], 0u8); 33];
    pending[0] = (domain.uv_domain()[axis], 0);
    let mut count = 1;
    while count > 0 {
        count -= 1;
        let (range, depth) = pending[count];
        work.charge(1, cancel)?;
        let mut cell = domain.uv_domain();
        cell[axis] = range;
        let tensor = domain.surface_evaluator().tensor(cell, 1, work, cancel)?;
        if !height(tensor[0][0], plane).contains_zero() {
            continue;
        }
        let derivative = if axis == 0 {
            tensor[1][0]
        } else {
            tensor[0][1]
        };
        if range[0] <= level
            && level <= range[1]
            && !dot(derivative, plane.normal.map(Interval::point)).contains_zero()
        {
            continue;
        }
        let middle = range[0] * 0.5 + range[1] * 0.5;
        require(
            depth < 32 && range[0] < middle && middle < range[1],
            "carrier zero complement remains unresolved",
        )?;
        pending[count] = ([middle, range[1]], depth + 1);
        pending[count + 1] = ([range[0], middle], depth + 1);
        count += 2;
    }
    Ok(())
}

struct Plan {
    edge: EdgeId,
    uses: [CoedgeUse; 2],
    witness: usize,
    axis: usize,
    level: f64,
    vertices: [VertexId; 2],
    representatives: [[f64; 3]; 2],
    zone_errors: [f64; 2],
}

fn cover_face(
    source: MaterialSourceView<'_, '_>,
    domain: &PreparedFaceTrimDomain<'_>,
    selected: usize,
    plan: &mut Plan,
    plane: PlaneMm,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<i8, GeometryError> {
    require(
        domain.loops().len() == 1,
        "multiple chart circuits need a separate level census",
    )?;
    require(
        domain.uv_domain()[plan.axis][0] <= plan.level
            && plan.level <= domain.uv_domain()[plan.axis][1],
        "boundary level leaves the original carrier restriction",
    )?;
    let selected_coedge = &domain.coedges()[selected];
    let edge = source
        .definition()
        .brep()
        .edge(plan.edge)
        .map_err(|_| unresolved("selected native edge is unavailable"))?;
    require(
        selected_coedge.edge == plan.edge
            && selected_coedge.vertices == plan.vertices
            && selected_coedge.native_range == [edge.interval.min(), edge.interval.max()],
        "selected boundary changed its native range or endpoint owners",
    )?;
    let support = supporting_sign(domain, plan.axis, plan.level, work, cancel)?;
    zero_cover(domain, plan.axis, plan.level, plane, work, cancel)?;
    for (index, coedge) in domain.coedges().iter().enumerate() {
        chart(coedge, work, cancel)?;
        if index != selected {
            let native = p::curve_prepared::native_curve_nurbs(coedge.curve())?.unwrap();
            let mut zeros = 0;
            for (control, point) in native.cpts().iter().enumerate() {
                work.charge(1, cancel)?;
                if [point.x, point.y][plan.axis] == plan.level {
                    zeros += 1;
                    require(
                        control == 0 || control + 1 == native.cpts().len(),
                        "unaccounted interior trim contact at the level",
                    )?;
                    let endpoint = usize::from(control != 0);
                    require(
                        plan.vertices.contains(&coedge.vertices[endpoint]),
                        "level contact belongs to a foreign native vertex",
                    )?;
                }
            }
            require(
                zeros <= 1,
                "another boundary interval needs its own retained-level census",
            )?;
        }
    }
    for index in 0..domain.coedges().len() {
        work.charge(1, cancel)?;
        let next = (index + 1) % domain.coedges().len();
        let (vertex, cover) = domain.native_joint_cover(index, next, work, cancel)?;
        if !cover[plan.axis].contains(plan.level) {
            continue;
        }
        let slot = plan
            .vertices
            .iter()
            .position(|value| *value == vertex)
            .ok_or_else(|| unresolved("unaccounted local zero component at a foreign joint"))?;
        let mut cell = cover.map(|value| [value.lo, value.hi]);
        cell[plan.axis] = [plan.level; 2];
        let image = domain.surface_evaluator().tensor(cell, 0, work, cancel)?[0][0];
        let error = norm(std::array::from_fn(|axis| {
            image[axis].sub(Interval::point(plan.representatives[slot][axis]))
        }))?;
        require(
            error <= source.effective_tolerance_mm(),
            "owned zero-joint image exceeds native material policy",
        )?;
        plan.zone_errors[slot] = plan.zone_errors[slot].max(error);
    }
    Ok(support)
}

fn sample_arc(
    source: MaterialSourceView<'_, '_>,
    plan: &Plan,
    query: &SectionQuery,
    admission: &mut Admission,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Arc, GeometryError> {
    let definition = source.definition();
    let use_ = plan.uses[plan.witness];
    let domain = source.face_domain(use_.face, 0, work, cancel)?;
    let coedge = &domain.coedges()[use_.coedge];
    let trace = p::curve_prepared::NativeChartCurve {
        surface: definition
            .face_surface(definition.face_ids()[use_.face])
            .unwrap(),
        chart: coedge.curve(),
    };
    require(
        trace.continuous(coedge.uv_range, work, cancel)?,
        "retained carrier trace lacks complete C0 coverage",
    )?;
    let prepared = p::PreparedValidatedCurve::new(
        &trace,
        coedge.uv_range,
        1.0,
        admission.limit - admission.used,
        work,
        cancel,
    )?;
    admission.used = admission
        .used
        .checked_add(prepared.retained_bytes())
        .filter(|bytes| *bytes <= admission.limit)
        .ok_or_else(|| {
            crate::section::resource_error("native boundary curve exceeds query allowance")
        })?;
    let mut points = Vec::new();
    let mut tangents = Vec::new();
    let mut errors = Vec::new();
    let mut maximum = 0.0_f64;
    let mut plane_error = 0.0_f64;
    admission.push(&mut points, plan.representatives[0])?;
    for original in trace.parameter_spans(coedge.uv_range, work, cancel)? {
        let mut pending = [([0.0; 2], 0u8); 33];
        pending[0] = (original, 0);
        let mut count = 1;
        while count > 0 {
            count -= 1;
            let (range, depth) = pending[count];
            work.charge(1, cancel)?;
            let jet = prepared.jet(range, work, cancel)?;
            let start = prepared.evaluate(range[0], work, cancel)?;
            let end = prepared.evaluate(range[1], work, cancel)?;
            let first = if range[0] == coedge.uv_range[0] {
                plan.representatives[0]
            } else {
                start.position
            };
            let last = if range[1] == coedge.uv_range[1] {
                plan.representatives[1]
            } else {
                end.position
            };
            let mut endpoint_error = 0.0_f64;
            for (actual, representative) in [(start, first), (end, last)] {
                endpoint_error = endpoint_error.max(norm(std::array::from_fn(|axis| {
                    Interval::point(actual.position[axis])
                        .add(actual.error[axis])
                        .sub(Interval::point(representative[axis]))
                }))?);
            }
            let width = Interval::point(range[1]).sub(Interval::point(range[0]));
            let smooth = prepared.is_smooth_on(range, work, cancel)?
                || trace.single_open_span(range, work, cancel)?;
            let interpolation = if smooth {
                Interval::point(norm(jet.second_derivative)?)
                    .mul(width.square())
                    .mul(Interval::point(0.125))
                    .hi
            } else {
                Interval::point(norm(jet.derivative)?)
                    .mul(width)
                    .mul(Interval::point(0.5))
                    .hi
            };
            let zone = if range[0] == coedge.uv_range[0] {
                plan.zone_errors[0]
            } else {
                0.0_f64
            }
            .max(if range[1] == coedge.uv_range[1] {
                plan.zone_errors[1]
            } else {
                0.0
            });
            let error = Interval::point(interpolation)
                .add(Interval::point(endpoint_error.max(zone)))
                .hi;
            let residual = [first, last]
                .into_iter()
                .map(|point| height(point.map(Interval::point), query.plane).abs_upper())
                .fold(0.0_f64, f64::max);
            if error <= query.maximum_chord_error_mm && residual <= query.maximum_plane_error_mm {
                require(
                    points.last() == Some(&first),
                    "sampling lost its exact native parameter chain",
                )?;
                admission.push(&mut points, last)?;
                admission.push(&mut tangents, jet.derivative)?;
                admission.push(&mut errors, error)?;
                maximum = maximum.max(error);
                plane_error = plane_error.max(residual);
                continue;
            }
            let middle = range[0] * 0.5 + range[1] * 0.5;
            require(
                depth < 32 && range[0] < middle && middle < range[1],
                "whole native boundary chord cannot meet caller allowances",
            )?;
            pending[count] = ([middle, range[1]], depth + 1);
            pending[count + 1] = ([range[0], middle], depth + 1);
            count += 2;
        }
    }
    let middle = coedge.uv_range[0] * 0.5 + coedge.uv_range[1] * 0.5;
    let uv = coedge
        .chart_jet([middle; 2], work, cancel)?
        .position
        .map(|value| [value.lo, value.hi]);
    let jet = p::surface_piecewise_enclosure(trace.surface, uv, work, cancel)?;
    let normal = p::cross(jet.du, jet.dv).map(|value| {
        if domain.orientation() {
            value
        } else {
            value.neg()
        }
    });
    let direction = dot(
        prepared.jet([middle; 2], work, cancel)?.derivative,
        p::cross(query.plane.normal.map(Interval::point), normal),
    );
    require(
        !direction.contains_zero(),
        "boundary trace material-left orientation is unresolved",
    )?;
    let reverse = direction.hi < 0.0;
    let mut endpoints = plan.vertices.map(SectionEndpoint::Vertex);
    if reverse {
        points.reverse();
        errors.reverse();
        tangents.reverse();
        for tangent in &mut tangents {
            *tangent = tangent.map(Interval::neg);
        }
        endpoints.swap(0, 1);
    }
    admission.used -= prepared.retained_bytes();
    Ok(Arc {
        span: Some(SectionSpan {
            face: definition.face_ids()[use_.face],
            source_face_ordinal: use_.face,
            definition_plane: query.plane,
            section_pose: RigidPoseMm::IDENTITY,
            curve: SectionCurve::NativeBoundaryLevel {
                edge: plan.edge,
                edgeuses: plan.uses.map(|owner| owner.edgeuse),
                source_faces: plan.uses.map(|owner| owner.face),
                witness: coedge.edgeuse,
                parameter: coedge.uv_range,
                endpoint_zone_error_mm: plan.zone_errors,
            },
            endpoints,
            attachments: SectionSpanAttachments::NativeBoundary {
                edge: plan.edge,
                vertices: plan.vertices,
            },
            reversed: reverse,
            maximum_chord_error_mm: maximum,
            maximum_plane_error_mm: plane_error,
        }),
        points,
        tangents,
        chord_errors: errors,
    })
}

pub(crate) fn intersect(
    source: MaterialSourceView<'_, '_>,
    query: &SectionQuery,
    byte_limit: usize,
    packed_limit: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<NativeSection>, GeometryError> {
    match attempt(source, query, byte_limit, packed_limit, work, cancel) {
        Ok(section) => Ok(Some(section)),
        Err(error)
            if matches!(
                error.code,
                GeometryErrorCode::AnalysisIndeterminate
                    | GeometryErrorCode::ConditioningFailure
                    | GeometryErrorCode::ParameterCorrespondenceUnavailable
                    | GeometryErrorCode::UnsupportedOperation
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn attempt(
    source: MaterialSourceView<'_, '_>,
    query: &SectionQuery,
    byte_limit: usize,
    packed_limit: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<NativeSection, GeometryError> {
    let definition = source.definition();
    let plane = query.plane;
    let functional = slab::HeightFunctional::from_frame(PlaneFrameMm::from_plane(plane)?);
    let mut admission = Admission::new(byte_limit.min(crate::MAX_PREPARED_SECTION_BYTES))?;
    admission.used = admission
        .used
        .checked_add(definition.id().allocated_bytes())
        .filter(|bytes| *bytes <= admission.limit)
        .ok_or_else(|| {
            crate::section::resource_error("native boundary identity exceeds query allowance")
        })?;
    let available = admission.limit - admission.used;
    let mut incidence = SourceEdgeIncidence::new(definition, available, work, cancel)?;
    incidence.prepare_vertices(available, work, cancel)?;
    admission.used += incidence.owned_bytes();
    let mut plans = Vec::new();
    let mut vertices = Vec::new();
    for &edge in definition.brep().edge_list() {
        work.charge(1, cancel)?;
        let Some(curve) = definition.edge_curve(edge) else {
            continue;
        };
        if slab::constant_native_curve_height(curve, functional, work, cancel)?
            != Some(Interval::point(0.0))
        {
            continue;
        }
        let pair = incidence
            .get(edge)
            .ok_or_else(|| unresolved("coincident edge has no complete owner"))?;
        let second = pair
            .second
            .ok_or_else(|| unresolved("coincident edge is not paired"))?;
        let uses = [pair.first, second];
        let domains = [
            source.face_domain(uses[0].face, 0, work, cancel)?,
            source.face_domain(uses[1].face, 0, work, cancel)?,
        ];
        require(
            uses[0].face != uses[1].face
                && uses[0].orientation != uses[1].orientation
                && domains[0].orientation() == domains[1].orientation(),
            "paired boundary orientation is inconsistent",
        )?;
        require(
            p::same_surface_coefficients(
                &domains[0].surface_operand(),
                &domains[1].surface_operand(),
                work,
                cancel,
            )?,
            "boundary partition does not have one native carrier",
        )?;
        let charts = [
            chart(&domains[0].coedges()[uses[0].coedge], work, cancel)?,
            chart(&domains[1].coedges()[uses[1].coedge], work, cancel)?,
        ];
        let mut witness = None;
        for slot in 0..2 {
            for axis in 0..2 {
                let coedge = &domains[slot].coedges()[uses[slot].coedge];
                let native = p::curve_prepared::native_curve_nurbs(coedge.curve())?.unwrap();
                work.charge(native.cpts().len() as u64, cancel)?;
                let level = charts[slot].ends[0][axis];
                if !charts[slot].increasing[1 - axis]
                    || !charts[1 - slot].increasing[1 - axis]
                    || !native
                        .cpts()
                        .iter()
                        .all(|point| [point.x, point.y][axis] == level)
                {
                    continue;
                }
                if charts[slot].ends[0][1 - axis] > charts[1 - slot].ends[0][1 - axis]
                    || charts[slot].ends[1][1 - axis] < charts[1 - slot].ends[1][1 - axis]
                {
                    continue;
                }
                let mut uv = domains[slot].uv_domain();
                uv[axis] = [level; 2];
                if height(
                    domains[slot]
                        .surface_evaluator()
                        .tensor(uv, 0, work, cancel)?[0][0],
                    plane,
                ) == Interval::point(0.0)
                {
                    witness = Some((slot, axis, level));
                    break;
                }
            }
            if witness.is_some() {
                break;
            }
        }
        let (witness, axis, level) = witness.ok_or_else(|| {
            unresolved("paired boundary has no exhaustive exact retained level witness")
        })?;
        let representative = |vertex| {
            definition
                .brep()
                .vertex(vertex)
                .map(|vertex| [vertex.point.x, vertex.point.y, vertex.point.z])
                .map_err(|_| unresolved("native vertex is unavailable"))
        };
        let representatives = [
            representative(pair.vertices[0])?,
            representative(pair.vertices[1])?,
        ];
        for point in representatives {
            require(
                height(point.map(Interval::point), plane) == Interval::point(0.0),
                "native vertex representative is not in the requested plane",
            )?;
        }
        let mut plan = Plan {
            edge,
            uses,
            witness,
            axis,
            level,
            vertices: pair.vertices,
            representatives,
            zone_errors: [0.0; 2],
        };
        let first = cover_face(
            source,
            domains[0],
            uses[0].coedge,
            &mut plan,
            plane,
            work,
            cancel,
        )?;
        let second = cover_face(
            source,
            domains[1],
            uses[1].coedge,
            &mut plan,
            plane,
            work,
            cancel,
        )?;
        require(
            first == -second,
            "paired retained faces do not cross the level in opposite directions",
        )?;
        for vertex in pair.vertices {
            work.charge(vertices.len() as u64, cancel)?;
            if vertices.iter().any(|(owned, _)| *owned == vertex) {
                continue;
            }
            require(
                collar::regular_native_vertex(
                    source,
                    &incidence,
                    vertex,
                    functional,
                    admission.limit - admission.used,
                    work,
                    cancel,
                )?,
                "complete native endpoint star is not height regular",
            )?;
            admission.push(
                &mut vertices,
                (vertex, collar::NativeCollarVertexProof::RegularHeight),
            )?;
        }
        admission.push(&mut plans, plan)?;
    }
    require(
        !plans.is_empty(),
        "no supporting-coordinate boundary strata",
    )?;
    for face in 0..definition.face_ids().len() {
        work.charge(plans.len() as u64, cancel)?;
        let owners = plans
            .iter()
            .filter(|plan| plan.uses.iter().any(|owner| owner.face == face))
            .count();
        require(owners <= 1, "a face requires more than one level interval")?;
        if owners == 0 {
            let domain = source.face_domain(face, 0, work, cancel)?;
            require(
                !height(
                    domain
                        .surface_evaluator()
                        .tensor(domain.uv_domain(), 0, work, cancel)?[0][0],
                    plane,
                )
                .contains_zero(),
                "an unaccounted face may contribute a section",
            )?;
        }
    }
    for &(vertex, _) in &vertices {
        work.charge(plans.len() as u64, cancel)?;
        require(
            plans
                .iter()
                .filter(|plan| plan.vertices.contains(&vertex))
                .count()
                == 2,
            "finite native star does not have exactly two external section ports",
        )?;
    }
    for plan in &plans {
        let edge = definition
            .brep()
            .edge(plan.edge)
            .map_err(|_| unresolved("coincident edge disappeared"))?;
        require(
            collar::regular_common_carrier_coedge(
                collar::RegularNativeCoedgeQuery {
                    source,
                    edges: &incidence,
                    qualified_vertices: &vertices,
                    face: plan.uses[0].face,
                    coedge: plan.uses[0].coedge,
                    edge: plan.edge,
                    range: [edge.interval.min(), edge.interval.max()],
                    curve: definition.edge_curve(plan.edge).unwrap(),
                    functional,
                    remaining_bytes: admission.limit - admission.used,
                },
                work,
                cancel,
            )?
            .is_ok(),
            "whole paired coedge continuation is unqualified",
        )?;
    }
    let mut arcs = Vec::new();
    for plan in &plans {
        let arc = sample_arc(source, plan, query, &mut admission, work, cancel)?;
        admission.push(&mut arcs, arc)?;
    }
    crate::section::finish_arcs(
        definition,
        query,
        arcs,
        admission,
        packed_limit,
        work,
        cancel,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unchanged_torus_zero_level_uses_owned_boundary_strata() {
        let definition = crate::from_step_bytes(include_bytes!(
            "../../../../fixtures/manufacturing/native/source-ring-torus.step"
        ))
        .unwrap();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::query();
        let prepared = super::super::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let source = prepared.source_view().unwrap();
        let query = SectionQuery {
            plane: PlaneMm {
                origin_mm: [0.0; 3],
                normal: [0.0, 0.0, 1.0],
            },
            maximum_chord_error_mm: 0.001,
            maximum_plane_error_mm: 1e-7,
        };
        let section = attempt(
            source,
            &query,
            crate::MAX_PREPARED_SECTION_BYTES - prepared.retained_bytes(),
            spiling_contracts::geometry::MAX_SECTION_BYTES as usize,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert_eq!(section.loops().len(), 2);
        assert_eq!(
            section
                .loops()
                .iter()
                .filter(|boundary| boundary.is_hole)
                .count(),
            1
        );
        assert_eq!(
            (0..2)
                .map(|index| section.spans(index).unwrap().len())
                .sum::<usize>(),
            8
        );
        let error = section.achieved_chord_error_mm();
        let mut area = 0.0;
        let mut edges = std::collections::BTreeSet::new();
        for (index, boundary) in section.loops().iter().enumerate() {
            let radius = if boundary.is_hole { 3.0 } else { 7.0 };
            for point in &boundary.points_mm {
                assert!((point[0].hypot(point[1]) - radius).abs() <= error);
                assert_eq!(point[2], 0.0);
            }
            let signed = boundary
                .points_mm
                .windows(2)
                .map(|pair| pair[0][0] * pair[1][1] - pair[0][1] * pair[1][0])
                .sum::<f64>()
                * 0.5;
            assert_eq!(signed < 0.0, boundary.is_hole);
            area += signed;
            let spans = section.spans(index).unwrap();
            for (slot, span) in spans.iter().enumerate() {
                let SectionCurve::NativeBoundaryLevel {
                    edge,
                    source_faces,
                    edgeuses,
                    witness,
                    ..
                } = &span.curve
                else {
                    panic!("zero level must retain its boundary ownership");
                };
                assert!(edges.insert(*edge));
                assert!(edgeuses.contains(witness));
                assert_ne!(source_faces[0], source_faces[1]);
                let genealogy = section.loop_source_face_ordinals(index).unwrap();
                assert!(source_faces.iter().all(|face| genealogy.contains(face)));
                assert!(
                    span.endpoints
                        .iter()
                        .all(|endpoint| matches!(endpoint, SectionEndpoint::Vertex(_)))
                );
                assert_eq!(
                    span.endpoints[1],
                    spans[(slot + 1) % spans.len()].endpoints[0]
                );
            }
        }
        assert!(
            (area - 40.0 * std::f64::consts::PI).abs()
                <= 20.0 * std::f64::consts::PI * error + 2.0 * std::f64::consts::PI * error.powi(2)
        );
        let slab_query = SectionQuery {
            maximum_chord_error_mm: spiling_contracts::geometry::SECTION_SAMPLING_TOLERANCE_MM
                * 0.5,
            ..query
        };
        attempt(
            source,
            &slab_query,
            crate::MAX_PREPARED_SECTION_BYTES - prepared.retained_bytes(),
            spiling_contracts::geometry::MAX_SECTION_BYTES as usize,
            &mut work,
            &cancel,
        )
        .unwrap();

        let mut shifted = query;
        shifted.plane.origin_mm[2] = 1e-12;
        assert!(!has_candidate(&definition, shifted.plane, &mut work, &cancel).unwrap());
        assert!(
            intersect(
                source,
                &shifted,
                crate::MAX_PREPARED_SECTION_BYTES - prepared.retained_bytes(),
                spiling_contracts::geometry::MAX_SECTION_BYTES as usize,
                &mut work,
                &cancel
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn boundary_level_requires_exact_witness_support_and_original_endpoint_owners() {
        let definition = crate::from_step_bytes(include_bytes!(
            "../../../../fixtures/manufacturing/native/source-ring-torus.step"
        ))
        .unwrap();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::query();
        let prepared = super::super::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let source = prepared.source_view().unwrap();
        let incidence = SourceEdgeIncidence::new(
            &definition,
            crate::MAX_PREPARED_SECTION_BYTES - prepared.retained_bytes(),
            &mut work,
            &cancel,
        )
        .unwrap();
        let pair = incidence
            .values()
            .find(|pair| {
                pair.second.is_some_and(|second| {
                    [pair.first.face, second.face].contains(&0)
                        && [pair.first.face, second.face].contains(&3)
                })
            })
            .unwrap();
        let uses = [pair.first, pair.second.unwrap()];
        let domain = source.face_domain(3, 0, &mut work, &cancel).unwrap();
        let witness = uses.iter().position(|owner| owner.face == 3).unwrap();
        let mut plan = Plan {
            edge: domain.coedges()[uses[witness].coedge].edge,
            uses,
            witness,
            axis: 1,
            level: 360.0,
            vertices: pair.vertices,
            representatives: pair.vertices.map(|vertex| {
                let point = definition.brep().vertex(vertex).unwrap().point;
                [point.x, point.y, point.z]
            }),
            zone_errors: [0.0; 2],
        };
        let plane = PlaneMm {
            origin_mm: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
        };
        assert_eq!(
            cover_face(
                source,
                domain,
                uses[witness].coedge,
                &mut plan,
                plane,
                &mut work,
                &cancel
            )
            .unwrap(),
            1
        );
        let mut non_singleton_cover = false;
        for incoming in 0..domain.coedges().len() {
            let outgoing = (incoming + 1) % domain.coedges().len();
            let (vertex, cover) = domain
                .native_joint_cover(incoming, outgoing, &mut work, &cancel)
                .unwrap();
            if !cover[1].contains(plan.level) {
                continue;
            }
            non_singleton_cover |= cover.iter().any(|coordinate| !coordinate.is_point());
            let slot = plan
                .vertices
                .iter()
                .position(|owner| *owner == vertex)
                .unwrap();
            let mut cell = cover.map(|coordinate| [coordinate.lo, coordinate.hi]);
            cell[1] = [plan.level; 2];
            let image = domain
                .surface_evaluator()
                .tensor(cell, 0, &mut work, &cancel)
                .unwrap()[0][0];
            let required = norm(std::array::from_fn(|axis| {
                image[axis].sub(Interval::point(plan.representatives[slot][axis]))
            }))
            .unwrap();
            assert!(required <= plan.zone_errors[slot]);
        }
        assert!(non_singleton_cover);
        assert!(domain.native_joint_cover(0, 2, &mut work, &cancel).is_err());
        plan.vertices.swap(0, 1);
        assert!(
            cover_face(
                source,
                domain,
                uses[witness].coedge,
                &mut plan,
                plane,
                &mut work,
                &cancel
            )
            .is_err()
        );
        let displaced = PlaneMm {
            origin_mm: [0.0, 0.0, 1e-12],
            ..plane
        };
        assert!(zero_cover(domain, 1, 360.0, displaced, &mut work, &cancel).is_err());
        let opposite = source.face_domain(0, 0, &mut work, &cancel).unwrap();
        assert!(supporting_sign(opposite, 1, 315.0, &mut work, &cancel).is_err());
    }

    #[test]
    fn boundary_level_public_queries_share_ownership_and_terminal_limits() {
        let definition = crate::from_step_bytes(include_bytes!(
            "../../../../fixtures/manufacturing/native/source-ring-torus.step"
        ))
        .unwrap();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::query();
        let prepared = super::super::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let plane = PlaneMm {
            origin_mm: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
        };
        let query = SectionQuery {
            plane,
            maximum_chord_error_mm: spiling_contracts::geometry::SECTION_SAMPLING_TOLERANCE_MM,
            maximum_plane_error_mm: spiling_contracts::geometry::SECTION_PLANE_TOLERANCE_MM,
        };
        let raw = crate::section::section(
            &definition,
            plane,
            &cancel,
            spiling_contracts::geometry::MAX_SECTION_BYTES,
            &mut work,
        )
        .unwrap();
        let borrowed = prepared
            .section_for_query(
                &query,
                &cancel,
                (crate::MAX_PREPARED_SECTION_BYTES - prepared.retained_bytes()) as u32,
                &mut work,
            )
            .unwrap();
        assert_eq!(raw.loops().len(), borrowed.loops().len());
        for (first, second) in raw.loops().iter().zip(borrowed.loops()) {
            assert_eq!(first.is_hole, second.is_hole);
            assert_eq!(first.points_mm, second.points_mm);
        }
        for (bytes, cancelled, allowance, expected) in [
            (
                crate::MAX_PREPARED_SECTION_BYTES,
                true,
                u64::MAX,
                GeometryErrorCode::Cancelled,
            ),
            (
                crate::MAX_PREPARED_SECTION_BYTES,
                false,
                1,
                GeometryErrorCode::ResourceLimit,
            ),
            (1024, false, u64::MAX, GeometryErrorCode::ResourceLimit),
        ] {
            let error = crate::intersect_native_section(
                &definition,
                None,
                Some(prepared.source_view().unwrap()),
                &query,
                bytes,
                bytes,
                &mut GeometryWorkBudget::new(allowance),
                &AtomicBool::new(cancelled),
            )
            .unwrap_err();
            assert_eq!(error.code, expected, "{error}");
        }
        let foreign = crate::from_step_bytes(include_bytes!(
            "../../../../fixtures/manufacturing/native/source-ring-torus.step"
        ))
        .unwrap();
        let error = crate::intersect_native_section(
            &foreign,
            None,
            Some(prepared.source_view().unwrap()),
            &query,
            crate::MAX_PREPARED_SECTION_BYTES,
            spiling_contracts::geometry::MAX_SECTION_BYTES as usize,
            &mut work,
            &cancel,
        )
        .unwrap_err();
        assert_eq!(error.code, GeometryErrorCode::InvalidGeometry);
    }
}
