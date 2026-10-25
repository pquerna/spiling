// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Complete oriented UV chains, source-loop bijection and coefficient-bounded
//! world-space trim transport. Inversion may propose display UVs; it never
//! supplies original chart or manufacturing authority.

use super::{
    audit::{norm_upper, triangle_uv},
    check_memory, invalid, native_error, resource, shape,
};
use crate::{
    GeometryWorkBudget,
    kernel::Definition,
    parametric::{self, Interval, PreparedValidatedCurve},
};
use solidcore::{curve::curve::Curve, surface::surface::Surface, topology::edgeuse::p1 as eu};
use solidcore_mesh::{EdgeSet, FaceMesh};
use spiling_contracts::geometry::{GeometryError, GeometryErrorCode, MESH_CARRIER_TOLERANCE_MM};
use std::{mem::size_of, sync::atomic::AtomicBool};

#[derive(Clone, Copy)]
struct HalfEdge {
    a: u32,
    b: u32,
}
#[derive(Clone, Copy)]
struct Boundary {
    a: u32,
    b: u32,
    source_loop: usize,
}
struct SourceSegment<'a> {
    key: [u32; 2],
    start_vertex: u32,
    source_loop: usize,
    curve: PreparedValidatedCurve<'a>,
    range: [f64; 2],
    uv_curve: Option<PreparedValidatedCurve<'a>>,
    uv_range: [f64; 2],
}
fn key(a: u32, b: u32) -> [u32; 2] {
    if a < b { [a, b] } else { [b, a] }
}

pub(super) fn certify(
    definition: &Definition,
    mesh: &FaceMesh,
    edges: &EdgeSet,
    sense: f64,
    outside: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(f64, super::singular::Inventory), GeometryError> {
    let brep = definition.brep();
    let face = brep.face(mesh.face).map_err(native_error)?;
    let surface = face
        .surface
        .as_ref()
        .ok_or_else(|| invalid("native display trim has no carrier"))?;
    let fu = brep
        .faceuse(
            face.faceuse
                .ok_or_else(|| invalid("native face has no loop owner"))?,
        )
        .map_err(native_error)?;
    if fu.loopuses.is_empty() || fu.loopuses.len() != face.loops.len() {
        return Err(invalid(
            "native display face has absent or inconsistent source loops",
        ));
    }
    let segment_count = face.loops.iter().try_fold(0usize, |count, &loop_| {
        let record = brep.loop_(loop_).map_err(native_error)?;
        record.edges.iter().try_fold(count, |count, edge| {
            let poly = edges
                .polyline(edge.edge)
                .ok_or_else(|| invalid("source trim edge is missing from native candidate"))?;
            Ok::<_, GeometryError>(count.saturating_add(poly.vertices.len().saturating_sub(1)))
        })
    })?;
    let use_count = face.loops.iter().try_fold(0usize, |count, &loop_| {
        Ok::<_, GeometryError>(
            count.saturating_add(brep.loop_(loop_).map_err(native_error)?.edges.len()),
        )
    })?;
    let boundary_capacity = edge_count_for_boundary(mesh, use_count)?;
    let edge_count = mesh
        .triangles
        .len()
        .checked_mul(3)
        .ok_or_else(|| resource("native triangle chain count overflow"))?;
    let requested = edge_count
        .saturating_mul(size_of::<HalfEdge>())
        .saturating_add(segment_count.saturating_mul(size_of::<SourceSegment<'_>>()))
        .saturating_add(boundary_capacity.saturating_mul(size_of::<Boundary>() + size_of::<bool>()))
        .saturating_add(
            mesh.positions
                .len()
                .saturating_add(2 * use_count)
                .saturating_mul(size_of::<Option<usize>>() + size_of::<bool>()),
        )
        .saturating_add(fu.loopuses.len().saturating_mul(size_of::<bool>()))
        .saturating_add(parametric::curve_prepared::PreparedValidatedCurve::scratch_bytes())
        .saturating_add(2 * size_of::<PreparedValidatedCurve<'_>>())
        .saturating_add(parametric::nurbs_eval::SURFACE_EVALUATION_SCRATCH_BYTES);
    check_memory(outside.saturating_add(requested))?;
    let mut source = Vec::new();
    source
        .try_reserve_exact(segment_count)
        .map_err(|_| resource("native trim source inventory allocation failed"))?;
    let requested = requested.saturating_add(
        source
            .capacity()
            .saturating_sub(segment_count)
            .saturating_mul(size_of::<SourceSegment<'_>>()),
    );
    check_memory(outside.saturating_add(requested))?;
    for (loop_index, &loopuse) in fu.loopuses.iter().enumerate() {
        work.charge(1, cancel)?;
        if brep.loopuse(loopuse).map_err(native_error)?.loop_ != Some(face.loops[loop_index]) {
            return Err(invalid(
                "native faceuse loop order changed original face topology",
            ));
        }
        let uses = eu::loopuse_edgeuses(brep, loopuse).map_err(native_error)?;
        if uses.is_empty() {
            return Err(invalid("native display source trim loop is empty"));
        }
        check_memory(
            outside.saturating_add(requested).saturating_add(
                uses.capacity()
                    .saturating_mul(size_of::<solidcore::topology::EdgeuseId>()),
            ),
        )?;
        for edgeuse in uses {
            work.charge(1, cancel)?;
            let edge = eu::edge(brep, edgeuse)
                .map_err(native_error)?
                .ok_or_else(|| invalid("native trim use has no edge"))?;
            let record = brep.edge(edge).map_err(native_error)?;
            let curve = record
                .curve
                .as_ref()
                .ok_or_else(|| invalid("native trim edge has no coefficient curve"))?;
            let poly = edges
                .polyline(edge)
                .ok_or_else(|| invalid("native trim candidate omitted an original edge"))?;
            if poly.degenerate {
                return Err(GeometryError::new(
                    GeometryErrorCode::SingularGeometry,
                    "native display pole trim needs coefficient-based one-sided closure authority",
                ));
            }
            if poly.params.len() != poly.vertices.len()
                || poly.params.len() < 2
                || poly.params[0] != record.interval.min()
                || *poly.params.last().unwrap() != record.interval.max()
            {
                return Err(invalid(
                    "native trim polyline does not cover the complete original edge interval",
                ));
            }
            let uv_curve = eu::uv_trim_curve(brep, edgeuse).map_err(native_error)?.0;
            for index in 0..poly.params.len() - 1 {
                work.charge(1, cancel)?;
                let range = [poly.params[index], poly.params[index + 1]];
                if !range[0].is_finite() || !range[1].is_finite() || range[0] >= range[1] {
                    return Err(invalid(
                        "native trim polyline parameters are not strictly increasing",
                    ));
                }
                let mut uv_range = [0.0; 2];
                if let Some(uv) = uv_curve {
                    let domain = uv.natural_interval();
                    let edge_domain = Interval::point(record.interval.max())
                        .sub(Interval::point(record.interval.min()));
                    for endpoint in 0..2 {
                        let fraction = Interval::point(range[endpoint])
                            .sub(Interval::point(record.interval.min()))
                            .div(edge_domain)?;
                        let t =
                            Interval::point(domain.min()).add(fraction.mul(
                                Interval::point(domain.max()).sub(Interval::point(domain.min())),
                            ));
                        // The whole rounded parameter enclosure is retained.
                        uv_range[endpoint] = if endpoint == 0 {
                            t.lo.max(domain.min())
                        } else {
                            t.hi.min(domain.max())
                        };
                    }
                }
                if source.len() == segment_count {
                    return Err(invalid(
                        "native faceuse contains additional unowned trim intervals",
                    ));
                }
                // Retain one immutable coefficient binding for this exact
                // original interval. Recursive trim leaves reuse its validated
                // knots, not a frozen jet or a candidate parameter sample.
                let owner_bytes = size_of::<PreparedValidatedCurve<'_>>()
                    .saturating_add(PreparedValidatedCurve::scratch_bytes());
                let prepared =
                    PreparedValidatedCurve::new(curve, range, 1.0, owner_bytes, work, cancel)?;
                let prepared_uv = uv_curve
                    .map(|curve| {
                        PreparedValidatedCurve::new(curve, uv_range, 1.0, owner_bytes, work, cancel)
                    })
                    .transpose()?;
                source.push(SourceSegment {
                    key: key(poly.vertices[index], poly.vertices[index + 1]),
                    start_vertex: poly.vertices[index],
                    source_loop: loop_index,
                    curve: prepared,
                    range,
                    uv_curve: prepared_uv,
                    uv_range,
                });
            }
        }
    }
    if source.len() != segment_count {
        return Err(invalid(
            "native face loop and faceuse trim inventories disagree",
        ));
    }
    let mut inventory = super::singular::inventory(
        definition,
        mesh,
        edges,
        use_count,
        outside.saturating_add(requested),
        work,
        cancel,
    )?;
    let outside = outside.saturating_add(inventory.retained_bytes());
    let mut half_edges = Vec::new();
    half_edges
        .try_reserve_exact(edge_count)
        .map_err(|_| resource("native display triangle-chain allocation failed"))?;
    let requested = requested.saturating_add(
        half_edges
            .capacity()
            .saturating_sub(edge_count)
            .saturating_mul(size_of::<HalfEdge>()),
    );
    check_memory(outside.saturating_add(requested))?;
    for &triangle in &mesh.triangles {
        work.charge(1, cancel)?;
        let uv = triangle_uv(mesh, triangle)?;
        let orientation = orient(uv[0], uv[1], uv[2]).mul(Interval::point(sense));
        if orientation.lo <= 0.0 {
            return Err(invalid(
                "native display has degenerate, reversed or uncertain UV triangles",
            ));
        }
        for (a, b) in [
            (triangle[0], triangle[1]),
            (triangle[1], triangle[2]),
            (triangle[2], triangle[0]),
        ] {
            half_edges.push(HalfEdge { a, b });
        }
    }
    work.charge(
        (half_edges.len() as u64).saturating_mul((half_edges.len().max(1).ilog2() + 1) as u64),
        cancel,
    )?;
    half_edges.sort_unstable_by_key(|edge| key(edge.a, edge.b));
    let mut boundary = Vec::new();
    boundary
        .try_reserve_exact(boundary_capacity)
        .map_err(|_| resource("native display trim boundary allocation failed"))?;
    check_memory(
        outside.saturating_add(requested).saturating_add(
            boundary
                .capacity()
                .saturating_sub(boundary_capacity)
                .saturating_mul(size_of::<Boundary>()),
        ),
    )?;
    let mut index = 0;
    while index < half_edges.len() {
        work.charge(1, cancel)?;
        let edge = half_edges[index];
        let mut end = index + 1;
        while end < half_edges.len()
            && key(half_edges[end].a, half_edges[end].b) == key(edge.a, edge.b)
        {
            end += 1;
        }
        match end - index {
            1 => {
                boundary.push(Boundary {
                    a: edge.a,
                    b: edge.b,
                    source_loop: usize::MAX,
                });
            }
            2 if edge.a == half_edges[index + 1].b && edge.b == half_edges[index + 1].a => {}
            _ => {
                return Err(invalid(
                    "native display interior edges do not cancel exactly once",
                ));
            }
        }
        index = end;
    }
    drop(half_edges);
    let mut fan_error = complete_fans(
        mesh,
        surface,
        &inventory,
        &mut boundary,
        sense,
        work,
        cancel,
    )?
    .max(inventory.carrier_error);
    let mut trim_error = 0.0_f64;
    let source_count = boundary
        .iter()
        .filter(|edge| edge.source_loop == usize::MAX)
        .count();
    if source_count != source.len() {
        return Err(invalid(
            "native display omitted an original trim segment or loop",
        ));
    }
    let shared_key = |edge: &Boundary| -> Result<[u32; 2], GeometryError> {
        let a = inventory.shared(mesh, edge.a).ok_or_else(|| {
            invalid("native trim boundary is not anchored on an original shared edge vertex")
        })?;
        let b = inventory.shared(mesh, edge.b).ok_or_else(|| {
            invalid("native trim boundary is not anchored on an original shared edge vertex")
        })?;
        Ok(key(a, b))
    };
    // Check keys before sorting: the comparator never substitutes missing provenance.
    for edge in &boundary {
        shared_key(edge)?;
    }
    work.charge(
        (source.len() as u64).saturating_mul(2 * (source.len().max(1).ilog2() + 1) as u64),
        cancel,
    )?;
    source.sort_unstable_by_key(|segment| segment.key);
    boundary.sort_unstable_by_key(|edge| {
        (
            edge.source_loop != usize::MAX,
            shared_key(edge).expect("checked native boundary provenance"),
        )
    });
    for (index, segment) in source.iter().enumerate() {
        let mut edge = boundary[index];
        work.charge(1, cancel)?;
        if shared_key(&edge)? != segment.key {
            return Err(invalid(
                "native mesh boundary does not biject the original trim intervals",
            ));
        }
        edge.source_loop = segment.source_loop;
        // A snapshot may omit the terminal source pole UV as well as interior
        // replicas. Complete that real curved fan before replacing its endpoint
        // in the proof graph; published vertices and triangles stay unchanged.
        for endpoint in 0..2 {
            let local = if endpoint == 0 { edge.a } else { edge.b };
            if let Some(junction) = inventory.at(local) {
                let curve = segment
                    .uv_curve
                    .as_ref()
                    .ok_or_else(|| {
                        invalid("singular source boundary has no native UV endpoint authority")
                    })?
                    .curve()
                    .native_curve()
                    .ok_or_else(|| invalid("singular display lost its native UV curve binding"))?;
                let shared = inventory.shared(mesh, local).unwrap();
                let phantom = inventory.source_endpoint(
                    mesh,
                    local,
                    curve,
                    shared != segment.start_vertex,
                    work,
                    cancel,
                )?;
                if inventory.uv(mesh, local) != inventory.uv(mesh, phantom) {
                    fan_error = fan_error.max(super::singular::fan(
                        surface,
                        mesh,
                        &inventory,
                        [edge.b, edge.a, phantom],
                        sense,
                        work,
                        cancel,
                    )?);
                    let row = if endpoint == 0 {
                        (local, phantom)
                    } else {
                        (phantom, local)
                    };
                    if boundary.len() == boundary_capacity {
                        return Err(resource(
                            "singular source chart closure exceeded admitted inventory",
                        ));
                    }
                    boundary.push(Boundary {
                        a: row.0,
                        b: row.1,
                        source_loop: junction.source_loop,
                    });
                    if endpoint == 0 {
                        edge.a = phantom;
                    } else {
                        edge.b = phantom;
                    }
                }
            }
        }
        boundary[index] = edge;
        let a = inventory.position(mesh, edge.a);
        let b = inventory.position(mesh, edge.b);
        let shared_a = inventory.shared(mesh, edge.a).unwrap();
        let chord = if shared_a == segment.start_vertex {
            [a, b]
        } else {
            [b, a]
        };
        trim_error = trim_error.max(curve_chord(
            &segment.curve,
            segment.range,
            None,
            chord,
            work,
            cancel,
        )?);
        if let Some(uv_curve) = &segment.uv_curve {
            trim_error = trim_error.max(curve_chord(
                uv_curve,
                segment.uv_range,
                Some(surface),
                chord,
                work,
                cancel,
            )?);
        }
    }
    circuits(
        mesh,
        &inventory,
        &boundary,
        fu.loopuses.len(),
        sense,
        work,
        cancel,
    )?;
    inventory.carrier_error = fan_error;
    Ok((trim_error, inventory))
}

fn edge_count_for_boundary(mesh: &FaceMesh, uses: usize) -> Result<usize, GeometryError> {
    mesh.triangles
        .len()
        .checked_mul(3)
        .and_then(|n| uses.checked_mul(2).and_then(|extra| n.checked_add(extra)))
        .ok_or_else(|| resource("singular boundary inventory count overflow"))
}

fn quotient_key(edge: &Boundary, inventory: &super::singular::Inventory) -> [(u8, u32); 2] {
    let identity = |local| inventory.at(local).map_or((0, local), |j| (1, j.shared));
    let a = identity(edge.a);
    let b = identity(edge.b);
    if a < b { [a, b] } else { [b, a] }
}

fn complete_fans(
    mesh: &FaceMesh,
    surface: &Surface,
    inventory: &super::singular::Inventory,
    boundary: &mut Vec<Boundary>,
    sense: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<f64, GeometryError> {
    work.charge(
        (boundary.len() as u64).saturating_mul((boundary.len().max(1).ilog2() + 1) as u64),
        cancel,
    )?;
    boundary.sort_unstable_by(|a, b| {
        quotient_key(a, inventory)
            .cmp(&quotient_key(b, inventory))
            .then_with(|| {
                let pole = |edge: &Boundary| {
                    if inventory.at(edge.a).is_some() {
                        edge.a
                    } else {
                        edge.b
                    }
                };
                inventory.uv(mesh, pole(a))[0].total_cmp(&inventory.uv(mesh, pole(b))[0])
            })
    });
    let mut index = 0;
    let mut written = 0;
    let mut error = 0.0_f64;
    while index < boundary.len() {
        work.charge(1, cancel)?;
        let key = quotient_key(&boundary[index], inventory);
        let mut end = index + 1;
        while end < boundary.len() && quotient_key(&boundary[end], inventory) == key {
            end += 1;
        }
        if end == index + 1 {
            boundary[written] = boundary[index];
            written += 1;
        } else {
            if (end - index) % 2 != 0 {
                return Err(invalid(
                    "singular display spokes have unbalanced multiplicity",
                ));
            }
            for pair in (index..end).step_by(2) {
                work.charge(1, cancel)?;
                let a = boundary[pair];
                let b = boundary[pair + 1];
                let (outgoing, incoming) = if inventory.at(a.a).is_some() {
                    (a, b)
                } else {
                    (b, a)
                };
                let junction = inventory.at(outgoing.a).ok_or_else(|| {
                    invalid("unowned extra boundary has no proved singular endpoint")
                })?;
                if inventory.at(outgoing.b).is_some()
                    || inventory.at(incoming.a).is_some()
                    || inventory.at(incoming.b).map(|j| j.shared) != Some(junction.shared)
                    || outgoing.b != incoming.a
                {
                    return Err(invalid(
                        "singular display spokes do not cancel with opposite exact regular incidence",
                    ));
                }
                error = error.max(super::singular::fan(
                    surface,
                    mesh,
                    inventory,
                    [outgoing.b, outgoing.a, incoming.b],
                    sense,
                    work,
                    cancel,
                )?);
                boundary[written] = Boundary {
                    a: outgoing.a,
                    b: incoming.b,
                    source_loop: junction.source_loop,
                };
                written += 1;
            }
        }
        index = end;
    }
    boundary.truncate(written);
    Ok(error)
}

fn curve_chord(
    curve: &PreparedValidatedCurve<'_>,
    range: [f64; 2],
    surface: Option<&Surface>,
    chord: [[f64; 3]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<f64, GeometryError> {
    // Every leaf covers a closed parameter interval. Taylor is used only with
    // independently established same-span C1; other leaves use full positions.
    let native = curve
        .curve()
        .native_curve()
        .ok_or_else(|| invalid("display trim lost its original native coefficient binding"))?;
    let mut stack = [([0.0; 2], 0u8); 33];
    stack[0] = (range, 0);
    let mut len = 1;
    let mut maximum = 0.0_f64;
    while len > 0 {
        len -= 1;
        let (part, depth) = stack[len];
        work.charge(1, cancel)?;
        let jet = curve.jet(part, work, cancel)?;
        let mut position = jet.position;
        let mut second = jet.second_derivative;
        let mut smooth = curve.is_smooth_on(part, work, cancel)?;
        if let Some(surface) = surface {
            let domain = bounded_uv(native, part, surface, position, work, cancel)?;
            let s = parametric::surface_piecewise_enclosure(surface, domain, work, cancel)?;
            smooth &= parametric::surface_parameter_span_smooth(surface, domain, work, cancel)?;
            second = std::array::from_fn(|i| {
                s.duu[i]
                    .mul(jet.derivative[0].square())
                    .add(
                        Interval::point(2.0)
                            .mul(s.duv[i])
                            .mul(jet.derivative[0])
                            .mul(jet.derivative[1]),
                    )
                    .add(s.dvv[i].mul(jet.derivative[1].square()))
                    .add(s.du[i].mul(jet.second_derivative[0]))
                    .add(s.dv[i].mul(jet.second_derivative[1]))
            });
            position = s.position;
        }
        let chord_at = |t: Interval| -> Result<[Interval; 3], GeometryError> {
            let fraction = t
                .sub(Interval::point(range[0]))
                .div(Interval::point(range[1]).sub(Interval::point(range[0])))?;
            Ok(std::array::from_fn(|i| {
                Interval::point(chord[0][i]).add(
                    fraction.mul(Interval::point(chord[1][i]).sub(Interval::point(chord[0][i]))),
                )
            }))
        };
        let error = if smooth {
            let mut residual = [Interval::point(0.0); 3];
            for t in part {
                let p = curve.position_bounds([t, t], work, cancel)?;
                let p = if let Some(s) = surface {
                    let domain = bounded_uv(native, [t, t], s, p, work, cancel)?;
                    parametric::surface_position_enclosure(s, domain, work, cancel)?
                } else {
                    p
                };
                let line = chord_at(Interval::point(t))?;
                for i in 0..3 {
                    residual[i] = residual[i].hull(p[i].sub(line[i]));
                }
            }
            let factor = Interval::point(part[1])
                .sub(Interval::point(part[0]))
                .square()
                .mul(Interval::point(0.125));
            norm_upper(std::array::from_fn(|i| {
                Interval::new_unchecked(-residual[i].abs_upper(), residual[i].abs_upper()).add(
                    Interval::new_unchecked(-second[i].abs_upper(), second[i].abs_upper())
                        .mul(factor),
                )
            }))
        } else {
            let line = chord_at(Interval::new(part[0], part[1])?)?;
            norm_upper(std::array::from_fn(|i| position[i].sub(line[i])))
        };
        if error.is_finite() && error <= MESH_CARRIER_TOLERANCE_MM * 0.25 {
            maximum = maximum.max(error);
            continue;
        }
        if depth == 32 {
            return Err(shape(
                "complete original trim interval exceeds display transport tolerance",
            ));
        }
        let middle = part[0] * 0.5 + part[1] * 0.5;
        if middle <= part[0] || middle >= part[1] {
            return Err(shape(
                "native trim interval cannot be resolved at finite precision",
            ));
        }
        stack[len] = ([middle, part[1]], depth + 1);
        stack[len + 1] = ([part[0], middle], depth + 1);
        len += 2;
    }
    Ok(maximum)
}

fn circuits(
    mesh: &FaceMesh,
    inventory: &super::singular::Inventory,
    boundary: &[Boundary],
    loop_count: usize,
    sense: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    let mut next = vec![None; inventory.node_count(mesh)];
    let mut incoming = vec![false; inventory.node_count(mesh)];
    for (index, edge) in boundary.iter().enumerate() {
        work.charge(1, cancel)?;
        if next[edge.a as usize].replace(index).is_some()
            || std::mem::replace(&mut incoming[edge.b as usize], true)
        {
            return Err(invalid("native trim boundary branches or repeats a vertex"));
        }
    }
    let mut seen = vec![false; boundary.len()];
    let mut loops = vec![false; loop_count];
    for start in 0..boundary.len() {
        if seen[start] {
            continue;
        }
        let owner = boundary[start].source_loop;
        if loops[owner] {
            return Err(invalid(
                "native source trim loop is split into multiple mesh circuits",
            ));
        }
        loops[owner] = true;
        let mut current = start;
        let origin = inventory.uv(mesh, boundary[start].a);
        let mut area = Interval::point(0.0);
        loop {
            work.charge(1, cancel)?;
            if seen[current] || boundary[current].source_loop != owner {
                return Err(invalid("native display circuit changes original trim loop"));
            }
            seen[current] = true;
            let a = inventory.uv(mesh, boundary[current].a);
            let b = inventory.uv(mesh, boundary[current].b);
            area = area.add(orient(origin, a, b));
            current = next[boundary[current].b as usize]
                .ok_or_else(|| invalid("native display trim circuit is open"))?;
            if current == start {
                break;
            }
        }
        let winding = area.mul(Interval::point(sense));
        if (owner == 0 && winding.lo <= 0.0) || (owner != 0 && winding.hi >= 0.0) {
            return Err(invalid(
                "native display outer/hole circuit winding disagrees with original loop ownership",
            ));
        }
    }
    if loops.iter().any(|v| !*v) {
        return Err(invalid("native display omitted an original trim loop"));
    }
    for (i, a) in boundary.iter().enumerate() {
        let a0 = inventory.uv(mesh, a.a);
        let a1 = inventory.uv(mesh, a.b);
        for b in &boundary[i + 1..] {
            work.charge(1, cancel)?;
            let b0 = inventory.uv(mesh, b.a);
            let b1 = inventory.uv(mesh, b.b);
            if (0..2).any(|axis| {
                a0[axis].max(a1[axis]) < b0[axis].min(b1[axis])
                    || b0[axis].max(b1[axis]) < a0[axis].min(a1[axis])
            }) {
                continue;
            }
            let shared = a.a == b.a || a.a == b.b || a.b == b.a || a.b == b.b;
            if shared {
                let (joint, p, q) = if a.a == b.a {
                    (a0, a1, b1)
                } else if a.a == b.b {
                    (a0, a1, b0)
                } else if a.b == b.a {
                    (a1, a0, b1)
                } else {
                    (a1, a0, b0)
                };
                let turn = orient(joint, p, q);
                if turn.lo > 0.0 || turn.hi < 0.0 {
                    continue;
                }
                let dot = (0..2).fold(Interval::point(0.0), |sum, k| {
                    sum.add(
                        Interval::point(p[k])
                            .sub(Interval::point(joint[k]))
                            .mul(Interval::point(q[k]).sub(Interval::point(joint[k]))),
                    )
                });
                if dot.hi < 0.0 {
                    continue;
                }
                return Err(invalid(
                    "native trim circuit has an overlapping or uncertain joint",
                ));
            }
            let c = orient(a0, a1, b0);
            let d = orient(a0, a1, b1);
            let e = orient(b0, b1, a0);
            let f = orient(b0, b1, a1);
            if (c.lo > 0.0 && d.lo > 0.0)
                || (c.hi < 0.0 && d.hi < 0.0)
                || (e.lo > 0.0 && f.lo > 0.0)
                || (e.hi < 0.0 && f.hi < 0.0)
            {
                continue;
            }
            return Err(invalid(
                "native display trim circuits cross, touch or have uncertain separation",
            ));
        }
    }
    Ok(())
}
pub(super) fn orient(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> Interval {
    let x = |p: [f64; 2], axis| Interval::point(p[axis]).sub(Interval::point(a[axis]));
    x(b, 0).mul(x(c, 1)).sub(x(b, 1).mul(x(c, 0)))
}

fn bounded_uv(
    curve: &Curve,
    range: [f64; 2],
    surface: &Surface,
    position: [Interval; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[[f64; 2]; 2], GeometryError> {
    let natural = surface.natural_uv_domain();
    let domain = [
        [natural.u_min(), natural.u_max()],
        [natural.v_min(), natural.v_max()],
    ];
    let proposed = [
        [position[0].lo, position[0].hi],
        [position[1].lo, position[1].hi],
    ];
    if (0..2).all(|a| proposed[a][0] >= domain[a][0] && proposed[a][1] <= domain[a][1]) {
        return Ok(proposed);
    }
    if !parametric::curve2d_image_within_box(curve, range, domain, work, cancel)? {
        return Err(GeometryError::new(
            GeometryErrorCode::ConditioningFailure,
            "native trim UV enclosure leaves the natural carrier chart without coefficient containment authority",
        ));
    }
    Ok(std::array::from_fn(|a| {
        [
            proposed[a][0].max(domain[a][0]),
            proposed[a][1].min(domain[a][1]),
        ]
    }))
}
