// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Complete native wall-component authority. Physical azimuth is a witness,
//! never a source-family selector. Whole native interval covers, paired
//! edgeuses and an independent complete physical fiber census are mandatory.

use super::*;
use crate::analysis::material_source::MaterialSourceView;
use crate::analysis::source_incidence::native_face;

type Box3 = [Interval; 3];
const STACK_BYTES: usize = std::mem::size_of::<[([[f64; 2]; 2], u8); MAX_DEPTH as usize + 1]>()
    + UV_EMBEDDING_STACK_BYTES
    + std::mem::size_of::<PairedMaterialLink<'_, '_>>();

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

/// An isolated single wall sheet on the ENTIRE native material column, with
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
        .ok_or_else(resource)?;
    let mut incidence = SourceEdgeIncidence::new(definition, available, work, cancel)?;
    incidence.prepare_vertices(available, work, cancel)?;
    available = available
        .checked_sub(incidence.owned_bytes())
        .ok_or_else(resource)?;
    let mut constants = allocate(definition.face_ids().len(), None, &mut available)?;
    let mut cap_signs = allocate(definition.face_ids().len(), 0_i8, &mut available)?;
    let height = HeightFunctional {
        direction: [0.0, 0.0, 1.0].map(Interval::point),
        offset: Interval::point(0.0),
    };
    for (fi, slot) in constants.iter_mut().enumerate() {
        if !faces.contains(&fi) {
            *slot = super::super::super::slab::constant_source_face_height(
                definition, fi, height, work, cancel,
            )?
            .filter(|h| h.lo == h.hi && endpoints.contains(&h.lo))
            .map(|h| h.lo);
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
    // Complete native carrier/trim covers conservatively enclose position.
    // Their raw-face census gains material degree only after the IDENTICAL
    // calibration fiber misses every native vertex coincidence zone below.
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
    let allowance = source.trim_source().effective_tolerance_mm();
    for uses in incidence.values() {
        let second = uses
            .second
            .ok_or_else(|| refusal("native wall has an unpaired edge"))?;
        if !faces.contains(&uses.first.face) && !faces.contains(&second.face) {
            continue;
        }
        let a = source.face_domain(uses.first.face, available, work, cancel)?;
        let b = source.face_domain(second.face, available, work, cancel)?;
        let range = a.coedges()[uses.first.coedge].native_range;
        let link = PairedMaterialLink::new(
            definition,
            (a, uses.first),
            (b, second),
            range,
            available,
            work,
            cancel,
        )?
        .ok_or_else(|| refusal("native wall edge has inconsistent material/chart ownership"))?;
        let cuts = native_coedge_parameter_cuts(
            [(a, uses.first.coedge), (b, second.coedge)],
            available,
            work,
            cancel,
        )?;
        let scratch = available
            .checked_sub(cuts.capacity().saturating_mul(std::mem::size_of::<f64>()))
            .ok_or_else(resource)?;
        a.with_native_spatial_curve(
            uses.first.coedge,
            scratch,
            work,
            cancel,
            |curve, work, cancel| {
                for pair in cuts.windows(2) {
                    if pair[0] >= pair[1] {
                        continue;
                    }
                    let mut stack = [([0.0; 2], 0_u8); MAX_DEPTH as usize + 1];
                    stack[0] = ([pair[0], pair[1]], 0);
                    let mut pending = 1;
                    while pending != 0 {
                        pending -= 1;
                        let (span, depth) = stack[pending];
                        let jet = crate::parametric::curve_enclosure(
                            curve,
                            (span[0], span[1]),
                            work,
                            cancel,
                        )?;
                        let h = height.height(jet.position);
                        if h.hi <= endpoints[0] || h.lo >= endpoints[1] {
                            continue;
                        }
                        let mut accepted = false;
                        if let Ok((position, na, nb)) = link.normals(span, &jet, work, cancel)? {
                            accepted = (!faces.contains(&uses.first.face)
                                || inward(position, na, center))
                                && (!faces.contains(&second.face) || inward(position, nb, center));
                        }
                        if accepted {
                            continue;
                        }
                        let mid = span[0] * 0.5 + span[1] * 0.5;
                        if depth >= MAX_DEPTH || mid <= span[0] || mid >= span[1] {
                            return Err(refusal("native wall paired-edge geometry is unresolved"));
                        }
                        if pending + 2 > stack.len() {
                            return Err(resource());
                        }
                        stack[pending] = ([mid, span[1]], depth + 1);
                        stack[pending + 1] = ([span[0], mid], depth + 1);
                        pending += 2;
                    }
                }
                Ok(())
            },
        )?;
    }
    for vertex in incidence.vertices().ok_or_else(resource)? {
        let point = source
            .trim_source()
            .native_vertex_point(vertex, work, cancel)?;
        if point[2] <= endpoints[0] || point[2] >= endpoints[1] {
            continue;
        }
        if !incidence.vertex_edges(vertex).any(|edge| {
            incidence.get(edge).is_some_and(|uses| {
                faces.contains(&uses.first.face)
                    || uses.second.is_some_and(|use_| faces.contains(&use_.face))
            })
        }) {
            continue;
        }
        let rows = [
            [-(point[1] - center[1]), point[0] - center[0], 0.0].map(Interval::point),
            [0.0, 0.0, 1.0].map(Interval::point),
        ];
        if !vertex_fan::qualify_native_vertex(
            source, &incidence, vertex, rows, available, work, cancel,
        )? {
            return Err(refusal(
                "native wall vertex projection has no degree-one link",
            ));
        }
    }
    let wall = wall.ok_or_else(|| refusal("source wall has no actual complete geometry cover"))?;
    // This ONE-envelope clearance is deliberately conservative. It excludes
    // EVERY other actual source stratum from the isolated wall, not just an E
    // tube around a sampled contour. Exact endpoint sheets are retained below,
    // and are omitted here ONLY from the open regular source-height band.
    for (fi, _) in definition.face_ids().iter().enumerate() {
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
            let fiber = fiber::NativeFiber {
                center: [center[0], center[1], z],
                rows,
                offsets: [Interval::point(0.0); 2],
                forward: cross(rows[0], rows[1]),
                forward_offset: Interval::point(0.0),
                min_forward: 0.0,
                core: None,
            };
            if !coverage::native_fiber_misses_vertex_supports(
                source, &incidence, &fiber, work, cancel,
            )? {
                continue;
            }
            if fiber::native_wall_fiber_degree_one(
                source, faces, &fiber, &request, height, available, work, cancel,
            )?
            .is_some()
            {
                return Ok(());
            }
        }
    }
    Err(refusal(
        "complete actual void wall has no support-disjoint degree-one native fiber",
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
            let smooth = prepared_carrier_c1_covers(domain, span)
                || surface_parameter_span_smooth(&surface, span, work, cancel)?;
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
