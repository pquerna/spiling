// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::{
    roots::{self, ScalarJet, sign},
    *,
};
use crate::{
    PlaneFrameMm,
    parametric::{self as p, Interval, SurfaceEnclosure},
};
use solidcore::curve::curve::Curve;
use solidcore::nurbs::b_spline_surface_n_lib::p2::OrientType;
use solidcore::surface::{surface::Surface, surface::p2::SurfaceKind};
use solidcore::topology::EdgeuseId;
use solidcore::topology::shell::ShellMembers;
use solidcore::{Brep, EdgeId, FaceId};
use spiling_contracts::geometry::{MAX_SECTION_BYTES, SECTION_BOOLEAN_TOLERANCE_MM};

const Z: Interval = Interval::point(0.0);
fn dot(a: [Interval; 3], b: [f64; 3]) -> Interval {
    (0..3).fold(Z, |s, i| s.add(a[i].mul(Interval::point(b[i]))))
}
fn norm(a: [Interval; 3]) -> f64 {
    a.into_iter()
        .fold(Z, |s, v| s.add(v.square()))
        .sqrt()
        .map_or(f64::INFINITY, |v| v.hi)
}
fn functional(position: [Interval; 3], plane: PlaneMm) -> Interval {
    dot(
        std::array::from_fn(|i| position[i].sub(Interval::point(plane.origin_mm[i]))),
        plane.normal,
    )
}
fn spatial(jet: &mut SurfaceEnclosure, scale: f64) {
    for row in [
        &mut jet.position,
        &mut jet.du,
        &mut jet.dv,
        &mut jet.duu,
        &mut jet.duv,
        &mut jet.dvv,
    ] {
        for value in row {
            *value = value.mul(Interval::point(scale));
        }
    }
}
fn surface_jet(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    scale: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SurfaceEnclosure, GeometryError> {
    let mut jet = p::surface_piecewise_enclosure(surface, uv, work, cancel)?;
    spatial(&mut jet, scale);
    Ok(jet)
}
fn topology<T>(value: solidcore::error::Result<T>) -> Result<T, GeometryError> {
    value.map_err(|e| indeterminate(format!("native section topology: {e:?}")))
}
#[derive(Clone, Copy)]
pub(super) struct EdgeRoot {
    pub(super) edge: EdgeId,
    pub(super) ordinal: u32,
    pub(super) parameter: Interval,
    pub(super) position: [Interval; 3],
    pub(super) point: [f64; 3],
}
#[derive(Clone, Copy)]
pub(super) struct Event {
    pub(super) edgeuse: EdgeuseId,
    pub(super) root_index: usize,
    pub(super) parameter: Interval,
    pub(super) uv: [Interval; 2],
    pub(super) crossing: i8,
}
pub(super) struct Arc {
    pub(super) span: Option<SectionSpan>,
    pub(super) points: Vec<[f64; 3]>,
    pub(super) tangents: Vec<[Interval; 3]>,
}

pub(super) fn intersect(
    definition: &Definition,
    prepared: Option<&PreparedSectionSource<'_>>,
    query: &SectionQuery,
    byte_limit: usize,
    packed_byte_limit: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<NativeSection, GeometryError> {
    work.charge(1, cancel)?;
    if prepared.is_some_and(|source| !std::ptr::eq(source.definition(), definition)) {
        return Err(indeterminate(
            "native section receipt belongs to another definition",
        ));
    }
    if !query.maximum_chord_error_mm.is_finite()
        || query.maximum_chord_error_mm <= 0.0
        || query.maximum_chord_error_mm > SECTION_SAMPLING_TOLERANCE_MM
        || !query.maximum_plane_error_mm.is_finite()
        || query.maximum_plane_error_mm <= 0.0
        || query.maximum_plane_error_mm > SECTION_PLANE_TOLERANCE_MM
    {
        return Err(GeometryError::new(
            GeometryErrorCode::InvalidGeometry,
            "section query must narrow the frozen chord/plane allowances",
        ));
    }
    let limit = byte_limit.min(MAX_PREPARED_SECTION_BYTES);
    let mut admission = Admission::new(limit)?;
    admission.used = admission
        .used
        .checked_add(definition.id().allocated_bytes())
        .filter(|n| *n <= limit)
        .ok_or_else(|| resource_error("native section identity exceeds query allowance"))?;
    let plane = query.plane.normalized()?;
    let frame = PlaneFrameMm::from_plane(plane)?;
    // Import converts the BREP once. Source unit scale is provenance, not a
    // second conversion to apply to the native coefficient law.
    let scale = 1.0;
    let brep = definition.brep();
    let edgeuse_limit = coedge_limit(brep, work, cancel)?;
    let mut edge_roots = Vec::new();
    let mut arcs = Vec::new();
    let accuracy = query
        .maximum_chord_error_mm
        .min(query.maximum_plane_error_mm)
        / 64.0;
    // Spatial/chart displacement spends the chord ledger. Root localization
    // separately spends the smaller plane ledger; neither borrows a source
    // tolerance label, and actual endpoint displacement is charged below.
    let correspondence_allowance = query.maximum_chord_error_mm / 64.0;
    for (ordinal, &face_id) in definition.face_ids().iter().enumerate() {
        work.charge(1, cancel)?;
        let face = topology(brep.face(face_id))?;
        let surface = definition
            .face_surface(face_id)
            .ok_or_else(|| indeterminate("native section face has no surface"))?;
        let lo = face.uv_domain.min();
        let hi = face.uv_domain.max();
        let domain = [[lo.x, hi.x], [lo.y, hi.y]];
        let graph = match classify_graph(surface, domain, scale, plane, work, cancel) {
            Ok(None) => continue,
            Ok(Some(graph)) => Some(graph),
            Err(error) if error.code == GeometryErrorCode::AnalysisIndeterminate => None,
            Err(mut error) => {
                error.message = format!(
                    "native section face={ordinal} id={face_id:?}: {}",
                    error.message
                );
                return Err(error);
            }
        };
        let outward = face_orientation(brep, face_id)?;
        let mut events = Vec::new();
        let mut boundary_root_free_side: Option<i8> = None;
        for &loop_id in &face.loops {
            let loop_ = topology(brep.loop_(loop_id))?;
            let lu_id = loop_
                .loopuse
                .ok_or_else(|| indeterminate("native trim has no primary loopuse"))?;
            let lu = topology(brep.loopuse(lu_id))?;
            let solidcore::topology::LoopuseStart::Edgeuse(Some(first_eu)) = lu.start else {
                return Err(indeterminate(
                    "native trim requires an explicit coedge cycle",
                ));
            };
            let mut eu_id = first_eu;
            for _ in 0..edgeuse_limit {
                work.charge(1, cancel)?;
                let eu = topology(brep.edgeuse(eu_id))?;
                if eu.owner != solidcore::topology::EdgeuseOwner::Loopuse(Some(lu_id)) {
                    return Err(indeterminate("native trim cycle enters a foreign loopuse"));
                }
                let edge_id = eu
                    .edge
                    .ok_or_else(|| indeterminate("native coedge has no original edge"))?;
                let forward = match eu.orientation {
                    OrientType::Same => true,
                    OrientType::Opposite => false,
                    _ => {
                        return Err(indeterminate(
                            "native trim coedge has no traversal orientation",
                        ));
                    }
                };
                ensure_edge_roots(
                    definition,
                    edge_id,
                    plane,
                    accuracy,
                    &mut edge_roots,
                    &mut admission,
                    work,
                    cancel,
                )?;
                let binding =
                    Boundary::new(definition, surface, eu_id, edge_id, scale, work, cancel)?;
                let accepted = prepared
                    .map(|source| source.native_correspondence_allowance(eu_id))
                    .transpose()?;
                let cached = accepted.map(|bound| bound.get());
                let cache_hit = cached.is_some_and(|bound| bound <= correspondence_allowance);
                let proof = if cache_hit {
                    Ok(cached.unwrap())
                } else {
                    binding.prove_correspondence(
                        correspondence_allowance,
                        admission.limit.checked_sub(admission.used).ok_or_else(|| {
                            resource_error(
                                "native trim correspondence exceeds live query allowance",
                            )
                        })?,
                        work,
                        cancel,
                    )
                };
                if !cache_hit {
                    let achieved = proof.map_err(|mut error| {
                            error.message = format!(
                                "native section face={face_id:?} edge={edge_id:?} edgeuse={eu_id:?}: {}",
                                error.message
                            );
                            error
                        })?;
                    // This exact immutable native coedge's complete positional
                    // correspondence is plane independent. Reuse only a bound
                    // at least as strict as the current caller's allowance;
                    // roots, crossings, endpoint gaps and sections stay fresh.
                    if let Some(bound) = accepted {
                        bound.set(bound.get().min(achieved));
                    }
                }
                let mut boundary_side = 0;
                let chart_roots = roots::isolate(
                    std::iter::once([0.0, 1.0]),
                    &mut |range, work, cancel| {
                        let (position, derivative, _, _) = binding.evaluate(range, work, cancel)?;
                        let value = functional(position, plane);
                        if range == [0.0, 1.0] {
                            // Reuse the complete coedge enclosure. A sampled
                            // endpoint or an empty root list supplies no side.
                            boundary_side = sign(value);
                        }
                        Ok(ScalarJet {
                            value,
                            derivative: dot(derivative, plane.normal),
                        })
                    },
                    binding.parameter_accuracy(accuracy, work, cancel)?,
                    &mut admission,
                    work,
                    cancel,
                ).map_err(|mut failure| {
                    if failure.code == GeometryErrorCode::AnalysisIndeterminate {
                        failure.message = format!(
                            "native chart-boundary plane roots: face={ordinal}/{face_id:?} edge={edge_id:?} edgeuse={eu_id:?} spatial_kind={} chart_kind={:?} parameter=[0,1]; {}; source={} plane={plane:?}",
                            binding.curve.type_string(),
                            binding.uv_curve.map(Curve::type_string), failure.message,
                            definition.id().as_str(),
                        );
                    }
                    failure
                })?;
                boundary_root_free_side = Some(match boundary_root_free_side {
                    None => boundary_side,
                    Some(previous) if previous == boundary_side => previous,
                    Some(_) => 0,
                });
                let mut native_index = 0;
                for chart_root in chart_roots {
                    let root_index = edge_roots
                        .iter()
                        .enumerate()
                        .filter(|(_, r)| r.edge == edge_id)
                        .nth(native_index)
                        .map(|(i, _)| i)
                        .ok_or_else(|| {
                            indeterminate("native chart trim has an unmatched plane root")
                        })?;
                    native_index += 1;
                    let native_root = edge_roots[root_index];
                    let (position, _, uv, _) = binding.evaluate(
                        [chart_root.parameter.lo, chart_root.parameter.hi],
                        work,
                        cancel,
                    )?;
                    let gap = norm(std::array::from_fn(|i| {
                        position[i].sub(native_root.position[i])
                    }));
                    if gap > query.maximum_chord_error_mm / 16.0 {
                        return Err(indeterminate(
                            "native edge/chart plane roots lack a bounded common endpoint",
                        ));
                    }
                    let crossing = sign(chart_root.derivative) * if forward { 1 } else { -1 };
                    if crossing == 0 {
                        return Err(indeterminate("native trim root lacks a directed crossing"));
                    }
                    admission.push(
                        &mut events,
                        Event {
                            edgeuse: eu_id,
                            root_index,
                            parameter: chart_root.parameter,
                            uv,
                            crossing,
                        },
                    )?;
                }
                if native_index != edge_roots.iter().filter(|r| r.edge == edge_id).count() {
                    return Err(indeterminate(
                        "native spatial edge and chart trim have different complete root inventories",
                    ));
                }
                eu_id = eu
                    .ccw
                    .ok_or_else(|| indeterminate("native trim coedge cycle is open"))?;
                if eu_id == first_eu {
                    break;
                }
            }
            if eu_id != first_eu {
                return Err(indeterminate(
                    "native trim coedge cycle exceeds its declared census",
                ));
            }
        }
        let Some((axis, dependent_sign, root_domain)) = graph else {
            super::continuation::intersect_face(
                surface,
                domain,
                scale,
                plane,
                outward,
                face_id,
                ordinal,
                &events,
                &edge_roots,
                query,
                &mut admission,
                &mut arcs,
                work,
                cancel,
            )
            .map_err(|mut error| {
                error.message = format!(
                    "native section atlas face={ordinal} id={face_id:?}: {}",
                    error.message
                );
                error
            })?;
            continue;
        };
        for event in &mut events {
            work.charge(1, cancel)?;
            event.crossing *= dependent_sign * if axis == 1 { 1 } else { -1 };
        }
        if events.is_empty() {
            // The complete carrier has a strict height derivative. On the
            // compact native face a height extremum therefore lies on its
            // boundary. If all complete boundary curves are strictly on one
            // side, the trimmed face has no root, even when the inflated
            // carrier box does. This also requires periodic boundaries to
            // agree; a cylinder cut between its caps cannot pass this gate.
            if boundary_root_free_side.is_some_and(|side| side != 0) {
                continue;
            }
            // A necessary root overcover is not root-existence authority. Before
            // requiring trim incidences, try complete coefficient exclusion at
            // finer parameter resolution; absent crossings alone grants nothing.
            if exclude_carrier_roots(surface, root_domain, scale, plane, 0, work, cancel)? {
                continue;
            }
            return Err(indeterminate(format!(
                "native section face={ordinal} cannot exclude an interior/periodic plane root without certified trim incidences: chart={domain:?}, root_cover={root_domain:?}, dependent_axis={axis}"
            )));
        }
        let independent = 1 - axis;
        events.sort_by(|a, b| a.uv[independent].lo.total_cmp(&b.uv[independent].lo));
        if events.len() % 2 != 0 {
            return Err(indeterminate(
                "native face intersection has odd trim incidence",
            ));
        }
        for pair in events.as_chunks::<2>().0 {
            work.charge(1, cancel)?;
            let [a, b] = [pair[0], pair[1]];
            if a.crossing != -1 || b.crossing != 1 || a.uv[independent].hi >= b.uv[independent].lo {
                return Err(indeterminate(
                    "native trim roots do not prove ordered material entry/exit",
                ));
            }
            let x = [a.uv[independent].mid(), b.uv[independent].mid()];
            let roots = [edge_roots[a.root_index], edge_roots[b.root_index]];
            let mut points = Vec::new();
            let mut tangents = Vec::new();
            admission.push(&mut points, roots[0].point)?;
            let graph = Graph {
                surface,
                domain: root_domain,
                axis,
                scale,
                plane,
                dependent_sign,
            };
            let mut achieved = [0.0_f64; 2];
            let endpoint_errors = [a, b].map(|event| -> Result<f64, GeometryError> {
                let (_, enclosure) = graph.enclose(
                    [event.uv[independent].lo, event.uv[independent].hi],
                    work,
                    cancel,
                )?;
                Ok(norm(std::array::from_fn(|i| {
                    enclosure.position[i]
                        .sub(Interval::point(edge_roots[event.root_index].point[i]))
                })))
            });
            let [a_error, b_error] = endpoint_errors;
            graph.sample(
                x,
                roots[0].point,
                roots[1].point,
                a_error?,
                b_error?,
                query,
                0,
                &mut points,
                &mut tangents,
                &mut achieved,
                &mut admission,
                work,
                cancel,
            )?;
            for (index, event) in [a, b].into_iter().enumerate() {
                let (_, jet) = graph.enclose(
                    [event.uv[independent].lo, event.uv[independent].hi],
                    work,
                    cancel,
                )?;
                let endpoint_tangent = graph.velocity(jet)?;
                let slot = if index == 0 { 0 } else { tangents.len() - 1 };
                for k in 0..3 {
                    tangents[slot][k] = tangents[slot][k].hull(endpoint_tangent[k]);
                }
            }
            let uv = [
                [a.uv[0].mid(), b.uv[0].mid()],
                [a.uv[1].mid(), b.uv[1].mid()],
            ];
            let midpoint = [
                uv[0][0] * 0.5 + uv[0][1] * 0.5,
                uv[1][0] * 0.5 + uv[1][1] * 0.5,
            ];
            let reverse = graph.material_reverse(midpoint[independent], outward, work, cancel)?;
            if reverse {
                points.reverse();
                tangents.reverse();
                for tangent in &mut tangents {
                    for value in tangent {
                        *value = value.neg();
                    }
                }
            }
            let endpoint = |r: EdgeRoot| SectionEndpoint::EdgeRoot {
                edge: r.edge,
                root: r.ordinal,
            };
            let mut endpoints = [endpoint(roots[0]), endpoint(roots[1])];
            if reverse {
                endpoints.swap(0, 1);
            }
            admission.push(
                &mut arcs,
                Arc {
                    span: Some(SectionSpan {
                        face: face_id,
                        source_face_ordinal: ordinal,
                        definition_plane: plane,
                        section_pose: spiling_contracts::geometry::RigidPoseMm::IDENTITY,
                        curve: SectionCurve::SurfacePlaneGraph {
                            independent_axis: independent as u8,
                            independent: [a.uv[independent].lo, b.uv[independent].hi],
                            dependent: root_domain[axis],
                        },
                        endpoints,
                        trim_edgeuses: [a.edgeuse, b.edgeuse],
                        trim_parameters: [
                            [a.parameter.lo, a.parameter.hi],
                            [b.parameter.lo, b.parameter.hi],
                        ],
                        native_edge_parameters: roots.map(|r| [r.parameter.lo, r.parameter.hi]),
                        reversed: reverse,
                        maximum_chord_error_mm: achieved[0],
                        maximum_plane_error_mm: achieved[1],
                    }),
                    points,
                    tangents,
                },
            )?;
        }
    }
    let mut result = NativeSection {
        plane,
        frame,
        loops: Vec::new(),
        loop_source_faces: Vec::new(),
        loop_spans: Vec::new(),
        sampling_tolerance_mm: SECTION_SAMPLING_TOLERANCE_MM,
        boolean_tolerance_mm: SECTION_BOOLEAN_TOLERANCE_MM,
        certificate: NativeSectionCertificate {
            definition_id: definition.id().clone(),
            material_realization: crate::kernel::MaterialRealizationIdentity::new(definition),
            pose: spiling_contracts::geometry::RigidPoseMm::IDENTITY,
            authority: SectionAuthority::NativeIntersection,
            maximum_chord_error_mm: 0.0,
            maximum_plane_error_mm: 0.0,
        },
    };
    assemble(&mut arcs, &mut result, &mut admission, work, cancel)?;
    if result.retained_bytes() > packed_byte_limit.min(MAX_SECTION_BYTES as usize) {
        return Err(resource_error(
            "native section packed output exceeds allowance",
        ));
    }
    Ok(result)
}

/// Empty only after every possible carrier root is excluded by whole
/// coefficients. No root sample, absent incidence or subdivision limit grants
/// emptiness. The same finite depth and cumulative work policy remain in force.
fn exclude_carrier_roots(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    scale: f64,
    plane: PlaneMm,
    depth: u8,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(1, cancel)?;
    let position = p::surface_position_enclosure(surface, uv, work, cancel)?
        .map(|value| value.mul(Interval::point(scale)));
    if sign(functional(position, plane)) != 0 {
        return Ok(true);
    }
    if depth >= 16 {
        return Ok(false);
    }
    let axis = usize::from(!depth.is_multiple_of(2));
    let middle = uv[axis][0] * 0.5 + uv[axis][1] * 0.5;
    if middle <= uv[axis][0] || middle >= uv[axis][1] {
        return Ok(false);
    }
    let mut left = uv;
    left[axis][1] = middle;
    let mut right = uv;
    right[axis][0] = middle;
    Ok(
        exclude_carrier_roots(surface, left, scale, plane, depth + 1, work, cancel)?
            && exclude_carrier_roots(surface, right, scale, plane, depth + 1, work, cancel)?,
    )
}

/// Uniform dependent derivative proves no hidden closed component, tangency or
/// branch pair. Every original knot rectangle is visited; exclusion uses whole
/// coefficient bounds, not mesh vertices or a few signed samples.
fn classify_graph(
    surface: &Surface,
    domain: [[f64; 2]; 2],
    scale: f64,
    plane: PlaneMm,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<(usize, i8, [[f64; 2]; 2])>, GeometryError> {
    if !p::surface_parameter_span_continuous(surface, domain, work, cancel)? {
        return Err(indeterminate(
            "native face chart has an unqualified C0 knot join",
        ));
    }
    for axis in [1, 0] {
        let mut derivative_sign = 0;
        let mut retained: Option<Interval> = None;
        let mut qualified = true;
        for box_ in p::surface_parameter_boxes(surface, domain, work, cancel)? {
            match cover_graph(
                surface,
                box_,
                axis,
                scale,
                plane,
                0,
                &mut derivative_sign,
                &mut retained,
                work,
                cancel,
            ) {
                Ok(()) => {}
                Err(error) if error.code == GeometryErrorCode::AnalysisIndeterminate => {
                    qualified = false;
                    break;
                }
                Err(error) => return Err(error),
            }
        }
        if !qualified {
            continue;
        }
        let Some(range) = retained else {
            return Ok(None);
        };
        let mut root_domain = domain;
        root_domain[axis] = [range.lo, range.hi];
        for box_ in p::surface_parameter_boxes(surface, root_domain, work, cancel)? {
            let j = surface_jet(surface, box_, scale, work, cancel)?;
            let derivative = dot(if axis == 0 { j.du } else { j.dv }, plane.normal);
            if sign(derivative) != derivative_sign {
                qualified = false;
                break;
            }
        }
        if qualified {
            return Ok(Some((axis, derivative_sign, root_domain)));
        }
    }
    Err(indeterminate(
        "native face-plane zero set has no complete monotone chart: tangent/coplanar or unresolved multiple roots",
    ))
}
#[allow(clippy::too_many_arguments)]
fn cover_graph(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    axis: usize,
    scale: f64,
    plane: PlaneMm,
    depth: u8,
    derivative_sign: &mut i8,
    retained: &mut Option<Interval>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    work.charge(1, cancel)?;
    let j = surface_jet(surface, uv, scale, work, cancel)?;
    let height = functional(j.position, plane);
    if sign(height) != 0 {
        return Ok(());
    }
    let derivative = dot(if axis == 0 { j.du } else { j.dv }, plane.normal);
    let s = sign(derivative);
    if s != 0 {
        // Retain a necessary zero-set interval, not the entire search leaf.
        // For every fixed independent parameter, the whole-cell derivative
        // bounds the mean-value slope from this midpoint to ANY actual root.
        // No midpoint is promoted to a root or used as existence evidence.
        let midpoint = uv[axis][0] * 0.5 + uv[axis][1] * 0.5;
        let mut middle = uv;
        middle[axis] = [midpoint; 2];
        let position = p::surface_position_enclosure(surface, middle, work, cancel)?
            .map(|value| value.mul(Interval::point(scale)));
        let necessary = Interval::point(midpoint).sub(functional(position, plane).div(derivative)?);
        let lo = necessary.lo.max(uv[axis][0]);
        let hi = necessary.hi.min(uv[axis][1]);
        if lo > hi {
            return Ok(());
        }
        if *derivative_sign != 0 && *derivative_sign != s {
            return Err(indeterminate(format!(
                "native face-plane roots have incompatible directed graph sheets: cell={uv:?} height={height:?} derivative={derivative:?}"
            )));
        }
        *derivative_sign = s;
        // A necessary root enclosure may end AT a root. Retain half of each
        // excluded strip so later whole-interval existence checks can still
        // obtain strict endpoint signs; those checks remain mandatory.
        let half = Interval::point(0.5);
        let r = Interval::new(
            Interval::point(uv[axis][0])
                .add(Interval::point(lo))
                .mul(half)
                .lo
                .max(uv[axis][0]),
            Interval::point(hi)
                .add(Interval::point(uv[axis][1]))
                .mul(half)
                .hi
                .min(uv[axis][1]),
        )?;
        *retained = Some(retained.map_or(r, |old| old.hull(r)));
        return Ok(());
    }
    if depth >= 16 || derivative.lo == 0.0 && derivative.hi == 0.0 {
        return Err(indeterminate(format!(
            "native critical/coplanar face cell has no transverse root graph proof: cell={uv:?} height={height:?} derivative={derivative:?}"
        )));
    }
    // Independent-parameter uncertainty can prevent both height exclusion and
    // a dependent derivative sign (for example on a broad spherical chart).
    // Refining only the dependent coordinate need not tighten either bound.
    // Alternate coordinates while retaining the same complete cover and cap.
    let split_axis = if depth.is_multiple_of(2) {
        axis
    } else {
        1 - axis
    };
    let mid = uv[split_axis][0] * 0.5 + uv[split_axis][1] * 0.5;
    if mid <= uv[split_axis][0] || mid >= uv[split_axis][1] {
        return Err(indeterminate(
            "native graph isolation exhausted representable parameters",
        ));
    }
    let mut left = uv;
    left[split_axis][1] = mid;
    let mut right = uv;
    right[split_axis][0] = mid;
    cover_graph(
        surface,
        left,
        axis,
        scale,
        plane,
        depth + 1,
        derivative_sign,
        retained,
        work,
        cancel,
    )?;
    cover_graph(
        surface,
        right,
        axis,
        scale,
        plane,
        depth + 1,
        derivative_sign,
        retained,
        work,
        cancel,
    )
}
fn face_orientation(brep: &Brep, face: FaceId) -> Result<f64, GeometryError> {
    let face_record = topology(brep.face(face))?;
    let shell = topology(brep.shell(face_record.shell))?;
    let ShellMembers::Faceuses(uses) = &shell.members else {
        return Err(indeterminate(
            "native section requires oriented shell faceuses",
        ));
    };
    let mut result = None;
    for &id in uses {
        let fu = topology(brep.faceuse(id))?;
        if fu.face == Some(face) {
            if result.is_some() {
                return Err(indeterminate(
                    "native shell contains duplicate material face uses",
                ));
            }
            result = Some(match fu.orientation {
                OrientType::Same => 1.0,
                OrientType::Opposite => -1.0,
                _ => return Err(indeterminate("native faceuse has no material orientation")),
            });
        }
    }
    result.ok_or_else(|| indeterminate("native section face is absent from its oriented shell"))
}
fn ensure_edge_roots(
    definition: &Definition,
    edge_id: EdgeId,
    plane: PlaneMm,
    accuracy: f64,
    output: &mut Vec<EdgeRoot>,
    admission: &mut Admission,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    // Rootless edges are deliberately recomputed rather than adding an
    // unaccounted secondary map; they carry no endpoint identity to cache.
    if output.iter().any(|r| r.edge == edge_id) {
        return Ok(());
    }
    let edge = topology(definition.brep().edge(edge_id))?;
    let curve = definition
        .edge_curve(edge_id)
        .ok_or_else(|| indeterminate("native section edge has no curve"))?;
    let range = [edge.interval.min(), edge.interval.max()];
    let scale = 1.0;
    if !p::curve_parameter_span_continuous(curve, range, work, cancel)? {
        return Err(indeterminate(
            "native spatial edge has an unqualified C0 knot join",
        ));
    }
    let prepared = p::PreparedValidatedCurve::new(curve, range, scale, usize::MAX, work, cancel)?;
    let speed = norm(prepared.jet(range, work, cancel)?.derivative);
    let parameter_accuracy = accuracy / speed.max(1.0);
    let spans = p::curve_parameter_spans(curve, range, work, cancel)?;
    let found = roots::isolate(
        spans,
        &mut |r, work, cancel| {
            let j = prepared.projected_jet(r, plane.origin_mm, plane.normal, work, cancel)?;
            Ok(ScalarJet {
                value: j.position,
                derivative: j.first,
            })
        },
        parameter_accuracy,
        admission,
        work,
        cancel,
    ).map_err(|mut failure| {
        if failure.code == GeometryErrorCode::AnalysisIndeterminate {
            failure.message = format!(
                "native spatial edge-plane roots: edge={edge_id:?} kind={} owned_parameter={range:?} accuracy={parameter_accuracy:.17e}; {}; source={} plane={plane:?}",
                curve.type_string(), failure.message, definition.id().as_str(),
            );
        }
        failure
    })?;
    for (ordinal, root) in found.into_iter().enumerate() {
        if root.parameter.contains(range[0]) || root.parameter.contains(range[1]) {
            return Err(indeterminate(
                "native section meets a source vertex; critical vertex incidence is unqualified",
            ));
        }
        let position =
            prepared.position_bounds([root.parameter.lo, root.parameter.hi], work, cancel)?;
        let point = position.map(Interval::mid);
        if norm(std::array::from_fn(|i| {
            position[i].sub(Interval::point(point[i]))
        })) > accuracy * 2.0
        {
            return Err(indeterminate(
                "native root enclosure exceeds endpoint error allowance",
            ));
        }
        admission.push(
            output,
            EdgeRoot {
                edge: edge_id,
                ordinal: ordinal as u32,
                parameter: root.parameter,
                position,
                point,
            },
        )?;
    }
    Ok(())
}

/// The actual edgeuse UV law is directed with the native edge, not its coedge.
/// Each operand retains its own literal gauge; a complete coefficient
/// calibration proves their physical correspondence independently.
struct Boundary<'a> {
    surface: &'a Surface,
    curve: &'a Curve,
    uv_curve: Option<&'a Curve>,
    edge_range: [f64; 2],
    scale: f64,
}
impl<'a> Boundary<'a> {
    fn new(
        definition: &'a Definition,
        surface: &'a Surface,
        eu: EdgeuseId,
        edge: EdgeId,
        scale: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        let e = topology(definition.brep().edge(edge))?;
        let curve = definition
            .edge_curve(edge)
            .ok_or_else(|| indeterminate("native trim edge has no spatial curve"))?;
        let uv_curve = definition.edgeuse_uv_trim_curve(eu);
        if uv_curve.is_none() && !matches!(surface.kind(), SurfaceKind::Plane(_)) {
            return Err(indeterminate(
                "native curved face lacks its own UV trim association",
            ));
        }
        if let Some(uv_curve) = uv_curve {
            let d = uv_curve.natural_interval();
            if !p::curve_parameter_span_continuous(uv_curve, [d.min(), d.max()], work, cancel)? {
                return Err(indeterminate(
                    "native UV trim has an unqualified C0 knot join",
                ));
            }
            let domain = p::surface_active_domain(surface)?;
            let min = solidcore::math::Point2::new(domain[0][0], domain[1][0]);
            let max = solidcore::math::Point2::new(domain[0][1], domain[1][1]);
            if !p::curve2d_image_within_box(
                uv_curve,
                [d.min(), d.max()],
                [[min.x, max.x], [min.y, max.y]],
                work,
                cancel,
            )? {
                let parameters = [d.min(), d.min() * 0.5 + d.max() * 0.5, d.max()];
                let mut images = [[Interval::point(0.0); 3]; 3];
                for (parameter, image) in parameters.into_iter().zip(&mut images) {
                    *image =
                        p::CurveEvaluator::tensor(uv_curve, [parameter; 2], 0, work, cancel)?[0];
                }
                return Err(GeometryError::new(
                    GeometryErrorCode::AnalysisIndeterminate,
                    format!(
                        "native UV trim has no coefficient proof of active-chart containment; \
                         edge={edge:?}; edgeuse={eu:?}; carrier=[[{},{}],[{},{}]]; \
                         native_point_parameters={parameters:?}; native_point_uv={images:?}",
                        min.x, max.x, min.y, max.y,
                    ),
                ));
            }
        }
        Ok(Self {
            surface,
            curve,
            uv_curve,
            edge_range: [e.interval.min(), e.interval.max()],
            scale,
        })
    }
    fn edge_jet(
        &self,
        r: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<p::CurveEnclosure, GeometryError> {
        let slope = Interval::point(self.edge_range[1]).sub(Interval::point(self.edge_range[0]));
        let parameter = Interval::point(self.edge_range[0])
            .add(Interval::new(r[0], r[1])?.mul(slope))
            .intersect(Interval::new(self.edge_range[0], self.edge_range[1])?)?;
        let mut j = p::curve_enclosure(self.curve, (parameter.lo, parameter.hi), work, cancel)?;
        for i in 0..3 {
            j.position[i] = j.position[i].mul(Interval::point(self.scale));
            j.derivative[i] = j.derivative[i].mul(slope).mul(Interval::point(self.scale));
        }
        Ok(j)
    }
    fn chart(
        &self,
        r: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<([Interval; 2], [Interval; 2]), GeometryError> {
        if let Some(curve) = self.uv_curve {
            let d = curve.natural_interval();
            let slope = Interval::point(d.max()).sub(Interval::point(d.min()));
            let t = Interval::point(d.min()).add(Interval::new(r[0], r[1])?.mul(slope));
            let j = p::curve2d_piecewise_jet(
                curve,
                [t.lo.max(d.min()), t.hi.min(d.max())],
                work,
                cancel,
            )?;
            return Ok((j.position, j.derivative.map(|v| v.mul(slope))));
        }
        let SurfaceKind::Plane(plane) = self.surface.kind() else {
            return Err(indeterminate("missing literal native plane chart"));
        };
        let j = self.edge_jet(r, work, cancel)?;
        let pos = plane.position();
        let origin = pos.origin();
        let x = pos.x_axis();
        let y = pos.y_axis();
        let uv_scale = plane.uv_scale();
        let axes = [[x.x, x.y, x.z], [y.x, y.y, y.z]];
        let origin = [origin.x, origin.y, origin.z];
        let relative = std::array::from_fn(|i| {
            j.position[i].sub(Interval::point(origin[i]).mul(Interval::point(self.scale)))
        });
        let xx = dot(axes[0].map(Interval::point), axes[0]);
        let xy = dot(axes[0].map(Interval::point), axes[1]);
        let yy = dot(axes[1].map(Interval::point), axes[1]);
        let determinant = xx.mul(yy).sub(xy.square());
        let denominators = [
            determinant
                .mul(Interval::point(uv_scale.x))
                .mul(Interval::point(self.scale)),
            determinant
                .mul(Interval::point(uv_scale.y))
                .mul(Interval::point(self.scale)),
        ];
        let invert = |v: [Interval; 3]| -> Result<[Interval; 2], GeometryError> {
            let a = dot(v, axes[0]);
            let b = dot(v, axes[1]);
            Ok([
                yy.mul(a).sub(xy.mul(b)).div(denominators[0])?,
                xx.mul(b).sub(xy.mul(a)).div(denominators[1])?,
            ])
        };
        Ok((invert(relative)?, invert(j.derivative)?))
    }
    fn evaluate(
        &self,
        r: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<([Interval; 3], [Interval; 3], [Interval; 2], [Interval; 2]), GeometryError> {
        let (mut uv, derivative) = self.chart(r, work, cancel)?;
        let domain = p::surface_active_domain(self.surface)?;
        let min = solidcore::math::Point2::new(domain[0][0], domain[1][0]);
        let max = solidcore::math::Point2::new(domain[0][1], domain[1][1]);
        // An outward chart interval may extend by rounding; intersecting with
        // the declared chart is valid only after the native trim binding itself
        // is known to stay there, not by silently clamping a source pcurve.
        if uv[0].lo < min.x || uv[0].hi > max.x || uv[1].lo < min.y || uv[1].hi > max.y {
            if self.uv_curve.is_none() {
                return Err(indeterminate(
                    "native plane trim exceeds the literal active chart",
                ));
            }
            // Boundary::new proved the *actual* native curve image lies in
            // this chart by homogeneous control inequalities, before any
            // interval overestimation is intersected away here.
            uv[0] = uv[0].intersect(Interval::new(min.x, max.x)?)?;
            uv[1] = uv[1].intersect(Interval::new(min.y, max.y)?)?;
        }
        let j = surface_jet(
            self.surface,
            [[uv[0].lo, uv[0].hi], [uv[1].lo, uv[1].hi]],
            self.scale,
            work,
            cancel,
        )?;
        Ok((
            j.position,
            std::array::from_fn(|i| j.du[i].mul(derivative[0]).add(j.dv[i].mul(derivative[1]))),
            uv,
            derivative,
        ))
    }
    fn parameter_accuracy(
        &self,
        accuracy: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<f64, GeometryError> {
        let (_, derivative, _, _) = self.evaluate([0.0, 1.0], work, cancel)?;
        Ok(accuracy / norm(derivative).max(1.0))
    }
    fn prove_correspondence(
        &self,
        allowance: f64,
        byte_limit: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<f64, GeometryError> {
        if let Some(chart) = self.uv_curve {
            let reference = p::curve_prepared::NativeChartCurve {
                surface: self.surface,
                chart,
            };
            // Both actual complete parameter branches are fixed before the
            // solver proposes any gauge. A coefficient-proved monotone map
            // with exact endpoint coverage and whole-span positional bounds,
            // not its point inverses, discharges correspondence. Stationary
            // chart endpoint germs need no source relative-tangent receipt.
            // Root inventories and physical endpoint gaps remain independent.
            let physical_allowance = Interval::point(allowance)
                .div(Interval::point(self.scale))?
                .lo;
            return crate::kernel::trim::calibrate_native_curve_position(
                &reference,
                self.curve,
                self.edge_range,
                physical_allowance,
                24,
                byte_limit,
                work,
                cancel,
            )
            .map(|calibration| {
                // Retain the achieved whole-branch bound, not the looser
                // request that stopped refinement. A later stricter query
                // may reuse this same immutable positional evidence.
                Interval::point(calibration.maximum_position_error_mm())
                    .mul(Interval::point(self.scale))
                    .hi
                    .min(allowance)
            })
            .map_err(|mut error| {
                if error.code == GeometryErrorCode::ParameterCorrespondenceUnavailable {
                    error.code = GeometryErrorCode::AnalysisIndeterminate;
                }
                error.message = format!(
                    "native trim/spatial coefficient correspondence unqualified: {}",
                    error.message
                );
                error
            });
        }
        // A missing pcurve is admitted only for the literal affine plane
        // inverse, whose gauge is constructed from this very spatial edge.
        self.prove_affine_correspondence(allowance, 0, [0.0, 1.0], work, cancel)
    }
    fn prove_affine_correspondence(
        &self,
        allowance: f64,
        depth: u8,
        r: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<f64, GeometryError> {
        work.charge(1, cancel)?;
        let (position, derivative, _, _) = self.evaluate(r, work, cancel)?;
        let edge = self.edge_jet(r, work, cancel)?;
        let mid = r[0] * 0.5 + r[1] * 0.5;
        let (at, _, _, _) = self.evaluate([mid; 2], work, cancel)?;
        let edge_at = self.edge_jet([mid; 2], work, cancel)?;
        let displacement = Interval::new(r[0], r[1])?.sub(Interval::point(mid));
        let error = std::array::from_fn(|i| {
            at[i]
                .sub(edge_at.position[i])
                .add(derivative[i].sub(edge.derivative[i]).mul(displacement))
        });
        let achieved = norm(error);
        if achieved <= allowance {
            return Ok(achieved);
        }
        if depth >= 24 || mid == r[0] || mid == r[1] {
            return Err(indeterminate(format!(
                "native trim/spatial coefficient correspondence unqualified: endpoint enclosure {:?}",
                position
            )));
        }
        let left =
            self.prove_affine_correspondence(allowance, depth + 1, [r[0], mid], work, cancel)?;
        let right =
            self.prove_affine_correspondence(allowance, depth + 1, [mid, r[1]], work, cancel)?;
        Ok(left.max(right))
    }
}

pub(super) struct Graph<'a> {
    pub(super) surface: &'a Surface,
    pub(super) domain: [[f64; 2]; 2],
    pub(super) axis: usize,
    pub(super) scale: f64,
    pub(super) plane: PlaneMm,
    pub(super) dependent_sign: i8,
}
impl Graph<'_> {
    fn box_uv(&self, x: [f64; 2], y: Interval) -> [[f64; 2]; 2] {
        let mut uv = self.domain;
        uv[1 - self.axis] = x;
        uv[self.axis] = [y.lo, y.hi];
        uv
    }
    /// Strict endpoint signs and a uniform nonzero partial derivative prove
    /// existence and uniqueness for every independent parameter in this box.
    pub(super) fn enclose(
        &self,
        x: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(Interval, SurfaceEnclosure), GeometryError> {
        work.charge(1, cancel)?;
        let mut y = Interval::new(self.domain[self.axis][0], self.domain[self.axis][1])?;
        let a = surface_jet(
            self.surface,
            self.box_uv(x, Interval::point(y.lo)),
            self.scale,
            work,
            cancel,
        )?;
        let b = surface_jet(
            self.surface,
            self.box_uv(x, Interval::point(y.hi)),
            self.scale,
            work,
            cancel,
        )?;
        let fa = functional(a.position, self.plane);
        let fb = functional(b.position, self.plane);
        let nonpositive = |v: Interval| v.hi <= 0.0;
        let nonnegative = |v: Interval| v.lo >= 0.0;
        let bracketed = if self.dependent_sign > 0 {
            nonpositive(fa) && nonnegative(fb)
        } else {
            nonnegative(fa) && nonpositive(fb)
        };
        if !bracketed {
            return Err(indeterminate(
                "native graph lacks a whole-interval root existence bracket",
            ));
        }
        for _ in 0..64 {
            let jet = surface_jet(self.surface, self.box_uv(x, y), self.scale, work, cancel)?;
            let derivative = dot(
                if self.axis == 0 { jet.du } else { jet.dv },
                self.plane.normal,
            );
            if sign(derivative) != self.dependent_sign {
                return Err(indeterminate(
                    "native graph lost its whole-chart nonzero partial derivative",
                ));
            }
            let mid = y.mid();
            let at = surface_jet(
                self.surface,
                self.box_uv(x, Interval::point(mid)),
                self.scale,
                work,
                cancel,
            )?;
            let next = y.intersect(
                Interval::point(mid).sub(functional(at.position, self.plane).div(derivative)?),
            )?;
            if next == y {
                return Ok((y, jet));
            }
            y = next;
        }
        Ok((
            y,
            surface_jet(self.surface, self.box_uv(x, y), self.scale, work, cancel)?,
        ))
    }
    fn point(
        &self,
        x: f64,
        accuracy: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<([f64; 3], f64), GeometryError> {
        let (_, jet) = self.enclose([x; 2], work, cancel)?;
        let point = jet.position.map(Interval::mid);
        let error = norm(std::array::from_fn(|i| {
            jet.position[i].sub(Interval::point(point[i]))
        }));
        if error > accuracy {
            return Err(indeterminate(
                "native graph point uncertainty exceeds chord allowance",
            ));
        }
        Ok((point, error))
    }
    pub(super) fn velocity(&self, jet: SurfaceEnclosure) -> Result<[Interval; 3], GeometryError> {
        let (dx, dy) = if self.axis == 1 {
            (jet.du, jet.dv)
        } else {
            (jet.dv, jet.du)
        };
        let slope = dot(dx, self.plane.normal)
            .neg()
            .div(dot(dy, self.plane.normal))?;
        Ok(std::array::from_fn(|i| dx[i].add(dy[i].mul(slope))))
    }
    fn single_knot_cell(
        &self,
        uv: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        for piece in p::surface_parameter_boxes(self.surface, uv, work, cancel)? {
            for axis in 0..2 {
                if piece[axis][0] < piece[axis][1]
                    && piece[axis][0] > uv[axis][0]
                    && piece[axis][0] < uv[axis][1]
                {
                    return Ok(false);
                }
            }
        }
        // C0 was established for the original chart. A single open knot cell
        // has bounded second derivatives and continuous one-sided endpoint
        // values, so interpolation needs no derivative agreement with a cell
        // *outside* this closed query interval.
        Ok(true)
    }
    fn second(&self, jet: SurfaceEnclosure) -> Result<[Interval; 3], GeometryError> {
        let (dx, dy, dxx, dxy, dyy) = if self.axis == 1 {
            (jet.du, jet.dv, jet.duu, jet.duv, jet.dvv)
        } else {
            (jet.dv, jet.du, jet.dvv, jet.duv, jet.duu)
        };
        let fy = dot(dy, self.plane.normal);
        let slope = dot(dx, self.plane.normal).neg().div(fy)?;
        let second = dot(dxx, self.plane.normal)
            .add(
                dot(dxy, self.plane.normal)
                    .mul(slope)
                    .mul(Interval::point(2.0)),
            )
            .add(dot(dyy, self.plane.normal).mul(slope.square()))
            .neg()
            .div(fy)?;
        Ok(std::array::from_fn(|i| {
            dxx[i]
                .add(dxy[i].mul(slope).mul(Interval::point(2.0)))
                .add(dyy[i].mul(slope.square()))
                .add(dy[i].mul(second))
        }))
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn sample(
        &self,
        x: [f64; 2],
        a: [f64; 3],
        b: [f64; 3],
        a_error: f64,
        b_error: f64,
        query: &SectionQuery,
        depth: u8,
        output: &mut Vec<[f64; 3]>,
        tangents: &mut Vec<[Interval; 3]>,
        achieved: &mut [f64; 2],
        admission: &mut Admission,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        work.charge(1, cancel)?;
        let enclosure = self.enclose(x, work, cancel);
        let velocity = match &enclosure {
            Ok((_, jet)) => Some(self.velocity(*jet)?),
            Err(_) => None,
        };
        let chord = match enclosure {
            Ok((y, jet)) => {
                let length = Interval::point(x[1]).sub(Interval::point(x[0]));
                let uv = self.box_uv(x, y);
                let deviation = if self.single_knot_cell(uv, work, cancel)?
                    || p::surface_parameter_span_smooth(self.surface, uv, work, cancel)?
                {
                    Interval::point(norm(self.second(jet)?))
                        .mul(length.square())
                        .mul(Interval::point(0.125))
                } else {
                    // A C0 piecewise-C1 graph is Lipschitz even if a true corner
                    // prevents the stronger second-order interpolation theorem.
                    Interval::point(norm(self.velocity(jet)?))
                        .mul(length)
                        .mul(Interval::point(0.5))
                };
                deviation.add(Interval::point(a_error.max(b_error))).hi
            }
            Err(error) if error.code == GeometryErrorCode::AnalysisIndeterminate => f64::INFINITY,
            Err(error) => return Err(error),
        };
        let plane_error = [a, b]
            .into_iter()
            .map(|point| functional(point.map(Interval::point), self.plane).abs_upper())
            .fold(0.0_f64, f64::max);
        let monotone =
            velocity.is_some_and(|v| dot(v, std::array::from_fn(|i| b[i] - a[i])).lo > 0.0);
        if monotone
            && chord <= query.maximum_chord_error_mm
            && plane_error <= query.maximum_plane_error_mm
        {
            achieved[0] = achieved[0].max(chord);
            achieved[1] = achieved[1].max(plane_error);
            admission.push(
                tangents,
                velocity
                    .ok_or_else(|| indeterminate("native graph segment lost its tangent proof"))?,
            )?;
            return admission.push(output, b);
        }
        let mid = x[0] * 0.5 + x[1] * 0.5;
        if depth >= 32 || mid <= x[0] || mid >= x[1] {
            return Err(indeterminate(
                "native graph discretization cannot prove whole-curve chord/plane bounds",
            ));
        }
        let (point, error) = self.point(mid, query.maximum_chord_error_mm / 64.0, work, cancel)?;
        self.sample(
            [x[0], mid],
            a,
            point,
            a_error,
            error,
            query,
            depth + 1,
            output,
            tangents,
            achieved,
            admission,
            work,
            cancel,
        )?;
        self.sample(
            [mid, x[1]],
            point,
            b,
            error,
            b_error,
            query,
            depth + 1,
            output,
            tangents,
            achieved,
            admission,
            work,
            cancel,
        )
    }
    pub(super) fn material_reverse(
        &self,
        x: f64,
        orientation: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let (_, jet) = self.enclose([x; 2], work, cancel)?;
        let (dx, dy) = if self.axis == 1 {
            (jet.du, jet.dv)
        } else {
            (jet.dv, jet.du)
        };
        let slope = dot(dx, self.plane.normal)
            .neg()
            .div(dot(dy, self.plane.normal))?;
        let tangent: [Interval; 3] = std::array::from_fn(|i| dx[i].add(dy[i].mul(slope)));
        let normal = p::cross(jet.du, jet.dv).map(|v| v.mul(Interval::point(orientation)));
        let left_tangent = p::cross(self.plane.normal.map(Interval::point), normal);
        let direction = (0..3).fold(Z, |s, i| s.add(tangent[i].mul(left_tangent[i])));
        match sign(direction) {
            1 => Ok(false),
            -1 => Ok(true),
            _ => Err(indeterminate(
                "native graph lacks a regular material-left orientation",
            )),
        }
    }
}

fn assemble(
    arcs: &mut [Arc],
    result: &mut NativeSection,
    admission: &mut Admission,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    // The manifold condition is exact degree one in each direction at each
    // native edge root. Nearby endpoints are never clustered or repaired.
    for a in arcs.iter() {
        for endpoint in a
            .span
            .as_ref()
            .ok_or_else(|| indeterminate("native section span was consumed before assembly"))?
            .endpoints
        {
            work.charge(2 * arcs.len() as u64, cancel)?;
            let incoming = arcs
                .iter()
                .filter(|b| {
                    b.span
                        .as_ref()
                        .is_some_and(|span| span.endpoints[1] == endpoint)
                })
                .count();
            let outgoing = arcs
                .iter()
                .filter(|b| {
                    b.span
                        .as_ref()
                        .is_some_and(|span| span.endpoints[0] == endpoint)
                })
                .count();
            if incoming != 1 || outgoing != 1 {
                return Err(indeterminate(
                    "native section shared-edge root does not have one material-left predecessor/successor",
                ));
            }
        }
    }
    while let Some(start) = arcs.iter().position(|a| a.span.is_some()) {
        work.charge(1, cancel)?;
        let first_key = arcs[start]
            .span
            .as_ref()
            .ok_or_else(|| indeterminate("native section start span was already consumed"))?
            .endpoints[0];
        let first_point = arcs[start].points[0];
        let mut points = std::mem::take(&mut arcs[start].points);
        let mut tangents = std::mem::take(&mut arcs[start].tangents);
        let mut faces = Vec::new();
        let mut spans = Vec::new();
        let mut current = start;
        loop {
            work.charge(1, cancel)?;
            let arc = &mut arcs[current];
            let span = arc.span.take().ok_or_else(|| {
                indeterminate("native section topology revisits a span before closure")
            })?;
            if current != start {
                for &point in &arc.points[1..] {
                    admission.push(&mut points, point)?;
                }
                for &tangent in &arc.tangents {
                    admission.push(&mut tangents, tangent)?;
                }
            }
            admission.push(&mut faces, span.source_face_ordinal)?;
            result.certificate.maximum_chord_error_mm = result
                .certificate
                .maximum_chord_error_mm
                .max(span.maximum_chord_error_mm);
            result.certificate.maximum_plane_error_mm = result
                .certificate
                .maximum_plane_error_mm
                .max(span.maximum_plane_error_mm);
            let next_key = span.endpoints[1];
            admission.push(&mut spans, span)?;
            if next_key == first_key {
                break;
            }
            work.charge(arcs.len() as u64, cancel)?;
            current = arcs
                .iter()
                .position(|a| {
                    a.span
                        .as_ref()
                        .is_some_and(|span| span.endpoints[0] == next_key)
                })
                .ok_or_else(|| indeterminate("native section incidence is open"))?;
        }
        if points.len() < 4 || points.last() != Some(&first_point) {
            return Err(indeterminate(
                "native section polygon is not exactly closed through native endpoint identities",
            ));
        }
        faces.sort_unstable();
        faces.dedup();
        let area = signed_area(&points, result.frame, work, cancel)?;
        if sign(area) == 0 {
            return Err(indeterminate(
                "native section orientation/area is unqualified",
            ));
        }
        certify_adjacent(&points, &tangents, work, cancel)?;
        canonicalize_points(&mut points, result.frame, work, cancel)?;
        admission.push(
            &mut result.loops,
            SectionLoopMm {
                is_hole: sign(area) < 0,
                points_mm: points,
            },
        )?;
        admission.push(&mut result.loop_source_faces, faces)?;
        admission.push(&mut result.loop_spans, spans)?;
    }
    canonicalize_loops(result, work, cancel)?;
    certify_polygons(result, work, cancel)
}

fn certify_adjacent(
    points: &[[f64; 3]],
    tangents: &[[Interval; 3]],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    if tangents.len() + 1 != points.len() {
        return Err(indeterminate(
            "native graph tangent/segment inventory mismatch",
        ));
    }
    let count = tangents.len();
    for i in 0..count {
        work.charge(1, cancel)?;
        let next = (i + 1) % count;
        let first: [f64; 3] = std::array::from_fn(|a| points[i + 1][a] - points[i][a]);
        let second: [f64; 3] = std::array::from_fn(|a| points[next + 1][a] - points[next][a]);
        let a = norm(first.map(Interval::point));
        let b = norm(second.map(Interval::point));
        if !a.is_finite() || !b.is_finite() || a <= 0.0 || b <= 0.0 {
            return Err(indeterminate(
                "native graph has a degenerate adjacent chord",
            ));
        }
        // This is merely a proposed separating direction. The interval dot
        // products, not the floating normalization, establish monotonicity.
        let direction = std::array::from_fn(|k| first[k] / a + second[k] / b);
        if dot(tangents[i], direction).lo <= 0.0 || dot(tangents[next], direction).lo <= 0.0 {
            return Err(indeterminate(
                "adjacent native graph spans lack a common strictly monotone separating direction",
            ));
        }
    }
    Ok(())
}

fn canonicalize_points(
    points: &mut [[f64; 3]],
    frame: PlaneFrameMm,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    let count = points.len() - 1;
    let mut first = 0;
    for i in 1..count {
        work.charge(1, cancel)?;
        let a = project(points[i], frame).map(Interval::mid);
        let b = project(points[first], frame).map(Interval::mid);
        if a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])).is_lt() {
            first = i;
        }
    }
    points[..count].rotate_left(first);
    points[count] = points[0];
    Ok(())
}
fn canonicalize_loops(
    section: &mut NativeSection,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    for i in 1..section.loops.len() {
        let mut j = i;
        while j > 0 {
            work.charge(1, cancel)?;
            let a = &section.loops[j];
            let b = &section.loops[j - 1];
            let ap = project(a.points_mm[0], section.frame).map(Interval::mid);
            let bp = project(b.points_mm[0], section.frame).map(Interval::mid);
            let order = a
                .is_hole
                .cmp(&b.is_hole)
                .then(ap[0].total_cmp(&bp[0]))
                .then(ap[1].total_cmp(&bp[1]));
            if !order.is_lt() {
                break;
            }
            section.loops.swap(j, j - 1);
            section.loop_source_faces.swap(j, j - 1);
            section.loop_spans.swap(j, j - 1);
            j -= 1;
        }
    }
    Ok(())
}
fn project(point: [f64; 3], frame: PlaneFrameMm) -> [Interval; 2] {
    let relative =
        std::array::from_fn(|i| Interval::point(point[i]).sub(Interval::point(frame.origin_mm[i])));
    [dot(relative, frame.x_axis), dot(relative, frame.y_axis)]
}
fn determinant(a: [Interval; 2], b: [Interval; 2], c: [Interval; 2]) -> Interval {
    b[0].sub(a[0])
        .mul(c[1].sub(a[1]))
        .sub(b[1].sub(a[1]).mul(c[0].sub(a[0])))
}
fn signed_area(
    points: &[[f64; 3]],
    frame: PlaneFrameMm,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Interval, GeometryError> {
    let mut area = Z;
    let anchor = project(points[0], frame);
    for edge in points[1..].windows(2) {
        work.charge(1, cancel)?;
        area = area.add(determinant(
            anchor,
            project(edge[0], frame),
            project(edge[1], frame),
        ));
    }
    Ok(area)
}
/// Certify the actual returned polygons as disjoint simple cycles. No
/// intersection is repaired and no sampled fill determines hole membership.
fn certify_polygons(
    section: &NativeSection,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    let projection_scale = norm(section.frame.x_axis.map(Interval::point))
        .max(norm(section.frame.y_axis.map(Interval::point)));
    let margin = Interval::point(section.achieved_chord_error_mm())
        .mul(Interval::point(2.0))
        .mul(Interval::point(projection_scale))
        .hi;
    for (li, left) in section.loops.iter().enumerate() {
        for (ei, a) in left.points_mm.windows(2).enumerate() {
            let a = [project(a[0], section.frame), project(a[1], section.frame)];
            for (lj, right) in section.loops.iter().enumerate().skip(li) {
                for (ej, b) in right.points_mm.windows(2).enumerate() {
                    work.charge(1, cancel)?;
                    if li == lj && (ej <= ei + 1 || (ei == 0 && ej + 2 == left.points_mm.len())) {
                        continue;
                    }
                    let b = [project(b[0], section.frame), project(b[1], section.frame)];
                    let separated = (0..2).any(|axis| {
                        Interval::point(a[0][axis].hi.max(a[1][axis].hi))
                            .add(Interval::point(margin))
                            .hi
                            < b[0][axis].lo.min(b[1][axis].lo)
                            || Interval::point(b[0][axis].hi.max(b[1][axis].hi))
                                .add(Interval::point(margin))
                                .hi
                                < a[0][axis].lo.min(a[1][axis].lo)
                    });
                    if separated {
                        continue;
                    }
                    let d = [determinant(a[0], a[1], b[0]), determinant(a[0], a[1], b[1])];
                    let e = [determinant(b[0], b[1], a[0]), determinant(b[0], b[1], a[1])];
                    let clear = |values: [Interval; 2], line: [[Interval; 2]; 2]| {
                        let same = sign(values[0]) != 0 && sign(values[0]) == sign(values[1]);
                        let length = Interval::point(line[1][0].sub(line[0][0]).abs_upper())
                            .add(Interval::point(line[1][1].sub(line[0][1]).abs_upper()));
                        same && values[0].abs_lower().min(values[1].abs_lower())
                            > length.mul(Interval::point(margin)).hi
                    };
                    if clear(d, a) || clear(e, b) {
                        continue;
                    }
                    return Err(indeterminate(
                        "nonadjacent native curve error tubes have an unresolved crossing/touch",
                    ));
                }
            }
        }
    }
    Ok(())
}

fn coedge_limit(
    brep: &Brep,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<usize, GeometryError> {
    let mut count = 0usize;
    for &edge in brep.edge_list() {
        work.charge(1, cancel)?;
        count = count
            .checked_add(topology(brep.edge(edge))?.edgeuses.len())
            .ok_or_else(|| resource_error("native coedge census overflows query limits"))?;
    }
    Ok(count)
}

/// Stratified transversality for every affine plane covector in the supplied
/// coefficient box. This proves a plane homotopy, not material admission.
pub(crate) fn certify_plane_functional_family(
    definition: &Definition,
    normal: [Interval; 3],
    offset: Interval,
    byte_limit: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    work.charge(1, cancel)?;
    let _admission = Admission::new(byte_limit.min(MAX_PREPARED_SECTION_BYTES))?;
    for value in normal.into_iter().chain([offset]) {
        Interval::new(value.lo, value.hi)?;
    }
    if normal.into_iter().fold(Z, |s, v| s.add(v.square())).lo <= 0.0 {
        return Err(indeterminate(
            "plane covector family may contain a zero normal",
        ));
    }
    let family = FunctionalFamily { normal, offset };
    let brep = definition.brep();
    let limit = coedge_limit(brep, work, cancel)?;
    for &vertex in brep.vertex_list() {
        work.charge(1, cancel)?;
        let v = topology(brep.vertex(vertex))?.point;
        if family
            .value([v.x, v.y, v.z].map(Interval::point))
            .contains_zero()
        {
            return Err(indeterminate(
                "plane homotopy may meet an original native vertex",
            ));
        }
    }
    for &face_id in definition.face_ids() {
        work.charge(1, cancel)?;
        let face = topology(brep.face(face_id))?;
        let surface = definition
            .face_surface(face_id)
            .ok_or_else(|| indeterminate("plane homotopy face has no carrier"))?;
        let min = face.uv_domain.min();
        let max = face.uv_domain.max();
        let domain = [[min.x, max.x], [min.y, max.y]];
        if !p::surface_parameter_span_smooth(surface, domain, work, cancel)? {
            return Err(indeterminate(
                "plane homotopy lacks whole native carrier C1 authority",
            ));
        }
        for box_ in p::surface_parameter_boxes(surface, domain, work, cancel)? {
            if face.loops.is_empty()
                && family
                    .value(surface_jet(surface, box_, 1.0, work, cancel)?.position)
                    .contains_zero()
            {
                return Err(indeterminate(
                    "plane family may enter a native periodic/interior chart without qualified boundary strata",
                ));
            }
            family.face_cell(surface, box_, 0, work, cancel)?;
        }
        for &loop_id in &face.loops {
            let loop_ = topology(brep.loop_(loop_id))?;
            let lu_id = loop_
                .loopuse
                .ok_or_else(|| indeterminate("plane homotopy trim has no loopuse"))?;
            let lu = topology(brep.loopuse(lu_id))?;
            let solidcore::topology::LoopuseStart::Edgeuse(Some(first)) = lu.start else {
                return Err(indeterminate(
                    "plane homotopy has an unqualified vertex trim",
                ));
            };
            let mut eu_id = first;
            for _ in 0..limit {
                work.charge(1, cancel)?;
                let eu = topology(brep.edgeuse(eu_id))?;
                if eu.owner != solidcore::topology::EdgeuseOwner::Loopuse(Some(lu_id)) {
                    return Err(indeterminate(
                        "plane homotopy coedge belongs to a foreign loop",
                    ));
                }
                let edge = eu
                    .edge
                    .ok_or_else(|| indeterminate("plane homotopy coedge has no edge"))?;
                let boundary = Boundary::new(definition, surface, eu_id, edge, 1.0, work, cancel)?;
                let (curve, range) = match boundary.uv_curve {
                    Some(curve) => {
                        let r = curve.natural_interval();
                        (curve, [r.min(), r.max()])
                    }
                    None => (boundary.curve, boundary.edge_range),
                };
                if !p::curve_parameter_span_smooth(curve, range, work, cancel)? {
                    return Err(indeterminate(
                        "plane homotopy lacks C1 native trim authority",
                    ));
                }
                for endpoint in [0.0, 1.0] {
                    if family
                        .value(boundary.evaluate([endpoint; 2], work, cancel)?.0)
                        .contains_zero()
                    {
                        return Err(indeterminate(
                            "plane homotopy may meet an actual chart trim vertex",
                        ));
                    }
                }
                family.boundary_cell(&boundary, [0.0, 1.0], 0, work, cancel)?;
                eu_id = eu
                    .ccw
                    .ok_or_else(|| indeterminate("plane homotopy trim is open"))?;
                if eu_id == first {
                    break;
                }
            }
            if eu_id != first {
                return Err(indeterminate("plane homotopy trim cycle does not close"));
            }
        }
    }
    Ok(())
}
struct FunctionalFamily {
    normal: [Interval; 3],
    offset: Interval,
}
impl FunctionalFamily {
    fn project(&self, vector: [Interval; 3]) -> Interval {
        (0..3).fold(Z, |sum, i| sum.add(self.normal[i].mul(vector[i])))
    }
    fn value(&self, position: [Interval; 3]) -> Interval {
        self.project(position).add(self.offset)
    }
    fn face_cell(
        &self,
        surface: &Surface,
        uv: [[f64; 2]; 2],
        depth: u8,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        work.charge(1, cancel)?;
        let jet = surface_jet(surface, uv, 1.0, work, cancel)?;
        if !self.value(jet.position).contains_zero() {
            return Ok(());
        }
        let normal = p::cross(jet.du, jet.dv);
        let regular = normal.into_iter().fold(Z, |sum, v| sum.add(v.square())).lo > 0.0;
        if regular && (sign(self.project(jet.du)) != 0 || sign(self.project(jet.dv)) != 0) {
            return Ok(());
        }
        if depth >= 24 {
            return Err(indeterminate(
                "plane family may meet a native carrier singularity or critical point",
            ));
        }
        let axis = depth as usize % 2;
        let mid = uv[axis][0] * 0.5 + uv[axis][1] * 0.5;
        if mid <= uv[axis][0] || mid >= uv[axis][1] {
            return Err(indeterminate(
                "plane family critical-cell subdivision exhausted",
            ));
        }
        let mut a = uv;
        a[axis][1] = mid;
        let mut b = uv;
        b[axis][0] = mid;
        self.face_cell(surface, a, depth + 1, work, cancel)?;
        self.face_cell(surface, b, depth + 1, work, cancel)
    }
    fn boundary_cell(
        &self,
        boundary: &Boundary<'_>,
        range: [f64; 2],
        depth: u8,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        work.charge(1, cancel)?;
        let (position, derivative, _, _) = boundary.evaluate(range, work, cancel)?;
        if !self.value(position).contains_zero() || sign(self.project(derivative)) != 0 {
            return Ok(());
        }
        if depth >= 32 {
            return Err(indeterminate(
                "plane family may be tangent to an original native trim",
            ));
        }
        let mid = range[0] * 0.5 + range[1] * 0.5;
        if mid <= range[0] || mid >= range[1] {
            return Err(indeterminate("plane family trim subdivision exhausted"));
        }
        self.boundary_cell(boundary, [range[0], mid], depth + 1, work, cancel)?;
        self.boundary_cell(boundary, [mid, range[1]], depth + 1, work, cancel)
    }
}

pub(crate) fn canonicalize_section_in_place(
    section: &mut NativeSection,
    byte_limit: u32,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    work.charge(1, cancel)?;
    if section.retained_bytes() > byte_limit as usize {
        return Err(resource_error(
            "placed section exceeds canonicalization allowance",
        ));
    }
    if section.loop_source_faces.len() != section.loops.len()
        || section.loop_spans.len() != section.loops.len()
    {
        return Err(indeterminate(
            "placed native section lost its paired original lineage",
        ));
    }
    for boundary in &mut section.loops {
        if boundary.points_mm.len() < 4 || boundary.points_mm.first() != boundary.points_mm.last() {
            return Err(indeterminate("placed native section is not exactly closed"));
        }
        let area = signed_area(&boundary.points_mm, section.frame, work, cancel)?;
        if sign(area) == 0 || (sign(area) < 0) != boundary.is_hole {
            return Err(indeterminate(
                "placed native section lost its material-left winding",
            ));
        }
        canonicalize_points(&mut boundary.points_mm, section.frame, work, cancel)?;
    }
    canonicalize_loops(section, work, cancel)?;
    certify_polygons(section, work, cancel)
}

#[cfg(test)]
mod correspondence_tests {
    use super::*;
    use solidcore::nurbs::{
        b_spline_curve::BSplineCurve,
        b_spline_surface::BSplineSurface,
        crv_basic,
        math::{CPoint, NOW, NOZ},
        srf_basic,
    };

    fn curve(points: Vec<CPoint>, degree: usize, knots: Vec<f64>, dim: u64) -> Curve {
        BSplineCurve::from_nurb(
            crv_basic::Curve::new(points, degree, knots).unwrap(),
            dim,
            true,
        )
        .into()
    }

    fn carrier() -> Surface {
        let net = [0.0, 1.0]
            .map(|u| vec![CPoint::new(u, 0.0, 0.0, NOW), CPoint::new(u, 1.0, 0.0, NOW)])
            .to_vec();
        Surface::new(SurfaceKind::BSpline(BSplineSurface::from_nurb(
            srf_basic::Surface::new(
                net,
                1,
                1,
                vec![0.0, 0.0, 1.0, 1.0],
                vec![0.0, 0.0, 1.0, 1.0],
            )
            .unwrap(),
            true,
        )))
    }

    fn spatial() -> Curve {
        // x(t)=2t/(1+t), not x(t)=t, despite identical directed trace.
        curve(
            vec![
                CPoint::new(0.0, 0.0, 0.0, 1.0),
                CPoint::new(2.0, 0.0, 0.0, 2.0),
            ],
            1,
            vec![0.0, 0.0, 1.0, 1.0],
            3,
        )
    }

    #[test]
    fn native_boundary_calibrates_nonuniform_gauge_without_losing_trace() {
        let surface = carrier();
        let spatial = spatial();
        let chart = curve(
            vec![
                CPoint::new(0.0, 0.0, NOZ, NOW),
                CPoint::new(1.0, 0.0, NOZ, NOW),
            ],
            1,
            vec![2.0, 2.0, 5.0, 5.0],
            2,
        );
        let boundary = Boundary {
            surface: &surface,
            curve: &spatial,
            uv_curve: Some(&chart),
            edge_range: [0.0, 1.0],
            scale: 1.0,
        };
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let (point, derivative, _, _) = boundary.evaluate([0.5; 2], &mut work, &cancel).unwrap();
        let reference = p::curve_prepared::NativeChartCurve {
            surface: &surface,
            chart: &chart,
        };
        let raw = p::CurveEvaluator::tensor(&reference, [3.5; 2], 1, &mut work, &cancel).unwrap();
        assert!(point[0].contains(0.5));
        assert!(derivative[0].contains(1.0));
        assert!(raw[0][0].contains(0.5));
        assert!(raw[1][0].mul(Interval::point(3.0)).contains(1.0));
        let native = boundary.edge_jet([0.5; 2], &mut work, &cancel).unwrap();
        assert!(norm(std::array::from_fn(|i| point[i].sub(native.position[i]))) > 0.1);
        boundary
            .prove_correspondence(1e-6, 1024 * 1024, &mut work, &cancel)
            .unwrap();
    }

    #[test]
    fn native_boundary_matches_complete_rational_row_and_checked_reverse_gauge() {
        let w = 0.5_f64.sqrt();
        let row = [
            CPoint::new(1.0, 0.0, 0.0, 1.0),
            CPoint::new(w, w, 0.0, w),
            CPoint::new(0.0, 1.0, 0.0, 1.0),
        ];
        let surface = Surface::new(SurfaceKind::BSpline(BSplineSurface::from_nurb(
            srf_basic::Surface::new(
                row.iter()
                    .map(|p| vec![*p, CPoint::new(p.x, p.y, 2.0 * p.w, p.w)])
                    .collect(),
                2,
                1,
                vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
                vec![0.0, 0.0, 1.0, 1.0],
            )
            .unwrap(),
            true,
        )));
        let spatial = curve(row.to_vec(), 2, vec![7.0, 7.0, 7.0, 11.0, 11.0, 11.0], 3);
        let cancel = AtomicBool::new(false);
        for (uv, target_range) in [([0.0, 1.0], [7.0, 11.0]), ([1.0, 0.0], [11.0, 7.0])] {
            let chart = curve(
                uv.map(|u| CPoint::new(u, 0.0, NOZ, NOW)).to_vec(),
                1,
                vec![2.0, 2.0, 5.0, 5.0],
                2,
            );
            let boundary = Boundary {
                surface: &surface,
                curve: &spatial,
                uv_curve: Some(&chart),
                edge_range: target_range,
                scale: 1.0,
            };
            // A whole rational span with a distinct literal gauge must not
            // spend the raw query's work on thousands of local calibrations.
            let mut work = GeometryWorkBudget::new(50_000);
            boundary
                .prove_correspondence(0.0, 1024 * 1024, &mut work, &cancel)
                .unwrap();
        }

        let chart = curve(
            vec![
                CPoint::new(0.0, 0.0, NOZ, NOW),
                CPoint::new(1.0, 0.0, NOZ, NOW),
            ],
            1,
            vec![2.0, 2.0, 5.0, 5.0],
            2,
        );
        let reference = p::curve_prepared::NativeChartCurve {
            surface: &surface,
            chart: &chart,
        };
        let mut changed = row;
        changed[1].x = changed[1].x.next_up();
        let near = curve(
            changed.to_vec(),
            2,
            vec![7.0, 7.0, 7.0, 11.0, 11.0, 11.0],
            3,
        );
        let mut work = GeometryWorkBudget::new(1_000_000);
        assert!(
            !reference
                .isoparametric_coefficient_identity(&near, [7.0, 11.0], &mut work, &cancel)
                .unwrap()
        );
        assert!(
            !reference
                .isoparametric_coefficient_identity(&spatial, [11.0, 7.0], &mut work, &cancel)
                .unwrap()
        );
        assert!(
            !reference
                .isoparametric_coefficient_identity(&spatial, [7.0, 9.0], &mut work, &cancel)
                .unwrap()
        );

        let wrong_chart = curve(
            vec![
                CPoint::new(0.0, 1.0, NOZ, NOW),
                CPoint::new(1.0, 1.0, NOZ, NOW),
            ],
            1,
            vec![2.0, 2.0, 5.0, 5.0],
            2,
        );
        let mut displaced = row;
        displaced[1].x += 0.001;
        let near = curve(
            displaced.to_vec(),
            2,
            vec![7.0, 7.0, 7.0, 11.0, 11.0, 11.0],
            3,
        );
        for (curve, chart, edge_range) in [
            (&near, &chart, [7.0, 11.0]),
            (&spatial, &chart, [11.0, 7.0]),
            (&spatial, &wrong_chart, [7.0, 11.0]),
        ] {
            let wrong = Boundary {
                surface: &surface,
                curve,
                uv_curve: Some(chart),
                edge_range,
                scale: 1.0,
            };
            let mut work = GeometryWorkBudget::new(1_000_000);
            assert_eq!(
                wrong
                    .prove_correspondence(1e-6, 1024 * 1024, &mut work, &cancel)
                    .unwrap_err()
                    .code,
                GeometryErrorCode::AnalysisIndeterminate,
            );
        }
    }

    #[test]
    fn original_fitting_whole_coedge_correspondence_fits_raw_query_work() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/geometry/independent-exporters/occt-tapered-fitting-mm.step");
        let cancel = AtomicBool::new(false);
        let definition =
            crate::from_step_bytes_with_cancel(&std::fs::read(path).unwrap(), &cancel).unwrap();
        let brep = definition.brep();
        let face_id = definition.face_ids()[0];
        let face = brep.face(face_id).unwrap();
        let surface = definition.face_surface(face_id).unwrap();
        // This is the actual original cap/rim correspondence which precedes
        // the Z6 raw section. No prepared receipt or plane-root work is reused.
        let mut work = GeometryWorkBudget::new(20_000_000);
        let mut proved = 0;
        for &loop_id in &face.loops {
            let loop_ = brep.loop_(loop_id).unwrap();
            let lu = brep.loopuse(loop_.loopuse.unwrap()).unwrap();
            let solidcore::topology::LoopuseStart::Edgeuse(Some(first)) = lu.start else {
                panic!("original fitting has an explicit native coedge cycle");
            };
            let mut id = first;
            for _ in 0..brep.edgeuses().len() {
                let eu = brep.edgeuse(id).unwrap();
                let boundary = Boundary::new(
                    &definition,
                    surface,
                    id,
                    eu.edge.unwrap(),
                    1.0,
                    &mut work,
                    &cancel,
                )
                .unwrap();
                boundary
                    .prove_correspondence(
                        SECTION_SAMPLING_TOLERANCE_MM / 64.0,
                        1024 * 1024,
                        &mut work,
                        &cancel,
                    )
                    .unwrap();
                proved += 1;
                id = eu.ccw.unwrap();
                if id == first {
                    break;
                }
            }
            assert_eq!(id, first);
        }
        assert!(proved > 0);
    }

    #[test]
    fn native_boundary_covers_stationary_germs_at_either_endpoint() {
        let surface = carrier();
        let spatial = spatial();
        for (middle, endpoint) in [(0.0, 0.0), (1.0, 1.0)] {
            // x(r)=r² or x(r)=1-(1-r)². The directed physical trace
            // still covers [0,1], but one chart endpoint has zero speed.
            let chart = curve(
                vec![
                    CPoint::new(0.0, 0.0, NOZ, NOW),
                    CPoint::new(middle, 0.0, NOZ, NOW),
                    CPoint::new(1.0, 0.0, NOZ, NOW),
                ],
                2,
                vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
                2,
            );
            let boundary = Boundary {
                surface: &surface,
                curve: &spatial,
                uv_curve: Some(&chart),
                edge_range: [0.0, 1.0],
                scale: 1.0,
            };
            let cancel = AtomicBool::new(false);
            let mut work = GeometryWorkBudget::new(1_000_000);
            let (position, derivative, _, _) = boundary
                .evaluate([endpoint; 2], &mut work, &cancel)
                .unwrap();
            assert!(position[0].contains(endpoint));
            assert!(derivative.into_iter().all(|v| v.lo == 0.0 && v.hi == 0.0));
            assert!(
                boundary
                    .edge_jet([endpoint; 2], &mut work, &cancel)
                    .unwrap()
                    .derivative[0]
                    .lo
                    > 0.0
            );
            boundary
                .prove_correspondence(1e-6, 1024 * 1024, &mut work, &cancel)
                .unwrap();
        }
    }

    #[test]
    fn native_boundary_rejects_bowed_trace_and_missing_chart() {
        let surface = carrier();
        let spatial = spatial();
        // Both original endpoints still agree; the interior trace is displaced.
        let chart = curve(
            vec![
                CPoint::new(0.0, 0.0, NOZ, NOW),
                CPoint::new(0.5, 0.01, NOZ, NOW),
                CPoint::new(1.0, 0.0, NOZ, NOW),
            ],
            2,
            vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            2,
        );
        let mut boundary = Boundary {
            surface: &surface,
            curve: &spatial,
            uv_curve: Some(&chart),
            edge_range: [0.0, 1.0],
            scale: 1.0,
        };
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        assert_eq!(
            boundary
                .prove_correspondence(1e-6, 1024 * 1024, &mut work, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::AnalysisIndeterminate
        );
        boundary.uv_curve = None;
        assert_eq!(
            boundary
                .prove_correspondence(1e-6, 1024 * 1024, &mut work, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::AnalysisIndeterminate
        );
    }
}
