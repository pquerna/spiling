// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Native arena incidence, independent of geometric or source correspondence.
//! A closed orientable combinatorial manifold is necessary, never sufficient,
//! for a qualified filled native material boundary.
use crate::{Definition, GeometryError, GeometryWorkBudget, check_cancel};
use solidcore::nurbs::b_spline_surface_n_lib::p2::OrientType;
use solidcore::{
    EdgeId, VertexId,
    topology::{EdgeuseId, EdgeuseOwner, LoopuseStart, shell::ShellMembers},
};
use spiling_contracts::geometry::{GeometryErrorCode, MAX_STEP_ENTITIES};
use std::{collections::HashMap, mem::size_of, sync::atomic::AtomicBool};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CoedgeUse {
    pub face: usize,
    pub coedge: usize,
    pub wire: usize,
    pub index: usize,
    /// Direction relative to the material-oriented boundary, not UV ownership.
    pub orientation: bool,
    pub edgeuse: EdgeuseId,
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct EdgeUses {
    pub first: CoedgeUse,
    pub second: Option<CoedgeUse>,
    pub vertices: [VertexId; 2],
}
struct VertexEdgeLink {
    edge: EdgeId,
    next: Option<usize>,
}
struct VertexLinks {
    heads: HashMap<VertexId, usize>,
    links: Vec<VertexEdgeLink>,
}
pub(crate) struct SourceEdgeIncidence<'d> {
    definition: &'d Definition,
    edges: HashMap<EdgeId, EdgeUses>,
    vertices: Option<VertexLinks>,
}
fn failure(code: GeometryErrorCode, message: impl AsRef<str>) -> GeometryError {
    GeometryError::new(code, message)
}
fn invalid(message: &'static str) -> GeometryError {
    failure(GeometryErrorCode::SingularGeometry, message)
}
fn native(error: solidcore::SolidError) -> GeometryError {
    failure(
        GeometryErrorCode::ParameterCorrespondenceUnavailable,
        format!("native source incidence: {error}"),
    )
}
fn same(orientation: OrientType) -> Result<bool, GeometryError> {
    match orientation {
        OrientType::Same => Ok(true),
        OrientType::Opposite => Ok(false),
        _ => Err(invalid("native source incidence has unknown orientation")),
    }
}
fn map_bytes<K, V>(map: &HashMap<K, V>) -> usize {
    map.capacity()
        .saturating_mul(size_of::<(K, V)>().saturating_add(16))
        .saturating_mul(2)
}
/// Direct arena access; these functions do not create a parallel face/edge
/// representation or confer geometric correspondence.
pub(crate) fn native_face(
    definition: &Definition,
    ordinal: usize,
) -> Result<(solidcore::FaceId, &solidcore::topology::Face), GeometryError> {
    let id = *definition
        .face_ids()
        .get(ordinal)
        .ok_or_else(|| invalid("native source face ordinal is out of bounds"))?;
    if definition.face_ordinal(id) != Some(ordinal) {
        return Err(invalid("native source face ordinal map changed"));
    }
    Ok((id, definition.brep().face(id).map_err(native)?))
}
pub(crate) fn boundary_loop(
    definition: &Definition,
    ordinal: usize,
    wire: usize,
) -> Result<&solidcore::topology::Loop, GeometryError> {
    let (id, face) = native_face(definition, ordinal)?;
    let loop_id = *face
        .loops
        .get(wire)
        .ok_or_else(|| invalid("native source boundary loop is out of bounds"))?;
    let loop_ = definition.brep().loop_(loop_id).map_err(native)?;
    if loop_.face != id {
        return Err(invalid("native source loop belongs to another face"));
    }
    Ok(loop_)
}
pub(crate) fn native_coedge(
    definition: &Definition,
    ordinal: usize,
    wire: usize,
    index: usize,
) -> Result<(solidcore::topology::LoopEdge, &solidcore::topology::Edge), GeometryError> {
    let entry = *boundary_loop(definition, ordinal, wire)?
        .edges
        .get(index)
        .ok_or_else(|| invalid("native source coedge ordinal is out of bounds"))?;
    Ok((entry, definition.brep().edge(entry.edge).map_err(native)?))
}
/// Outward sign of the actual material shell faceuse. The primary faceuse is
/// the chart owner and must not be substituted for this material orientation.
pub(crate) fn face_orientation(
    definition: &Definition,
    ordinal: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let (id, face) = native_face(definition, ordinal)?;
    let shell = definition.brep().shell(face.shell).map_err(native)?;
    let ShellMembers::Faceuses(uses) = &shell.members else {
        return Err(invalid("native material shell lacks oriented face uses"));
    };
    let mut orientation = None;
    for &use_id in uses {
        work.charge(1, cancel)?;
        let use_ = definition.brep().faceuse(use_id).map_err(native)?;
        if use_.face == Some(id) && orientation.replace(same(use_.orientation)?).is_some() {
            return Err(invalid("native shell repeats a material face use"));
        }
    }
    orientation.ok_or_else(|| invalid("native source face is absent from its material shell"))
}
impl<'d> SourceEdgeIncidence<'d> {
    pub(crate) fn owns_definition(&self, definition: &Definition) -> bool {
        std::ptr::eq(self.definition, definition)
    }
    pub(crate) fn new(
        definition: &'d Definition,
        max_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        work.charge(1, cancel)?;
        let brep = definition.brep();
        let mut result = Self {
            definition,
            edges: HashMap::new(),
            vertices: None,
        };
        if result.owned_bytes() > max_bytes {
            return Err(failure(
                GeometryErrorCode::ResourceLimit,
                "native source incidence header exceeds allowance",
            ));
        }
        if definition.face_ids().is_empty() {
            return Err(invalid("native material boundary has no faces"));
        }
        for (face_index, &face_id) in definition.face_ids().iter().enumerate() {
            work.charge(1, cancel)?;
            if definition.face_ordinal(face_id) != Some(face_index) {
                return Err(invalid("native source face ordinal map is not bijective"));
            }
            let face = brep.face(face_id).map_err(native)?;
            let shell = brep.shell(face.shell).map_err(native)?;
            let ShellMembers::Faceuses(faceuses) = &shell.members else {
                return Err(invalid("native material shell lacks oriented face uses"));
            };
            let mut outward = None;
            for &id in faceuses {
                work.charge(1, cancel)?;
                let record = brep.faceuse(id).map_err(native)?;
                if record.face == Some(face_id) {
                    if outward.replace(same(record.orientation)?).is_some() {
                        return Err(invalid("native material shell repeats a face use"));
                    }
                }
            }
            let outward =
                outward.ok_or_else(|| invalid("native face has no material-oriented shell use"))?;
            let owner = face
                .faceuse
                .ok_or_else(|| invalid("native face lacks its primary chart owner"))?;
            let primary = brep.faceuse(owner).map_err(native)?;
            if primary.face != Some(face_id)
                || !same(primary.orientation)?
                || primary.loopuses.len() != face.loops.len()
                || face.loops.is_empty()
            {
                return Err(invalid(
                    "native primary face/chart ownership is inconsistent",
                ));
            }
            let mut flat = 0usize;
            for (wire, (&loop_id, &loopuse_id)) in
                face.loops.iter().zip(&primary.loopuses).enumerate()
            {
                work.charge(1, cancel)?;
                let loop_ = brep.loop_(loop_id).map_err(native)?;
                let loopuse = brep.loopuse(loopuse_id).map_err(native)?;
                if loop_.face != face_id
                    || loopuse.faceuse != Some(owner)
                    || loopuse.loop_ != Some(loop_id)
                    || loop_.edges.is_empty()
                    || loop_.edges.len() > 2 * MAX_STEP_ENTITIES as usize
                {
                    return Err(invalid(
                        "native face loop has missing or inconsistent source incidence",
                    ));
                }
                let LoopuseStart::Edgeuse(Some(start)) = loopuse.start else {
                    return Err(invalid("native face loop is not a complete edge-use cycle"));
                };
                let mut current = start;
                for (index, loop_edge) in loop_.edges.iter().enumerate() {
                    work.charge(1, cancel)?;
                    let use_ = brep.edgeuse(current).map_err(native)?;
                    let forward = same(use_.orientation)?;
                    if use_.owner != EdgeuseOwner::Loopuse(Some(loopuse_id))
                        || use_.edge != Some(loop_edge.edge)
                        || forward != loop_edge.forward
                    {
                        return Err(invalid("native coedge and ordered loop edge disagree"));
                    }
                    let edge = brep.edge(loop_edge.edge).map_err(native)?;
                    let next = use_
                        .ccw
                        .ok_or_else(|| invalid("native coedge has no next edge use"))?;
                    let next_use = brep.edgeuse(next).map_err(native)?;
                    if next_use.cw != Some(current) {
                        return Err(invalid("native coedge links are not reciprocal"));
                    }
                    let next_edge = brep
                        .edge(
                            next_use
                                .edge
                                .ok_or_else(|| invalid("native next coedge has no edge"))?,
                        )
                        .map_err(native)?;
                    let end = edge.vertices[usize::from(forward)];
                    let next_start = next_edge.vertices[usize::from(!same(next_use.orientation)?)];
                    if end != next_start {
                        return Err(invalid(
                            "native face loop does not close on shared vertex identities",
                        ));
                    }
                    for vertex in edge.vertices {
                        brep.vertex(vertex).map_err(native)?;
                    }
                    let vertexuse_id = use_
                        .vertexuse
                        .ok_or_else(|| invalid("native coedge lacks its starting vertex use"))?;
                    let vertexuse = brep.vertexuse(vertexuse_id).map_err(native)?;
                    let start_vertex = edge.vertices[usize::from(!forward)];
                    let vertex = brep.vertex(start_vertex).map_err(native)?;
                    work.charge(vertex.vertexuses.len() as u64 + 1, cancel)?;
                    if vertexuse.vertex != Some(start_vertex)
                        || vertexuse.owner
                            != solidcore::topology::VertexuseOwner::Edgeuse(Some(current))
                        || !vertex.vertexuses.contains(&vertexuse_id)
                    {
                        return Err(invalid(
                            "native coedge endpoint use has different vertex or owner incidence",
                        ));
                    }
                    let occurrence = CoedgeUse {
                        face: face_index,
                        coedge: flat,
                        wire,
                        index,
                        orientation: forward == outward,
                        edgeuse: current,
                    };
                    flat = flat
                        .checked_add(1)
                        .ok_or_else(|| invalid("native coedge ordinal overflow"))?;
                    if let Some(uses) = result.edges.get_mut(&loop_edge.edge) {
                        if uses.second.is_some()
                            || uses.first.orientation == occurrence.orientation
                            || uses.vertices != edge.vertices
                            || uses.first.edgeuse == current
                        {
                            return Err(invalid(
                                "native material edge is not exactly two oppositely oriented uses",
                            ));
                        }
                        uses.second = Some(occurrence);
                    } else {
                        let prospective = result
                            .edges
                            .len()
                            .saturating_add(16)
                            .saturating_mul(size_of::<(EdgeId, EdgeUses)>().saturating_add(16))
                            .saturating_mul(4)
                            .saturating_add(size_of::<Self>());
                        if result.edges.len() >= 2 * MAX_STEP_ENTITIES as usize
                            || prospective > max_bytes
                        {
                            return Err(failure(
                                GeometryErrorCode::ResourceLimit,
                                "native source incidence capacity exhausted",
                            ));
                        }
                        result.edges.try_reserve(1).map_err(|_| {
                            failure(
                                GeometryErrorCode::ResourceLimit,
                                "native source incidence allocation refused",
                            )
                        })?;
                        result.edges.insert(
                            loop_edge.edge,
                            EdgeUses {
                                first: occurrence,
                                second: None,
                                vertices: edge.vertices,
                            },
                        );
                        if result.owned_bytes() > max_bytes {
                            return Err(failure(
                                GeometryErrorCode::ResourceLimit,
                                "native source incidence retained allowance exhausted",
                            ));
                        }
                    }
                    current = next;
                    if (current == start) != (index + 1 == loop_.edges.len()) {
                        return Err(invalid(
                            "native coedge cycle length differs from its ordered source loop",
                        ));
                    }
                }
            }
        }
        // The ordinal census may not silently omit an arena boundary operand.
        // Mates are uses of the same physical edge and are not extra edges.
        for face in brep.faces().ids() {
            work.charge(1, cancel)?;
            if definition.face_ordinal(face).is_none() {
                return Err(invalid("native source face census omits an arena face"));
            }
        }
        for edge in brep.edges().ids() {
            work.charge(1, cancel)?;
            if !result.edges.contains_key(&edge) {
                return Err(invalid(
                    "native material boundary contains an unowned wire or orphan edge",
                ));
            }
        }
        for (&id, uses) in &result.edges {
            work.charge(1, cancel)?;
            if uses.second.is_none() {
                return Err(invalid("native material boundary has an unpaired edge"));
            }
            let edge = brep.edge(id).map_err(native)?;
            work.charge((edge.edgeuses.len() as u64).saturating_mul(2), cancel)?;
            if !edge.edgeuses.contains(&uses.first.edgeuse)
                || !edge.edgeuses.contains(&uses.second.unwrap().edgeuse)
            {
                return Err(invalid(
                    "native physical coedge is absent from its edge radial list",
                ));
            }
        }
        Ok(result)
    }
    pub(crate) fn owned_bytes(&self) -> usize {
        size_of::<Self>()
            .saturating_add(map_bytes(&self.edges))
            .saturating_add(self.vertices.as_ref().map_or(0, |v| {
                map_bytes(&v.heads).saturating_add(
                    v.links
                        .capacity()
                        .saturating_mul(size_of::<VertexEdgeLink>()),
                )
            }))
    }
    pub(crate) fn values(&self) -> impl Iterator<Item = &EdgeUses> {
        self.edges.values()
    }
    pub(crate) fn get(&self, edge: EdgeId) -> Option<&EdgeUses> {
        self.edges.get(&edge)
    }
    pub(crate) fn prepare_vertices(
        &mut self,
        max_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        check_cancel(cancel)?;
        if self.vertices.is_some() {
            return Ok(());
        }
        let count = self
            .edges
            .len()
            .checked_mul(2)
            .ok_or_else(|| invalid("native vertex incidence count overflow"))?;
        let prospective = self
            .owned_bytes()
            .saturating_add(
                count
                    .saturating_add(16)
                    .saturating_mul(size_of::<(VertexId, usize)>().saturating_add(16))
                    .saturating_mul(4),
            )
            .saturating_add(count.saturating_mul(size_of::<VertexEdgeLink>()));
        if prospective > max_bytes {
            return Err(failure(
                GeometryErrorCode::ResourceLimit,
                "native source vertex incidence capacity exhausted",
            ));
        }
        work.charge(count as u64, cancel)?;
        let mut vertices = VertexLinks {
            heads: HashMap::new(),
            links: Vec::new(),
        };
        vertices.heads.try_reserve(count).map_err(|_| {
            failure(
                GeometryErrorCode::ResourceLimit,
                "native source vertex map allocation refused",
            )
        })?;
        vertices.links.try_reserve_exact(count).map_err(|_| {
            failure(
                GeometryErrorCode::ResourceLimit,
                "native source vertex links allocation refused",
            )
        })?;
        for (&edge, uses) in &self.edges {
            for (endpoint, &vertex) in uses.vertices.iter().enumerate() {
                work.charge(1, cancel)?;
                if endpoint == 1 && vertex == uses.vertices[0] {
                    continue;
                }
                let index = vertices.links.len();
                let next = vertices.heads.insert(vertex, index);
                vertices.links.push(VertexEdgeLink { edge, next });
            }
        }
        self.vertices = Some(vertices);
        if self.owned_bytes() > max_bytes {
            return Err(failure(
                GeometryErrorCode::ResourceLimit,
                "native source vertex incidence retained allowance exhausted",
            ));
        }
        Ok(())
    }
    pub(crate) fn vertices(&self) -> Option<impl Iterator<Item = VertexId> + '_> {
        Some(self.vertices.as_ref()?.heads.keys().copied())
    }
    pub(crate) fn vertex_edges(&self, vertex: VertexId) -> impl Iterator<Item = EdgeId> + '_ {
        let graph = self.vertices.as_ref();
        let mut next = graph.and_then(|graph| graph.heads.get(&vertex).copied());
        std::iter::from_fn(move || {
            let link = graph?.links.get(next?)?;
            next = link.next;
            Some(link.edge)
        })
    }
    /// Follow the actual corner/paired-edge permutation. Closed edges contribute
    /// TWO endpoint germs even when their vertices coincide. A single cycle must
    /// cover every germ; paired edges alone cannot exclude a pinched vertex.
    pub(crate) fn vertex_link_is_cycle(
        &self,
        definition: &Definition,
        vertex: VertexId,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        check_cancel(cancel)?;
        if !self.owns_definition(definition) {
            return Err(failure(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "native incidence belongs to a different borrowed BREP owner",
            ));
        }
        let mut count = 0usize;
        let mut start = None;
        for edge in self.vertex_edges(vertex) {
            work.charge(1, cancel)?;
            let uses = self
                .edges
                .get(&edge)
                .ok_or_else(|| invalid("native vertex link references a missing edge"))?;
            for endpoint in 0..2 {
                if uses.vertices[endpoint] == vertex {
                    count += 1;
                    start.get_or_insert((uses.first, endpoint));
                }
            }
        }
        let Some(start) = start else {
            return Ok(false);
        };
        let mut current = start;
        let brep = definition.brep();
        for step in 0..count {
            work.charge(1, cancel)?;
            let use_ = brep.edgeuse(current.0.edgeuse).map_err(native)?;
            let edge = brep
                .edge(
                    use_.edge
                        .ok_or_else(|| invalid("native vertex link coedge has no edge"))?,
                )
                .map_err(native)?;
            if edge.vertices[current.1] != vertex {
                return Ok(false);
            }
            let at_start = current.1 == usize::from(!same(use_.orientation)?);
            let neighbor_id = (if at_start { use_.cw } else { use_.ccw })
                .ok_or_else(|| invalid("native vertex link has no face corner"))?;
            let neighbor = brep.edgeuse(neighbor_id).map_err(native)?;
            if neighbor.owner != use_.owner {
                return Ok(false);
            }
            let neighbor_edge = neighbor
                .edge
                .ok_or_else(|| invalid("native corner coedge has no edge"))?;
            let endpoint = if at_start {
                usize::from(same(neighbor.orientation)?)
            } else {
                usize::from(!same(neighbor.orientation)?)
            };
            let Some(pair) = self.edges.get(&neighbor_edge) else {
                return Ok(false);
            };
            if pair.vertices[endpoint] != vertex {
                return Ok(false);
            }
            let next = if pair.first.edgeuse == neighbor_id {
                pair.second
            } else if pair
                .second
                .is_some_and(|other| other.edgeuse == neighbor_id)
            {
                Some(pair.first)
            } else {
                None
            };
            let Some(next) = next else {
                return Ok(false);
            };
            current = (next, endpoint);
            if current == start {
                return Ok(step + 1 == count);
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const LIMIT: usize = 4 * 1024 * 1024;

    #[test]
    fn native_closed_edge_keeps_both_vertex_link_germs() {
        let cancel = AtomicBool::new(false);
        let definition = crate::from_step_bytes_with_cancel(
            include_bytes!(
                "../../../../fixtures/geometry/independent-exporters/occt-cylinder-mm.step"
            ),
            &cancel,
        )
        .unwrap();
        let mut work = GeometryWorkBudget::new(20_000_000);
        let mut incidence =
            SourceEdgeIncidence::new(&definition, LIMIT, &mut work, &cancel).unwrap();
        assert!(
            incidence
                .values()
                .any(|edge| edge.vertices[0] == edge.vertices[1]),
            "the native periodic-cylinder control must retain a closed edge"
        );
        incidence
            .prepare_vertices(LIMIT, &mut work, &cancel)
            .unwrap();
        for vertex in incidence.vertices().unwrap() {
            assert!(
                incidence
                    .vertex_link_is_cycle(&definition, vertex, &mut work, &cancel)
                    .unwrap(),
                "every endpoint germ, including both ends of a closed edge, belongs to one link"
            );
        }
    }

    #[test]
    fn native_incidence_rejects_broken_reciprocal_loop_without_geometry_tolerance() {
        let cancel = AtomicBool::new(false);
        let mut definition = crate::from_step_bytes_with_cancel(
            include_bytes!("../../../../fixtures/geometry/box-mm.step"),
            &cancel,
        )
        .unwrap();
        let mut work = GeometryWorkBudget::new(20_000_000);
        let incidence = SourceEdgeIncidence::new(&definition, LIMIT, &mut work, &cancel).unwrap();
        let edgeuse = incidence.values().next().unwrap().first.edgeuse;
        definition.brep.edgeuse_mut(edgeuse).unwrap().ccw = None;
        let error = SourceEdgeIncidence::new(&definition, LIMIT, &mut work, &cancel)
            .err()
            .unwrap();
        assert_eq!(error.code, GeometryErrorCode::SingularGeometry);
    }

    #[test]
    fn native_incidence_admits_capacity_before_inventory_growth() {
        let cancel = AtomicBool::new(false);
        let definition = crate::from_step_bytes_with_cancel(
            include_bytes!("../../../../fixtures/geometry/box-mm.step"),
            &cancel,
        )
        .unwrap();
        let mut work = GeometryWorkBudget::new(20_000_000);
        let error = SourceEdgeIncidence::new(&definition, 0, &mut work, &cancel)
            .err()
            .unwrap();
        assert_eq!(error.code, GeometryErrorCode::ResourceLimit);
        assert_eq!(work.consumed(), 1);
    }
}
