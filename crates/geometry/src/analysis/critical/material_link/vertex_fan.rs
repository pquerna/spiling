// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::super::*;

/// Prove a positive, degree-one projected fan in the supplied original order.
///
/// The caller owns complete original incidence, its single-cycle permutation,
/// the common projected vertex, and all transport/derivative uncertainty. This
/// predicate uses neither source identity nor a representative of a ray box.
/// Each box must exclude zero, and *every* pair of adjacent enclosed vectors
/// must have positive determinant. Thus every possible polygon joining one
/// vector from each box avoids zero, and its angular sectors lie in `(0, PI)`.
///
/// For such polygons the usual half-open horizontal-ray winding rule counts
/// only transitions from `y <= 0` to `y > 0`: their determinants are positive;
/// downward crossings therefore contribute zero, not minus one. The bounded
/// dynamic program below encloses that integer count over every allowed sign
/// of every box, including boxes crossing an axis. It retains the initial sign
/// until the closing edge, so an uncertain first ray is not counted twice.
/// Accepting only the singleton count one excludes multiwinding and ray-order
/// aliases, not merely adjacent reversals. In particular, overlapping angular
/// cones anywhere in the cycle cannot pass: choosing the same direction in
/// those two cones would force more than one turn of strictly positive sectors.
pub(super) fn projected_vertex_fan(
    rays: &[[Interval; 2]],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(1, cancel)?;
    if rays.len() < 3 {
        return Ok(false);
    }
    for ray in rays {
        work.charge(1, cancel)?;
        if ray
            .iter()
            .any(|v| !v.lo.is_finite() || !v.hi.is_finite() || v.lo > v.hi)
            || (ray[0].contains_zero() && ray[1].contains_zero())
        {
            return Ok(false);
        }
    }
    for (index, a) in rays.iter().enumerate() {
        work.charge(1, cancel)?;
        let b = &rays[if index + 1 == rays.len() {
            0
        } else {
            index + 1
        }];
        let determinant = a[0].mul(b[1]).sub(a[1].mul(b[0]));
        if !determinant.lo.is_finite() || determinant.lo <= 0.0 {
            return Ok(false);
        }
    }

    // Sign 0 includes the axis; sign 1 is strictly above it. A non-singleton
    // y interval may admit both signs. Min/max entries are indexed by initial
    // and current sign. No path choices or per-ray storage are retained.
    let first = [rays[0][1].lo <= 0.0, rays[0][1].hi > 0.0];
    let mut minimum = [[usize::MAX; 2]; 2];
    let mut maximum = [[0_usize; 2]; 2];
    for sign in 0..2 {
        if first[sign] {
            minimum[sign][sign] = 0;
        }
    }
    for ray in &rays[1..] {
        work.charge(1, cancel)?;
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
    let mut winding_minimum = usize::MAX;
    let mut winding_maximum = 0;
    for initial in 0..2 {
        for last in 0..2 {
            work.charge(1, cancel)?;
            if minimum[initial][last] == usize::MAX {
                continue;
            }
            let crossing = usize::from(last == 0 && initial == 1);
            winding_minimum = winding_minimum.min(minimum[initial][last] + crossing);
            winding_maximum = winding_maximum.max(maximum[initial][last] + crossing);
        }
    }
    Ok(winding_minimum == 1 && winding_maximum == 1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OriginalVertexFanProof {
    OutsideClosedSlab,
    ExactAffine,
}

/// Certify the complete original vertex by corrected CLOSED-slab exclusion or
/// an independently proved exact affine degree-one physical corner complex.
///
/// Endpoint corrections use source-only first/last parameter fractions of
/// 1/1024; neither support nor correction radius depends on the query slab.
/// Active supports additionally admit an exact zero-transport affine corner
/// complex: original coefficient identities, positive radial face normals,
/// nonreflex original corners, and complete projected degree one are required.
/// Nonzero/unproved face transport remains an explicit refusal.
#[allow(clippy::too_many_arguments)]
pub(super) fn original_vertex_fan(
    definition: &Definition,
    source: OriginalTrimSource<'_, '_>,
    incidence: &SourceEdgeIncidence,
    vertex: monstertruck_topology::VertexId<monstertruck_modeling::Point3>,
    prism: &SamplePrism,
    request: &SlabRequest,
    source_allowance_mm: f64,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<OriginalVertexFanProof, GeometryError> {
    work.charge(1, cancel)?;
    if !std::ptr::eq(source.definition(), definition)
        || !source_allowance_mm.is_finite()
        || source_allowance_mm < 0.0
        || definition.provenance.uncertainty_mm != Some(source_allowance_mm)
        || prism.range != request.height_interval_mm
    {
        return Err(GeometryError::new(
            GeometryErrorCode::ParameterCorrespondenceUnavailable,
            "original vertex support lost its immutable source or admitted correction allowance",
        ));
    }
    let mut support = None;
    let mut incident_count = 0_usize;
    for edge_id in incidence.vertex_edges(vertex) {
        work.charge(1, cancel)?;
        incident_count += 1;
        let uses = incidence.get(edge_id).ok_or_else(|| {
            GeometryError::new(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "original vertex edge is missing its incidence",
            )
        })?;
        let second = uses.second.ok_or_else(|| {
            GeometryError::new(
                GeometryErrorCode::SingularGeometry,
                "original vertex has an unpaired edge use",
            )
        })?;
        for use_ in [uses.first, second] {
            work.charge(1, cancel)?;
            let domain = source.face_domain(use_.face, remaining_bytes, work, cancel)?;
            let face = domain.native().face();
            let wire = face.absolute_boundaries().get(use_.wire).ok_or_else(|| {
                GeometryError::new(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "original vertex face wire is missing",
                )
            })?;
            let edge = wire.get(use_.index).ok_or_else(|| {
                GeometryError::new(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "original vertex coedge occurrence is missing",
                )
            })?;
            if edge.id() != edge_id
                || (edge.orientation() == face.orientation()) != use_.orientation
                || !uses.vertices.contains(&vertex)
            {
                return Err(GeometryError::new(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "original vertex coedge ownership changed",
                ));
            }
            if use_ == uses.first {
                let curve_bytes = remaining_bytes
                    .checked_sub(domain.owned_bytes())
                    .ok_or_else(|| {
                        GeometryError::new(
                            GeometryErrorCode::ResourceLimit,
                            "original vertex curve and face support exceed scratch allowance",
                        )
                    })?;
                edge.with_curve(|curve| -> Result<(), GeometryError> {
                    let range = curve.range_tuple();
                    let prepared = definition.prepare_boundary_curve(
                        edge_id, curve, range, curve_bytes, work, cancel,
                    )?;
                    for (high, endpoint) in [
                        (false, edge.absolute_front()),
                        (true, edge.absolute_back()),
                    ] {
                        work.charge(1, cancel)?;
                        if endpoint.id() != vertex {
                            continue;
                        }
                        let parameter = if high { range.1 } else { range.0 };
                        let point = prepared.jet((parameter, parameter), work, cancel)?.position;
                        let target: [f64; 3] = endpoint.point().into();
                        let delta = std::array::from_fn(|axis| {
                            Interval::point(target[axis]).sub(point[axis])
                        });
                        if length(delta)?.hi > source_allowance_mm {
                            return Err(GeometryError::new(
                                GeometryErrorCode::ShapeErrorExceeded,
                                "original vertex endpoint correction exceeds admitted source allowance",
                            ));
                        }
                        hull_vertex_support(&mut support, target.map(Interval::point));
                        hull_vertex_support(&mut support, point);
                        let span = endpoint_support_span([range.0, range.1], high)?;
                        hull_vertex_support(
                            &mut support,
                            prepared.position_bounds((span[0], span[1]), work, cancel)?,
                        );
                    }
                    Ok(())
                })?;
            }

            // Each absolute wire corner is owned by its incoming occurrence.
            // Both sides of paired edges are visited, including same-face
            // seams. A closed original edge may own two endpoint parameters.
            if edge.back().id() != vertex {
                continue;
            }
            let next_index = if use_.index + 1 == wire.len() {
                0
            } else {
                use_.index + 1
            };
            let next = &wire[next_index];
            if next.front().id() != vertex {
                return Err(GeometryError::new(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "original vertex face corner is not closed by native incidence",
                ));
            }
            let mut uv: Option<[Interval; 2]> = None;
            for (index, back) in [(use_.index, true), (next_index, false)] {
                let mut selected = None;
                for (flat, coedge) in domain.native().coedges().iter().enumerate() {
                    work.charge(1, cancel)?;
                    let identity = coedge.identity;
                    if identity.face_id == face.id()
                        && identity.wire_index == use_.wire
                        && identity.coedge_index == index
                        && identity.edge_id == wire[index].id()
                        && identity.orientation == (wire[index].orientation() == face.orientation())
                    {
                        if selected.is_some() {
                            return Err(GeometryError::new(
                                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                                "original vertex corner has duplicate native UV ownership",
                            ));
                        }
                        selected = Some((flat, coedge));
                    }
                }
                let (flat, coedge) = selected.ok_or_else(|| {
                    GeometryError::new(
                        GeometryErrorCode::ParameterCorrespondenceUnavailable,
                        "original vertex corner is missing its native UV endpoint",
                    )
                })?;
                let range = coedge.oriented_range;
                let parameter = range[usize::from(back == face.orientation())];
                let bounds = [range[0].min(range[1]), range[0].max(range[1])];
                let span = endpoint_support_span(bounds, parameter == bounds[1])?;
                let image = domain.boundary_position_enclosure(flat, span, work, cancel)?;
                let mut image = image.map(|v| Interval { lo: v[0], hi: v[1] });
                let proof = domain.original_coedge_correspondence(flat)?;
                if proof.position_error_mm() != 0.0 {
                    let native_range = proof.native_range();
                    let native_high = wire[index].absolute_back().id() == vertex;
                    let native_span = endpoint_support_span(native_range, native_high)?;
                    let jet = domain.original_coedge_uv_jet(flat, native_span, work, cancel)?;
                    let width = Interval::point(native_range[1])
                        .sub(Interval::point(native_range[0]))
                        .mul(Interval::point(
                            domain.material_realization().collar_parameter_fraction(),
                        ))
                        .mul(Interval::point(0.25));
                    let transverse = [jet.derivative[1], jet.derivative[0]];
                    for axis in 0..2 {
                        let radius = width.mul(Interval::point(transverse[axis].abs_upper())).hi;
                        image[axis] = image[axis].add(Interval {
                            lo: -radius,
                            hi: radius,
                        });
                    }
                }
                uv = Some(match uv {
                    Some(previous) => std::array::from_fn(|axis| previous[axis].hull(image[axis])),
                    None => image,
                });
            }
            // Exact coefficient-closed corners have no PreparedTrimJoint.
            // Nonexact corners retain EVERY matching original joint envelope;
            // no nearest-vertex, endpoint label, or pole-name substitution.
            let mut pole_joint = false;
            for joint in domain.native().joints() {
                work.charge(1, cancel)?;
                if joint.vertex_id != vertex {
                    continue;
                }
                let (incoming, outgoing) = if face.orientation() {
                    (use_.index, next_index)
                } else {
                    (next_index, use_.index)
                };
                let owns_corner = [(joint.incoming, incoming), (joint.outgoing, outgoing)]
                    .into_iter()
                    .all(|(identity, index)| {
                        identity.face_id == face.id()
                            && identity.wire_index == use_.wire
                            && identity.coedge_index == index
                            && identity.edge_id == wire[index].id()
                            && identity.orientation
                                == (wire[index].orientation() == face.orientation())
                    });
                if !owns_corner {
                    continue;
                }
                pole_joint |= matches!(
                    joint.kind,
                    monstertruck_modeling::TrimJointKind::SphericalPole { .. }
                );
                let image = joint.enclosure.map(|v| Interval {
                    lo: v.lower,
                    hi: v.upper,
                });
                uv = Some(match uv {
                    Some(previous) => std::array::from_fn(|axis| previous[axis].hull(image[axis])),
                    None => image,
                });
            }
            let mut uv = uv.ok_or_else(|| {
                GeometryError::new(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "original vertex corner has no complete source support",
                )
            })?;
            let margin = domain.native().uncertainty_uv();
            for axis in 0..2 {
                uv[axis] = uv[axis].add(Interval {
                    lo: -margin[axis],
                    hi: margin[axis],
                });
            }
            face.with_surface(|surface| -> Result<(), GeometryError> {
                if pole_joint
                    && source_chart::SphereChart::new(surface, &domain, work, cancel)?.is_none()
                {
                    return Err(GeometryError::new(
                        GeometryErrorCode::AnalysisIndeterminate,
                        "original vertex pole corner lacks its independent source chart proof",
                    ));
                }
                for patch in
                    surface_parameter_boxes(surface, uv.map(|v| [v.lo, v.hi]), work, cancel)?
                {
                    work.charge(1, cancel)?;
                    let image = crate::parametric::surface_piecewise_enclosure(
                        surface, patch, work, cancel,
                    )?
                    .position;
                    hull_vertex_support(&mut support, image);
                }
                Ok(())
            })?;
        }
    }
    let support = support.filter(|_| incident_count != 0).ok_or_else(|| {
        GeometryError::new(
            GeometryErrorCode::ParameterCorrespondenceUnavailable,
            "original vertex incidence is empty or was not prepared",
        )
    })?;
    // Cubic endpoint corrections and the separately bounded corner/face
    // transport share ONE admitted physical displacement allowance. Inflate
    // the entire original support, not just the stored native vertex point.
    let corrected = support.map(|v| {
        v.add(Interval {
            lo: -source_allowance_mm,
            hi: source_allowance_mm,
        })
    });
    let height = prism.source_height.height(corrected);
    if height.hi < request.height_interval_mm[0] || height.lo > request.height_interval_mm[1] {
        return Ok(OriginalVertexFanProof::OutsideClosedSlab);
    }
    if exact_affine_vertex_fan(
        definition,
        source,
        incidence,
        vertex,
        incident_count,
        prism,
        remaining_bytes,
        work,
        cancel,
    )? {
        return Ok(OriginalVertexFanProof::ExactAffine);
    }
    Err(GeometryError::new(
        GeometryErrorCode::AnalysisIndeterminate,
        "active original vertex requires complete face-transport and outer-star degree proof",
    ))
}

#[allow(clippy::too_many_arguments)]
fn exact_affine_vertex_fan(
    definition: &Definition,
    source: OriginalTrimSource<'_, '_>,
    incidence: &SourceEdgeIncidence,
    vertex: monstertruck_topology::VertexId<monstertruck_modeling::Point3>,
    incident_count: usize,
    prism: &SamplePrism,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(1, cancel)?;
    if incident_count < 3 || definition.solid.boundaries().len() != 1 {
        return Ok(false);
    }
    let shell = &definition.solid.boundaries()[0];
    let Some(first_edge) = incidence.vertex_edges(vertex).next() else {
        return Ok(false);
    };
    let Some(first_uses) = incidence.get(first_edge) else {
        return Ok(false);
    };
    let Some(second_use) = first_uses.second else {
        return Ok(false);
    };
    let mut start = None;
    let mut target = None;
    // Choose material-outgoing, never spatially sort the original permutation
    // or reverse a failed winding sign after inspecting projected geometry.
    for candidate in [first_uses.first, second_use] {
        work.charge(1, cancel)?;
        let Some(face) = shell.get(candidate.face) else {
            return Ok(false);
        };
        let Some(wire) = face.absolute_boundaries().get(candidate.wire) else {
            return Ok(false);
        };
        let Some(edge) = wire.get(candidate.index) else {
            return Ok(false);
        };
        if edge.front().id() == edge.back().id() {
            return Ok(false);
        }
        if (edge.front().id() == vertex) == face.orientation() {
            let endpoint = if edge.front().id() == vertex {
                edge.front()
            } else {
                edge.back()
            };
            start = Some(candidate);
            let point = endpoint.point();
            target = Some([point.x, point.y, point.z]);
            break;
        }
    }
    let (Some(start), Some(target)) = (start, target) else {
        return Ok(false);
    };
    let point = target.map(Interval::point);
    let Some((radial, _)) = prism.radial_projection_frame(point)? else {
        return Ok(false);
    };
    if length(radial)?.lo <= 0.0 {
        return Ok(false);
    }
    // Any fixed coordinate axis not parallel to the entire radial enclosure
    // gives an oriented transverse frame. No angle, fitted plane or sampled
    // normal is used. Interval rows cover the complete radial uncertainty.
    let mut rows = None;
    for axis in 0..3 {
        work.charge(1, cancel)?;
        let unit = std::array::from_fn(|i| Interval::point(if i == axis { 1.0 } else { 0.0 }));
        let u = cross(unit, radial);
        if length(u)?.lo > 0.0 {
            rows = Some([u, cross(radial, u)]);
            break;
        }
    }
    let Some(rows) = rows else {
        return Ok(false);
    };
    let bytes = incident_count
        .checked_mul(std::mem::size_of::<[Interval; 2]>())
        .and_then(|v| v.checked_add(std::mem::size_of::<Vec<[Interval; 2]>>()))
        .filter(|bytes| *bytes <= remaining_bytes)
        .ok_or_else(|| {
            GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "original vertex projected fan exceeds remaining scratch allowance",
            )
        })?;
    let mut rays = Vec::new();
    rays.try_reserve_exact(incident_count).map_err(|_| {
        GeometryError::new(
            GeometryErrorCode::ResourceLimit,
            "original vertex projected fan allocation refused",
        )
    })?;
    let ray_bytes = rays
        .capacity()
        .checked_mul(std::mem::size_of::<[Interval; 2]>())
        .and_then(|v| v.checked_add(std::mem::size_of::<Vec<[Interval; 2]>>()))
        .ok_or_else(|| {
            GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "original vertex projected fan capacity overflow",
            )
        })?;
    let domain_bytes = remaining_bytes
        .checked_sub(ray_bytes.max(bytes))
        .ok_or_else(|| {
            GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "original vertex fan and face domain exceed remaining scratch allowance",
            )
        })?;
    let mut current = start;
    for step in 0..incident_count {
        work.charge(1, cancel)?;
        let domain = source.face_domain(current.face, domain_bytes, work, cancel)?;
        let face = domain.native().face();
        let Some(wire) = face.absolute_boundaries().get(current.wire) else {
            return Ok(false);
        };
        let Some(edge) = wire.get(current.index) else {
            return Ok(false);
        };
        if edge.front().id() == edge.back().id()
            || (edge.front().id() == vertex) != face.orientation()
            || (edge.orientation() == face.orientation()) != current.orientation
        {
            return Ok(false);
        }
        let Some(direction) = edge.with_curve(|curve| {
            affine_line_direction(
                curve,
                edge.absolute_front().point().into(),
                edge.absolute_back().point().into(),
                work,
                cancel,
            )
        })?
        else {
            return Ok(false);
        };
        let outward = if edge.absolute_front().id() == vertex {
            direction
        } else if edge.absolute_back().id() == vertex {
            direction.map(Interval::neg)
        } else {
            return Ok(false);
        };
        let Some(carrier) =
            face.with_surface(|surface| exact_affine_surface(surface, work, cancel))?
        else {
            return Ok(false);
        };
        let Some(mut n) = exact_affine_normal(&carrier, work, cancel)? else {
            return Ok(false);
        };
        if !face.orientation() {
            n = n.map(Interval::neg);
        }
        if length(n)?.lo <= 0.0 || dot(n, radial).lo <= 0.0 {
            return Ok(false);
        }
        // Independently prove the complete ORIGINAL UV coefficient trace is
        // this native affine curve. A retained Forward proof can describe an
        // exact algebraic identity despite nonzero numerical enclosure radii;
        // its enum or residual is neither identity authority nor a rejection.
        let mut flat = 0;
        for (wire_index, boundary) in face.absolute_boundaries().iter().enumerate() {
            for (index, boundary_edge) in boundary.iter().enumerate() {
                work.charge(1, cancel)?;
                let proof = domain.original_coedge_correspondence(flat)?;
                let Some(binding) = proof.binding() else {
                    return Ok(false);
                };
                if binding.owner != [current.face, wire_index, index]
                    || binding.edge != boundary_edge.id()
                    || binding.endpoints != [boundary_edge.front().id(), boundary_edge.back().id()]
                {
                    return Ok(false);
                }
                let Some(coedge) = domain.native().coedges().get(flat) else {
                    return Ok(false);
                };
                flat += 1;
                let a: [f64; 3] = boundary_edge.absolute_front().point().into();
                let b: [f64; 3] = boundary_edge.absolute_back().point().into();
                let Some(d) = boundary_edge.with_curve(|curve| {
                    let range = curve.range_tuple();
                    if proof.native_range() != [range.0, range.1] {
                        return Ok(None);
                    }
                    affine_line_direction(curve, a, b, work, cancel)
                })?
                else {
                    return Ok(false);
                };
                if !exact_affine_trace(
                    &carrier,
                    coedge.curve,
                    proof.uv_range(),
                    [a, b],
                    work,
                    cancel,
                )? {
                    return Ok(false);
                }
                if boundary_edge.absolute_front().id() != vertex
                    && boundary_edge.absolute_back().id() != vertex
                {
                    let va = std::array::from_fn(|i| point[i].sub(Interval::point(a[i])));
                    let vb = std::array::from_fn(|i| point[i].sub(Interval::point(b[i])));
                    if length(cross(d, va))?.lo <= 0.0 && dot(va, vb).lo <= 0.0 {
                        return Ok(false);
                    }
                }
            }
        }
        for joint in domain.native().joints() {
            work.charge(1, cancel)?;
            if joint.vertex_id == vertex {
                // All incoming/outgoing coefficient endpoints above equal
                // NativeV. Injectivity of the exact affine carrier gives one
                // exact UV endpoint, independent of interval rounding width.
                if !matches!(
                    joint.kind,
                    monstertruck_modeling::TrimJointKind::EndpointUncertainty
                ) || [
                    joint.vertex_point.x,
                    joint.vertex_point.y,
                    joint.vertex_point.z,
                ] != target
                {
                    return Ok(false);
                }
            }
        }
        let adjacent = if face.orientation() {
            if current.index == 0 {
                wire.len() - 1
            } else {
                current.index - 1
            }
        } else if current.index + 1 == wire.len() {
            0
        } else {
            current.index + 1
        };
        let neighbor = &wire[adjacent];
        let Some(d) = neighbor.with_curve(|curve| {
            affine_line_direction(
                curve,
                neighbor.absolute_front().point().into(),
                neighbor.absolute_back().point().into(),
                work,
                cancel,
            )
        })?
        else {
            return Ok(false);
        };
        let next_outward = if neighbor.absolute_front().id() == vertex {
            d
        } else if neighbor.absolute_back().id() == vertex {
            d.map(Interval::neg)
        } else {
            return Ok(false);
        };
        if dot(n, cross(outward, next_outward)).lo <= 0.0 {
            return Ok(false);
        }
        rays.push([dot(outward, rows[0]), dot(outward, rows[1])]);
        let selected = crate::analysis::source_incidence::CoedgeUse {
            face: current.face,
            wire: current.wire,
            index: adjacent,
            coedge: current.coedge - current.index + adjacent,
            orientation: neighbor.orientation() == face.orientation(),
        };
        let Some(pair) = incidence.get(neighbor.id()) else {
            return Ok(false);
        };
        current = if pair.first == selected {
            let Some(other) = pair.second else {
                return Ok(false);
            };
            other
        } else if pair.second == Some(selected) {
            pair.first
        } else {
            return Ok(false);
        };
        if current == start && step + 1 != incident_count {
            return Ok(false);
        }
    }
    if current != start {
        return Ok(false);
    }
    projected_vertex_fan(&rays, work, cancel)
}

/// Numerators of O + u*A + v*B over one nonzero exact denominator.
/// Constant-weight homogeneous tensors are covered without dehomogenizing or
/// rounding their control points, axes, knot sums or origins.
struct ExactAffineSurface {
    origin: [ExactCoefficient; 3],
    du: [ExactCoefficient; 3],
    dv: [ExactCoefficient; 3],
    denominator: ExactCoefficient,
    support: Option<[[f64; 2]; 2]>,
}

fn add_exact(target: &mut ExactCoefficient, value: &ExactCoefficient) -> bool {
    value.terms[..value.len]
        .iter()
        .all(|&term| target.push(term).is_some())
}

fn exact_interval(value: &ExactCoefficient) -> Option<Interval> {
    let result = value.terms[..value.len]
        .iter()
        .fold(Interval::point(0.0), |sum, &term| {
            sum.add(Interval::point(term))
        });
    (result.lo.is_finite() && result.hi.is_finite()).then_some(result)
}

fn exact_affine_normal(
    carrier: &ExactAffineSurface,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[Interval; 3]>, GeometryError> {
    let mut result = [Interval::point(0.0); 3];
    for i in 0..3 {
        work.charge(1, cancel)?;
        let j = (i + 1) % 3;
        let k = (i + 2) % 3;
        let Some(mut component) = carrier.du[j].product(&carrier.dv[k], work, cancel)? else {
            return Ok(None);
        };
        let Some(other) = carrier.du[k].product(&carrier.dv[j], work, cancel)? else {
            return Ok(None);
        };
        if component.subtract(&other).is_none() {
            return Ok(None);
        }
        let Some(component) = exact_interval(&component) else {
            return Ok(None);
        };
        result[i] = component;
    }
    // The actual affine Jacobian normal is this numerator divided by D^2.
    // D^2 is strictly positive, so orientation and all radial/sector signs
    // are preserved without a rounded normalization or sampled surface jet.
    Ok(Some(result))
}

fn exact_affine_surface(
    surface: &Surface,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<ExactAffineSurface>, GeometryError> {
    work.charge(1, cancel)?;
    if let Surface::Plane(plane) = surface {
        let p = plane.defining_points().map(|p| [p.x, p.y, p.z]);
        if p.into_iter().flatten().any(|v| !v.is_finite()) {
            return Ok(None);
        }
        let mut du = std::array::from_fn(|_| ExactCoefficient::point(0.0));
        let mut dv = std::array::from_fn(|_| ExactCoefficient::point(0.0));
        match plane.stored_axes() {
            Some(axes) => {
                let axes = axes.map(|v| [v.x, v.y, v.z]);
                if axes.into_iter().flatten().any(|v| !v.is_finite()) {
                    return Ok(None);
                }
                du = axes[0].map(ExactCoefficient::point);
                dv = axes[1].map(ExactCoefficient::point);
            }
            None => {
                for i in 0..3 {
                    let Some(a) = ExactCoefficient::difference(p[1][i], p[0][i]) else {
                        return Ok(None);
                    };
                    let Some(b) = ExactCoefficient::difference(p[2][i], p[0][i]) else {
                        return Ok(None);
                    };
                    du[i] = a;
                    dv[i] = b;
                }
            }
        }
        return Ok(Some(ExactAffineSurface {
            origin: p[0].map(ExactCoefficient::point),
            du,
            dv,
            denominator: ExactCoefficient::point(1.0),
            support: None,
        }));
    }
    let (knots, degree, count): ([&[f64]; 2], [usize; 2], [usize; 2]) = match surface {
        Surface::BsplineSurface(s) => (
            [s.knot_vector_u().as_ref(), s.knot_vector_v().as_ref()],
            [s.udegree(), s.vdegree()],
            [s.control_points().len(), s.control_points()[0].len()],
        ),
        Surface::NurbsSurface(s) => (
            [s.knot_vector_u().as_ref(), s.knot_vector_v().as_ref()],
            [s.udegree(), s.vdegree()],
            [s.control_points().len(), s.control_points()[0].len()],
        ),
        _ => return Ok(None),
    };
    if degree.contains(&0) || count.into_iter().any(|n| n < 2) {
        return Ok(None);
    }
    let mut support = [[0.0; 2]; 2];
    for axis in 0..2 {
        for pair in knots[axis].windows(2) {
            work.charge(1, cancel)?;
            if !pair[0].is_finite() || !pair[1].is_finite() || pair[0] > pair[1] {
                return Ok(None);
            }
        }
        let (Some(&lower), Some(&upper)) =
            (knots[axis].get(degree[axis]), knots[axis].get(count[axis]))
        else {
            return Ok(None);
        };
        if lower >= upper {
            return Ok(None);
        }
        support[axis] = [lower, upper];
    }
    let control = |u, v| match surface {
        Surface::BsplineSurface(s) => {
            let p = s.control_point(u, v);
            [p.x, p.y, p.z, 1.0]
        }
        Surface::NurbsSurface(s) => (*s.control_point(u, v)).into(),
        _ => unreachable!(),
    };
    let base = control(0, 0);
    let last_u = control(count[0] - 1, 0);
    let last_v = control(0, count[1] - 1);
    if base
        .into_iter()
        .chain(last_u)
        .chain(last_v)
        .any(|v| !v.is_finite())
        || base[3] == 0.0
    {
        return Ok(None);
    }
    let mut first = std::array::from_fn::<_, 2, _>(|_| ExactCoefficient::point(0.0));
    let mut span = std::array::from_fn::<_, 2, _>(|_| ExactCoefficient::point(0.0));
    for axis in 0..2 {
        let Some(begin) = exact_knot_sum(knots[axis], 0, degree[axis], work, cancel)? else {
            return Ok(None);
        };
        let Some(mut end) =
            exact_knot_sum(knots[axis], count[axis] - 1, degree[axis], work, cancel)?
        else {
            return Ok(None);
        };
        if end.subtract(&begin).is_none() || end.sign() <= 0 {
            return Ok(None);
        }
        first[axis] = begin;
        span[axis] = end;
    }
    let Some(area) = span[0].product(&span[1], work, cancel)? else {
        return Ok(None);
    };
    let Some(denominator) = area.product(&ExactCoefficient::point(base[3]), work, cancel)? else {
        return Ok(None);
    };
    let mut delta_u = std::array::from_fn::<_, 3, _>(|_| ExactCoefficient::point(0.0));
    let mut delta_v = std::array::from_fn::<_, 3, _>(|_| ExactCoefficient::point(0.0));
    for i in 0..3 {
        let Some(u) = ExactCoefficient::difference(last_u[i], base[i]) else {
            return Ok(None);
        };
        let Some(v) = ExactCoefficient::difference(last_v[i], base[i]) else {
            return Ok(None);
        };
        delta_u[i] = u;
        delta_v[i] = v;
    }
    // Linear precision: sum_i B_i(u)*Greville_i = u. Verify EVERY
    // homogeneous coefficient against that identity, with constant weight;
    // degree elevation, nonunit domains and internal knots need no fitting.
    for u in 0..count[0] {
        let Some(mut su) = exact_knot_sum(knots[0], u, degree[0], work, cancel)? else {
            return Ok(None);
        };
        if su.subtract(&first[0]).is_none() {
            return Ok(None);
        }
        for v in 0..count[1] {
            work.charge(1, cancel)?;
            let p = control(u, v);
            if p.into_iter().any(|v| !v.is_finite()) || p[3] != base[3] {
                return Ok(None);
            }
            let Some(mut sv) = exact_knot_sum(knots[1], v, degree[1], work, cancel)? else {
                return Ok(None);
            };
            if sv.subtract(&first[1]).is_none() {
                return Ok(None);
            }
            for i in 0..3 {
                let Some(difference) = ExactCoefficient::difference(p[i], base[i]) else {
                    return Ok(None);
                };
                let Some(mut residual) = difference.product(&area, work, cancel)? else {
                    return Ok(None);
                };
                let Some(ut) = delta_u[i].product(&su, work, cancel)? else {
                    return Ok(None);
                };
                let Some(ut) = ut.product(&span[1], work, cancel)? else {
                    return Ok(None);
                };
                let Some(vt) = delta_v[i].product(&sv, work, cancel)? else {
                    return Ok(None);
                };
                let Some(vt) = vt.product(&span[0], work, cancel)? else {
                    return Ok(None);
                };
                if residual.subtract(&ut).is_none()
                    || residual.subtract(&vt).is_none()
                    || residual.sign() != 0
                {
                    return Ok(None);
                }
            }
        }
    }
    let mut result = ExactAffineSurface {
        origin: std::array::from_fn(|_| ExactCoefficient::point(0.0)),
        du: std::array::from_fn(|_| ExactCoefficient::point(0.0)),
        dv: std::array::from_fn(|_| ExactCoefficient::point(0.0)),
        denominator,
        support: Some(support),
    };
    for i in 0..3 {
        let Some(mut origin) = ExactCoefficient::point(base[i]).product(&area, work, cancel)?
        else {
            return Ok(None);
        };
        let Some(u0) = delta_u[i].product(&first[0], work, cancel)? else {
            return Ok(None);
        };
        let Some(u0) = u0.product(&span[1], work, cancel)? else {
            return Ok(None);
        };
        let Some(v0) = delta_v[i].product(&first[1], work, cancel)? else {
            return Ok(None);
        };
        let Some(v0) = v0.product(&span[0], work, cancel)? else {
            return Ok(None);
        };
        if origin.subtract(&u0).is_none() || origin.subtract(&v0).is_none() {
            return Ok(None);
        }
        let Some(du) =
            delta_u[i].product(&ExactCoefficient::point(degree[0] as f64), work, cancel)?
        else {
            return Ok(None);
        };
        let Some(du) = du.product(&span[1], work, cancel)? else {
            return Ok(None);
        };
        let Some(dv) =
            delta_v[i].product(&ExactCoefficient::point(degree[1] as f64), work, cancel)?
        else {
            return Ok(None);
        };
        let Some(dv) = dv.product(&span[0], work, cancel)? else {
            return Ok(None);
        };
        result.origin[i] = origin;
        result.du[i] = du;
        result.dv[i] = dv;
    }
    Ok(Some(result))
}

fn exact_knot_sum(
    knots: &[f64],
    index: usize,
    degree: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<ExactCoefficient>, GeometryError> {
    let Some(values) = knots.get(index + 1..index + degree + 1) else {
        return Ok(None);
    };
    let mut sum = ExactCoefficient::point(0.0);
    for &value in values {
        work.charge(1, cancel)?;
        if !value.is_finite() || sum.push(value).is_none() {
            return Ok(None);
        }
    }
    Ok(Some(sum))
}

/// Prove affine dependence on the original UV curve parameter by complete
/// coefficients. This does not assert a nonzero tangent, rank, original owner
/// or material-sector membership; those remain the caller's obligations.
/// Every retained affine pose and exact declared reflection is included.
pub(super) fn original_uv_affine(
    mut curve: &monstertruck_modeling::Curve2D,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    use monstertruck_modeling::Curve2D;
    work.charge(1, cancel)?;
    let range = curve.range_tuple();
    if !range.0.is_finite() || !range.1.is_finite() || range.0 >= range.1 {
        return Ok(false);
    }
    let mut ends = [range.0, range.1].map(ExactCoefficient::point);
    let mut depth = 0;
    while let Curve2D::TrimmedCurve(wrapper) = curve {
        work.charge(1, cancel)?;
        if depth == 32 {
            return Err(GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "original affine UV wrapper nesting exceeds source limit",
            ));
        }
        depth += 1;
        let matrix = wrapper.transform();
        for col in 0..3 {
            for row in 0..3 {
                work.charge(1, cancel)?;
                let value = matrix[col][row];
                if !value.is_finite() || (row == 2 && value != if col == 2 { 1.0 } else { 0.0 }) {
                    return Ok(false);
                }
            }
        }
        let (a, b) = wrapper.entity().range_tuple();
        if !a.is_finite() || !b.is_finite() || a >= b || !exact_parameters_inside(&ends, [a, b]) {
            return Ok(false);
        }
        if !wrapper.orientation() {
            for end in &mut ends {
                let mut reflected = ExactCoefficient::point(a);
                if reflected.push(b).is_none() || reflected.subtract(end).is_none() {
                    return Ok(false);
                }
                *end = reflected;
            }
        }
        curve = wrapper.entity().curve().as_ref();
    }
    if let Curve2D::Line(line) = curve {
        return Ok([line.0.x, line.0.y, line.1.x, line.1.y]
            .into_iter()
            .all(f64::is_finite));
    }
    let (knots, degree, count): (&[f64], usize, usize) = match curve {
        Curve2D::BsplineCurve(c) => (
            c.knot_vector().as_ref(),
            c.degree(),
            c.control_points().len(),
        ),
        Curve2D::NurbsCurve(c) => (
            c.knot_vector().as_ref(),
            c.degree(),
            c.control_points().len(),
        ),
        _ => return Ok(false),
    };
    for pair in knots.windows(2) {
        work.charge(1, cancel)?;
        if !pair[0].is_finite() || !pair[1].is_finite() || pair[0] > pair[1] {
            return Ok(false);
        }
    }
    let (Some(&lower), Some(&upper)) = (knots.get(degree), knots.get(count)) else {
        return Ok(false);
    };
    if count == 0 || lower >= upper || !exact_parameters_inside(&ends, [lower, upper]) {
        return Ok(false);
    }
    let control = |index: usize| -> [f64; 3] {
        match curve {
            Curve2D::BsplineCurve(c) => {
                let p: monstertruck_modeling::Point2 = c.control_points()[index];
                [p.x, p.y, 1.0]
            }
            Curve2D::NurbsCurve(c) => {
                let p: monstertruck_modeling::base::Vector3 = c.control_points()[index];
                [p.x, p.y, p.z]
            }
            _ => unreachable!(),
        }
    };
    let base = control(0);
    if base.into_iter().any(|v| !v.is_finite()) || base[2] == 0.0 {
        return Ok(false);
    }
    if degree == 0 {
        for index in 0..count {
            work.charge(1, cancel)?;
            if control(index) != base {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    let Some(first) = exact_knot_sum(knots, 0, degree, work, cancel)? else {
        return Ok(false);
    };
    let Some(mut span) = exact_knot_sum(knots, count - 1, degree, work, cancel)? else {
        return Ok(false);
    };
    if span.subtract(&first).is_none() || span.sign() <= 0 {
        return Ok(false);
    }
    let last = control(count - 1);
    let mut delta = std::array::from_fn::<_, 2, _>(|_| ExactCoefficient::point(0.0));
    for axis in 0..2 {
        let Some(difference) = ExactCoefficient::difference(last[axis], base[axis]) else {
            return Ok(false);
        };
        delta[axis] = difference;
    }
    for index in 0..count {
        work.charge(1, cancel)?;
        let point = control(index);
        if point.into_iter().any(|v| !v.is_finite()) || point[2] != base[2] {
            return Ok(false);
        }
        let Some(mut coordinate) = exact_knot_sum(knots, index, degree, work, cancel)? else {
            return Ok(false);
        };
        if coordinate.subtract(&first).is_none() {
            return Ok(false);
        }
        for axis in 0..2 {
            let Some(difference) = ExactCoefficient::difference(point[axis], base[axis]) else {
                return Ok(false);
            };
            let Some(mut residual) = difference.product(&span, work, cancel)? else {
                return Ok(false);
            };
            let Some(reference) = delta[axis].product(&coordinate, work, cancel)? else {
                return Ok(false);
            };
            if residual.subtract(&reference).is_none() || residual.sign() != 0 {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Prove equality of two original affine UV endpoint expressions. Homogeneous
/// cross products avoid rounding rational coordinates or interpreting an
/// interval that merely contains zero as an identity. False means unproved,
/// including unsupported/nonaffine operators, not a proof of inequality.
pub(super) fn original_uv_endpoints_equal(
    first: &monstertruck_modeling::Curve2D,
    first_parameter: f64,
    second: &monstertruck_modeling::Curve2D,
    second_parameter: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(1, cancel)?;
    let Some(first) = original_affine_uv_point(first, first_parameter, work, cancel)? else {
        return Ok(false);
    };
    let Some(second) = original_affine_uv_point(second, second_parameter, work, cancel)? else {
        return Ok(false);
    };
    if first[2].sign() == 0 || second[2].sign() == 0 {
        return Ok(false);
    }
    for axis in 0..2 {
        work.charge(1, cancel)?;
        let Some(mut left) = first[axis].product(&second[2], work, cancel)? else {
            return Ok(false);
        };
        let Some(right) = second[axis].product(&first[2], work, cancel)? else {
            return Ok(false);
        };
        if left.subtract(&right).is_none() || left.sign() != 0 {
            return Ok(false);
        }
    }
    Ok(true)
}

fn original_affine_uv_point(
    curve: &monstertruck_modeling::Curve2D,
    parameter: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[ExactCoefficient; 3]>, GeometryError> {
    if !original_uv_affine(curve, work, cancel)? {
        return Ok(None);
    }
    affine_uv_point(curve, parameter, work, cancel)
}

/// Evaluate an already coefficient-proved affine original chart expression.
/// Retain its homogeneous denominator through every original pose.
fn affine_uv_point(
    mut curve: &monstertruck_modeling::Curve2D,
    parameter: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[ExactCoefficient; 3]>, GeometryError> {
    use monstertruck_modeling::Curve2D;
    if !parameter.is_finite() {
        return Ok(None);
    }
    let range = curve.range_tuple();
    if parameter < range.0 || parameter > range.1 {
        return Ok(None);
    }
    let mut parameter = ExactCoefficient::point(parameter);
    let mut operators = [None; 32];
    let mut depth = 0;
    while let Curve2D::TrimmedCurve(wrapper) = curve {
        work.charge(1, cancel)?;
        if depth == operators.len() {
            return Err(GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "original affine UV endpoint wrapper nesting exceeds source limit",
            ));
        }
        if !wrapper.orientation() {
            let (a, b) = wrapper.entity().range_tuple();
            let mut reflected = ExactCoefficient::point(a);
            if reflected.push(b).is_none() || reflected.subtract(&parameter).is_none() {
                return Ok(None);
            }
            parameter = reflected;
        }
        operators[depth] = Some(wrapper.transform());
        depth += 1;
        curve = wrapper.entity().curve().as_ref();
    }
    let mut point = std::array::from_fn::<_, 3, _>(|_| ExactCoefficient::point(0.0));
    if let Curve2D::Line(line) = curve {
        let a = [line.0.x, line.0.y];
        let b = [line.1.x, line.1.y];
        for axis in 0..2 {
            let Some(delta) = ExactCoefficient::difference(b[axis], a[axis]) else {
                return Ok(None);
            };
            let Some(mut value) = delta.product(&parameter, work, cancel)? else {
                return Ok(None);
            };
            if value.push(a[axis]).is_none() {
                return Ok(None);
            }
            point[axis] = value;
        }
        point[2] = ExactCoefficient::point(1.0);
    } else {
        let (knots, degree, count): (&[f64], usize, usize) = match curve {
            Curve2D::BsplineCurve(c) => (
                c.knot_vector().as_ref(),
                c.degree(),
                c.control_points().len(),
            ),
            Curve2D::NurbsCurve(c) => (
                c.knot_vector().as_ref(),
                c.degree(),
                c.control_points().len(),
            ),
            _ => return Ok(None),
        };
        let control = |index: usize| -> [f64; 3] {
            match curve {
                Curve2D::BsplineCurve(c) => {
                    let p: monstertruck_modeling::Point2 = c.control_points()[index];
                    [p.x, p.y, 1.0]
                }
                Curve2D::NurbsCurve(c) => {
                    let p: monstertruck_modeling::base::Vector3 = c.control_points()[index];
                    [p.x, p.y, p.z]
                }
                _ => unreachable!(),
            }
        };
        let base = control(0);
        if degree == 0 {
            point = base.map(ExactCoefficient::point);
        } else {
            let Some(first) = exact_knot_sum(knots, 0, degree, work, cancel)? else {
                return Ok(None);
            };
            let Some(mut span) = exact_knot_sum(knots, count - 1, degree, work, cancel)? else {
                return Ok(None);
            };
            if span.subtract(&first).is_none() {
                return Ok(None);
            }
            let Some(mut coordinate) =
                parameter.product(&ExactCoefficient::point(degree as f64), work, cancel)?
            else {
                return Ok(None);
            };
            if coordinate.subtract(&first).is_none() {
                return Ok(None);
            }
            let last = control(count - 1);
            for axis in 0..2 {
                let Some(delta) = ExactCoefficient::difference(last[axis], base[axis]) else {
                    return Ok(None);
                };
                let Some(mut numerator) = delta.product(&coordinate, work, cancel)? else {
                    return Ok(None);
                };
                let Some(origin) =
                    ExactCoefficient::point(base[axis]).product(&span, work, cancel)?
                else {
                    return Ok(None);
                };
                if !add_exact(&mut numerator, &origin) {
                    return Ok(None);
                }
                point[axis] = numerator;
            }
            let Some(weight) = ExactCoefficient::point(base[2]).product(&span, work, cancel)?
            else {
                return Ok(None);
            };
            point[2] = weight;
        }
    }
    for matrix in operators[..depth].iter().rev() {
        let Some(matrix) = matrix else {
            return Ok(None);
        };
        let mut transformed = std::array::from_fn::<_, 3, _>(|_| ExactCoefficient::point(0.0));
        for row in 0..3 {
            for (col, coordinate) in point.iter().enumerate() {
                work.charge(1, cancel)?;
                let Some(term) =
                    coordinate.product(&ExactCoefficient::point(matrix[col][row]), work, cancel)?
                else {
                    return Ok(None);
                };
                if !add_exact(&mut transformed[row], &term) {
                    return Ok(None);
                }
            }
        }
        point = transformed;
    }
    Ok(Some(point))
}

fn exact_parameters_inside(parameters: &[ExactCoefficient; 2], bounds: [f64; 2]) -> bool {
    parameters.iter().all(|parameter| {
        let mut lower = parameter.clone();
        let mut upper = parameter.clone();
        lower.push(-bounds[0]).is_some()
            && lower.sign() >= 0
            && upper.push(-bounds[1]).is_some()
            && upper.sign() <= 0
    })
}

/// Evaluate the actual affine coefficients at the two retained parameters.
/// Wrapper reflection anchors and each original matrix remain separate; no
/// rounded composed pose, numeric endpoint sample or chord replaces the curve.
fn affine_curve_endpoints(
    mut curve: &Curve,
    parameters: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[[ExactCoefficient; 3]; 2]>, GeometryError> {
    if parameters.into_iter().any(|v| !v.is_finite()) {
        return Ok(None);
    }
    let mut parameters = parameters.map(ExactCoefficient::point);
    let mut operators = [None; 32];
    let mut depth = 0;
    let (a, b) = loop {
        work.charge(1, cancel)?;
        let next = match curve {
            Curve::Line(line) => {
                break (
                    [line.0.x, line.0.y, line.0.z],
                    [line.1.x, line.1.y, line.1.z],
                );
            }
            Curve::TrimmedCurve(wrapper) => (
                wrapper.entity().curve().as_ref(),
                wrapper.entity().range_tuple(),
                !wrapper.orientation(),
            ),
            _ => return Ok(None),
        };
        if depth == operators.len() {
            return Err(GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "exact affine curve wrapper nesting exceeds original source limit",
            ));
        }
        let Curve::TrimmedCurve(wrapper) = curve else {
            return Ok(None);
        };
        let matrix = wrapper.transform();
        for row in 0..4 {
            for col in 0..4 {
                work.charge(1, cancel)?;
                let value = matrix[col][row];
                if !value.is_finite() || (row == 3 && value != if col == 3 { 1.0 } else { 0.0 }) {
                    return Ok(None);
                }
            }
        }
        if !next.1.0.is_finite() || !next.1.1.is_finite() {
            return Ok(None);
        }
        if next.2 {
            for parameter in &mut parameters {
                let mut reflected = ExactCoefficient::point(next.1.0);
                if reflected.push(next.1.1).is_none() || reflected.subtract(parameter).is_none() {
                    return Ok(None);
                }
                *parameter = reflected;
            }
        }
        operators[depth] = Some(matrix);
        depth += 1;
        curve = next.0;
    };
    if a.into_iter().chain(b).any(|v| !v.is_finite()) {
        return Ok(None);
    }
    let mut points = std::array::from_fn::<_, 2, _>(|_| {
        std::array::from_fn::<_, 3, _>(|_| ExactCoefficient::point(0.0))
    });
    for endpoint in 0..2 {
        for i in 0..3 {
            let Some(delta) = ExactCoefficient::difference(b[i], a[i]) else {
                return Ok(None);
            };
            let Some(mut value) = delta.product(&parameters[endpoint], work, cancel)? else {
                return Ok(None);
            };
            if value.push(a[i]).is_none() {
                return Ok(None);
            }
            points[endpoint][i] = value;
        }
    }
    for operator in operators[..depth].iter().rev() {
        let Some(operator) = operator else {
            return Ok(None);
        };
        for point in &mut points {
            let mut transformed = std::array::from_fn(|_| ExactCoefficient::point(0.0));
            for row in 0..3 {
                let mut value = ExactCoefficient::point(operator[3][row]);
                for (col, coordinate) in point.iter().enumerate() {
                    let Some(term) = coordinate.product(
                        &ExactCoefficient::point(operator[col][row]),
                        work,
                        cancel,
                    )?
                    else {
                        return Ok(None);
                    };
                    if !add_exact(&mut value, &term) {
                        return Ok(None);
                    }
                }
                transformed[row] = value;
            }
            *point = transformed;
        }
    }
    Ok(Some(points))
}

fn affine_line_direction(
    curve: &Curve,
    front: [f64; 3],
    back: [f64; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[Interval; 3]>, GeometryError> {
    if front == back || front.into_iter().chain(back).any(|v| !v.is_finite()) {
        return Ok(None);
    }
    let range = curve.range_tuple();
    if !range.0.is_finite() || !range.1.is_finite() || range.0 >= range.1 {
        return Ok(None);
    }
    let Some(points) = affine_curve_endpoints(curve, [range.0, range.1], work, cancel)? else {
        return Ok(None);
    };
    for (endpoint, target) in points.iter().zip([front, back]) {
        for i in 0..3 {
            let mut residual = endpoint[i].clone();
            if residual.push(-target[i]).is_none() || residual.sign() != 0 {
                return Ok(None);
            }
        }
    }
    // The coefficient-proved affine derivative is this vector divided by the
    // strictly positive retained parameter span. Positive scaling preserves
    // its entire angular cone, without evaluating a native tangent sample.
    Ok(Some(std::array::from_fn(|i| {
        Interval::point(back[i]).sub(Interval::point(front[i]))
    })))
}

fn exact_affine_trace(
    carrier: &ExactAffineSurface,
    chart: &monstertruck_modeling::Curve2D,
    parameters: [f64; 2],
    targets: [[f64; 3]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    // Native's exact planar projection of even a LINE is a constant-weight
    // rational chart. Its homogeneous UV coefficients, not its enum variant
    // or rounded dehomogenized endpoints, determine the composed identity.
    if parameters[0] == parameters[1] || !original_uv_affine(chart, work, cancel)? {
        return Ok(false);
    }
    for (parameter, target) in parameters.into_iter().zip(targets) {
        let Some(uv) = affine_uv_point(chart, parameter, work, cancel)? else {
            return Ok(false);
        };
        let weight_sign = uv[2].sign();
        if weight_sign == 0 {
            return Ok(false);
        }
        if let Some(support) = carrier.support {
            for axis in 0..2 {
                work.charge(1, cancel)?;
                let Some(lower_bound) =
                    uv[2].product(&ExactCoefficient::point(support[axis][0]), work, cancel)?
                else {
                    return Ok(false);
                };
                let Some(upper_bound) =
                    uv[2].product(&ExactCoefficient::point(support[axis][1]), work, cancel)?
                else {
                    return Ok(false);
                };
                let mut lower = uv[axis].clone();
                let mut upper = uv[axis].clone();
                if lower.subtract(&lower_bound).is_none()
                    || lower.sign() * weight_sign < 0
                    || upper.subtract(&upper_bound).is_none()
                    || upper.sign() * weight_sign > 0
                {
                    return Ok(false);
                }
            }
        }
        let Some(denominator) = carrier.denominator.product(&uv[2], work, cancel)? else {
            return Ok(false);
        };
        for (i, coordinate) in target.iter().enumerate() {
            work.charge(1, cancel)?;
            let Some(mut residual) = carrier.origin[i].product(&uv[2], work, cancel)? else {
                return Ok(false);
            };
            let Some(u) = carrier.du[i].product(&uv[0], work, cancel)? else {
                return Ok(false);
            };
            let Some(v) = carrier.dv[i].product(&uv[1], work, cancel)? else {
                return Ok(false);
            };
            let Some(reference) =
                denominator.product(&ExactCoefficient::point(*coordinate), work, cancel)?
            else {
                return Ok(false);
            };
            if !add_exact(&mut residual, &u)
                || !add_exact(&mut residual, &v)
                || residual.subtract(&reference).is_none()
                || residual.sign() != 0
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn hull_vertex_support(support: &mut Option<[Interval; 3]>, image: [Interval; 3]) {
    *support = Some(match *support {
        Some(previous) => std::array::from_fn(|axis| previous[axis].hull(image[axis])),
        None => image,
    });
}

fn endpoint_support_span(range: [f64; 2], high: bool) -> Result<[f64; 2], GeometryError> {
    if !range[0].is_finite() || !range[1].is_finite() || range[0] >= range[1] {
        return Err(GeometryError::new(
            GeometryErrorCode::ConditioningFailure,
            "original endpoint support has no finite increasing source range",
        ));
    }
    let width = Interval::point(range[1]).sub(Interval::point(range[0]));
    let fraction = width.mul(Interval::point(1.0 / 1024.0));
    let span = if high {
        [
            Interval::point(range[1]).sub(fraction).lo.max(range[0]),
            range[1],
        ]
    } else {
        [
            range[0],
            Interval::point(range[0]).add(fraction).hi.min(range[1]),
        ]
    };
    if !fraction.hi.is_finite() || span[0] >= span[1] {
        return Err(GeometryError::new(
            GeometryErrorCode::ConditioningFailure,
            "original endpoint support fraction cannot be enclosed",
        ));
    }
    Ok(span)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_uv_endpoint_identity_preserves_rational_values_and_rejects_rounding() {
        use monstertruck_modeling::base::Vector3;
        use monstertruck_modeling::{BsplineCurve, Curve2D, KnotVector, Line, NurbsCurve, Point2};
        let rational = |scale| {
            Curve2D::NurbsCurve(NurbsCurve::new(BsplineCurve::new(
                KnotVector::bezier_knot(1),
                vec![
                    Vector3::new(scale, 0.0, 3.0 * scale),
                    Vector3::new(4.0 * scale, 0.0, 3.0 * scale),
                ],
            )))
        };
        let first = rational(1.0);
        let scaled = rational(2.0);
        let rounded = Point2::new(5.0 / 6.0, 0.0);
        let rounded = Curve2D::Line(Line(rounded, rounded));
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        assert!(
            original_uv_endpoints_equal(&first, 0.5, &scaled, 0.5, &mut work, &cancel,).unwrap()
        );
        assert!(
            !original_uv_endpoints_equal(&first, 0.5, &rounded, 0.0, &mut work, &cancel,).unwrap()
        );
        assert!(
            !original_uv_endpoints_equal(&first, 0.0, &scaled, 1.0, &mut work, &cancel,).unwrap()
        );
        let polynomial = Curve2D::BsplineCurve(BsplineCurve::new(
            KnotVector::bezier_knot(2),
            vec![
                Point2::new(0.0, 0.0),
                Point2::new(1.0, 1.5),
                Point2::new(2.0, 3.0),
            ],
        ));
        let line = Curve2D::Line(Line(Point2::new(0.0, 0.0), Point2::new(2.0, 3.0)));
        assert!(
            original_uv_endpoints_equal(&polynomial, 0.5, &line, 0.5, &mut work, &cancel,).unwrap()
        );
    }

    #[test]
    fn original_uv_endpoint_identity_tracks_actual_declared_reflection_and_pose() {
        use monstertruck_modeling::base::{Invertible, Matrix3, Vector3};
        use monstertruck_modeling::{Curve2D, Line, Point2, Processor, TrimmedCurve};
        let mut wrapper = Processor::with_transform(
            TrimmedCurve::new(
                Box::new(Curve2D::Line(Line(
                    Point2::new(0.0, 0.0),
                    Point2::new(1.0, 2.0),
                ))),
                (2.0, 5.0),
            ),
            Matrix3::from_cols(
                Vector3::new(1.0, 0.0, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
                Vector3::new(10.0, 20.0, 1.0),
            ),
        );
        wrapper.invert();
        let curve = Curve2D::TrimmedCurve(wrapper);
        let point = Point2::new(15.0, 30.0);
        let point = Curve2D::Line(Line(point, point));
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        assert!(
            original_uv_endpoints_equal(&curve, 2.0, &point, 0.0, &mut work, &cancel,).unwrap()
        );
        assert!(
            !original_uv_endpoints_equal(&curve, 5.0, &point, 0.0, &mut work, &cancel,).unwrap()
        );
        assert_eq!(
            original_uv_endpoints_equal(
                &curve,
                2.0,
                &point,
                0.0,
                &mut GeometryWorkBudget::new(0),
                &cancel,
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::ResourceLimit
        );
        assert_eq!(
            original_uv_endpoints_equal(
                &curve,
                2.0,
                &point,
                0.0,
                &mut work,
                &AtomicBool::new(true),
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::Cancelled
        );
    }

    #[test]
    fn original_uv_affine_uses_every_polynomial_and_homogeneous_coefficient() {
        use monstertruck_modeling::base::Vector3;
        use monstertruck_modeling::{BsplineCurve, Curve2D, KnotVector, NurbsCurve, Point2};
        let knots = KnotVector::from(vec![-2.0, -2.0, -2.0, 0.0, 4.0, 4.0, 4.0]);
        let points: Vec<_> = [-2.0, -1.0, 2.0, 4.0]
            .into_iter()
            .map(|u| Point2::new(u + 2.0, 2.0 * u + 1.0))
            .collect();
        let polynomial = Curve2D::BsplineCurve(BsplineCurve::new(knots.clone(), points.clone()));
        let weighted: Vec<_> = points
            .iter()
            .map(|p| Vector3::new(2.0 * p.x, 2.0 * p.y, 2.0))
            .collect();
        let rational = Curve2D::NurbsCurve(NurbsCurve::new(BsplineCurve::new(
            knots.clone(),
            weighted.clone(),
        )));
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        assert!(original_uv_affine(&polynomial, &mut work, &cancel).unwrap());
        assert!(original_uv_affine(&rational, &mut work, &cancel).unwrap());
        let mut bent = points;
        bent[1].x = bent[1].x.next_up();
        let bent = Curve2D::BsplineCurve(BsplineCurve::new(knots.clone(), bent));
        assert!(!original_uv_affine(&bent, &mut work, &cancel).unwrap());
        let mut nonconstant_weight = weighted;
        nonconstant_weight[1].z = 3.0;
        let nonconstant_weight = Curve2D::NurbsCurve(NurbsCurve::new(BsplineCurve::new(
            knots,
            nonconstant_weight,
        )));
        assert!(!original_uv_affine(&nonconstant_weight, &mut work, &cancel).unwrap());
    }

    #[test]
    fn original_uv_affine_preserves_declared_wrappers_domains_and_typed_limits() {
        use monstertruck_modeling::base::{Invertible, Matrix3, Vector3};
        use monstertruck_modeling::{
            BsplineCurve, Curve2D, KnotVector, Point2, Processor, TrimmedCurve,
        };
        let basis = Curve2D::BsplineCurve(BsplineCurve::new(
            KnotVector::from(vec![-2.0, -2.0, 4.0, 4.0]),
            vec![Point2::new(-1.0, -3.0), Point2::new(5.0, 9.0)],
        ));
        let mut wrapper = Processor::with_transform(
            TrimmedCurve::new(Box::new(basis.clone()), (-1.0, 2.0)),
            Matrix3::from_cols(
                Vector3::new(2.0, 0.0, 0.0),
                Vector3::new(0.0, -1.0, 0.0),
                Vector3::new(3.0, 4.0, 1.0),
            ),
        );
        wrapper.invert();
        let wrapped = Curve2D::TrimmedCurve(wrapper);
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        assert!(original_uv_affine(&wrapped, &mut work, &cancel).unwrap());
        let outside = Curve2D::TrimmedCurve(Processor::new(TrimmedCurve::new(
            Box::new(basis.clone()),
            (-3.0, 1.0),
        )));
        assert!(!original_uv_affine(&outside, &mut work, &cancel).unwrap());
        let projective = Curve2D::TrimmedCurve(Processor::with_transform(
            TrimmedCurve::new(Box::new(basis), (-1.0, 2.0)),
            Matrix3::from_cols(
                Vector3::new(1.0, 0.0, 0.25),
                Vector3::new(0.0, 1.0, 0.0),
                Vector3::new(0.0, 0.0, 1.0),
            ),
        ));
        assert!(!original_uv_affine(&projective, &mut work, &cancel).unwrap());
        assert_eq!(
            original_uv_affine(&wrapped, &mut GeometryWorkBudget::new(0), &cancel,)
                .unwrap_err()
                .code,
            GeometryErrorCode::ResourceLimit
        );
        assert_eq!(
            original_uv_affine(&wrapped, &mut work, &AtomicBool::new(true),)
                .unwrap_err()
                .code,
            GeometryErrorCode::Cancelled
        );
    }

    #[test]
    fn exact_line_lane_uses_coefficient_identity_not_endpoint_nearness() {
        use monstertruck_modeling::{Line, Point3};
        let curve = Curve::Line(Line(Point3::new(0.0, 0.0, 0.0), Point3::new(2.0, 3.0, 4.0)));
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        assert!(
            affine_line_direction(&curve, [0.0; 3], [2.0, 3.0, 4.0], &mut work, &cancel,)
                .unwrap()
                .is_some()
        );
        assert!(
            affine_line_direction(
                &curve,
                [f64::from_bits(1), 0.0, 0.0],
                [2.0, 3.0, 4.0],
                &mut work,
                &cancel,
            )
            .unwrap()
            .is_none()
        );
        assert!(
            affine_line_direction(&curve, [2.0, 3.0, 4.0], [0.0; 3], &mut work, &cancel,)
                .unwrap()
                .is_none()
        );
        let zero = Curve::Line(Line(Point3::new(0.0, 0.0, 0.0), Point3::new(0.0, 0.0, 0.0)));
        assert!(
            affine_line_direction(&zero, [0.0; 3], [0.0; 3], &mut work, &cancel,)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn complete_affine_tensor_coefficients_cover_nonunit_knots_and_constant_weights() {
        use monstertruck_modeling::base::Vector4;
        use monstertruck_modeling::{
            BsplineSurface, Curve2D, KnotVector, Line, NurbsSurface, Point2, Point3,
        };
        let knots = (
            KnotVector::from(vec![-2.0, -2.0, -2.0, 0.0, 4.0, 4.0, 4.0]),
            KnotVector::from(vec![1.0, 1.0, 3.0, 3.0]),
        );
        let points: Vec<Vec<Point3>> = [-2.0, -1.0, 2.0, 4.0]
            .into_iter()
            .map(|u| {
                [1.0, 3.0]
                    .into_iter()
                    .map(|v| Point3::new(u + 2.0, v - 1.0, u + 2.0 * v))
                    .collect()
            })
            .collect();
        let polynomial =
            Surface::BsplineSurface(BsplineSurface::new(knots.clone(), points.clone()));
        let weighted = points
            .iter()
            .map(|row| {
                row.iter()
                    .map(|p| Vector4::new(2.0 * p.x, 2.0 * p.y, 2.0 * p.z, 2.0))
                    .collect()
            })
            .collect();
        let rational = Surface::NurbsSurface(NurbsSurface::new(BsplineSurface::new(
            knots.clone(),
            weighted,
        )));
        let trace = Curve2D::Line(Line(Point2::new(-2.0, 1.0), Point2::new(4.0, 3.0)));
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        for surface in [&polynomial, &rational] {
            let affine = exact_affine_surface(surface, &mut work, &cancel)
                .unwrap()
                .unwrap();
            assert!(
                exact_affine_trace(
                    &affine,
                    &trace,
                    [0.0, 1.0],
                    [[0.0; 3], [6.0, 2.0, 10.0]],
                    &mut work,
                    &cancel,
                )
                .unwrap()
            );
            assert!(
                length(
                    exact_affine_normal(&affine, &mut work, &cancel)
                        .unwrap()
                        .unwrap()
                )
                .unwrap()
                .lo > 0.0
            );
            assert!(
                !exact_affine_trace(
                    &affine,
                    &trace,
                    [0.0, 1.0],
                    [[0.0; 3], [6.0, 2.0, 10.0_f64.next_up()]],
                    &mut work,
                    &cancel,
                )
                .unwrap()
            );
        }
        let mut wrong = points;
        wrong[1][0].z = wrong[1][0].z.next_up();
        let wrong = Surface::BsplineSurface(BsplineSurface::new(knots, wrong));
        assert!(
            exact_affine_surface(&wrong, &mut work, &cancel)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn exact_affine_trace_composes_original_homogeneous_uv_coefficients() {
        use monstertruck_modeling::base::{Invertible, Matrix3, Vector3};
        use monstertruck_modeling::{
            BsplineCurve, Curve2D, KnotVector, NurbsCurve, Plane, Point3, Processor, TrimmedCurve,
        };
        let plane = Surface::Plane(Plane::from_origin_vectors(
            Point3::new(10.0, 20.0, 30.0),
            Vector3::new(3.0, 0.0, 0.0),
            Vector3::new(0.0, 6.0, 0.0),
        ));
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let affine = exact_affine_surface(&plane, &mut work, &cancel)
            .unwrap()
            .unwrap();
        let chart = |scale| {
            Curve2D::NurbsCurve(NurbsCurve::new(BsplineCurve::new(
                KnotVector::from(vec![-2.0, -2.0, 4.0, 4.0]),
                vec![
                    Vector3::new(scale, scale, 3.0 * scale),
                    Vector3::new(4.0 * scale, 4.0 * scale, 3.0 * scale),
                ],
            )))
        };
        for scale in [-2.0, 1.0] {
            assert!(
                exact_affine_trace(
                    &affine,
                    &chart(scale),
                    [-2.0, 4.0],
                    [[11.0, 22.0, 30.0], [14.0, 28.0, 30.0]],
                    &mut work,
                    &cancel,
                )
                .unwrap()
            );
            assert!(
                !exact_affine_trace(
                    &affine,
                    &chart(scale),
                    [-2.0, 4.0],
                    [[11.0, 22.0, 30.0], [14.0_f64.next_up(), 28.0, 30.0]],
                    &mut work,
                    &cancel,
                )
                .unwrap()
            );
        }
        let mut wrapper = Processor::with_transform(
            TrimmedCurve::new(Box::new(chart(1.0)), (-1.0, 2.0)),
            Matrix3::from_cols(
                Vector3::new(1.0, 0.0, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
                Vector3::new(1.0, 2.0, 1.0),
            ),
        );
        wrapper.invert();
        assert!(
            exact_affine_trace(
                &affine,
                &Curve2D::TrimmedCurve(wrapper),
                [-1.0, 2.0],
                [[16.0, 38.0, 30.0], [14.5, 35.0, 30.0]],
                &mut work,
                &cancel,
            )
            .unwrap()
        );
    }

    #[test]
    fn native_planar_rational_chart_has_exact_affine_trace() {
        use monstertruck_modeling::base::Vector3;
        use monstertruck_modeling::{
            Curve2D, Line, Plane, Point3, ValidatedGeometryBudget, certified_planar_parameter_curve,
        };
        let plane = Plane::from_origin_vectors(
            Point3::new(10.0, 20.0, 30.0),
            Vector3::new(3.0, 0.0, 0.0),
            Vector3::new(0.0, 6.0, 0.0),
        );
        let curve = Curve::Line(Line(
            Point3::new(11.0, 22.0, 30.0),
            Point3::new(14.0, 28.0, 30.0),
        ));
        let cancel = AtomicBool::new(false);
        let chart = certified_planar_parameter_curve(
            &curve,
            &plane,
            &mut ValidatedGeometryBudget::new(1_000_000),
            &cancel,
        )
        .unwrap();
        assert!(matches!(&chart, Curve2D::NurbsCurve(_)));
        let mut work = GeometryWorkBudget::new(1_000_000);
        let affine = exact_affine_surface(&Surface::Plane(plane), &mut work, &cancel)
            .unwrap()
            .unwrap();
        assert!(
            exact_affine_trace(
                &affine,
                &chart,
                [0.0, 1.0],
                [[11.0, 22.0, 30.0], [14.0, 28.0, 30.0]],
                &mut work,
                &cancel,
            )
            .unwrap()
        );
    }

    #[test]
    fn affine_line_operators_retain_exact_pose_and_declared_reflection() {
        use monstertruck_modeling::base::{Invertible, Matrix4, Vector3};
        use monstertruck_modeling::{Line, Point3, Processor, TrimmedCurve};
        let mut wrapper = Processor::with_transform(
            TrimmedCurve::new(
                Box::new(Curve::Line(Line(
                    Point3::new(0.0, 0.0, 0.0),
                    Point3::new(1.0, 0.0, 0.0),
                ))),
                (2.0, 5.0),
            ),
            Matrix4::from_translation(Vector3::new(10.0, 2.0, 3.0)),
        );
        wrapper.invert();
        let curve = Curve::TrimmedCurve(wrapper);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        assert!(
            affine_line_direction(
                &curve,
                [15.0, 2.0, 3.0],
                [12.0, 2.0, 3.0],
                &mut work,
                &cancel,
            )
            .unwrap()
            .is_some()
        );
        assert!(
            affine_line_direction(
                &curve,
                [12.0, 2.0, 3.0],
                [15.0, 2.0, 3.0],
                &mut work,
                &cancel,
            )
            .unwrap()
            .is_none()
        );
        let error = affine_line_direction(
            &curve,
            [15.0, 2.0, 3.0],
            [12.0, 2.0, 3.0],
            &mut GeometryWorkBudget::new(0),
            &cancel,
        )
        .unwrap_err();
        assert_eq!(error.code, GeometryErrorCode::ResourceLimit);
        let error = affine_line_direction(
            &curve,
            [15.0, 2.0, 3.0],
            [12.0, 2.0, 3.0],
            &mut work,
            &AtomicBool::new(true),
        )
        .unwrap_err();
        assert_eq!(error.code, GeometryErrorCode::Cancelled);
    }

    #[test]
    fn complete_exact_original_affine_corner_has_positive_active_degree() {
        use monstertruck_io::step::load::step_geometry::Curve3D;
        use monstertruck_modeling::builder;
        let vertices = builder::vertices([
            (0.0, 0.0, 0.0),
            (4.0, 0.0, 0.0),
            (4.0, 3.0, 0.0),
            (0.0, 3.0, 0.0),
        ]);
        let wire = (0..vertices.len())
            .map(|i| builder::line::<Curve3D>(&vertices[i], &vertices[(i + 1) % vertices.len()]))
            .collect();
        let solid = crate::parity_fixtures::extruded(vec![wire], 4.0);
        let text = crate::parity_fixtures::original("exact-affine-corner.step", solid);
        let cancel = AtomicBool::new(false);
        let definition = crate::import_step(text.as_bytes(), &cancel).unwrap();
        let mut work = GeometryWorkBudget::new(100_000_000);
        let prepared = crate::prepare_section_source(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &cancel,
            &mut work,
        )
        .unwrap();
        let request = SlabRequest {
            frame: PlaneFrameMm {
                origin_mm: [0.0; 3],
                x_axis: [1.0, 0.0, 0.0],
                y_axis: [0.0, 1.0, 0.0],
                z_axis: [0.0, 0.0, 1.0],
            },
            height_interval_mm: [0.0, 4.0],
            sample_height_mm: 2.0,
            native_shape_error_mm: 0.1,
        };
        let section = prepared
            .section_with_byte_limit(
                crate::PlaneMm {
                    origin_mm: [0.0, 0.0, 2.0],
                    normal: [0.0, 0.0, 1.0],
                },
                &cancel,
                (crate::MAX_PREPARED_SECTION_BYTES - prepared.retained_bytes()) as u32,
                &mut work,
            )
            .unwrap();
        let prism = SamplePrism::new(
            &section,
            &request,
            HeightFunctional::from_frame(request.frame),
            Interval::point(2.0),
            &mut work,
            &cancel,
        )
        .unwrap();
        let mut incidence =
            SourceEdgeIncidence::new(&definition, MAX_EDGE_BYTES, &mut work, &cancel).unwrap();
        incidence
            .prepare_vertices(MAX_EDGE_BYTES, &mut work, &cancel)
            .unwrap();
        for face in definition.solid.face_iter() {
            for wire in face.absolute_boundaries() {
                for edge in wire {
                    let vertex = edge.front().id();
                    let mut count = 0;
                    for _ in incidence.vertex_edges(vertex) {
                        count += 1;
                    }
                    assert!(
                        exact_affine_vertex_fan(
                            &definition,
                            prepared.trim_source(),
                            &incidence,
                            vertex,
                            count,
                            &prism,
                            MAX_SCRATCH_BYTES,
                            &mut work,
                            &cancel,
                        )
                        .unwrap()
                    );
                }
            }
        }
    }

    #[test]
    fn endpoint_support_recipe_encloses_fixed_fraction_without_query_input() {
        for range in [[0.0, 1.0], [-16.0, 48.0], [1.0e100, 2.0e100]] {
            let low = endpoint_support_span(range, false).unwrap();
            let high = endpoint_support_span(range, true).unwrap();
            let width = (range[1] - range[0]) / 1024.0;
            assert_eq!(low[0], range[0]);
            assert_eq!(high[1], range[1]);
            assert!(low[1] >= range[0] + width);
            assert!(high[0] <= range[1] - width);
            assert!(low[1] < high[0]);
        }
        for range in [[0.0, 0.0], [2.0, 1.0], [0.0, f64::INFINITY]] {
            assert_eq!(
                endpoint_support_span(range, false).unwrap_err().code,
                GeometryErrorCode::ConditioningFailure,
            );
        }
    }

    fn point(x: f64, y: f64) -> [Interval; 2] {
        [Interval::point(x), Interval::point(y)]
    }

    fn enclosed(x: [f64; 2], y: [f64; 2]) -> [Interval; 2] {
        [
            Interval::new(x[0], x[1]).unwrap(),
            Interval::new(y[0], y[1]).unwrap(),
        ]
    }

    fn proved(rays: &[[Interval; 2]]) -> bool {
        projected_vertex_fan(
            rays,
            &mut GeometryWorkBudget::new(u64::MAX),
            &AtomicBool::new(false),
        )
        .unwrap()
    }

    #[test]
    fn three_four_and_axis_crossing_cones_have_degree_one() {
        assert!(proved(&[
            point(2.0, 0.0),
            point(-1.0, 2.0),
            point(-1.0, -2.0)
        ]));
        assert!(proved(&[
            point(1.0, 0.0),
            point(0.0, 1.0),
            point(-1.0, 0.0),
            point(0.0, -1.0),
        ]));
        let cones = [
            enclosed([1.0, 2.0], [-0.125, 0.125]),
            enclosed([-0.125, 0.125], [1.0, 2.0]),
            enclosed([-2.0, -1.0], [-0.125, 0.125]),
            enclosed([-0.125, 0.125], [-2.0, -1.0]),
        ];
        for shift in 0..cones.len() {
            let rotated = std::array::from_fn::<_, 4, _>(|i| cones[(i + shift) % cones.len()]);
            assert!(proved(&rotated));
        }
    }

    #[test]
    fn reversed_duplicate_and_distinct_multiwinding_fans_are_refused() {
        let mut axes = [
            point(1.0, 0.0),
            point(0.0, 1.0),
            point(-1.0, 0.0),
            point(0.0, -1.0),
        ];
        axes.reverse();
        assert!(!proved(&axes));
        assert!(!proved(&[
            point(1.0, 0.0),
            point(2.0, 0.0),
            point(0.0, 1.0),
            point(-1.0, -1.0),
        ]));
        let twice = [
            point(1.0, 0.0),
            point(0.0, 1.0),
            point(-1.0, 0.0),
            point(0.0, -1.0),
            point(1.0, 0.0),
            point(0.0, 1.0),
            point(-1.0, 0.0),
            point(0.0, -1.0),
        ];
        assert!(!proved(&twice));
        // Every adjacent determinant is positive, every ray is distinct, and
        // the permutation is one cycle; nevertheless this star turns twice.
        assert!(!proved(&[
            point(2.0, 0.0),
            point(-2.0, 1.0),
            point(1.0, -2.0),
            point(1.0, 2.0),
            point(-2.0, -1.0),
        ]));
    }

    #[test]
    fn zero_uncertain_overlapping_and_half_turn_sectors_are_refused() {
        assert!(!proved(&[]));
        assert!(!proved(&[point(1.0, 0.0), point(0.0, 1.0)]));
        for bad in [
            point(0.0, 0.0),
            enclosed([-1.0, 1.0], [-1.0, 1.0]),
            enclosed([0.0, 2.0], [0.0, 2.0]),
        ] {
            assert!(!proved(&[bad, point(-1.0, 2.0), point(-1.0, -2.0)]));
        }
        assert!(!proved(&[
            enclosed([1.0, 2.0], [-0.25, 0.25]),
            enclosed([1.0, 2.0], [0.125, 0.5]),
            point(-1.0, 1.0),
            point(-1.0, -1.0),
        ]));
        assert!(!proved(&[
            point(1.0, 0.0),
            point(-1.0, 0.0),
            point(0.0, -1.0)
        ]));
        for invalid in [
            Interval {
                lo: f64::NAN,
                hi: 1.0,
            },
            Interval {
                lo: 1.0,
                hi: f64::INFINITY,
            },
            Interval { lo: 2.0, hi: 1.0 },
        ] {
            assert!(!proved(&[
                [invalid, Interval::point(0.0)],
                point(-1.0, 2.0),
                point(-1.0, -2.0),
            ]));
        }
    }

    #[test]
    fn arbitrary_subdivision_and_positive_radial_scaling_preserve_degree() {
        for divisions in 1..=16 {
            let n = divisions as f64;
            let mut rays = Vec::new();
            for side in 0..4 {
                for step in 0..divisions {
                    let t = -n + 2.0 * step as f64;
                    let (x, y) = match side {
                        0 => (n, t),
                        1 => (-t, n),
                        2 => (-n, -t),
                        _ => (t, -n),
                    };
                    // Independent positive radial intervals change lengths,
                    // not the certified cyclic ordering of the ray cones.
                    let scale = Interval::new(1.0, 1.001).unwrap();
                    rays.push([Interval::point(x).mul(scale), Interval::point(y).mul(scale)]);
                }
            }
            assert!(proved(&rays));
            rays.reverse();
            assert!(!proved(&rays));
        }
    }

    #[test]
    fn all_octant_permutations_accept_only_the_degree_one_cyclic_order() {
        let rays = [
            point(1.0, 0.0),
            point(1.0, 1.0),
            point(0.0, 1.0),
            point(-1.0, 1.0),
            point(-1.0, 0.0),
            point(-1.0, -1.0),
            point(0.0, -1.0),
            point(1.0, -1.0),
        ];
        let mut permutation = [0, 1, 2, 3, 4, 5, 6, 7];
        loop {
            let ordered = permutation.map(|index| rays[index]);
            let cyclic = (0..8).all(|i| permutation[(i + 1) % 8] == (permutation[i] + 1) % 8);
            assert_eq!(proved(&ordered), cyclic, "permutation {permutation:?}");
            let Some(pivot) = (0..7).rev().find(|&i| permutation[i] < permutation[i + 1]) else {
                break;
            };
            let successor = (pivot + 1..8)
                .rev()
                .find(|&i| permutation[pivot] < permutation[i])
                .unwrap();
            permutation.swap(pivot, successor);
            permutation[pivot + 1..].reverse();
        }
    }

    #[test]
    fn work_and_cancellation_remain_typed_and_cumulative() {
        let rays = [point(2.0, 0.0), point(-1.0, 2.0), point(-1.0, -2.0)];
        let mut work = GeometryWorkBudget::new(8);
        assert_eq!(
            projected_vertex_fan(&rays, &mut work, &AtomicBool::new(false))
                .unwrap_err()
                .code,
            GeometryErrorCode::ResourceLimit,
        );
        assert_eq!(work.consumed(), 8);
        assert_eq!(
            projected_vertex_fan(&rays, &mut work, &AtomicBool::new(true))
                .unwrap_err()
                .code,
            GeometryErrorCode::Cancelled,
        );
        assert_eq!(work.consumed(), 8);
        let mut work = GeometryWorkBudget::new(100);
        assert_eq!(
            projected_vertex_fan(&rays, &mut work, &AtomicBool::new(true))
                .unwrap_err()
                .code,
            GeometryErrorCode::Cancelled,
        );
        assert_eq!(work.consumed(), 1);
    }
}
