// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{kernel_error, resource_error};
use crate::{Definition, GeometryError, GeometryWorkBudget};
use monstertruck_modeling::{
    Conic2D, Curve, Curve2D, ParameterRange, ParametricCurve, Surface, Tolerance,
};
use monstertruck_solid::{
    BooleanCoedgeOrigin, BooleanCutSide, BooleanEdgeDerivation, BooleanOperand,
    BooleanWithProvenance,
};
use monstertruck_topology::{EdgeId, trimmed::ChartCoedgeIdentity};
use spiling_contracts::geometry::GeometryErrorCode;
use std::{collections::HashMap, ops::Bound, sync::atomic::AtomicBool};

/// An original oriented trim occurrence at an actual shared Boolean endpoint.
/// This is genealogy, not a geometric proof; native coverage must certify it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SectionPlaneEndpointOrigin {
    pub shell_index: usize,
    pub face_index: usize,
    pub identity: ChartCoedgeIdentity<Curve, Surface>,
    pub candidate_index: usize,
    pub endpoint_is_back: bool,
    pub source_cut_parameter: f64,
}

fn unresolved(message: &str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::AnalysisIndeterminate, message)
}

/// Collect only adjacent original trim uses on the actual source-side output
/// face. Shared vertices and exact construction history select the occurrence;
/// coordinates, guide proximity and guessed parameter correspondence do not.
pub(super) fn collect(
    definition: &Definition,
    cut: &BooleanWithProvenance<Curve, Surface>,
    candidates: &HashMap<(EdgeId<Curve>, bool), usize>,
    byte_limit: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(Vec<SectionPlaneEndpointOrigin>, usize), GeometryError> {
    let mut origins = Vec::new();
    let mut offset = 0_usize;
    for output_face in cut.solid.face_iter() {
        for (wire_index, wire) in output_face.absolute_boundaries().iter().enumerate() {
            work.charge(1, cancel)?;
            let end = offset.checked_add(wire.len()).ok_or_else(|| {
                resource_error("native endpoint genealogy occurrence count overflow")
            })?;
            let lineage = cut.coedge_origins.get(offset..end).ok_or_else(|| {
                kernel_error("native endpoint genealogy omitted an output occurrence")
            })?;
            offset = end;
            for (coedge_index, (edge, coedge)) in wire.iter().zip(lineage).enumerate() {
                work.charge(1, cancel)?;
                if coedge.output_face != output_face.id()
                    || coedge.wire_index != wire_index
                    || coedge.coedge_index != coedge_index
                    || coedge.output_edge != edge.id()
                    || coedge.orientation != (edge.orientation() == output_face.orientation())
                {
                    return Err(kernel_error(
                        "native endpoint genealogy changed output occurrence",
                    ));
                }
                let owner = coedge.source_face();
                if owner.operand != BooleanOperand::Source {
                    continue;
                }
                for orientation in [true, false] {
                    let Some(&candidate_index) = candidates.get(&(edge.id(), orientation)) else {
                        continue;
                    };
                    for endpoint_is_back in [false, true] {
                        work.charge(1, cancel)?;
                        let vertex = if orientation == endpoint_is_back {
                            edge.absolute_back()
                        } else {
                            edge.absolute_front()
                        };
                        // Directed output-wire incidence distinguishes the two
                        // chart corners even when front/back share one vertex.
                        let source_endpoint_is_back =
                            endpoint_is_back == (orientation == edge.orientation());
                        let neighbor_index = if source_endpoint_is_back {
                            (coedge_index + 1) % wire.len()
                        } else {
                            (coedge_index + wire.len() - 1) % wire.len()
                        };
                        let neighbor = &wire[neighbor_index];
                        if neighbor.id() == edge.id() {
                            continue;
                        }
                        let shared = if source_endpoint_is_back {
                            neighbor.front()
                        } else {
                            neighbor.back()
                        };
                        if shared.id() != vertex.id() {
                            return Err(kernel_error(
                                "native endpoint genealogy lost directed shared vertex incidence",
                            ));
                        }
                        work.charge(1, cancel)?;
                        let (
                            face,
                            source_wire,
                            source_coedge,
                            source_edge,
                            material_orientation,
                            derivation,
                        ) = match &lineage[neighbor_index].source {
                            BooleanCoedgeOrigin::Original {
                                face,
                                wire_index,
                                coedge_index,
                                edge_id,
                                orientation,
                                derivation,
                            } => (
                                face,
                                *wire_index,
                                *coedge_index,
                                *edge_id,
                                *orientation,
                                *derivation,
                            ),
                            BooleanCoedgeOrigin::GeneratedIntersection { .. } => continue,
                            BooleanCoedgeOrigin::AmbiguousOriginalUse { .. } => {
                                return Err(unresolved(
                                    "native endpoint has ambiguous original trim-use genealogy",
                                ));
                            }
                        };
                        if face.operand != BooleanOperand::Source
                            || face.face_id != owner.face_id
                            || face.shell_index != owner.shell_index
                            || face.face_index != owner.face_index
                            || face.orientation != owner.orientation
                        {
                            return Err(kernel_error(
                                "native endpoint trim belongs to another source-face use",
                            ));
                        }
                        let original_face = definition
                            .solid
                            .boundaries()
                            .get(face.shell_index)
                            .and_then(|shell| shell.get(face.face_index))
                            .filter(|original| {
                                original.id() == face.face_id
                                    && original.orientation() == face.orientation
                            })
                            .ok_or_else(|| {
                                kernel_error("native endpoint lost its original source-face use")
                            })?;
                        let original_edge = original_face
                            .absolute_boundaries()
                            .get(source_wire)
                            .and_then(|boundary| boundary.get(source_coedge))
                            .filter(|original| {
                                original.id() == source_edge
                                    && (original.orientation() == original_face.orientation())
                                        == material_orientation
                            })
                            .ok_or_else(|| {
                                kernel_error(
                                    "native endpoint lost its oriented original trim occurrence",
                                )
                            })?;
                        let output_domain = neighbor.with_curve(ParametricCurve::parameter_range);
                        let output_is_back = source_endpoint_is_back != neighbor.orientation();
                        let source_cut_parameter = original_edge.with_curve(|curve| {
                            source_parameter(
                                curve, source_edge, &cut.edge_derivations, derivation,
                                output_domain, output_is_back, work, cancel,
                            )
                        }).map_err(|mut error| {
                            error.message = format!(
                                "original source shell {} face {} / trim wire {} coedge {} / endpoint genealogy: {}",
                                face.shell_index, face.face_index, source_wire, source_coedge, error.message,
                            );
                            error
                        })?;
                        let origin = SectionPlaneEndpointOrigin {
                            shell_index: face.shell_index,
                            face_index: face.face_index,
                            identity: ChartCoedgeIdentity {
                                face_id: face.face_id,
                                wire_index: source_wire,
                                coedge_index: source_coedge,
                                edge_id: source_edge,
                                orientation: material_orientation,
                            },
                            candidate_index,
                            endpoint_is_back,
                            source_cut_parameter,
                        };
                        if origins.len() == origins.capacity() {
                            let capacity = origins
                                .capacity()
                                .checked_mul(2)
                                .map(|capacity| capacity.max(4))
                                .ok_or_else(|| {
                                    resource_error("native endpoint genealogy capacity overflow")
                                })?;
                            capacity
                                .checked_mul(std::mem::size_of::<SectionPlaneEndpointOrigin>())
                                .filter(|bytes| *bytes <= byte_limit)
                                .ok_or_else(|| {
                                    resource_error(
                                        "native endpoint genealogy exceeds scratch capacity",
                                    )
                                })?;
                            origins
                                .try_reserve_exact(capacity - origins.len())
                                .map_err(|_| {
                                    resource_error("native endpoint genealogy allocation failed")
                                })?;
                            origins
                                .capacity()
                                .checked_mul(std::mem::size_of::<SectionPlaneEndpointOrigin>())
                                .filter(|bytes| *bytes <= byte_limit)
                                .ok_or_else(|| {
                                    resource_error(
                                        "native endpoint genealogy exceeds scratch capacity",
                                    )
                                })?;
                        }
                        origins.push(origin);
                    }
                }
            }
        }
    }
    if offset != cut.coedge_origins.len() {
        return Err(kernel_error(
            "native endpoint genealogy has unmatched output occurrences",
        ));
    }
    let bytes = origins.capacity() * std::mem::size_of::<SectionPlaneEndpointOrigin>();
    Ok((origins, bytes))
}

#[derive(Clone, Copy)]
enum CutGauge {
    Preserved,
    UnitLine,
    Polyline,
    ReflectedConic,
}

fn curve_gauge(
    curve: &Curve,
    depth: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<CutGauge, GeometryError> {
    work.charge(1, cancel)?;
    if depth > monstertruck_modeling::MAX_NATIVE_GEOMETRY_NESTING {
        return Err(resource_error(
            "native endpoint source nesting exceeds frozen cap",
        ));
    }
    match curve {
        Curve::Line(_) => Ok(CutGauge::UnitLine),
        Curve::BsplineCurve(_) | Curve::NurbsCurve(_) | Curve::TrimmedCurve(_) => {
            Ok(CutGauge::Preserved)
        }
        Curve::IntersectionCurve(curve) => curve_gauge(curve.leader(), depth + 1, work, cancel),
        Curve::ParameterCurve(curve) => {
            work.charge(1, cancel)?;
            Ok(match curve.curve() {
                Curve2D::Line(_) => CutGauge::UnitLine,
                Curve2D::Polyline(_) => CutGauge::Polyline,
                Curve2D::BsplineCurve(_) | Curve2D::NurbsCurve(_) | Curve2D::TrimmedCurve(_) => {
                    CutGauge::Preserved
                }
                Curve2D::Conic(conic) => {
                    let orientation = match conic {
                        Conic2D::Ellipse(curve) => curve.orientation(),
                        Conic2D::Hyperbola(curve) => curve.orientation(),
                        Conic2D::Parabola(curve) => curve.orientation(),
                    };
                    if orientation {
                        CutGauge::Preserved
                    } else {
                        CutGauge::ReflectedConic
                    }
                }
            })
        }
    }
}

fn included_domain(domain: ParameterRange) -> Result<[f64; 2], GeometryError> {
    match domain {
        (Bound::Included(a), Bound::Included(b)) if a.is_finite() && b.is_finite() && a < b => {
            Ok([a, b])
        }
        _ => Err(unresolved(
            "native endpoint derivation has no exact included finite gauge",
        )),
    }
}

/// Interpret the concrete native cut implementation, never an affine fit of
/// reported parent/child domains. Every graph link and actual domain must agree.
fn source_parameter(
    curve: &Curve,
    edge: EdgeId<Curve>,
    graph: &[BooleanEdgeDerivation<Curve>],
    mut index: usize,
    mut domain: ParameterRange,
    endpoint_is_back: bool,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<f64, GeometryError> {
    let gauge = curve_gauge(curve, 0, work, cancel)?;
    let bounds = included_domain(domain)?;
    let mut value = bounds[usize::from(endpoint_is_back)];
    loop {
        work.charge(1, cancel)?;
        match graph
            .get(index)
            .ok_or_else(|| unresolved("native endpoint derivation index is invalid"))?
        {
            BooleanEdgeDerivation::Source {
                edge_id,
                domain: source_domain,
            } => {
                if *edge_id != edge
                    || *source_domain != curve.parameter_range()
                    || domain != *source_domain
                {
                    return Err(unresolved(
                        "native endpoint derivation does not reach its exact original edge gauge",
                    ));
                }
                let [a, b] = included_domain(*source_domain)?;
                if !value.is_finite() || value < a || value > b {
                    return Err(unresolved(
                        "native endpoint seed leaves its original active gauge",
                    ));
                }
                return Ok(value);
            }
            BooleanEdgeDerivation::Cut {
                parent,
                parent_domain,
                parameter,
                side,
                child_domain,
            } => {
                if *parent >= index || *child_domain != domain {
                    return Err(unresolved(
                        "native endpoint cut genealogy is cyclic or has the wrong child gauge",
                    ));
                }
                let [a, b] = included_domain(*parent_domain)?;
                let child = included_domain(*child_domain)?;
                let t = *parameter;
                if !t.is_finite() || t <= a || t >= b || value < child[0] || value > child[1] {
                    return Err(unresolved(
                        "native endpoint cut parameter is outside its exact parent gauge",
                    ));
                }
                let before = *side == BooleanCutSide::Before;
                let expected = match gauge {
                    CutGauge::Preserved => {
                        if before {
                            [a, t]
                        } else {
                            [t, b]
                        }
                    }
                    CutGauge::UnitLine => {
                        if [a, b] != [0.0, 1.0] {
                            return Err(unresolved("native line cut lost its unit raw gauge"));
                        }
                        value = if value == 0.0 {
                            if before { a } else { t }
                        } else if value == 1.0 {
                            if before { t } else { b }
                        } else if before {
                            value * t
                        } else {
                            t + value * (1.0 - t)
                        };
                        [0.0, 1.0]
                    }
                    CutGauge::ReflectedConic => {
                        let reflected = a + b - t;
                        value = if value == child[0] {
                            if before { a } else { t }
                        } else if value == child[1] {
                            if before { t } else { b }
                        } else if before {
                            a + (value - reflected)
                        } else {
                            t + (value - a)
                        };
                        if before {
                            [reflected, b]
                        } else {
                            [a, reflected]
                        }
                    }
                    CutGauge::Polyline => {
                        if a != 0.0 || b.fract() != 0.0 {
                            return Err(unresolved(
                                "native polyline cut lost its integer raw gauge",
                            ));
                        }
                        let knot = t.floor();
                        let snapped = t.near(&knot);
                        let expected = if before {
                            [0.0, if snapped { knot } else { knot + 1.0 }]
                        } else {
                            [0.0, b - knot]
                        };
                        value = if before {
                            if value == expected[1] {
                                t
                            } else if snapped || value <= knot {
                                value
                            } else {
                                knot + (value - knot) * (t - knot)
                            }
                        } else if value == 0.0 {
                            t
                        } else if snapped {
                            knot + value
                        } else if value <= 1.0 {
                            t + value * (knot + 1.0 - t)
                        } else {
                            knot + value
                        };
                        expected
                    }
                };
                if child != expected || !value.is_finite() || value < a || value > b {
                    return Err(unresolved(
                        "native endpoint cut contract disagrees with actual child domain",
                    ));
                }
                domain = *parent_domain;
                index = *parent;
            }
        }
    }
}
