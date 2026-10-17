// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Actual original native occurrence incidence, shared by source-link proofs.
use crate::{Definition, GeometryError, GeometryWorkBudget, check_cancel};
use monstertruck_modeling::{Curve, Point3};
use monstertruck_topology::{EdgeId, VertexId};
use spiling_contracts::geometry::{GeometryErrorCode, MAX_STEP_ENTITIES};
use std::{collections::HashMap, mem::size_of, sync::atomic::AtomicBool};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct CoedgeUse {
    pub face: usize,
    pub coedge: usize,
    pub wire: usize,
    pub index: usize,
    pub orientation: bool,
}
#[derive(Clone, Copy)]
pub(super) struct EdgeUses {
    pub first: CoedgeUse,
    pub second: Option<CoedgeUse>,
    pub vertices: [VertexId<Point3>; 2],
}
struct VertexEdgeLink {
    edge: EdgeId<Curve>,
    next: Option<usize>,
}
struct VertexLinks {
    heads: HashMap<VertexId<Point3>, usize>,
    links: Vec<VertexEdgeLink>,
}
pub(super) struct SourceEdgeIncidence {
    edges: HashMap<EdgeId<Curve>, EdgeUses>,
    vertices: Option<VertexLinks>,
}
fn failure(code: GeometryErrorCode, message: &'static str) -> GeometryError {
    GeometryError::new(code, message)
}
fn map_bytes<K, V>(map: &HashMap<K, V>) -> usize {
    // Account actual usable capacity, bucket/control overhead and alignment.
    map.capacity()
        .saturating_mul(size_of::<(K, V)>().saturating_add(16))
        .saturating_mul(2)
}
impl SourceEdgeIncidence {
    pub fn new(
        definition: &Definition,
        max_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        work.charge(1, cancel)?;
        let mut result = Self {
            edges: HashMap::new(),
            vertices: None,
        };
        let mut faces = 0;
        for (face_index, face) in definition.solid.face_iter().enumerate() {
            work.charge(1, cancel)?;
            let retained = definition.face_boundaries.get(face_index).ok_or_else(|| {
                failure(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "original native face incidence is missing",
                )
            })?;
            if retained.face != face.id()
                || retained.wires.len() != face.absolute_boundaries().len()
            {
                return Err(failure(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "original native face incidence changed",
                ));
            }
            let mut flat = 0;
            for (wire_index, (wire, references)) in face
                .absolute_boundaries()
                .iter()
                .zip(&retained.wires)
                .enumerate()
            {
                work.charge(1, cancel)?;
                if wire.len() != references.len() || wire.is_empty() {
                    return Err(failure(
                        GeometryErrorCode::ParameterCorrespondenceUnavailable,
                        "original native wire incidence changed",
                    ));
                }
                for (index, (edge, reference)) in wire.iter().zip(references).enumerate() {
                    work.charge(1, cancel)?;
                    if reference.edge != edge.id()
                        || reference.orientation != edge.orientation()
                        || edge.back().id() != wire[(index + 1) % wire.len()].front().id()
                    {
                        return Err(failure(
                            GeometryErrorCode::ParameterCorrespondenceUnavailable,
                            "original native coedge association or closed wire changed",
                        ));
                    }
                    let use_ = CoedgeUse {
                        face: face_index,
                        coedge: flat,
                        wire: wire_index,
                        index,
                        orientation: edge.orientation() == face.orientation(),
                    };
                    flat += 1;
                    if let Some(uses) = result.edges.get_mut(&edge.id()) {
                        if uses.second.is_some()
                            || uses.first.orientation == use_.orientation
                            || uses.vertices
                                != [edge.absolute_front().id(), edge.absolute_back().id()]
                        {
                            return Err(failure(
                                GeometryErrorCode::SingularGeometry,
                                "oriented source edge is not a two-sided manifold incidence",
                            ));
                        }
                        uses.second = Some(use_);
                    } else {
                        let prospective = result
                            .edges
                            .len()
                            .saturating_add(16)
                            .saturating_mul(
                                size_of::<(EdgeId<Curve>, EdgeUses)>().saturating_add(16),
                            )
                            .saturating_mul(4);
                        if result.edges.len() >= 2 * MAX_STEP_ENTITIES as usize
                            || prospective > max_bytes
                        {
                            return Err(failure(
                                GeometryErrorCode::ResourceLimit,
                                "source incidence complex capacity exhausted",
                            ));
                        }
                        result.edges.try_reserve(1).map_err(|_| {
                            failure(
                                GeometryErrorCode::ResourceLimit,
                                "source incidence allocation refused",
                            )
                        })?;
                        result.edges.insert(
                            edge.id(),
                            EdgeUses {
                                first: use_,
                                second: None,
                                vertices: [edge.absolute_front().id(), edge.absolute_back().id()],
                            },
                        );
                        if result.owned_bytes() > max_bytes {
                            return Err(failure(
                                GeometryErrorCode::ResourceLimit,
                                "source incidence retained capacity exhausted",
                            ));
                        }
                    }
                }
            }
            faces += 1;
        }
        if faces != definition.face_boundaries.len() {
            return Err(failure(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "original native face count changed",
            ));
        }
        for uses in result.edges.values() {
            work.charge(1, cancel)?;
            if uses.second.is_none() {
                return Err(failure(
                    GeometryErrorCode::SingularGeometry,
                    "source boundary complex has an unpaired edge",
                ));
            }
        }
        Ok(result)
    }
    pub fn owned_bytes(&self) -> usize {
        size_of::<Self>()
            .saturating_add(map_bytes(&self.edges))
            .saturating_add(self.vertices.as_ref().map_or(0, |vertices| {
                map_bytes(&vertices.heads).saturating_add(
                    vertices
                        .links
                        .capacity()
                        .saturating_mul(size_of::<VertexEdgeLink>()),
                )
            }))
    }
    pub fn values(&self) -> impl Iterator<Item = &EdgeUses> {
        self.edges.values()
    }
    pub fn get(&self, edge: EdgeId<Curve>) -> Option<&EdgeUses> {
        self.edges.get(&edge)
    }
    pub fn prepare_vertices(
        &mut self,
        max_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        check_cancel(cancel)?;
        if self.vertices.is_some() {
            return Ok(());
        }
        work.charge(1, cancel)?;
        let count = self.edges.len().checked_mul(2).ok_or_else(|| {
            failure(
                GeometryErrorCode::ResourceLimit,
                "source vertex incidence count overflow",
            )
        })?;
        let prospective = self
            .owned_bytes()
            .saturating_add(
                count
                    .saturating_add(16)
                    .saturating_mul(size_of::<(VertexId<Point3>, usize)>().saturating_add(16))
                    .saturating_mul(4),
            )
            .saturating_add(count.saturating_mul(size_of::<VertexEdgeLink>()));
        if prospective > max_bytes {
            return Err(failure(
                GeometryErrorCode::ResourceLimit,
                "source vertex incidence capacity exhausted",
            ));
        }
        let mut vertices = VertexLinks {
            heads: HashMap::new(),
            links: Vec::new(),
        };
        vertices.heads.try_reserve(count).map_err(|_| {
            failure(
                GeometryErrorCode::ResourceLimit,
                "source vertex map allocation refused",
            )
        })?;
        vertices.links.try_reserve_exact(count).map_err(|_| {
            failure(
                GeometryErrorCode::ResourceLimit,
                "source vertex links allocation refused",
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
                "source vertex incidence retained capacity exhausted",
            ));
        }
        Ok(())
    }
    pub fn vertices(&self) -> Option<impl Iterator<Item = VertexId<Point3>> + '_> {
        Some(self.vertices.as_ref()?.heads.keys().copied())
    }
    pub fn vertex_edges(
        &self,
        vertex: VertexId<Point3>,
    ) -> impl Iterator<Item = EdgeId<Curve>> + '_ {
        let graph = self.vertices.as_ref();
        let mut next = graph.and_then(|graph| graph.heads.get(&vertex).copied());
        std::iter::from_fn(move || {
            let link = graph?.links.get(next?)?;
            next = link.next;
            Some(link.edge)
        })
    }
    /// Traverse the actual face-corner/edge-use permutations. A single closed
    /// cycle must cover EVERY incident original edge; paired edges alone do not
    /// exclude a disconnected or pinched source vertex link.
    pub fn vertex_link_is_cycle(
        &self,
        definition: &Definition,
        vertex: VertexId<Point3>,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        check_cancel(cancel)?;
        let mut count = 0;
        let mut first = None;
        for edge in self.vertex_edges(vertex) {
            work.charge(1, cancel)?;
            count += 1;
            first.get_or_insert(edge);
        }
        let Some(edge) = first else {
            return Ok(false);
        };
        let Some(uses) = self.get(edge) else {
            return Ok(false);
        };
        let start = uses.first;
        let mut current = start;
        for step in 0..count {
            work.charge(1, cancel)?;
            let Some(shell) = definition.solid.boundaries().first() else {
                return Ok(false);
            };
            if definition.solid.boundaries().len() != 1 {
                return Ok(false);
            }
            let Some(face) = shell.get(current.face) else {
                return Ok(false);
            };
            let Some(wire) = face.absolute_boundaries().get(current.wire) else {
                return Ok(false);
            };
            let Some(edge) = wire.get(current.index) else {
                return Ok(false);
            };
            if edge.absolute_front().id() == edge.absolute_back().id() {
                return Ok(false);
            }
            let adjacent = if edge.back().id() == vertex {
                (current.index + 1) % wire.len()
            } else if edge.front().id() == vertex {
                (current.index + wire.len() - 1) % wire.len()
            } else {
                return Ok(false);
            };
            let neighbor = &wire[adjacent];
            let selected = CoedgeUse {
                face: current.face,
                wire: current.wire,
                index: adjacent,
                coedge: current.coedge - current.index + adjacent,
                orientation: neighbor.orientation() == face.orientation(),
            };
            let Some(pair) = self.get(neighbor.id()) else {
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
            if current == start {
                return Ok(step + 1 == count);
            }
        }
        Ok(false)
    }
}
