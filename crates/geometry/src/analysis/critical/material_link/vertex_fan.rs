// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::super::*;
use solidcore::{VertexId, curve::{curve::Curve, curve::p2::CurveKind},
    surface::surface::p2::SurfaceKind};
use crate::analysis::source_incidence::{boundary_loop, native_coedge, native_face, face_orientation};
use crate::analysis::MaterialSourceView;
use crate::parametric::{CurveEvaluator, SurfaceEvaluator};
use solidcraft_step::geom::{Curve as SourceCurve, Surface as SourceSurface};

fn native_topology_error(error: solidcore::SolidError) -> GeometryError {
    GeometryError::new(GeometryErrorCode::ParameterCorrespondenceUnavailable,
        format!("original vertex fan native topology: {error}"))
}

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
/// Every material-owned joint contributes its whole UV image, and a spherical
/// completion also requires its actual source connector/world-image proof.
/// Active supports additionally admit an exact zero-transport affine corner
/// complex: original coefficient identities (including joint endpoint closure),
/// positive radial face normals, nonreflex original corners, and complete
/// projected degree one are required. Nonzero/unproved face transport refuses.
#[allow(clippy::too_many_arguments)]
pub(super) fn original_vertex_fan(
    definition: &Definition,
    source: MaterialSourceView<'_, '_>,
    incidence: &SourceEdgeIncidence,
    vertex: VertexId,
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
    let vertex_point = source.trim_source().source_vertex_point(vertex, work, cancel)?;
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
            let (face_id, _) = native_face(definition, use_.face)?;
            let orientation = face_orientation(definition, use_.face, work, cancel)?;
            let wire = boundary_loop(definition, use_.face, use_.wire)?;
            let (entry, edge) = native_coedge(definition, use_.face, use_.wire, use_.index)?;
            let coedge = domain.coedges().get(use_.coedge).ok_or_else(|| GeometryError::new(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "original vertex coedge has no native chart occurrence"))?;
            if entry.edge != edge_id || (entry.forward == orientation) != use_.orientation
                || coedge.edgeuse != use_.edgeuse || !uses.vertices.contains(&vertex)
                || domain.face() != face_id {
                return Err(GeometryError::new(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "original vertex coedge ownership changed",
                ));
            }
            let proof = domain.original_coedge_correspondence(use_.coedge)?;
            let binding = proof.binding().ok_or_else(|| GeometryError::new(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "original vertex coedge lost its selected spatial provider binding"))?;
            if binding.owner != [use_.face, use_.wire, use_.index]
                || binding.face != face_id || binding.edge != entry.edge
                || binding.edgeuse != use_.edgeuse
                || binding.native_interval != [edge.interval.min(), edge.interval.max()]
                || binding.endpoints != edge.vertices
                || binding.source.native_orientation != entry.forward
            {
                return Err(GeometryError::new(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "original vertex spatial binding changed its native interval or endpoints"));
            }
            if use_ == uses.first {
                let range = proof.spatial_range();
                let endpoints = binding.spatial_endpoints();
                domain.with_original_spatial_curve(
                    use_.coedge, remaining_bytes, work, cancel, |curve, work, cancel| {
                        if !crate::parametric::curve_parameter_span_continuous(curve, range, work, cancel)? {
                            return Err(GeometryError::new(
                                GeometryErrorCode::AnalysisIndeterminate,
                                "original vertex spatial endpoint supports lack whole-range C0 authority"));
                        }
                        for (high, endpoint) in [(false, endpoints[0]), (true, endpoints[1])] {
                            work.charge(1, cancel)?;
                            if endpoint != vertex { continue; }
                            let parameter = range[usize::from(high)];
                            let point = crate::parametric::curve_enclosure(
                                curve, (parameter, parameter), work, cancel)?.position;
                            let target = vertex_point;
                            let delta = std::array::from_fn(|axis|
                                Interval::point(target[axis]).sub(point[axis]));
                            if length(delta)?.hi > source_allowance_mm {
                                return Err(GeometryError::new(
                                    GeometryErrorCode::ShapeErrorExceeded,
                                    "original vertex endpoint correction exceeds admitted source allowance"));
                            }
                            hull_vertex_support(&mut support, target.map(Interval::point));
                            hull_vertex_support(&mut support, point);
                            let span = endpoint_support_span(range, high)?;
                            hull_vertex_support(&mut support, crate::parametric::curve_enclosure(
                                curve, (span[0], span[1]), work, cancel)?.position);
                        }
                        Ok(())
                    },
                )?;
            }

            // Each absolute wire corner is owned by its incoming occurrence.
            // Both sides of paired edges are visited, including same-face
            // seams. A closed original edge may own two endpoint parameters.
            if edge.vertices[usize::from(entry.forward)] != vertex { continue; }
            let next_index = if use_.index + 1 == wire.edges.len() { 0 } else { use_.index + 1 };
            let (next_entry, next) = native_coedge(definition, use_.face, use_.wire, next_index)?;
            if next.vertices[usize::from(!next_entry.forward)] != vertex {
                return Err(GeometryError::new(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "original vertex face corner is not closed by native incidence",
                ));
            }
            let mut uv: Option<[Interval; 2]> = None;
            let mut corner = [None; 2];
            let mut corner_sources = [None; 2];
            for (slot, (index, back)) in [(use_.index, true), (next_index, false)].into_iter().enumerate() {
                let mut selected = None;
                for (flat, coedge) in domain.coedges().iter().enumerate() {
                    work.charge(1, cancel)?;
                    let identity = coedge.identity();
                    if identity.face_id == face_id
                        && identity.wire_index == use_.wire
                        && identity.coedge_index == index
                        && identity.edge_id == wire.edges[index].edge
                        && identity.orientation == wire.edges[index].forward
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
                corner[slot] = Some(coedge.identity());
                let proof = domain.original_coedge_correspondence(flat)?;
                let binding = proof.binding().ok_or_else(|| GeometryError::new(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "original vertex corner lost its original parameter endpoint binding"))?;
                let (corner_entry, corner_edge) =
                    native_coedge(definition, use_.face, use_.wire, index)?;
                if binding.owner != [use_.face, use_.wire, index]
                    || binding.face != face_id || binding.edge != corner_entry.edge
                    || binding.edgeuse != coedge.edgeuse
                    || binding.native_interval != [corner_edge.interval.min(), corner_edge.interval.max()]
                    || binding.endpoints != corner_edge.vertices
                    || binding.source.native_orientation != corner_entry.forward
                {
                    return Err(GeometryError::new(
                        GeometryErrorCode::ParameterCorrespondenceUnavailable,
                        "original vertex corner binding changed its native ownership"));
                }
                let native_high = back == wire.edges[index].forward;
                let spatial_high = native_high == binding.spatial_forward;
                let endpoint = binding.native_endpoint_index(spatial_high);
                if binding.endpoints[endpoint] != vertex
                    || binding.source_vertex_points_mm[endpoint] != vertex_point
                {
                    return Err(GeometryError::new(
                        GeometryErrorCode::ParameterCorrespondenceUnavailable,
                        "original vertex corner changed its captured source endpoint"));
                }
                corner_sources[slot] = Some(binding.source_vertex_ids[endpoint]);
                let spatial_range = proof.spatial_range();
                let spatial_span = endpoint_support_span(spatial_range, spatial_high)?;
                // Preserve the selected gamma branch over the SOURCE endpoint
                // support. A fraction of native/raw UV t is not this support
                // when the original correspondence is nonlinear or reversed.
                let mut image = domain.original_coedge_uv_piecewise_jet(
                    flat, spatial_span, work, cancel,
                )?.position;
                if proof.position_error_mm() != 0.0 {
                    let jet = domain.original_coedge_uv_jet(flat, spatial_span, work, cancel)?;
                    let width = Interval::point(spatial_range[1])
                        .sub(Interval::point(spatial_range[0]))
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
            let surface = domain.surface_operand();
            for joint in domain.original_joints()? {
                work.charge(1, cancel)?;
                if joint.vertex_id != vertex { continue; }
                let (incoming, outgoing) = if orientation {
                    (corner[0], corner[1])
                } else {
                    (corner[1], corner[0])
                };
                if Some(joint.incoming) != incoming || Some(joint.outgoing) != outgoing {
                    continue;
                }
                if joint.vertex_point != vertex_point
                    || corner_sources != [Some(joint.source_vertex_id); 2]
                {
                    return Err(GeometryError::new(
                        GeometryErrorCode::ParameterCorrespondenceUnavailable,
                        "original vertex joint changed its captured source apex"));
                }
                let image = super::atlas::original_joint_uv_enclosure(joint, work, cancel)?;
                if matches!(joint.kind, crate::kernel::trim::TrimJointKind::SphericalPole { .. }) {
                    let image = source_chart::qualify_original_pole_joint(
                        &surface, domain, joint, image.map(|v| [v.lo, v.hi]), work, cancel,
                    )?.ok_or_else(|| GeometryError::new(
                        GeometryErrorCode::AnalysisIndeterminate,
                        "original vertex pole corner lacks its complete physical connector proof"))?;
                    hull_vertex_support(&mut support, image);
                }
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
            let margin = domain.original_uv_margin()?;
            for axis in 0..2 {
                uv[axis] = uv[axis].add(Interval {
                    lo: -margin[axis],
                    hi: margin[axis],
                });
            }
            for patch in surface_parameter_boxes(&surface, uv.map(|v| [v.lo, v.hi]), work, cancel)? {
                work.charge(1, cancel)?;
                let image = crate::parametric::surface_piecewise_enclosure(&surface, patch, work, cancel)?.position;
                hull_vertex_support(&mut support, image);
            }
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
    source: MaterialSourceView<'_, '_>,
    incidence: &SourceEdgeIncidence,
    vertex: VertexId,
    incident_count: usize,
    prism: &SamplePrism,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(1, cancel)?;
    if incident_count < 3 { return Ok(false); }
    let Some(&first_face) = definition.face_ids().first() else { return Ok(false); };
    let first_shell = definition.brep().face(first_face).map_err(native_topology_error)?.shell;
    for &face_id in definition.face_ids() {
        work.charge(1, cancel)?;
        if definition.brep().face(face_id).map_err(native_topology_error)?.shell != first_shell {
            return Ok(false);
        }
    }
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
        let orientation = face_orientation(definition, candidate.face, work, cancel)?;
        let (entry, edge) = native_coedge(definition, candidate.face, candidate.wire, candidate.index)?;
        let front = edge.vertices[usize::from(!entry.forward)];
        let back = edge.vertices[usize::from(entry.forward)];
        if front == back { return Ok(false); }
        if (front == vertex) == orientation {
            start = Some(candidate);
            target = Some(source.trim_source().source_vertex_point(vertex, work, cancel)?);
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
        let (_, face) = native_face(definition, current.face)?;
        let orientation = face_orientation(definition, current.face, work, cancel)?;
        let wire = boundary_loop(definition, current.face, current.wire)?;
        let (entry, edge) = native_coedge(definition, current.face, current.wire, current.index)?;
        if edge.vertices[0] == edge.vertices[1]
            || (edge.vertices[usize::from(!entry.forward)] == vertex) != orientation
            || (entry.forward == orientation) != current.orientation
            || domain.coedges().get(current.coedge).is_none_or(|c| c.edgeuse != current.edgeuse) {
            return Ok(false);
        }
        let Some(direction) = original_affine_edge_direction(
            source, domain, current.coedge, edge, domain_bytes, work, cancel,
        )? else { return Ok(false); };
        let outward = if edge.vertices[0] == vertex {
            direction
        } else if edge.vertices[1] == vertex {
            direction.map(Interval::neg)
        } else {
            return Ok(false);
        };
        let surface = domain.surface_operand();
        let Some(carrier) = exact_affine_surface(&surface, work, cancel)? else {
            return Ok(false);
        };
        let Some(mut n) = exact_affine_normal(&carrier, work, cancel)? else {
            return Ok(false);
        };
        if !domain.orientation() {
            n = n.map(Interval::neg);
        }
        if length(n)?.lo <= 0.0 || dot(n, radial).lo <= 0.0 {
            return Ok(false);
        }
        // Independently prove the complete ORIGINAL UV coefficient trace is
        // the selected original spatial affine law. A retained Forward proof
        // can describe an exact algebraic identity despite numerical radii;
        // its enum or residual is neither identity authority nor a rejection.
        let mut flat = 0;
        for wire_index in 0..face.loops.len() {
            let boundary = boundary_loop(definition, current.face, wire_index)?;
            for (index, boundary_entry) in boundary.edges.iter().enumerate() {
                let (_, boundary_edge) = native_coedge(definition, current.face, wire_index, index)?;
                work.charge(1, cancel)?;
                let proof = domain.original_coedge_correspondence(flat)?;
                let Some(binding) = proof.binding() else {
                    return Ok(false);
                };
                if binding.owner != [current.face, wire_index, index]
                    || binding.edge != boundary_entry.edge
                    || binding.endpoints != boundary_edge.vertices
                    || binding.source.native_orientation != boundary_entry.forward
                {
                    return Ok(false);
                }
                let coedge = flat;
                flat += 1;
                let low = binding.native_endpoint_index(false);
                let a = source.trim_source().source_vertex_point(boundary_edge.vertices[low], work, cancel)?;
                let b = source.trim_source().source_vertex_point(boundary_edge.vertices[1 - low], work, cancel)?;
                if binding.native_interval != [boundary_edge.interval.min(), boundary_edge.interval.max()] {
                    return Ok(false);
                }
                let Some(d) = original_affine_edge_direction(
                    source, domain, coedge, boundary_edge, domain_bytes, work, cancel,
                )? else { return Ok(false); };
                // Endpoint affinity of the raw PCurve does not flatten a
                // nonlinear retained gamma map into a zero-transport law.
                if proof.affine_spatial_parameter_preimage(proof.uv_range())?.is_none() {
                    return Ok(false);
                }
                let chart = domain.coedge_operand(coedge)?;
                if !exact_affine_trace(
                    &carrier, &chart, proof.uv_endpoints(), [a, b], work, cancel,
                )? {
                    return Ok(false);
                }
                if boundary_edge.vertices[0] != vertex && boundary_edge.vertices[1] != vertex
                {
                    let va = std::array::from_fn(|i| point[i].sub(Interval::point(a[i])));
                    let vb = std::array::from_fn(|i| point[i].sub(Interval::point(b[i])));
                    if length(cross(d, va))?.lo <= 0.0 && dot(va, vb).lo <= 0.0 {
                        return Ok(false);
                    }
                }
            }
        }
        if flat != domain.coedges().len() { return Ok(false); }
        for joint in domain.original_joints()? {
            work.charge(1, cancel)?;
            if joint.vertex_id != vertex { continue; }
            if joint.vertex_point != target { return Ok(false); }
            let mut endpoints = [(0, 0.0); 2];
            for (slot, identity) in [joint.incoming, joint.outgoing].into_iter().enumerate() {
                let mut selected = None;
                for (flat, coedge) in domain.coedges().iter().enumerate() {
                    work.charge(1, cancel)?;
                    if coedge.identity() == identity {
                        if selected.is_some() { return Ok(false); }
                        selected = Some(flat);
                    }
                }
                let Some(flat) = selected else { return Ok(false); };
                let proof = domain.original_coedge_correspondence(flat)?;
                let Some(binding) = proof.binding() else { return Ok(false); };
                let spatial_high = binding.material_forward == (slot == 0);
                let endpoint = binding.native_endpoint_index(spatial_high);
                if binding.face != domain.face() || binding.edgeuse != identity.edgeuse
                    || binding.edge != identity.edge_id || binding.endpoints[endpoint] != vertex
                    || binding.source_vertex_ids[endpoint] != joint.source_vertex_id
                    || binding.source_vertex_points_mm[endpoint] != target
                {
                    return Ok(false);
                }
                endpoints[slot] = (flat, proof.uv_endpoint(spatial_high));
            }
            let incoming = domain.coedge_operand(endpoints[0].0)?;
            let outgoing = domain.coedge_operand(endpoints[1].0)?;
            // The actual material endpoint coefficient expressions must agree.
            // Their validated affine SourceEndpointConnector then has this
            // single UV image; the injective affine carrier and complete trace
            // identities above put it at ORIGINAL V. Neither a joint label nor
            // an interval enclosure's width can discharge this obligation.
            if !original_uv_endpoints_equal(
                &incoming, endpoints[0].1, &outgoing, endpoints[1].1, work, cancel,
            )? {
                return Ok(false);
            }
        }
        let adjacent = if orientation {
            if current.index == 0 {
                wire.edges.len() - 1
            } else {
                current.index - 1
            }
        } else if current.index + 1 == wire.edges.len() {
            0
        } else {
            current.index + 1
        };
        let (neighbor_entry, neighbor) = native_coedge(definition, current.face, current.wire, adjacent)?;
        let adjacent_flat = current.coedge - current.index + adjacent;
        let Some(d) = original_affine_edge_direction(
            source, domain, adjacent_flat, neighbor, domain_bytes, work, cancel,
        )? else { return Ok(false); };
        let next_outward = if neighbor.vertices[0] == vertex {
            d
        } else if neighbor.vertices[1] == vertex {
            d.map(Interval::neg)
        } else {
            return Ok(false);
        };
        if dot(n, cross(outward, next_outward)).lo <= 0.0 {
            return Ok(false);
        }
        rays.push([dot(outward, rows[0]), dot(outward, rows[1])]);
        let Some(pair) = incidence.get(neighbor_entry.edge) else { return Ok(false); };
        let Some(chart) = domain.coedges().get(adjacent_flat) else { return Ok(false); };
        let selected = crate::analysis::source_incidence::CoedgeUse {
            face: current.face, wire: current.wire, index: adjacent, coedge: adjacent_flat,
            orientation: neighbor_entry.forward == orientation, edgeuse: chart.edgeuse,
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

/// Prove the ORIGINAL spatial affine law first, then express its direction in
/// native canonical endpoint order solely for combinatorial fan traversal.
/// Native interval equality is never tested against the original query gauge.
#[allow(clippy::too_many_arguments)]
fn original_affine_edge_direction(
    source: MaterialSourceView<'_, '_>,
    domain: &crate::kernel::trim::PreparedFaceTrimDomain<'_>,
    coedge: usize,
    edge: &solidcore::topology::Edge,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[Interval; 3]>, GeometryError> {
    let Some(chart) = domain.coedges().get(coedge) else { return Ok(None); };
    let identity = chart.identity();
    let proof = domain.original_coedge_correspondence(coedge)?;
    let Some(binding) = proof.binding() else { return Ok(None); };
    if binding.owner != [domain.face_index(), identity.wire_index, identity.coedge_index]
        || binding.face != domain.face() || binding.edge != identity.edge_id
        || binding.edgeuse != identity.edgeuse
        || binding.native_interval != [edge.interval.min(), edge.interval.max()]
        || binding.endpoints != edge.vertices
        || binding.source.native_orientation != identity.orientation
    {
        return Ok(None);
    }
    let low = binding.native_endpoint_index(false);
    let front = source.trim_source().source_vertex_point(edge.vertices[low], work, cancel)?;
    let back = source.trim_source().source_vertex_point(edge.vertices[1 - low], work, cancel)?;
    let range = proof.spatial_range();
    domain.with_original_spatial_curve(coedge, remaining_bytes, work, cancel,
        |curve, work, cancel| {
            if !crate::parametric::curve_parameter_span_continuous(curve, range, work, cancel)? {
                return Ok(None);
            }
            let Some(direction) = affine_line_direction(curve, range, front, back, work, cancel)? else {
                return Ok(None);
            };
            Ok(Some(if binding.spatial_forward { direction } else { direction.map(Interval::neg) }))
        })
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
    evaluator: &(impl SurfaceEvaluator + ?Sized),
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<ExactAffineSurface>, GeometryError> {
    work.charge(1, cancel)?;
    if let Some(source) = evaluator.original_surface() {
        let Some(mut carrier) = original_affine_surface_coefficients(source, work, cancel)? else {
            return Ok(None);
        };
        let active = evaluator.active_domain();
        let support = carrier.support.map_or(active, |support| std::array::from_fn(|axis|
            [active[axis][0].max(support[axis][0]), active[axis][1].min(support[axis][1])]));
        if support.iter().any(|r| !r[0].is_finite() || !r[1].is_finite() || r[0] >= r[1]) {
            return Ok(None);
        }
        carrier.support = Some(support);
        return Ok(Some(carrier));
    }
    let Some(mut surface) = evaluator.native_surface() else { return Ok(None); };
    let active = surface.natural_uv_domain();
    let active = [[active.u_min(), active.u_max()], [active.v_min(), active.v_max()]];
    if active.iter().any(|r| !r[0].is_finite() || !r[1].is_finite() || r[0] >= r[1]) {
        return Ok(None);
    }
    for depth in 0..=32 {
        work.charge(1, cancel)?;
        match surface.kind() {
            SurfaceKind::CurveBounded(bound) if depth < 32 => surface = bound.basis(),
            SurfaceKind::Step(step) if depth < 32 => {
                surface = step.surface().ok_or_else(|| GeometryError::new(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "original affine surface has an empty STEP carrier"))?;
            }
            SurfaceKind::CurveBounded(_) | SurfaceKind::Step(_) => return Err(GeometryError::new(
                GeometryErrorCode::ResourceLimit, "original affine surface nesting exceeds source limit")),
            _ => break,
        }
    }
    if let SurfaceKind::Plane(plane) = surface.kind() {
        let position = plane.position();
        let p = position.origin();
        let x = position.x_axis();
        let y = position.y_axis();
        let scale = plane.uv_scale();
        let origin = [p.x, p.y, p.z];
        let x = [x.x, x.y, x.z];
        let y = [y.x, y.y, y.z];
        if origin.into_iter().chain(x).chain(y).chain([scale.x, scale.y]).any(|v| !v.is_finite()) {
            return Ok(None);
        }
        let mut du = std::array::from_fn(|_| ExactCoefficient::point(0.0));
        let mut dv = std::array::from_fn(|_| ExactCoefficient::point(0.0));
        for axis in 0..3 {
            let Some(u) = ExactCoefficient::point(x[axis]).product(&ExactCoefficient::point(scale.x), work, cancel)? else { return Ok(None); };
            let Some(v) = ExactCoefficient::point(y[axis]).product(&ExactCoefficient::point(scale.y), work, cancel)? else { return Ok(None); };
            du[axis] = u;
            dv[axis] = v;
        }
        return Ok(Some(ExactAffineSurface {
            origin: origin.map(ExactCoefficient::point), du, dv,
            denominator: ExactCoefficient::point(1.0), support: Some(active),
        }));
    }
    let Some(n) = crate::parametric::native_surface_nurbs(surface)? else { return Ok(None); };
    let (u_degree, v_degree) = n.degrees();
    let knots = [n.u_knots(), n.v_knots()];
    let degree = [u_degree, v_degree];
    let count = [n.n() + 1, n.m() + 1];
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
        let lower = lower.max(active[axis][0]);
        let upper = upper.min(active[axis][1]);
        if lower >= upper || knots[axis].len() != count[axis] + degree[axis] + 1 {
            return Ok(None);
        }
        support[axis] = [lower, upper];
    }
    affine_tensor_coefficients(support, degree, count,
    |axis, index, degree, work, cancel| exact_knot_sum(knots[axis], index, degree, work, cancel),
    |u, v| {
        let p = n.cpt(u, v).map_err(|error| GeometryError::new(
            GeometryErrorCode::ConditioningFailure, format!("original tensor coefficient unavailable: {error}")))?;
        Ok([p.x, p.y, p.z, if n.is_rational() { p.w } else { 1.0 }])
    }, work, cancel)
}

fn affine_tensor_coefficients(
    active: [[f64; 2]; 2],
    degree: [usize; 2],
    count: [usize; 2],
    knot_sum: impl Fn(usize, usize, usize, &mut GeometryWorkBudget, &AtomicBool)
        -> Result<Option<ExactCoefficient>, GeometryError>,
    control: impl Fn(usize, usize) -> Result<[f64; 4], GeometryError>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<ExactAffineSurface>, GeometryError> {
    let support = active;
    let base = control(0, 0)?;
    let last_u = control(count[0] - 1, 0)?;
    let last_v = control(0, count[1] - 1)?;
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
        let Some(begin) = knot_sum(axis, 0, degree[axis], work, cancel)? else {
            return Ok(None);
        };
        let Some(mut end) =
            knot_sum(axis, count[axis] - 1, degree[axis], work, cancel)?
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
        let Some(mut su) = knot_sum(0, u, degree[0], work, cancel)? else {
            return Ok(None);
        };
        if su.subtract(&first[0]).is_none() {
            return Ok(None);
        }
        for v in 0..count[1] {
            work.charge(1, cancel)?;
            let p = control(u, v)?;
            if p.into_iter().any(|v| !v.is_finite()) || p[3] != base[3] {
                return Ok(None);
            }
            let Some(mut sv) = knot_sum(1, v, degree[1], work, cancel)? else {
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

/// Exact source knots stay distinct with multiplicities; coefficient proofs
/// need neither expanded scratch nor rounded Greville abscissae.
struct SourceKnots<'a> {
    values: &'a [f64],
    multiplicities: &'a [usize],
}

impl SourceKnots<'_> {
    fn value(&self, mut index: usize, work: &mut GeometryWorkBudget, cancel: &AtomicBool)
        -> Result<Option<f64>, GeometryError>
    {
        if self.values.len() != self.multiplicities.len() { return Ok(None); }
        for (&value, &count) in self.values.iter().zip(self.multiplicities) {
            work.charge(1, cancel)?;
            if !value.is_finite() || count == 0 { return Ok(None); }
            if index < count { return Ok(Some(value)); }
            index -= count;
        }
        Ok(None)
    }

    fn sum(&self, index: usize, degree: usize, work: &mut GeometryWorkBudget, cancel: &AtomicBool)
        -> Result<Option<ExactCoefficient>, GeometryError>
    {
        let mut sum = ExactCoefficient::point(0.0);
        for i in index + 1..index + degree + 1 {
            let Some(value) = self.value(i, work, cancel)? else { return Ok(None); };
            if sum.push(value).is_none() { return Ok(None); }
        }
        Ok(Some(sum))
    }
}

fn original_affine_basis<'a>(
    mut source: &'a SourceCurve,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<&'a SourceCurve, GeometryError> {
    for _ in 0..16 {
        work.charge(1, cancel)?;
        source = match source {
            SourceCurve::Trimmed { basis, .. } => basis,
            SourceCurve::SurfaceCurve {
                curve_3d, master: solidcraft_step::geom::PreferredRep::Curve3d, ..
            } => curve_3d,
            _ => return Ok(source),
        };
    }
    Err(GeometryError::new(GeometryErrorCode::ResourceLimit,
        "original affine coefficient wrapper depth exceeded"))
}

fn original_source_affine(source: &SourceCurve, work: &mut GeometryWorkBudget, cancel: &AtomicBool)
    -> Result<bool, GeometryError>
{
    match original_affine_basis(source, work, cancel)? {
        SourceCurve::Line { origin, direction, magnitude } =>
            Ok(origin.iter().chain(direction).chain([magnitude]).all(|v| v.is_finite())),
        SourceCurve::BSpline(spline) => {
            let count = spline.points.len();
            if count == 0 { return Ok(false); }
            if let Some(weights) = &spline.weights {
                if weights.len() != count || !weights[0].is_finite() || weights[0] == 0.0
                    || weights.iter().any(|&weight| weight != weights[0]) { return Ok(false); }
            }
            let base = spline.points[0];
            if base.into_iter().any(|v| !v.is_finite()) { return Ok(false); }
            if spline.degree == 0 {
                for point in &spline.points {
                    work.charge(1, cancel)?;
                    if *point != base { return Ok(false); }
                }
                return Ok(true);
            }
            let knots = SourceKnots { values: &spline.knots, multiplicities: &spline.multiplicities };
            let Some(first) = knots.sum(0, spline.degree, work, cancel)? else { return Ok(false); };
            let Some(mut span) = knots.sum(count - 1, spline.degree, work, cancel)? else { return Ok(false); };
            if span.subtract(&first).is_none() || span.sign() <= 0 { return Ok(false); }
            let last = spline.points[count - 1];
            for (i, point) in spline.points.iter().enumerate() {
                work.charge(1, cancel)?;
                if point.iter().any(|v| !v.is_finite()) { return Ok(false); }
                let Some(mut coordinate) = knots.sum(i, spline.degree, work, cancel)? else { return Ok(false); };
                if coordinate.subtract(&first).is_none() { return Ok(false); }
                for axis in 0..3 {
                    let Some(delta) = ExactCoefficient::difference(last[axis], base[axis]) else { return Ok(false); };
                    let Some(difference) = ExactCoefficient::difference(point[axis], base[axis]) else { return Ok(false); };
                    let Some(mut residual) = difference.product(&span, work, cancel)? else { return Ok(false); };
                    let Some(reference) = delta.product(&coordinate, work, cancel)? else { return Ok(false); };
                    if residual.subtract(&reference).is_none() || residual.sign() != 0 { return Ok(false); }
                }
            }
            Ok(true)
        }
        SourceCurve::Polyline(points) => {
            if points.len() < 2 { return Ok(false); }
            for (index, point) in points.iter().enumerate() {
                work.charge(1, cancel)?;
                for axis in 0..3 {
                    if !point[axis].is_finite() { return Ok(false); }
                    let Some(delta) = ExactCoefficient::difference(points[1][axis], points[0][axis]) else { return Ok(false); };
                    let Some(mut reference) = delta.product(&ExactCoefficient::point(index as f64), work, cancel)? else { return Ok(false); };
                    let Some(value) = ExactCoefficient::difference(point[axis], points[0][axis]) else { return Ok(false); };
                    if reference.subtract(&value).is_none() || reference.sign() != 0 { return Ok(false); }
                }
            }
            Ok(true)
        }
        source @ SourceCurve::SurfaceCurve { .. } => {
            let Some(pcurve) = crate::kernel::source_operand::preferred_source_pcurve(source)? else {
                return Ok(false);
            };
            Ok(original_source_affine(&pcurve.curve, work, cancel)?
                && original_affine_surface_coefficients(&pcurve.surface, work, cancel)?.is_some())
        }
        _ => Ok(false),
    }
}

fn original_affine_surface_coefficients(
    mut source: &SourceSurface,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<ExactAffineSurface>, GeometryError> {
    let mut support: Option<[[f64; 2]; 2]> = None;
    for depth in 0..=16 {
        work.charge(1, cancel)?;
        match source {
            SourceSurface::RectangularTrimmed { basis, u1, u2, v1, v2, .. } if depth < 16 => {
                let bounds = [[u1.min(*u2), u1.max(*u2)], [v1.min(*v2), v1.max(*v2)]];
                support = Some(match support {
                    Some(previous) => std::array::from_fn(|i|
                        [previous[i][0].max(bounds[i][0]), previous[i][1].min(bounds[i][1])]),
                    None => bounds,
                });
                source = basis;
            }
            SourceSurface::RectangularTrimmed { .. } => return Err(GeometryError::new(
                GeometryErrorCode::ResourceLimit, "original affine surface wrapper depth exceeded")),
            _ => break,
        }
    }
    let mut carrier = ExactAffineSurface {
        origin: std::array::from_fn(|_| ExactCoefficient::point(0.0)),
        du: std::array::from_fn(|_| ExactCoefficient::point(0.0)),
        dv: std::array::from_fn(|_| ExactCoefficient::point(0.0)),
        denominator: ExactCoefficient::point(1.0), support,
    };
    match source {
        SourceSurface::Plane { position } => {
            if position.origin.iter().chain(&position.axis).chain(&position.ref_direction)
                .any(|v| !v.is_finite()) { return Ok(None); }
            carrier.origin = position.origin.map(ExactCoefficient::point);
            carrier.du = position.ref_direction.map(ExactCoefficient::point);
            for i in 0..3 {
                let j = (i + 1) % 3; let k = (i + 2) % 3;
                let Some(mut y) = ExactCoefficient::point(position.axis[j])
                    .product(&ExactCoefficient::point(position.ref_direction[k]), work, cancel)? else { return Ok(None); };
                let Some(other) = ExactCoefficient::point(position.axis[k])
                    .product(&ExactCoefficient::point(position.ref_direction[j]), work, cancel)? else { return Ok(None); };
                if y.subtract(&other).is_none() { return Ok(None); }
                carrier.dv[i] = y;
            }
            Ok(Some(carrier))
        }
        SourceSurface::Extrusion { profile, direction, magnitude } => {
            if direction.iter().chain([magnitude]).any(|v| !v.is_finite())
                || !original_source_affine(profile, work, cancel)? { return Ok(None); }
            let zero = ExactCoefficient::point(0.0);
            let one = ExactCoefficient::point(1.0);
            // These are symbolic values of the coefficient-proved affine
            // extension, not geometry samples outside a restricted profile.
            let Some(a) = source_affine_point(profile, [&zero, &one], work, cancel)? else { return Ok(None); };
            let Some(b) = source_affine_point(profile, [&one, &one], work, cancel)? else { return Ok(None); };
            let Some(denominator) = a[3].product(&b[3], work, cancel)? else { return Ok(None); };
            if denominator.sign() == 0 { return Ok(None); }
            for axis in 0..3 {
                let Some(origin) = a[axis].product(&b[3], work, cancel)? else { return Ok(None); };
                let Some(mut du) = b[axis].product(&a[3], work, cancel)? else { return Ok(None); };
                if du.subtract(&origin).is_none() { return Ok(None); }
                let Some(dv) = ExactCoefficient::point(direction[axis])
                    .product(&ExactCoefficient::point(*magnitude), work, cancel)? else { return Ok(None); };
                let Some(dv) = dv.product(&denominator, work, cancel)? else { return Ok(None); };
                carrier.origin[axis] = origin;
                carrier.du[axis] = du;
                carrier.dv[axis] = dv;
            }
            carrier.denominator = denominator;
            Ok(Some(carrier))
        }
        SourceSurface::BSpline(spline) => {
            let degree = [spline.u_degree, spline.v_degree];
            let count = [spline.points.len(), spline.points.first().map_or(0, Vec::len)];
            if degree.contains(&0) || count.into_iter().any(|n| n < 2)
                || spline.points.iter().any(|row| row.len() != count[1]) { return Ok(None); }
            let knots = [
                SourceKnots { values: &spline.u_knots, multiplicities: &spline.u_multiplicities },
                SourceKnots { values: &spline.v_knots, multiplicities: &spline.v_multiplicities },
            ];
            let mut active = [[0.0; 2]; 2];
            for axis in 0..2 {
                let Some(lower) = knots[axis].value(degree[axis], work, cancel)? else { return Ok(None); };
                let Some(upper) = knots[axis].value(count[axis], work, cancel)? else { return Ok(None); };
                active[axis] = [lower, upper];
                if let Some(support) = support {
                    active[axis] = [lower.max(support[axis][0]), upper.min(support[axis][1])];
                }
                if active[axis][0] >= active[axis][1] { return Ok(None); }
            }
            if let Some(weights) = &spline.weights {
                let Some(&weight) = weights.first().and_then(|row| row.first()) else { return Ok(None); };
                if !weight.is_finite() || weight == 0.0 || weights.len() != count[0]
                    || weights.iter().any(|row| row.len() != count[1] || row.iter().any(|&w| w != weight)) {
                    return Ok(None);
                }
            }
            // Equal source weights cancel symbolically, before multiplication;
            // no rounded homogeneous conversion supplies the original tensor.
            affine_tensor_coefficients(active, degree, count,
                |axis, index, degree, work, cancel| knots[axis].sum(index, degree, work, cancel),
                |u, v| { let p = spline.points[u][v]; Ok([p[0], p[1], p[2], 1.0]) },
                work, cancel)
        }
        _ => Ok(None),
    }
}

fn original_affine_point(
    curve: &(impl CurveEvaluator + ?Sized),
    source: &SourceCurve,
    parameter: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[ExactCoefficient; 4]>, GeometryError> {
    work.charge(1, cancel)?;
    let active = curve.active_range();
    if !parameter.is_finite() || parameter < active[0] || parameter > active[1] {
        return Ok(None);
    }
    let Some([offset, slope]) = curve.source_parameter_map() else { return Ok(None); };
    // Normalized LINE providers supply their unrounded physical law through
    // exact_affine_coefficients. Only an explicit point-valued raw map may
    // enter this literal source-coefficient fallback.
    if offset.lo != offset.hi || slope.lo != slope.hi { return Ok(None); }
    let Some(mut numerator) = ExactCoefficient::point(parameter)
        .product(&ExactCoefficient::point(slope.lo), work, cancel)? else { return Ok(None); };
    if numerator.push(offset.lo).is_none() { return Ok(None); }
    source_affine_point(source, [&numerator, &ExactCoefficient::point(1.0)], work, cancel)
}

fn coefficient_affine_point(
    curve: &(impl CurveEvaluator + ?Sized),
    coefficients: &crate::parametric::ExactAffineCurve,
    parameter: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[ExactCoefficient; 4]>, GeometryError> {
    work.charge(1, cancel)?;
    let active = curve.active_range();
    if !parameter.is_finite() || parameter < active[0] || parameter > active[1]
        || coefficients.denominator.sign() == 0 { return Ok(None); }
    let mut point = std::array::from_fn(|_| ExactCoefficient::point(0.0));
    for axis in 0..3 {
        let Some(mut value) = coefficients.derivative[axis]
            .product(&ExactCoefficient::point(parameter), work, cancel)? else { return Ok(None); };
        if !add_exact(&mut value, &coefficients.position[axis]) { return Ok(None); }
        point[axis] = value;
    }
    point[3] = coefficients.denominator.clone();
    Ok(Some(point))
}

/// Homogeneous evaluation of the coefficient-qualified source operator. The
/// query-to-source affine map is supplied exactly, not fitted from jet boxes.
fn source_affine_point(
    source: &SourceCurve,
    parameter: [&ExactCoefficient; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[ExactCoefficient; 4]>, GeometryError> {
    let source = original_affine_basis(source, work, cancel)?;
    let mut point = std::array::from_fn(|_| ExactCoefficient::point(0.0));
    match source {
        SourceCurve::Line { origin, direction, magnitude } => {
            for axis in 0..3 {
                let Some(velocity) = ExactCoefficient::point(direction[axis])
                    .product(&ExactCoefficient::point(*magnitude), work, cancel)? else { return Ok(None); };
                let Some(mut value) = velocity.product(parameter[0], work, cancel)? else { return Ok(None); };
                let Some(origin) = ExactCoefficient::point(origin[axis])
                    .product(parameter[1], work, cancel)? else { return Ok(None); };
                if !add_exact(&mut value, &origin) { return Ok(None); }
                point[axis] = value;
            }
            point[3] = parameter[1].clone();
        }
        SourceCurve::BSpline(spline) => {
            let base = spline.points[0];
            if spline.degree == 0 {
                return Ok(Some([base[0], base[1], base[2], 1.0].map(ExactCoefficient::point)));
            }
            let knots = SourceKnots { values: &spline.knots, multiplicities: &spline.multiplicities };
            let Some(first) = knots.sum(0, spline.degree, work, cancel)? else { return Ok(None); };
            let Some(mut span) = knots.sum(spline.points.len() - 1, spline.degree, work, cancel)? else { return Ok(None); };
            if span.subtract(&first).is_none() || span.sign() <= 0 { return Ok(None); }
            let Some(mut coordinate) = parameter[0]
                .product(&ExactCoefficient::point(spline.degree as f64), work, cancel)? else { return Ok(None); };
            let Some(offset) = first.product(parameter[1], work, cancel)? else { return Ok(None); };
            if coordinate.subtract(&offset).is_none() { return Ok(None); }
            let Some(denominator) = span.product(parameter[1], work, cancel)? else { return Ok(None); };
            let last = spline.points[spline.points.len() - 1];
            for axis in 0..3 {
                let Some(delta) = ExactCoefficient::difference(last[axis], base[axis]) else { return Ok(None); };
                let Some(mut value) = delta.product(&coordinate, work, cancel)? else { return Ok(None); };
                let Some(origin) = ExactCoefficient::point(base[axis]).product(&denominator, work, cancel)? else { return Ok(None); };
                if !add_exact(&mut value, &origin) { return Ok(None); }
                point[axis] = value;
            }
            point[3] = denominator;
        }
        SourceCurve::Polyline(points) => {
            for axis in 0..3 {
                let Some(delta) = ExactCoefficient::difference(points[1][axis], points[0][axis]) else { return Ok(None); };
                let Some(mut value) = delta.product(parameter[0], work, cancel)? else { return Ok(None); };
                let Some(origin) = ExactCoefficient::point(points[0][axis]).product(parameter[1], work, cancel)? else { return Ok(None); };
                if !add_exact(&mut value, &origin) { return Ok(None); }
                point[axis] = value;
            }
            point[3] = parameter[1].clone();
        }
        source @ SourceCurve::SurfaceCurve { .. } => {
            let Some(pcurve) = crate::kernel::source_operand::preferred_source_pcurve(source)? else {
                return Ok(None);
            };
            let Some(uv) = source_affine_point(&pcurve.curve, parameter, work, cancel)? else {
                return Ok(None);
            };
            let Some(carrier) = original_affine_surface_coefficients(&pcurve.surface, work, cancel)? else {
                return Ok(None);
            };
            for axis in 0..3 {
                let Some(mut value) = carrier.origin[axis].product(&uv[3], work, cancel)? else { return Ok(None); };
                let Some(u) = carrier.du[axis].product(&uv[0], work, cancel)? else { return Ok(None); };
                let Some(v) = carrier.dv[axis].product(&uv[1], work, cancel)? else { return Ok(None); };
                if !add_exact(&mut value, &u) || !add_exact(&mut value, &v) { return Ok(None); }
                point[axis] = value;
            }
            let Some(weight) = carrier.denominator.product(&uv[3], work, cancel)? else { return Ok(None); };
            point[3] = weight;
        }
        _ => return Ok(None),
    }
    Ok(Some(point))
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

/// Complete coefficients in the actual evaluator's parameter law. Original
/// source weights/maps and literal native transforms/reflections stay distinct;
/// no endpoint chord, enum label or fitted tangent supplies affinity.
pub(super) fn original_uv_affine(
    curve: &(impl CurveEvaluator + ?Sized),
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    if curve.exact_affine_coefficients(work, cancel)?.is_some() { return Ok(true); }
    if let Some(source) = curve.original_curve() {
        return original_source_affine(source, work, cancel);
    }
    let Some(native) = curve.native_curve() else { return Ok(false); };
    if native.dim() != 2 {
        work.charge(1, cancel)?;
        return Ok(false);
    }
    native_curve_affine(native, work, cancel)
}

fn native_curve_affine(
    curve: &Curve,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(1, cancel)?;
    let range = curve.natural_interval();
    if !matches!(curve.dim(), 2 | 3)
        || range.min().is_nan() || range.max().is_nan() || range.min() >= range.max() {
        return Ok(false);
    }
    if let CurveKind::Line(line) = curve.kind() {
        let c = line.canonical_unit();
        let p = [c.point.x, c.point.y, c.point.z];
        let v = [c.vector.x, c.vector.y, c.vector.z];
        return Ok(p[..curve.dim() as usize].iter().chain(&v[..curve.dim() as usize])
            .chain([&c.scale]).all(|v| v.is_finite())
            && (!line.is_bounded() || [range.min(), range.max(), c.domain.min(), c.domain.max()]
                .into_iter().all(f64::is_finite)));
    }
    let Some(n) = crate::parametric::native_curve_nurbs(curve)? else {
        return Ok(false);
    };
    let (knots, degree, count) = (n.knots(), n.degree(), n.cpts().len());
    for pair in knots.windows(2) {
        work.charge(1, cancel)?;
        if !pair[0].is_finite() || !pair[1].is_finite() || pair[0] > pair[1] {
            return Ok(false);
        }
    }
    let (Some(&lower), Some(&upper)) = (knots.get(degree), knots.get(count)) else {
        return Ok(false);
    };
    if count <= degree || knots.len() != count + degree + 1 || lower >= upper
        || range.min() < lower || range.max() > upper {
        return Ok(false);
    }
    let control = |i: usize| {
        let p = n.cpts()[i];
        [p.x, p.y, if curve.dim() == 2 { 0.0 } else { p.z },
            if n.is_rational() { p.w } else { 1.0 }]
    };
    let base = control(0);
    if base.into_iter().any(|v| !v.is_finite()) || base[3] == 0.0 {
        return Ok(false);
    }
    if degree == 0 {
        for i in 0..count {
            work.charge(1, cancel)?;
            if control(i) != base { return Ok(false); }
        }
        return Ok(true);
    }
    let Some(first) = exact_knot_sum(knots, 0, degree, work, cancel)? else { return Ok(false); };
    let Some(mut span) = exact_knot_sum(knots, count - 1, degree, work, cancel)? else { return Ok(false); };
    if span.subtract(&first).is_none() || span.sign() <= 0 { return Ok(false); }
    let last = control(count - 1);
    for i in 0..count {
        work.charge(1, cancel)?;
        let p = control(i);
        if p.into_iter().any(|v| !v.is_finite()) || p[3] != base[3] { return Ok(false); }
        let Some(mut coordinate) = exact_knot_sum(knots, i, degree, work, cancel)? else { return Ok(false); };
        if coordinate.subtract(&first).is_none() { return Ok(false); }
        for axis in 0..3 {
            let Some(difference) = ExactCoefficient::difference(p[axis], base[axis]) else { return Ok(false); };
            let Some(delta) = ExactCoefficient::difference(last[axis], base[axis]) else { return Ok(false); };
            let Some(mut residual) = difference.product(&span, work, cancel)? else { return Ok(false); };
            let Some(reference) = delta.product(&coordinate, work, cancel)? else { return Ok(false); };
            if residual.subtract(&reference).is_none() || residual.sign() != 0 { return Ok(false); }
        }
    }
    Ok(true)
}

/// Prove equality of two original affine UV endpoint expressions. Homogeneous
/// cross products avoid rounding rational coordinates or interpreting an
/// interval that merely contains zero as an identity. False means unproved,
/// including unsupported/nonaffine operators, not a proof of inequality.
pub(super) fn original_uv_endpoints_equal(
    first: &(impl CurveEvaluator + ?Sized),
    first_parameter: f64,
    second: &(impl CurveEvaluator + ?Sized),
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

// Fixed coefficient, homogeneous-point and bounded wrapper-operator scratch
// coexisting in the affine coordinate-function proof below. No heap storage.
pub(super) const ORIGINAL_UV_AFFINE_COORDINATE_STACK_BYTES: usize =
    80 * std::mem::size_of::<ExactCoefficient>();

/// Equality of one ORIGINAL affine UV coordinate FUNCTION on a common
/// qualified parameter interval. Complete coefficients establish affinity
/// first; the two exact homogeneous values then determine that function.
/// Neither rounded endpoint samples nor a zero-containing box are identities.
/// The caller retains both source/map/use bindings and supplies their common
/// original parameter interval; this helper does not supply that association.
pub(super) fn original_uv_affine_coordinate_functions_equal(
    first: &(impl CurveEvaluator + ?Sized),
    second: &(impl CurveEvaluator + ?Sized),
    parameters: [f64; 2],
    axis: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(1, cancel)?;
    if axis >= 2
        || parameters.into_iter().any(|value| !value.is_finite())
        || parameters[0] == parameters[1]
    {
        return Ok(false);
    }
    if !original_uv_affine(first, work, cancel)?
        || !original_uv_affine(second, work, cancel)?
    {
        return Ok(false);
    }
    for parameter in parameters {
        work.charge(1, cancel)?;
        let Some(a) = affine_uv_point(first, parameter, work, cancel)? else {
            return Ok(false);
        };
        let Some(b) = affine_uv_point(second, parameter, work, cancel)? else {
            return Ok(false);
        };
        if a[2].sign() == 0 || b[2].sign() == 0 {
            return Ok(false);
        }
        let Some(mut left) = a[axis].product(&b[2], work, cancel)? else {
            return Ok(false);
        };
        let Some(right) = b[axis].product(&a[2], work, cancel)? else {
            return Ok(false);
        };
        if left.subtract(&right).is_none() || left.sign() != 0 {
            return Ok(false);
        }
    }
    Ok(true)
}

fn original_affine_uv_point(
    curve: &(impl CurveEvaluator + ?Sized),
    parameter: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[ExactCoefficient; 3]>, GeometryError> {
    if !original_uv_affine(curve, work, cancel)? {
        return Ok(None);
    }
    affine_uv_point(curve, parameter, work, cancel)
}

/// Homogeneous evaluation of the same coefficient-proved literal chart.
fn affine_uv_point(
    curve: &(impl CurveEvaluator + ?Sized),
    parameter: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[ExactCoefficient; 3]>, GeometryError> {
    if let Some(coefficients) = curve.exact_affine_coefficients(work, cancel)? {
        let Some([x, y, _, w]) = coefficient_affine_point(
            curve, &coefficients, parameter, work, cancel,
        )? else { return Ok(None); };
        return Ok(Some([x, y, w]));
    }
    if let Some(source) = curve.original_curve() {
        let Some([x, y, _, w]) = original_affine_point(curve, source, parameter, work, cancel)? else {
            return Ok(None);
        };
        return Ok(Some([x, y, w]));
    }
    let Some(curve) = curve.native_curve() else { return Ok(None); };
    let Some([x, y, _, w]) = native_affine_point(curve, parameter, work, cancel)? else {
        return Ok(None);
    };
    Ok(Some([x, y, w]))
}

/// Keep the bounded LINE parameter denominator and every original homogeneous
/// control weight exact. Native canonical-unit evaluation is not a unit-edge
/// parameter map, especially after a reflected or nonunit bounded restriction.
fn native_affine_point(
    curve: &Curve,
    parameter: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[ExactCoefficient; 4]>, GeometryError> {
    work.charge(1, cancel)?;
    let range = curve.natural_interval();
    if !parameter.is_finite() || parameter < range.min() || parameter > range.max() {
        return Ok(None);
    }
    let mut point = std::array::from_fn(|_| ExactCoefficient::point(0.0));
    if let CurveKind::Line(line) = curve.kind() {
        let c = line.canonical_unit();
        let sign = ExactCoefficient::point(if c.inside_out { -1.0 } else { 1.0 });
        let (mapped, denominator) = if line.is_bounded() {
            let Some(denominator) = ExactCoefficient::difference(range.max(), range.min()) else { return Ok(None); };
            let Some(active) = ExactCoefficient::difference(c.domain.max(), c.domain.min()) else { return Ok(None); };
            let origin = if c.inside_out { range.max() } else { range.min() };
            let Some(delta) = ExactCoefficient::difference(parameter, origin) else { return Ok(None); };
            let Some(mapped) = delta.product(&active, work, cancel)? else { return Ok(None); };
            let Some(mut mapped) = mapped.product(&sign, work, cancel)? else { return Ok(None); };
            let Some(offset) = denominator.product(&ExactCoefficient::point(c.domain.min()), work, cancel)? else { return Ok(None); };
            if denominator.sign() <= 0 || !add_exact(&mut mapped, &offset) { return Ok(None); }
            (mapped, denominator)
        } else {
            let Some(mapped) = ExactCoefficient::point(parameter).product(&sign, work, cancel)? else { return Ok(None); };
            (mapped, ExactCoefficient::point(1.0))
        };
        let p = [c.point.x, c.point.y, if curve.dim() == 2 { 0.0 } else { c.point.z }];
        let v = [c.vector.x, c.vector.y, if curve.dim() == 2 { 0.0 } else { c.vector.z }];
        for axis in 0..3 {
            let Some(velocity) = ExactCoefficient::point(v[axis]).product(&ExactCoefficient::point(c.scale), work, cancel)? else { return Ok(None); };
            let Some(mut value) = velocity.product(&mapped, work, cancel)? else { return Ok(None); };
            let Some(origin) = ExactCoefficient::point(p[axis]).product(&denominator, work, cancel)? else { return Ok(None); };
            if !add_exact(&mut value, &origin) { return Ok(None); }
            point[axis] = value;
        }
        point[3] = denominator;
        return Ok(Some(point));
    }
    let Some(n) = crate::parametric::native_curve_nurbs(curve)? else { return Ok(None); };
    let (knots, degree, count) = (n.knots(), n.degree(), n.cpts().len());
    let control = |i: usize| {
        let p = n.cpts()[i];
        [p.x, p.y, if curve.dim() == 2 { 0.0 } else { p.z },
            if n.is_rational() { p.w } else { 1.0 }]
    };
    let base = control(0);
    if degree == 0 { return Ok(Some(base.map(ExactCoefficient::point))); }
    let Some(first) = exact_knot_sum(knots, 0, degree, work, cancel)? else { return Ok(None); };
    let Some(mut span) = exact_knot_sum(knots, count - 1, degree, work, cancel)? else { return Ok(None); };
    if span.subtract(&first).is_none() || span.sign() <= 0 { return Ok(None); }
    let Some(mut coordinate) = ExactCoefficient::point(parameter)
        .product(&ExactCoefficient::point(degree as f64), work, cancel)? else { return Ok(None); };
    if coordinate.subtract(&first).is_none() { return Ok(None); }
    let last = control(count - 1);
    for axis in 0..3 {
        let Some(delta) = ExactCoefficient::difference(last[axis], base[axis]) else { return Ok(None); };
        let Some(mut value) = delta.product(&coordinate, work, cancel)? else { return Ok(None); };
        let Some(origin) = ExactCoefficient::point(base[axis]).product(&span, work, cancel)? else { return Ok(None); };
        if !add_exact(&mut value, &origin) { return Ok(None); }
        point[axis] = value;
    }
    let Some(weight) = ExactCoefficient::point(base[3]).product(&span, work, cancel)? else { return Ok(None); };
    point[3] = weight;
    Ok(Some(point))
}


/// Evaluate the literal affine coefficient law, preserving rational values.
fn affine_curve_endpoints(
    curve: &(impl CurveEvaluator + ?Sized),
    parameters: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[[ExactCoefficient; 4]; 2]>, GeometryError> {
    let (a, b) = if let Some(coefficients) = curve.exact_affine_coefficients(work, cancel)? {
        (coefficient_affine_point(curve, &coefficients, parameters[0], work, cancel)?,
         coefficient_affine_point(curve, &coefficients, parameters[1], work, cancel)?)
    } else if let Some(source) = curve.original_curve() {
        if !original_source_affine(source, work, cancel)? { return Ok(None); }
        (original_affine_point(curve, source, parameters[0], work, cancel)?,
         original_affine_point(curve, source, parameters[1], work, cancel)?)
    } else {
        let Some(curve) = curve.native_curve() else { return Ok(None); };
        if !native_curve_affine(curve, work, cancel)? { return Ok(None); }
        (native_affine_point(curve, parameters[0], work, cancel)?,
         native_affine_point(curve, parameters[1], work, cancel)?)
    };
    let (Some(a), Some(b)) = (a, b) else { return Ok(None); };
    Ok(Some([a, b]))
}

fn affine_line_direction(
    curve: &(impl CurveEvaluator + ?Sized),
    range: [f64; 2],
    front: [f64; 3],
    back: [f64; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[Interval; 3]>, GeometryError> {
    if front == back || front.into_iter().chain(back).any(|v| !v.is_finite()) {
        return Ok(None);
    }
    if !range[0].is_finite() || !range[1].is_finite() || range[0] >= range[1] {
        return Ok(None);
    }
    let Some(points) = affine_curve_endpoints(curve, range, work, cancel)? else {
        return Ok(None);
    };
    for (endpoint, target) in points.iter().zip([front, back]) {
        if endpoint[3].sign() == 0 { return Ok(None); }
        for i in 0..3 {
            let Some(reference) = endpoint[3].product(&ExactCoefficient::point(target[i]), work, cancel)? else { return Ok(None); };
            let mut residual = endpoint[i].clone();
            if residual.subtract(&reference).is_none() || residual.sign() != 0 {
                return Ok(None);
            }
        }
    }
    // The coefficient-proved affine derivative is this vector divided by the
    // strictly positive retained parameter span. Positive scaling preserves
    // its entire angular cone, without substituting any tangent sample.
    Ok(Some(std::array::from_fn(|i| {
        Interval::point(back[i]).sub(Interval::point(front[i]))
    })))
}

fn exact_affine_trace(
    carrier: &ExactAffineSurface,
    chart: &(impl CurveEvaluator + ?Sized),
    parameters: [f64; 2],
    targets: [[f64; 3]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    // The selected original PCurve need not be syntactically LINE. Its complete
    // coefficient law and exact homogeneous endpoint expressions determine the
    // composition; a converted native chart or rounded sample is not authority.
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
    use solidcore::surface::surface::Surface;
    use crate::kernel::source_operand::{PreparedOriginalCurve, PreparedOriginalSurface};
    use solidcraft_step::geom::{self as original, Logical, CurveForm, KnotSpec};

    fn source_spline(points: Vec<[f64; 3]>, weights: Option<Vec<f64>>) -> SourceCurve {
        SourceCurve::BSpline(Box::new(original::BSplineCurve {
            degree: 2, points, weights, knots: vec![-2.0, 0.0, 4.0],
            multiplicities: vec![3, 1, 3], form: CurveForm::Unspecified,
            closed: Logical::False, self_intersect: Logical::False,
            knot_spec: KnotSpec::Unspecified,
        }))
    }

    #[test]
    fn original_fan_affinity_follows_declared_master_not_spatial_counterpart() {
        use std::sync::Arc;
        let placement = original::Placement {
            origin: [10.0, 20.0, 30.0], axis: [0.0, 0.0, 1.0],
            ref_direction: [1.0, 0.0, 0.0],
        };
        let counterpart = Arc::new(SourceCurve::Line {
            origin: [0.0; 3], direction: [1.0, 0.0, 0.0], magnitude: 1.0,
        });
        let chart = Arc::new(SourceCurve::Line {
            origin: [0.0; 3], direction: [1.0, 0.0, 0.0], magnitude: 1.0,
        });
        let make = |surface, master| SourceCurve::SurfaceCurve {
            kind: original::SurfaceCurveKind::Surface, curve_3d: counterpart.clone(),
            associated: vec![original::Associated::Pcurve(original::Pcurve {
                surface: Arc::new(surface), curve: chart.clone(),
            })], master,
        };
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let plane = make(SourceSurface::Plane { position: placement }, original::PreferredRep::PcurveS1);
        let selected = PreparedOriginalCurve::new(&plane, [0.0, 1.0],
            MAX_SCRATCH_BYTES, &mut work, &cancel).unwrap();
        assert!(affine_line_direction(&selected, [0.0, 1.0],
            [10.0, 20.0, 30.0], [11.0, 20.0, 30.0], &mut work, &cancel).unwrap().is_some());
        assert!(affine_line_direction(&selected, [0.0, 1.0],
            [0.0; 3], [1.0, 0.0, 0.0], &mut work, &cancel).unwrap().is_none());

        let cylinder = make(SourceSurface::Cylinder { position: placement, radius: 1.0 },
            original::PreferredRep::PcurveS1);
        let selected = PreparedOriginalCurve::new(&cylinder, [0.0, 1.0],
            MAX_SCRATCH_BYTES, &mut work, &cancel).unwrap();
        assert!(!original_source_affine(&cylinder, &mut work, &cancel).unwrap());
        assert!(affine_line_direction(&selected, [0.0, 1.0],
            [0.0; 3], [1.0, 0.0, 0.0], &mut work, &cancel).unwrap().is_none(),
            "an affine unselected 3D counterpart cannot qualify a preferred circular PCurve image");

        let spatial = make(SourceSurface::Cylinder { position: placement, radius: 1.0 },
            original::PreferredRep::Curve3d);
        let selected = PreparedOriginalCurve::new(&spatial, [0.0, 1.0],
            MAX_SCRATCH_BYTES, &mut work, &cancel).unwrap();
        assert!(affine_line_direction(&selected, [0.0, 1.0],
            [0.0; 3], [1.0, 0.0, 0.0], &mut work, &cancel).unwrap().is_some());
    }

    #[test]
    fn normalized_original_line_fan_keeps_nondyadic_parameter_image_and_reversal() {
        use crate::analysis::source_restriction::SourceLineRestriction;
        let source = SourceCurve::Line {
            origin: [0.0; 3], direction: [1.0, 0.0, 0.0], magnitude: 3.0,
        };
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        for (vertices, same_sense) in [
            ([[1.0, 0.0, 0.0], [2.0, 0.0, 0.0]], true),
            ([[2.0, 0.0, 0.0], [1.0, 0.0, 0.0]], false),
        ] {
            let curve = SourceLineRestriction::new(
                &source, &vertices, same_sense, 0.0, &mut work, &cancel,
            ).unwrap().unwrap();
            assert!(curve.endpoint_parameters().iter().all(|p| p.lo < p.hi));
            assert!(affine_line_direction(&curve, [0.0, 1.0], vertices[0], vertices[1],
                &mut work, &cancel).unwrap().is_some());
            assert!(affine_line_direction(&curve, [0.0, 1.0], [0.0; 3], [3.0, 0.0, 0.0],
                &mut work, &cancel).unwrap().is_none(),
                "raw source t must not replace the normalized query gauge");
            assert!(affine_line_direction(&curve, [16.0, 48.0], vertices[0], vertices[1],
                &mut work, &cancel).unwrap().is_none(),
                "a native stored interval is not the original spatial query range");
            let mut perturbed = vertices[1];
            perturbed[0] = perturbed[0].next_up();
            assert!(affine_line_direction(&curve, [0.0, 1.0], vertices[0], perturbed,
                &mut work, &cancel).unwrap().is_none());
        }
    }

    #[test]
    fn original_affine_fan_coefficients_use_raw_source_gauge_and_all_controls() {
        let points: Vec<_> = [-2.0, -1.0, 2.0, 4.0].into_iter()
            .map(|t| [t + 2.0, 2.0 * t + 1.0, 3.0 * t]).collect();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        for weights in [None, Some(vec![3.0; 4])] {
            let source = source_spline(points.clone(), weights);
            let curve = PreparedOriginalCurve::new(&source, [-2.0, 4.0],
                MAX_SCRATCH_BYTES, &mut work, &cancel).unwrap();
            assert!(affine_line_direction(&curve, [-2.0, 4.0],
                [0.0, -3.0, -6.0], [6.0, 9.0, 12.0], &mut work, &cancel).unwrap().is_some());
            assert!(affine_line_direction(&curve, [-2.0, 4.0],
                [0.0, -3.0, -6.0], [6.0, 9.0, 12.0_f64.next_up()],
                &mut work, &cancel).unwrap().is_none());
            assert!(original_uv_endpoints_equal(&curve, 0.0, &curve, 0.0,
                &mut work, &cancel).unwrap());
            assert!(!original_uv_endpoints_equal(&curve, -3.0, &curve, -2.0,
                &mut work, &cancel).unwrap());
        }
        let mut bent = points;
        bent[1][0] = bent[1][0].next_up();
        let source = source_spline(bent, None);
        let curve = PreparedOriginalCurve::new(&source, [-2.0, 4.0],
            MAX_SCRATCH_BYTES, &mut work, &cancel).unwrap();
        assert!(!original_uv_affine(&curve, &mut work, &cancel).unwrap());
        assert_eq!(original_uv_affine(&curve, &mut GeometryWorkBudget::new(0), &cancel)
            .unwrap_err().code, GeometryErrorCode::ResourceLimit);
        assert_eq!(original_uv_affine(&curve, &mut work, &AtomicBool::new(true))
            .unwrap_err().code, GeometryErrorCode::Cancelled);
    }

    #[test]
    fn original_affine_trace_composes_selected_source_plane_and_pcurve() {
        let source = SourceSurface::Plane { position: original::Placement {
            origin: [10.0, 20.0, 30.0], axis: [0.0, 0.0, 1.0],
            ref_direction: [1.0, 0.0, 0.0],
        }};
        let pcurve = SourceCurve::Line {
            origin: [1.0, 2.0, 0.0], direction: [1.0, 2.0, 0.0], magnitude: 3.0,
        };
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let surface = PreparedOriginalSurface::new(&source, [[-16.0, 16.0]; 2],
            MAX_SCRATCH_BYTES, &mut work, &cancel).unwrap();
        let chart = PreparedOriginalCurve::new(&pcurve, [-2.0, 4.0],
            MAX_SCRATCH_BYTES, &mut work, &cancel).unwrap();
        let affine = exact_affine_surface(&surface, &mut work, &cancel).unwrap().unwrap();
        assert!(exact_affine_trace(&affine, &chart, [-2.0, 2.0],
            [[5.0, 10.0, 30.0], [17.0, 34.0, 30.0]], &mut work, &cancel).unwrap());
        assert!(exact_affine_trace(&affine, &chart, [2.0, -2.0],
            [[17.0, 34.0, 30.0], [5.0, 10.0, 30.0]], &mut work, &cancel).unwrap());
        assert!(!exact_affine_trace(&affine, &chart, [-2.0, 2.0],
            [[17.0, 34.0, 30.0], [5.0, 10.0, 30.0]], &mut work, &cancel).unwrap(),
            "sorting a reversed selected-PCurve map loses its spatial endpoint binding");
        assert!(!exact_affine_trace(&affine, &chart, [-2.0, 2.0],
            [[5.0, 10.0, 30.0], [17.0, 34.0_f64.next_up(), 30.0]], &mut work, &cancel).unwrap());
    }

    #[test]
    fn original_affine_tensor_trace_retains_source_nonunit_knots_and_weights() {
        let points: Vec<Vec<_>> = [-2.0, -1.0, 2.0, 4.0].into_iter().map(|u|
            [1.0, 3.0].into_iter().map(|v| [u + 2.0, v - 1.0, u + 2.0 * v]).collect()).collect();
        let pcurve = SourceCurve::Line {
            origin: [-2.0, 1.0, 0.0], direction: [3.0, 1.0, 0.0], magnitude: 2.0,
        };
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let chart = PreparedOriginalCurve::new(&pcurve, [0.0, 1.0],
            MAX_SCRATCH_BYTES, &mut work, &cancel).unwrap();
        for weights in [None, Some(vec![vec![3.0; 2]; 4])] {
            let source = SourceSurface::BSpline(Box::new(original::BSplineSurface {
                u_degree: 2, v_degree: 1, points: points.clone(), weights,
                u_knots: vec![-2.0, 0.0, 4.0], u_multiplicities: vec![3, 1, 3],
                v_knots: vec![1.0, 3.0], v_multiplicities: vec![2, 2],
                form: original::SurfaceForm::Unspecified, u_closed: Logical::False,
                v_closed: Logical::False, self_intersect: Logical::False,
                knot_spec: KnotSpec::Unspecified,
            }));
            let surface = PreparedOriginalSurface::new(&source, [[-2.0, 4.0], [1.0, 3.0]],
                MAX_SCRATCH_BYTES, &mut work, &cancel).unwrap();
            let affine = exact_affine_surface(&surface, &mut work, &cancel).unwrap().unwrap();
            assert!(exact_affine_trace(&affine, &chart, [0.0, 1.0],
                [[0.0; 3], [6.0, 2.0, 10.0]], &mut work, &cancel).unwrap());
            assert!(!exact_affine_trace(&affine, &chart, [0.0, 1.0],
                [[0.0; 3], [6.0, 2.0, 10.0_f64.next_up()]], &mut work, &cancel).unwrap());
        }
    }

    #[test]
    fn original_affine_extrusion_keeps_profile_u_and_physical_v_laws() {
        let source = SourceSurface::Extrusion {
            profile: std::sync::Arc::new(SourceCurve::Line {
                origin: [2.0, -1.0, 0.0], direction: [1.0, 0.0, 1.0], magnitude: 1.0,
            }),
            direction: [0.0, 1.0, 2.0], magnitude: 1.0,
        };
        let pcurve = SourceCurve::Line {
            origin: [-2.0, 1.0, 0.0], direction: [3.0, 1.0, 0.0], magnitude: 2.0,
        };
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let surface = PreparedOriginalSurface::new(&source, [[-2.0, 4.0], [1.0, 3.0]],
            MAX_SCRATCH_BYTES, &mut work, &cancel).unwrap();
        let chart = PreparedOriginalCurve::new(&pcurve, [0.0, 1.0],
            MAX_SCRATCH_BYTES, &mut work, &cancel).unwrap();
        let affine = exact_affine_surface(&surface, &mut work, &cancel).unwrap().unwrap();
        assert!(exact_affine_trace(&affine, &chart, [0.0, 1.0],
            [[0.0; 3], [6.0, 2.0, 10.0]], &mut work, &cancel).unwrap());
        assert!(length(exact_affine_normal(&affine, &mut work, &cancel).unwrap().unwrap())
            .unwrap().lo > 0.0);
    }


    use solidcore::{Point3, Vec3, Vec2, nurbs::{
        b_spline_curve::BSplineCurve, b_spline_surface::BSplineSurface,
        crv_basic, srf_basic, math::{CPoint, NOW, NOZ},
    }};

    fn native_curve(points: Vec<CPoint>, degree: usize, knots: Vec<f64>, dim: u64) -> Curve {
        BSplineCurve::from_nurb(crv_basic::Curve::new(points, degree, knots).unwrap(), dim, true).into()
    }

    fn uv_curve(points: &[[f64; 3]], degree: usize, knots: Vec<f64>) -> Curve {
        native_curve(points.iter().map(|p| CPoint::new(p[0], p[1], NOZ, p[2])).collect(),
            degree, knots, 2)
    }

    fn uv_line(a: [f64; 2], b: [f64; 2]) -> Curve {
        uv_curve(&[[a[0], a[1], NOW], [b[0], b[1], NOW]], 1, vec![0.0, 0.0, 1.0, 1.0])
    }

    fn native_tensor(points: Vec<Vec<CPoint>>, degree: [usize; 2], knots: [Vec<f64>; 2]) -> Surface {
        let [u, v] = knots;
        let raw = srf_basic::Surface::new(points, degree[0], degree[1], u, v).unwrap();
        Surface::new(SurfaceKind::BSpline(BSplineSurface::from_nurb(raw, true)))
    }

    fn plane() -> Surface {
        solidcore::surface::plane::Plane::new(
            Point3::new(10.0, 20.0, 30.0), Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0), Vec2::new(3.0, 6.0),
            &solidcore::geom::extent2d::Extent2d::from_bounds(-16.0, -16.0, 16.0, 16.0),
        ).into()
    }

    // Importer-style literal members keep both the natural interval and the
    // declared analytic reflection anchor, without constructor normalization.
    fn analytic_line(point: [f64; 3], vector: [f64; 3], domain: [f64; 2], reflected: bool, dim: u64) -> Curve {
        let ends = if reflected { [domain[1], domain[0]] } else { domain };
        let points = ends.map(|t| CPoint::new(point[0] + vector[0] * t,
            point[1] + vector[1] * t, if dim == 2 { NOZ } else { point[2] + vector[2] * t }, NOW));
        let base = BSplineCurve::from_nurb(crv_basic::Curve::new(points.to_vec(), 1,
            vec![domain[0], domain[0], domain[1], domain[1]]).unwrap(), dim, true);
        Curve::new(CurveKind::Line(solidcore::curve::line::Line::from_parts(
            base, Point3::new(point[0], point[1], point[2]), Vec3::new(vector[0], vector[1], vector[2]),
            solidcore::geom::extent1d::Extent1d::from_min_max(domain[0], domain[1]), 1.0, reflected)))
    }

    #[test]
    fn original_affine_uv_coordinate_identity_keeps_closed_gauges_and_one_ulp_kinks() {
        let a = uv_line([3.0, 1.0], [2.0, 1.0]);
        let b = uv_line([3.0, 7.0], [2.0, 7.0]);
        let perturbed = uv_line([3.0, 7.0], [2.0_f64.next_up(), 7.0]);
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        for parameters in [[0.0, 1.0], [0.25, 0.75], [1.0, 0.0]] {
            assert!(original_uv_affine_coordinate_functions_equal(&a, &b, parameters, 0, &mut work, &cancel).unwrap());
        }
        assert!(!original_uv_affine_coordinate_functions_equal(&a, &b, [0.0, 1.0], 1, &mut work, &cancel).unwrap());
        assert!(!original_uv_affine_coordinate_functions_equal(&a, &perturbed, [0.0, 1.0], 0, &mut work, &cancel).unwrap());
        let nonlinear = uv_curve(&[[3.0, 7.0, NOW], [2.75, 7.0, NOW], [2.0, 7.0, NOW]],
            2, vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0]);
        assert!(!original_uv_affine_coordinate_functions_equal(&a, &nonlinear, [0.0, 1.0], 0, &mut work, &cancel).unwrap());
        for parameters in [[-0.25, 1.0], [0.5, 0.5]] {
            assert!(!original_uv_affine_coordinate_functions_equal(&a, &b, parameters, 0, &mut work, &cancel).unwrap());
        }
        assert_eq!(original_uv_affine_coordinate_functions_equal(&a, &b, [0.0, 1.0], 0,
            &mut GeometryWorkBudget::new(0), &cancel).unwrap_err().code, GeometryErrorCode::ResourceLimit);
        assert_eq!(original_uv_affine_coordinate_functions_equal(&a, &b, [0.0, 1.0], 0,
            &mut work, &AtomicBool::new(true)).unwrap_err().code, GeometryErrorCode::Cancelled);
    }

    #[test]
    fn original_uv_endpoint_identity_preserves_rational_values_and_rejects_rounding() {
        let rational = |s| uv_curve(&[[s, 0.0, 3.0 * s], [4.0 * s, 0.0, 3.0 * s]],
            1, vec![0.0, 0.0, 1.0, 1.0]);
        let first = rational(1.0);
        let scaled = rational(2.0);
        let rounded = uv_line([5.0 / 6.0, 0.0], [5.0 / 6.0, 0.0]);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        assert!(original_uv_endpoints_equal(&first, 0.5, &scaled, 0.5, &mut work, &cancel).unwrap());
        assert!(!original_uv_endpoints_equal(&first, 0.5, &rounded, 0.0, &mut work, &cancel).unwrap());
        assert!(!original_uv_endpoints_equal(&first, 0.0, &scaled, 1.0, &mut work, &cancel).unwrap());
        let polynomial = uv_curve(&[[0.0, 0.0, NOW], [1.0, 1.5, NOW], [2.0, 3.0, NOW]],
            2, vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0]);
        assert!(original_uv_endpoints_equal(&polynomial, 0.5,
            &uv_line([0.0, 0.0], [2.0, 3.0]), 0.5, &mut work, &cancel).unwrap());
    }

    #[test]
    fn original_uv_endpoint_identity_tracks_actual_declared_reflection_and_pose() {
        let curve = analytic_line([10.0, 20.0, 0.0], [1.0, 2.0, 0.0], [2.0, 5.0], true, 2);
        let point = uv_line([15.0, 30.0], [15.0, 30.0]);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        assert!(original_uv_endpoints_equal(&curve, 2.0, &point, 0.0, &mut work, &cancel).unwrap());
        assert!(!original_uv_endpoints_equal(&curve, 5.0, &point, 0.0, &mut work, &cancel).unwrap());
        assert_eq!(original_uv_endpoints_equal(&curve, 2.0, &point, 0.0,
            &mut GeometryWorkBudget::new(0), &cancel).unwrap_err().code, GeometryErrorCode::ResourceLimit);
        assert_eq!(original_uv_endpoints_equal(&curve, 2.0, &point, 0.0,
            &mut work, &AtomicBool::new(true)).unwrap_err().code, GeometryErrorCode::Cancelled);
    }

    #[test]
    fn original_uv_affine_uses_every_polynomial_and_homogeneous_coefficient() {
        let knots = vec![-2.0, -2.0, -2.0, 0.0, 4.0, 4.0, 4.0];
        let points: Vec<_> = [-2.0, -1.0, 2.0, 4.0].into_iter()
            .map(|u| [u + 2.0, 2.0 * u + 1.0, NOW]).collect();
        let weighted: Vec<_> = points.iter().map(|p| [2.0 * p[0], 2.0 * p[1], 2.0]).collect();
        let polynomial = uv_curve(&points, 2, knots.clone());
        let rational = uv_curve(&weighted, 2, knots.clone());
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        assert!(original_uv_affine(&polynomial, &mut work, &cancel).unwrap());
        assert!(original_uv_affine(&rational, &mut work, &cancel).unwrap());
        let mut bent = points;
        bent[1][0] = bent[1][0].next_up();
        assert!(!original_uv_affine(&uv_curve(&bent, 2, knots.clone()), &mut work, &cancel).unwrap());
        let mut nonconstant_weight = weighted;
        nonconstant_weight[1][2] = 3.0;
        assert!(!original_uv_affine(&uv_curve(&nonconstant_weight, 2, knots), &mut work, &cancel).unwrap());
    }

    #[test]
    fn original_uv_affine_preserves_declared_native_domains_and_typed_limits() {
        // The native representation stores the realized affine coefficients;
        // keep its nonunit active restriction, reflection and pose literal.
        let curve = analytic_line([5.0, 3.0, 0.0], [2.0, -2.0, 0.0], [-1.0, 2.0], true, 2);
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        assert!(original_uv_affine(&curve, &mut work, &cancel).unwrap());
        assert!(!original_uv_endpoints_equal(&curve, -2.0, &curve, -1.0, &mut work, &cancel).unwrap());
        let projective = uv_curve(&[[-1.0, -3.0, 0.75], [5.0, 9.0, 2.25]],
            1, vec![-2.0, -2.0, 4.0, 4.0]);
        assert!(!original_uv_affine(&projective, &mut work, &cancel).unwrap());
        assert_eq!(original_uv_affine(&curve, &mut GeometryWorkBudget::new(0), &cancel).unwrap_err().code,
            GeometryErrorCode::ResourceLimit);
        assert_eq!(original_uv_affine(&curve, &mut work, &AtomicBool::new(true)).unwrap_err().code,
            GeometryErrorCode::Cancelled);
    }

    #[test]
    fn exact_line_lane_uses_coefficient_identity_not_endpoint_nearness() {
        let curve = analytic_line([0.0; 3], [2.0, 3.0, 4.0], [0.0, 1.0], false, 3);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        assert!(affine_line_direction(&curve, [0.0, 1.0], [0.0; 3], [2.0, 3.0, 4.0], &mut work, &cancel).unwrap().is_some());
        assert!(affine_line_direction(&curve, [0.0, 1.0], [f64::from_bits(1), 0.0, 0.0],
            [2.0, 3.0, 4.0], &mut work, &cancel).unwrap().is_none());
        assert!(affine_line_direction(&curve, [0.0, 1.0], [2.0, 3.0, 4.0], [0.0; 3],
            &mut work, &cancel).unwrap().is_none());
        let zero = native_curve(vec![CPoint::new(0.0, 0.0, 0.0, NOW); 2], 1,
            vec![0.0, 0.0, 1.0, 1.0], 3);
        assert!(affine_line_direction(&zero, [0.0, 1.0], [0.0; 3], [0.0; 3], &mut work, &cancel).unwrap().is_none());
    }

    #[test]
    fn complete_affine_tensor_coefficients_cover_nonunit_knots_and_constant_weights() {
        let knots = [vec![-2.0, -2.0, -2.0, 0.0, 4.0, 4.0, 4.0], vec![1.0, 1.0, 3.0, 3.0]];
        let points: Vec<Vec<_>> = [-2.0, -1.0, 2.0, 4.0].into_iter().map(|u|
            [1.0, 3.0].into_iter().map(|v| CPoint::new(u + 2.0, v - 1.0, u + 2.0 * v, NOW)).collect()).collect();
        let polynomial = native_tensor(points.clone(), [2, 1], knots.clone());
        let weighted = points.iter().map(|row| row.iter()
            .map(|p| CPoint::new(2.0 * p.x, 2.0 * p.y, 2.0 * p.z, 2.0)).collect()).collect();
        let rational = native_tensor(weighted, [2, 1], knots.clone());
        let trace = uv_line([-2.0, 1.0], [4.0, 3.0]);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        for surface in [&polynomial, &rational] {
            let affine = exact_affine_surface(surface, &mut work, &cancel).unwrap().unwrap();
            assert!(exact_affine_trace(&affine, &trace, [0.0, 1.0],
                [[0.0; 3], [6.0, 2.0, 10.0]], &mut work, &cancel).unwrap());
            assert!(length(exact_affine_normal(&affine, &mut work, &cancel).unwrap().unwrap()).unwrap().lo > 0.0);
            assert!(!exact_affine_trace(&affine, &trace, [0.0, 1.0],
                [[0.0; 3], [6.0, 2.0, 10.0_f64.next_up()]], &mut work, &cancel).unwrap());
        }
        let mut wrong = points;
        wrong[1][0].z = wrong[1][0].z.next_up();
        assert!(exact_affine_surface(&native_tensor(wrong, [2, 1], knots), &mut work, &cancel).unwrap().is_none());
    }

    #[test]
    fn exact_affine_trace_composes_original_homogeneous_uv_coefficients() {
        let plane = plane();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let affine = exact_affine_surface(&plane, &mut work, &cancel).unwrap().unwrap();
        let chart = |s| uv_curve(&[[s, s, 3.0 * s], [4.0 * s, 4.0 * s, 3.0 * s]],
            1, vec![-2.0, -2.0, 4.0, 4.0]);
        for scale in [-2.0, 1.0] {
            assert!(exact_affine_trace(&affine, &chart(scale), [-2.0, 4.0],
                [[11.0, 22.0, 30.0], [14.0, 28.0, 30.0]], &mut work, &cancel).unwrap());
            assert!(!exact_affine_trace(&affine, &chart(scale), [-2.0, 4.0],
                [[11.0, 22.0, 30.0], [14.0_f64.next_up(), 28.0, 30.0]], &mut work, &cancel).unwrap());
        }
        // Exact realized native coefficients of the original reflected,
        // translated homogeneous chart on its nonunit active restriction.
        let reflected = uv_curve(&[[12.0, 18.0, 6.0], [9.0, 15.0, 6.0]],
            1, vec![-1.0, -1.0, 2.0, 2.0]);
        assert!(exact_affine_trace(&affine, &reflected, [-1.0, 2.0],
            [[16.0, 38.0, 30.0], [14.5, 35.0, 30.0]], &mut work, &cancel).unwrap());
    }

    #[test]
    fn native_planar_rational_chart_has_exact_affine_trace() {
        let surface = plane();
        let chart = uv_curve(&[[1.0, 1.0, 3.0], [4.0, 4.0, 3.0]],
            1, vec![0.0, 0.0, 1.0, 1.0]);
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let affine = exact_affine_surface(&surface, &mut work, &cancel).unwrap().unwrap();
        assert!(exact_affine_trace(&affine, &chart, [0.0, 1.0],
            [[11.0, 22.0, 30.0], [14.0, 28.0, 30.0]], &mut work, &cancel).unwrap());
    }

    #[test]
    fn affine_line_operators_retain_exact_pose_and_declared_reflection() {
        let curve = analytic_line([10.0, 2.0, 3.0], [1.0, 0.0, 0.0], [2.0, 5.0], true, 3);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        assert!(affine_line_direction(&curve, [2.0, 5.0], [15.0, 2.0, 3.0], [12.0, 2.0, 3.0],
            &mut work, &cancel).unwrap().is_some());
        assert!(affine_line_direction(&curve, [2.0, 5.0], [12.0, 2.0, 3.0], [15.0, 2.0, 3.0],
            &mut work, &cancel).unwrap().is_none());
        assert_eq!(affine_line_direction(&curve, [2.0, 5.0], [15.0, 2.0, 3.0], [12.0, 2.0, 3.0],
            &mut GeometryWorkBudget::new(0), &cancel).unwrap_err().code, GeometryErrorCode::ResourceLimit);
        assert_eq!(affine_line_direction(&curve, [2.0, 5.0], [15.0, 2.0, 3.0], [12.0, 2.0, 3.0],
            &mut work, &AtomicBool::new(true)).unwrap_err().code, GeometryErrorCode::Cancelled);
    }

    #[test]
    fn complete_exact_original_affine_corner_has_positive_active_degree() {
        let bytes = std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/geometry/box-mm.step")).unwrap();
        let cancel = AtomicBool::new(false);
        let definition = crate::from_step_bytes_with_cancel(&bytes, &cancel).unwrap();
        let mut work = GeometryWorkBudget::new(100_000_000);
        let prepared = crate::analysis::prepare_solid(&definition, crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work, &cancel).unwrap();
        let bounds = definition.bounds_mm();
        let height = (bounds.min[2] + bounds.max[2]) * 0.5;
        let request = SlabRequest {
            frame: PlaneFrameMm { origin_mm: [0.0; 3], x_axis: [1.0, 0.0, 0.0],
                y_axis: [0.0, 1.0, 0.0], z_axis: [0.0, 0.0, 1.0] },
            height_interval_mm: [bounds.min[2], bounds.max[2]], sample_height_mm: height,
            native_shape_error_mm: 0.1,
        };
        let section = prepared.section_for_query(&crate::section::SectionQuery {
            plane: crate::PlaneMm { origin_mm: [0.0, 0.0, height], normal: [0.0, 0.0, 1.0] },
            maximum_chord_error_mm: spiling_contracts::geometry::SECTION_SAMPLING_TOLERANCE_MM,
            maximum_plane_error_mm: spiling_contracts::geometry::SECTION_PLANE_TOLERANCE_MM,
        }, &cancel, (crate::MAX_PREPARED_SECTION_BYTES - prepared.retained_bytes()) as u32,
            &mut work).unwrap();
        let prism = SamplePrism::new(&section, &request, HeightFunctional::from_frame(request.frame),
            Interval::point(height), &mut work, &cancel).unwrap();
        let source = prepared.source_view().unwrap();
        let mut incidence = SourceEdgeIncidence::new(&definition, MAX_EDGE_BYTES, &mut work, &cancel).unwrap();
        incidence.prepare_vertices(MAX_EDGE_BYTES, &mut work, &cancel).unwrap();
        for vertex in incidence.vertices().unwrap() {
            let count = incidence.vertex_edges(vertex).count();
            assert!(exact_affine_vertex_fan(&definition, source, &incidence, vertex, count,
                &prism, MAX_SCRATCH_BYTES, &mut work, &cancel).unwrap());
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
