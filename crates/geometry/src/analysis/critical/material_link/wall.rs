// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Relative, complete original-wall authority. Azimuth is a physical witness,
//! never a source-shape selector. All geometry is evaluated by the shared raw,
//! collar and conical source recipes; failure of this witness refuses the lane.

use super::*;
use crate::analysis::material_source::MaterialSourceView;
use crate::analysis::source_incidence::{native_coedge, native_face};

type Box3 = [Interval; 3];
const STACK_BYTES: usize = std::mem::size_of::<[([[f64; 2]; 2], u8); MAX_DEPTH as usize + 1]>();

fn refusal(message: &'static str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::AnalysisIndeterminate, message)
}
fn resource() -> GeometryError {
    GeometryError::new(
        GeometryErrorCode::ResourceLimit,
        "complete source wall proof exhausts remaining storage",
    )
}
fn hull(a: &mut Option<Box3>, b: Box3) {
    *a = Some(match *a {
        Some(a) => std::array::from_fn(|i| a[i].hull(b[i])),
        None => b,
    });
}
fn disjoint(a: Box3, b: Box3) -> bool {
    (0..3).any(|i| a[i].hi < b[i].lo || b[i].hi < a[i].lo)
}
fn inward(position: Box3, normal: Box3, center: [f64; 2]) -> bool {
    // The Jacobian of (physical azimuth, actual source height) is, up
    // to its strictly positive angular denominator, n dot (x-cx,y-cy,0).
    let radial = [
        position[0].sub(Interval::point(center[0])),
        position[1].sub(Interval::point(center[1])),
        Interval::point(0.0),
    ];
    dot(normal, radial).hi < 0.0
}
fn allocate<T: Clone>(count: usize, value: T, bytes: &mut usize) -> Result<Vec<T>, GeometryError> {
    let needed = count
        .checked_mul(std::mem::size_of::<T>())
        .ok_or_else(resource)?;
    *bytes = bytes.checked_sub(needed).ok_or_else(resource)?;
    let mut result = Vec::new();
    result.try_reserve_exact(count).map_err(|_| resource())?;
    *bytes = bytes
        .checked_sub(
            result
                .capacity()
                .saturating_sub(count)
                .saturating_mul(std::mem::size_of::<T>()),
        )
        .ok_or_else(resource)?;
    result.resize(count, value);
    Ok(result)
}

fn record_box(output: &mut Vec<Box3>, p: Box3) -> Result<(), GeometryError> {
    if output.len() == output.capacity() {
        let limit = MAX_CORRECTION_BYTES / std::mem::size_of::<Box3>();
        let capacity = output.capacity().saturating_mul(2).max(32).min(limit);
        if capacity <= output.capacity() {
            return Err(resource());
        }
        output
            .try_reserve_exact(capacity - output.len())
            .map_err(|_| resource())?;
        if output.capacity() > limit {
            return Err(resource());
        }
    }
    output.push(p);
    Ok(())
}

/// An isolated single wall sheet on the ENTIRE original source column, with
/// its actual finite material endpoint sheets retained. No error bound can
/// construct this receipt: error is consumed only after this function returns.
#[allow(clippy::too_many_arguments)]
pub(crate) fn prove_source_wall_component(
    source: MaterialSourceView<'_, '_>,
    faces: &[usize],
    endpoints: [f64; 2],
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    work.charge(1, cancel)?;
    let definition = source.definition();
    if faces.is_empty()
        || endpoints.iter().any(|x| !x.is_finite())
        || endpoints[0] >= endpoints[1]
        || faces
            .iter()
            .enumerate()
            .any(|(i, &f)| f >= definition.face_ids().len() || faces[..i].contains(&f))
    {
        return Err(refusal(
            "source wall proof has no complete finite original component",
        ));
    }
    let mut available = remaining_bytes
        .checked_sub(STACK_BYTES)
        .and_then(|bytes| bytes.checked_sub(transport::EVALUATION_SCRATCH_BYTES))
        .ok_or_else(resource)?;
    let mut incidence = SourceEdgeIncidence::new(definition, available, work, cancel)?;
    incidence.prepare_vertices(available, work, cancel)?;
    available = available
        .checked_sub(incidence.owned_bytes())
        .ok_or_else(resource)?;
    let mut constants = allocate(definition.face_ids().len(), None, &mut available)?;
    let mut others = Vec::new();
    // Reserve the existing complete correction cap before native corner
    // plans are live. Nonwall cells use the same bounded storage allowance;
    // neither collector may overlap a live plan's scratch accounting.
    let other_capacity = MAX_CORRECTION_BYTES / std::mem::size_of::<Box3>();
    available = available
        .checked_sub(other_capacity.saturating_mul(std::mem::size_of::<Box3>()))
        .and_then(|n| n.checked_sub(MAX_CORRECTION_BYTES))
        .ok_or_else(resource)?;
    let mut cap_signs = allocate(definition.face_ids().len(), 0_i8, &mut available)?;
    let height = HeightFunctional {
        direction: [0.0, 0.0, 1.0].map(Interval::point),
        offset: Interval::point(0.0),
    };
    for (fi, slot) in constants.iter_mut().enumerate() {
        if !faces.contains(&fi) {
            *slot = constant_material_face_height(source, fi, height, work, cancel)?
                .filter(|h| endpoints.contains(h));
        }
    }
    // Every incident boundary use must either continue this SAME component or
    // be an exact endpoint sheet. Paired NativeUse IDs, not positional welding,
    // enumerate all exits, including extra wires and closed-edge endpoint germs.
    let mut lower_seen = false;
    let mut upper_seen = false;
    for uses in incidence.values() {
        work.charge(1, cancel)?;
        let second = uses
            .second
            .ok_or_else(|| refusal("source wall inventory has an unpaired original use"))?;
        if uses.first.orientation == second.orientation {
            return Err(refusal(
                "source wall inventory has nonopposed original uses",
            ));
        }
        let a = faces.contains(&uses.first.face);
        let b = faces.contains(&second.face);
        if a == b {
            continue;
        }
        let other = if a { second.face } else { uses.first.face };
        match constants[other] {
            Some(h) if h == endpoints[0] => lower_seen = true,
            Some(h) if h == endpoints[1] => upper_seen = true,
            _ => {
                return Err(refusal(
                    "source wall has a nonterminal incident boundary exit",
                ));
            }
        }
    }
    if !lower_seen || !upper_seen {
        return Err(refusal(
            "source wall lacks both complete exact endpoint boundaries",
        ));
    }
    // A proposal from the COMPLETE source wall image, not sample-loop shape
    // dispatch. Strict coefficient Jacobians below independently certify it.
    let mut raw_wall = None;
    for &fi in faces {
        visit_raw(source, fi, available, work, cancel, &mut |p, _| {
            hull(&mut raw_wall, p);
            Ok(true)
        })?;
    }
    let raw_wall = raw_wall.ok_or_else(|| refusal("source wall has no original position cover"))?;
    let center = std::array::from_fn(|i| raw_wall[i].lo * 0.5 + raw_wall[i].hi * 0.5);
    let mut wall = None;
    // Complete raw carrier/trim covers remain a conservative over-cover where
    // G replaces them. No raw coefficient root is used until ALL replacements
    // have been separately excluded from the identical calibration fiber.
    for fi in 0..definition.face_ids().len() {
        if !faces.contains(&fi) && constants[fi].is_none() {
            continue;
        }
        visit_raw(source, fi, available, work, cancel, &mut |p, n| {
            if faces.contains(&fi) {
                if !inward(p, n, center) {
                    return Ok(false);
                }
                hull(&mut wall, p);
            } else {
                let sign = if n[2].lo > 0.0 {
                    1
                } else if n[2].hi < 0.0 {
                    -1
                } else {
                    return Ok(false);
                };
                if cap_signs[fi] != 0 && cap_signs[fi] != sign {
                    return Ok(false);
                }
                cap_signs[fi] = sign;
            }
            Ok(true)
        })?;
    }
    let mut links = coverage::ClosedRadialLinks::geometry(
        source,
        &incidence,
        available,
        work,
        cancel,
        &mut |fi, p, n| {
            if faces.contains(&fi) {
                if !inward(p, n, center) {
                    return Err(refusal(
                        "actual source cone does not certify the whole wall azimuth-height Jacobian",
                    ));
                }
                hull(&mut wall, p);
            } else if constants[fi].is_some() {
                if (cap_signs[fi] > 0 && n[2].lo <= 0.0) || (cap_signs[fi] < 0 && n[2].hi >= 0.0) {
                    return Err(refusal(
                        "actual endpoint cone lacks the retained planar material orientation",
                    ));
                }
            } else {
                record_box(&mut others, p)?;
            }
            Ok(())
        },
    )?;
    let allowance = source_allowance(definition)?;
    for uses in incidence.values() {
        let second = uses
            .second
            .ok_or_else(|| refusal("source collar has no opposed owner"))?;
        for use_ in [uses.first, second] {
            let domain = source.face_domain(use_.face, available, work, cancel)?;
            let scratch = available
                .checked_sub(UV_EMBEDDING_STACK_BYTES)
                .and_then(|bytes| bytes.checked_sub(std::mem::size_of::<OwnedUvRibbon<'_, '_>>()))
                .ok_or_else(resource)?;
            let collar =
                OriginalCollar::for_enclosure_with_byte_limit(domain, use_, scratch, work, cancel)?
                    .ok_or_else(|| {
                        refusal("source wall constructor has no original collar geometry")
                    })?;
            let (edge_use, edge) = native_coedge(definition, use_.face, use_.wire, use_.index)?;
            let coedge = domain
                .coedges()
                .get(use_.coedge)
                .ok_or_else(|| refusal("source wall lost its original coedge binding"))?;
            if coedge.edge != edge_use.edge || coedge.edgeuse != use_.edgeuse {
                return Err(refusal(
                    "source wall belongs to another original native use",
                ));
            }
            let correspondence = domain.original_coedge_correspondence(use_.coedge)?;
            let binding = correspondence
                .binding()
                .ok_or_else(|| refusal("actual source collar lost its original native binding"))?;
            if binding.edge != edge_use.edge
                || binding.owner != [use_.face, use_.wire, use_.index]
                || binding.endpoints != edge.vertices
                || binding.native_interval != [edge.interval.min(), edge.interval.max()]
                || correspondence.spatial_range() != collar.range
            {
                return Err(refusal(
                    "actual source collar belongs to a different original native use",
                ));
            }
            let cuts =
                original_coedge_parameter_cuts([(domain, use_.coedge); 2], scratch, work, cancel)?;
            let scratch = scratch
                .checked_sub(cuts.capacity().saturating_mul(std::mem::size_of::<f64>()))
                .ok_or_else(resource)?;
            let central = links.central_spatial_range(
                edge_use.edge,
                correspondence.spatial_range(),
                [use_.face, use_.wire, use_.index],
            )?;
            let embedding = collar
                .owned_embedding(central, scratch, work, cancel)?
                .ok_or_else(|| {
                    refusal("source wall constructor has no complete boundary-owned UV ribbon")
                })?;
            let provider_query_bytes = scratch
                .checked_add(transport::EVALUATION_SCRATCH_BYTES)
                .ok_or_else(resource)?;
            domain.with_original_spatial_curve(
                use_.coedge, provider_query_bytes, work, cancel,
                |prepared, work, cancel| {
                    let spatial_range = correspondence.spatial_range();
                    // The complete selected source law must be C0 before its
                    // one-sided closed jets can support a wall topology proof.
                    if !crate::parametric::curve_parameter_span_continuous(
                        prepared, spatial_range, work, cancel,
                    )? {
                        return Err(refusal("source wall spatial provider has no whole-branch continuity proof"));
                    }
                    crate::parametric::curve_enclosure(
                        prepared, (spatial_range[0], spatial_range[1]), work, cancel,
                    )?;
                    for span in crate::parametric::curve_parameter_spans(
                        prepared, spatial_range, work, cancel,
                    )? {
                        let lo = span[0].max(central[0]);
                        let hi = span[1].min(central[1]);
                        if lo >= hi { continue; }
                        let first = cuts.partition_point(|cut| *cut <= lo);
                        let last = cuts.partition_point(|cut| *cut < hi).max(first);
                        let mut start = lo;
                        for end in cuts[first..last].iter().copied().chain(std::iter::once(hi)) {
                            let mut stack = [([0.0; 2], 0_u8); MAX_DEPTH as usize + 1];
                            stack[0] = ([start, end], 0);
                            start = end;
                            let mut pending = 1;
                            while pending != 0 {
                                pending -= 1;
                                let (range, depth) = stack[pending];
                                work.charge(1, cancel)?;
                                let jet = crate::parametric::curve_enclosure(
                                    prepared, (range[0], range[1]), work, cancel,
                                )?;
                                let cell = collar.geometry_cell(&embedding, range, &jet, allowance, work, cancel)?;
                                let acceptable = cell.as_ref().is_some_and(|c| {
                                    if faces.contains(&use_.face) { inward(c.position, c.outward, center) }
                                    else if constants[use_.face].is_some() {
                                        if cap_signs[use_.face] > 0 { c.outward[2].lo > 0.0 } else { c.outward[2].hi < 0.0 }
                                    } else { wall.is_some_and(|w| disjoint(c.position, w)) }
                                });
                                if acceptable {
                                    let cell = cell.expect("acceptable actual collar retains its geometry");
                                    links.record(&cell, work, cancel)?;
                                    if faces.contains(&use_.face) { hull(&mut wall, cell.position); }
                                    else if constants[use_.face].is_none() {
                                        record_box(&mut others, cell.position)?;
                                    }
                                    continue;
                                }
                                let mid = range[0] * 0.5 + range[1] * 0.5;
                                if depth >= MAX_DEPTH || mid <= range[0] || mid >= range[1] {
                                    return Err(refusal("actual source collar lacks complete wall/endpoint local geometry"));
                                }
                                stack[pending] = ([range[0], mid], depth + 1);
                                stack[pending + 1] = ([mid, range[1]], depth + 1);
                                pending += 2;
                            }
                        }
                    }
                    Ok(())
                },
            )?;
        }
    }
    let wall = wall.ok_or_else(|| refusal("source wall has no actual complete geometry cover"))?;
    // This ONE-envelope clearance is deliberately conservative. It excludes
    // EVERY other actual source stratum from the isolated wall, not just an E
    // tube around a sampled contour. Exact endpoint sheets are retained below,
    // and are omitted here ONLY from the open regular source-height band.
    for &p in &others {
        work.charge(1, cancel)?;
        if !disjoint(p, wall) {
            return Err(refusal(
                "nonincident actual source correction is not separated from the complete void wall",
            ));
        }
    }
    for fi in 0..definition.face_ids().len() {
        if faces.contains(&fi) || constants[fi].is_some() {
            continue;
        }
        visit_raw(source, fi, available, work, cancel, &mut |p, _| {
            Ok(disjoint(p, wall))
        })?;
    }
    // A material-facing endpoint sheet must be an actual disk whose ENTIRE
    // original boundary is this wall. A cap wire inside a larger face is an
    // opening, not a pocket floor. Extra floor holes/exits refuse the theorem.
    for (fi, constant) in constants.iter().enumerate() {
        let Some(h) = constant else {
            continue;
        };
        let floor =
            (*h == endpoints[0] && cap_signs[fi] > 0) || (*h == endpoints[1] && cap_signs[fi] < 0);
        if !floor {
            continue;
        }
        let (_, face) = native_face(definition, fi)?;
        let incident = incidence.values().any(|u| {
            let Some(b) = u.second else {
                return false;
            };
            (u.first.face == fi && faces.contains(&b.face))
                || (b.face == fi && faces.contains(&u.first.face))
        });
        if !incident {
            continue;
        }
        if face.loops.len() != 1 {
            return Err(refusal(
                "actual material floor has extra original boundary exits",
            ));
        }
        let loop_ = definition.brep().loop_(face.loops[0]).map_err(|error| {
            GeometryError::new(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                format!("source floor original loop is unavailable: {error}"),
            )
        })?;
        for edge in &loop_.edges {
            let uses = incidence
                .get(edge.edge)
                .ok_or_else(|| refusal("actual floor boundary lost its original use"))?;
            let second = uses
                .second
                .ok_or_else(|| refusal("actual floor boundary is unpaired"))?;
            let neighbor = if uses.first.face == fi {
                second.face
            } else {
                uses.first.face
            };
            if !faces.contains(&neighbor) {
                return Err(refusal(
                    "actual material floor boundary is not owned entirely by this void wall",
                ));
            }
        }
    }
    // Compact native manifold-with-boundary + complete opposed uses, conical
    // fan closure and strict SAME-oriented local Jacobians make Psi a proper
    // local homeomorphism on the OPEN endpoint cylinder. The exact coefficient
    // laws above put ALL boundary exits on its two actual endpoint circles.
    // One COMPLETE unchanged-coefficient regular fiber now calibrates its total
    // covering cardinality, not a sampled loop count or cyclic source labels.
    let request = SlabRequest {
        frame: PlaneFrameMm {
            origin_mm: [0.0; 3],
            x_axis: [1.0, 0.0, 0.0],
            y_axis: [0.0, 1.0, 0.0],
            z_axis: [0.0, 0.0, 1.0],
        },
        height_interval_mm: endpoints,
        sample_height_mm: endpoints[0] * 0.5 + endpoints[1] * 0.5,
        native_shape_error_mm: allowance,
    };
    for fraction in [0.5, 0.375, 0.625] {
        let z = endpoints[0] * (1.0 - fraction) + endpoints[1] * fraction;
        for alpha in [0.0, 1.0, -1.0, 0.5, -0.5, 2.0, -2.0, 0.2718281828459045] {
            work.charge(1, cancel)?;
            let rows = [[-alpha, 1.0, 0.0], [0.0, 0.0, 1.0]].map(|r| r.map(Interval::point));
            let fiber = fiber::OriginalFiber {
                center: [center[0], center[1], z],
                rows,
                offsets: [Interval::point(0.0); 2],
                forward: cross(rows[0], rows[1]),
                forward_offset: Interval::point(0.0),
                min_forward: 0.0,
                core: None,
            };
            if !links.misses_all_supports(&fiber, work, cancel)? {
                continue;
            }
            if fiber::original_wall_fiber_degree_one(
                source, faces, &fiber, &request, height, available, work, cancel,
            )?
            .is_some()
            {
                return Ok(());
            }
        }
    }
    Err(refusal(
        "complete actual void wall has no support-disjoint degree-one original fiber",
    ))
}

fn visit_raw(
    source: MaterialSourceView<'_, '_>,
    fi: usize,
    remaining: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    visitor: &mut impl FnMut(Box3, Box3) -> Result<bool, GeometryError>,
) -> Result<(), GeometryError> {
    let domain = source.face_domain(fi, remaining, work, cancel)?;
    let surface = domain.surface_operand();
    let orientation = domain.orientation();
    {
        for span in surface_parameter_boxes(&surface, domain.uv_domain(), work, cancel)? {
            let smooth = surface_parameter_span_smooth(&surface, span, work, cancel)?;
            let mut stack = [([[0.0; 2]; 2], 0_u8); MAX_DEPTH as usize + 1];
            stack[0] = (span, 0);
            let mut pending = 1;
            while pending != 0 {
                pending -= 1;
                let (uv, depth) = stack[pending];
                work.charge(1, cancel)?;
                if domain.classify(uv, work, cancel)? == TrimClassification::Outside {
                    continue;
                }
                let jet = if smooth {
                    surface_enclosure(&surface, uv, work, cancel)?
                } else {
                    crate::parametric::surface_piecewise_enclosure(&surface, uv, work, cancel)?
                };
                if visitor(jet.position, normal(&jet, orientation))? {
                    continue;
                }
                if depth >= MAX_DEPTH {
                    return Err(refusal(
                        "complete original source wall/clearance cover is unresolved",
                    ));
                }
                let children = split(uv, depth, fi)?;
                for uv in children {
                    stack[pending] = (uv, depth + 1);
                    pending += 1;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_wall_witness_rejects_outward_folds_axis_and_uncertain_sign() {
        let point = [2.0, 0.0, 5.0].map(Interval::point);
        let normal = [-1.0, 0.0, 0.0].map(Interval::point);
        assert!(inward(point, normal, [0.0; 2]));
        assert!(!inward(point, normal.map(Interval::neg), [0.0; 2]));
        assert!(!inward(point, normal, [2.0, 0.0]));
        assert!(!inward(
            point,
            [
                Interval { lo: -1.0, hi: 0.0 },
                Interval::point(0.0),
                Interval::point(0.0),
            ],
            [0.0; 2]
        ));
    }

    #[test]
    fn closed_nonincident_clearance_does_not_discard_contact() {
        let wall = [
            Interval { lo: -2.0, hi: 2.0 },
            Interval { lo: -2.0, hi: 2.0 },
            Interval { lo: 4.0, hi: 8.0 },
        ];
        assert!(!disjoint(wall, [2.0, 0.0, 6.0].map(Interval::point)));
        assert!(!disjoint(wall, [0.0, 0.0, 4.0].map(Interval::point)));
        assert!(disjoint(wall, [3.0, 0.0, 6.0].map(Interval::point)));
        // Only the coefficient-supported endpoint law, NOT a nearzero scalar
        // interval or a closed-box contact, can remove a floor from open flow.
        assert!(!disjoint(
            wall,
            [
                Interval::point(0.0),
                Interval::point(0.0),
                Interval {
                    lo: 3.999999999999999,
                    hi: 4.000000000000001
                },
            ]
        ));
    }
}
