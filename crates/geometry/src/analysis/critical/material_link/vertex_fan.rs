// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use solidcore::VertexId;

/// Every allowed ray polygon has strictly positive sectors and winding one.
/// The sign dynamic program counts all half-open positive-axis crossings,
/// retaining the initial sign through the closing edge.
pub(super) fn projected_vertex_fan(
    rays: &[[Interval; 2]],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(1, cancel)?;
    if rays.len() < 3 {
        return Ok(false);
    }
    for (index, a) in rays.iter().enumerate() {
        work.charge(1, cancel)?;
        if a.iter()
            .any(|v| !v.lo.is_finite() || !v.hi.is_finite() || v.lo > v.hi)
            || a.iter().all(|v| v.contains_zero())
        {
            return Ok(false);
        }
        let b = rays[(index + 1) % rays.len()];
        if a[0].mul(b[1]).sub(a[1].mul(b[0])).lo <= 0.0 {
            return Ok(false);
        }
    }
    let first = [rays[0][1].lo <= 0.0, rays[0][1].hi > 0.0];
    let mut minimum = [[usize::MAX; 2]; 2];
    let mut maximum = [[0_usize; 2]; 2];
    for sign in 0..2 {
        if first[sign] {
            minimum[sign][sign] = 0;
        }
    }
    for ray in &rays[1..] {
        let allowed = [ray[1].lo <= 0.0, ray[1].hi > 0.0];
        let mut next_minimum = [[usize::MAX; 2]; 2];
        let mut next_maximum = [[0_usize; 2]; 2];
        for initial in 0..2 {
            for from in 0..2 {
                for to in 0..2 {
                    work.charge(1, cancel)?;
                    if minimum[initial][from] == usize::MAX || !allowed[to] {
                        continue;
                    }
                    let crossing = usize::from(from == 0 && to == 1);
                    next_minimum[initial][to] =
                        next_minimum[initial][to].min(minimum[initial][from] + crossing);
                    next_maximum[initial][to] =
                        next_maximum[initial][to].max(maximum[initial][from] + crossing);
                }
            }
        }
        minimum = next_minimum;
        maximum = next_maximum;
    }
    let mut lower = usize::MAX;
    let mut upper = 0;
    for initial in 0..2 {
        for last in 0..2 {
            work.charge(1, cancel)?;
            if minimum[initial][last] == usize::MAX {
                continue;
            }
            let crossing = usize::from(last == 0 && initial == 1);
            lower = lower.min(minimum[initial][last] + crossing);
            upper = upper.max(maximum[initial][last] + crossing);
        }
    }
    Ok(lower == 1 && upper == 1)
}

fn native_vertex_germ_count(
    incidence: &SourceEdgeIncidence<'_>,
    vertex: VertexId,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<usize, GeometryError> {
    work.charge(1, cancel)?;
    let mut count = 0usize;
    for edge in incidence.vertex_edges(vertex) {
        work.charge(1, cancel)?;
        let pair = incidence.get(edge).ok_or_else(resource)?;
        count = count
            .checked_add(pair.vertices.iter().filter(|&&id| id == vertex).count())
            .ok_or_else(resource)?;
    }
    Ok(count)
}

/// The existing fewer-than-three-germ common-carrier lane, independently of
/// any projection. This seals only a complete native same-carrier seam:
/// callers still owe their closed-cut height/rank or radial-map obligations.
pub(in crate::analysis) fn qualify_native_common_carrier_vertex(
    source: MaterialSourceView<'_, '_>,
    incidence: &SourceEdgeIncidence<'_>,
    vertex: VertexId,
    remaining: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let count = native_vertex_germ_count(incidence, vertex, work, cancel)?;
    qualify_small_common_carrier_vertex(source, incidence, vertex, count, remaining, work, cancel)
}

fn qualify_small_common_carrier_vertex(
    source: MaterialSourceView<'_, '_>,
    incidence: &SourceEdgeIncidence<'_>,
    vertex: VertexId,
    count: usize,
    remaining: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    if count == 0 || count >= 3 {
        return Ok(false);
    }
    let available = remaining
        .checked_sub(UV_EMBEDDING_STACK_BYTES)
        .ok_or_else(resource)?;
    let definition = source.definition();
    let point = source
        .trim_source()
        .native_vertex_point(vertex, work, cancel)?;
    let tolerance = source.trim_source().effective_tolerance_mm();
    let mut regular = true;
    let mut common_face = None;
    let cycle = incidence.visit_vertex_link(
        definition,
        vertex,
        work,
        cancel,
        &mut |use_, endpoint, work, cancel| {
            let domain = source.face_domain(use_.face, available, work, cancel)?;
            if !domain.is_native_material() {
                return Err(GeometryError::new(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "vertex seam requires native material charts",
                ));
            }
            if let Some(face) = common_face {
                if face != use_.face {
                    let other = source.face_domain(face, available, work, cancel)?;
                    regular &= crate::parametric::same_surface_coefficients(
                        &domain.surface_operand(),
                        &other.surface_operand(),
                        work,
                        cancel,
                    )?;
                }
            } else {
                common_face = Some(use_.face);
            }
            let parameter = domain.coedges()[use_.coedge].native_range[endpoint];
            domain.with_native_spatial_curve(
                use_.coedge,
                available,
                work,
                cancel,
                |curve, work, cancel| {
                    let jet = crate::parametric::curve_enclosure(
                        curve,
                        (parameter, parameter),
                        work,
                        cancel,
                    )?;
                    let residual = std::array::from_fn(|axis| {
                        jet.position[axis].sub(Interval::point(point[axis]))
                    });
                    regular &= length(residual)?.hi <= tolerance;
                    Ok(())
                },
            )
        },
    )?;
    if !cycle || !regular {
        return Ok(false);
    }
    let Some(face) = common_face else {
        return Ok(false);
    };
    let domain = source.face_domain(face, available, work, cancel)?;
    Ok(prepared_carrier_c1_covers(domain, domain.uv_domain())
        || critical_face_gluing(&domain.surface_operand(), domain.uv_domain(), work, cancel)?)
}

/// Native absolute endpoint germs in their actual corner permutation. Rows
/// must be the differential of the consumer's SAME actual projection family.
/// A complete single-cycle check and all-ray winding exclude pinched and
/// multiple-cover vertices; native allocation order never chooses angular order.
/// The consumer still owes its joint Jacobians, metric and closed-cut gates;
/// projected endpoint germs do not exclude a whole native vertex tolerance zone.
pub(in crate::analysis) fn qualify_native_vertex(
    source: MaterialSourceView<'_, '_>,
    incidence: &SourceEdgeIncidence<'_>,
    vertex: VertexId,
    rows: [[Interval; 3]; 2],
    remaining: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let definition = source.definition();
    let count = native_vertex_germ_count(incidence, vertex, work, cancel)?;
    if count < 3 {
        return qualify_small_common_carrier_vertex(
            source, incidence, vertex, count, remaining, work, cancel,
        );
    }
    let bytes = count
        .checked_mul(std::mem::size_of::<[Interval; 2]>())
        .ok_or_else(resource)?;
    let available = remaining
        .checked_sub(bytes + UV_EMBEDDING_STACK_BYTES)
        .ok_or_else(resource)?;
    let point = source
        .trim_source()
        .native_vertex_point(vertex, work, cancel)?;
    let tolerance = source.trim_source().effective_tolerance_mm();
    let mut rays = Vec::new();
    rays.try_reserve_exact(count).map_err(|_| resource())?;
    if rays
        .capacity()
        .saturating_mul(std::mem::size_of::<[Interval; 2]>())
        > bytes
    {
        return Err(resource());
    }
    let mut regular = true;
    let cycle = incidence.visit_vertex_link(
        definition,
        vertex,
        work,
        cancel,
        &mut |use_, endpoint, work, cancel| {
            let domain = source.face_domain(use_.face, available, work, cancel)?;
            if !domain.is_native_material() {
                return Err(GeometryError::new(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "vertex fan requires native material charts",
                ));
            }
            let coedge = &domain.coedges()[use_.coedge];
            let parameter = coedge.native_range[endpoint];
            let tangent = domain.with_native_spatial_curve(
                use_.coedge,
                available,
                work,
                cancel,
                |curve, work, cancel| {
                    let jet = crate::parametric::curve_enclosure(
                        curve,
                        (parameter, parameter),
                        work,
                        cancel,
                    )?;
                    let residual = std::array::from_fn(|axis| {
                        jet.position[axis].sub(Interval::point(point[axis]))
                    });
                    regular &= length(residual)?.hi <= tolerance;
                    Ok(if endpoint == 0 {
                        jet.derivative
                    } else {
                        jet.derivative.map(Interval::neg)
                    })
                },
            )?;
            rays.push(rows.map(|row| dot(row, tangent)));
            Ok(())
        },
    )?;
    if !cycle || !regular {
        return Ok(false);
    }
    if projected_vertex_fan(&rays, work, cancel)? {
        return Ok(true);
    }
    rays.reverse();
    projected_vertex_fan(&rays, work, cancel)
}
