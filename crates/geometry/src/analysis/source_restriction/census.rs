// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Identity/incidence closure of already qualified source coedge operators.
//! This census proves neither coefficient correspondence nor material embedding.
use crate::{Definition, GeometryError, GeometryWorkBudget};
use crate::analysis::source_incidence::{self, CoedgeUse, SourceEdgeIncidence};
use crate::kernel::trim::OriginalCoedgeBinding;
use solidcore::VertexId;
use spiling_contracts::geometry::{GeometryErrorCode, StepEntityId};
use std::{mem::size_of, sync::atomic::AtomicBool};

#[derive(Clone, Copy)]
pub(crate) struct SourceVertexRecord {
    pub native: VertexId,
    pub source: StepEntityId,
    pub point: [f64; 3],
}

pub(crate) struct SourceBindingCensus<'d> {
    definition: &'d Definition,
    expected: usize,
    allowance: usize,
    bindings: Vec<OriginalCoedgeBinding>,
}

fn unavailable(message: &'static str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::ParameterCorrespondenceUnavailable, message)
}
fn resource() -> GeometryError {
    GeometryError::new(GeometryErrorCode::ResourceLimit, "source binding census exceeds its live byte allowance")
}
fn bytes<T>(count: usize) -> Result<usize, GeometryError> {
    count.checked_mul(size_of::<T>()).ok_or_else(resource)
}
fn sort_work(count: usize, work: &mut GeometryWorkBudget, cancel: &AtomicBool) -> Result<(), GeometryError> {
    let levels = usize::BITS - count.leading_zeros();
    let charge = (count as u64).checked_mul(u64::from(levels) + 1).ok_or_else(resource)?;
    work.charge(charge, cancel)
}
fn owns(binding: &OriginalCoedgeBinding, use_: &CoedgeUse) -> bool {
    binding.edgeuse == use_.edgeuse
        && binding.owner == [use_.face, use_.wire, use_.index]
        && binding.material_forward == (binding.spatial_forward == use_.orientation)
}

impl<'d> SourceBindingCensus<'d> {
    pub(crate) fn new(
        definition: &'d Definition, expected_native_uses: usize, allowance: usize,
        work: &mut GeometryWorkBudget, cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        work.charge(1, cancel)?;
        if expected_native_uses == 0 || expected_native_uses != definition.original_source().coedges.len() {
            return Err(unavailable("native and complete original source coedge-use inventories differ"));
        }
        let required = size_of::<Self>().checked_add(bytes::<OriginalCoedgeBinding>(expected_native_uses)?)
            .ok_or_else(resource)?;
        if required > allowance { return Err(resource()); }
        let mut bindings = Vec::new();
        bindings.try_reserve_exact(expected_native_uses).map_err(|_| resource())?;
        let result = Self { definition, expected: expected_native_uses, allowance, bindings };
        if result.retained_bytes() > allowance { return Err(resource()); }
        Ok(result)
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        size_of::<Self>() + self.bindings.capacity() * size_of::<OriginalCoedgeBinding>()
    }

    pub(crate) fn insert(
        &mut self, binding: &OriginalCoedgeBinding,
        work: &mut GeometryWorkBudget, cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        work.charge(1, cancel)?;
        if self.bindings.len() == self.expected { return Err(unavailable("original coedge census contains extra uses")); }
        let source = self.definition.original_source().coedge(binding.source.source_index)?;
        let face = self.definition.original_source().face(binding.face)?;
        if self.definition.face_ordinal(binding.face) != Some(binding.owner[0])
            || source.source_face != face.source_face
            || binding.source_face != Some(face.face_entity.0)
            || binding.source_face_use != Some(face.source_face.0)
            || binding.source.source_edge != Some(source.edge_curve_id.0)
            || binding.source.source_coedge != Some(source.oriented_edge_id.0) {
            return Err(unavailable("source coedge binding has foreign face/use or literal edge genealogy"));
        }
        let (entry, edge) = source_incidence::native_coedge(
            self.definition, binding.owner[0], binding.owner[1], binding.owner[2],
        )?;
        if entry.edge != binding.edge || entry.forward != binding.source.native_orientation
            || edge.vertices != binding.endpoints
            || binding.native_interval != [edge.interval.min(), edge.interval.max()]
            || !binding.native_interval.iter().all(|x| x.is_finite())
            || binding.native_interval[0] >= binding.native_interval[1] {
            return Err(unavailable("source coedge binding does not own the actual native edge restriction/endpoints"));
        }
        for source_endpoint in 0..2 {
            work.charge(1, cancel)?;
            let native_endpoint = binding.native_endpoint_index(source_endpoint != 0);
            if binding.source_vertex_ids[native_endpoint] != source.vertex_ids[source_endpoint]
                || binding.source_vertex_points_mm[native_endpoint] != source.vertex_points_mm[source_endpoint]
                || !source.vertex_points_mm[source_endpoint].iter().all(|x| x.is_finite()) {
                return Err(unavailable("source coedge endpoint identity or captured original coefficients changed"));
            }
        }
        self.bindings.push(*binding);
        Ok(())
    }

    pub(crate) fn finish(
        mut self, incidence: &SourceEdgeIncidence<'_>,
        work: &mut GeometryWorkBudget, cancel: &AtomicBool,
    ) -> Result<Vec<SourceVertexRecord>, GeometryError> {
        work.charge(1, cancel)?;
        if !incidence.owns_definition(self.definition) || self.bindings.len() != self.expected {
            return Err(unavailable("original source census is incomplete or belongs to another definition"));
        }
        sort_work(self.bindings.len(), work, cancel)?;
        self.bindings.sort_unstable_by_key(|binding| binding.source.source_index);
        for (index, binding) in self.bindings.iter().enumerate() {
            work.charge(1, cancel)?;
            if binding.source.source_index != index {
                return Err(unavailable("original source coedge-use assignment is not a complete bijection"));
            }
        }
        sort_work(self.bindings.len(), work, cancel)?;
        self.bindings.sort_unstable_by_key(|binding| (binding.source.source_edge, binding.edge));
        for pair in self.bindings.windows(2) {
            work.charge(1, cancel)?;
            if pair[0].source.source_edge == pair[1].source.source_edge && pair[0].edge != pair[1].edge {
                return Err(unavailable("one original EDGE_CURVE was assigned to multiple native physical edges"));
            }
        }
        sort_work(self.bindings.len(), work, cancel)?;
        self.bindings.sort_unstable_by_key(|binding| (binding.edge, binding.edgeuse));
        let mut groups = 0usize;
        let mut index = 0usize;
        while index < self.bindings.len() {
            work.charge(1, cancel)?;
            let first = &self.bindings[index];
            let second = self.bindings.get(index + 1).ok_or_else(|| unavailable("source native edge has only one coedge use"))?;
            if second.edge != first.edge || self.bindings.get(index + 2).is_some_and(|next| next.edge == first.edge) {
                return Err(unavailable("source native edge does not have exactly two original uses"));
            }
            let actual = incidence.get(first.edge).ok_or_else(|| unavailable("source edge is absent from actual native incidence"))?;
            let opposite = actual.second.as_ref().ok_or_else(|| unavailable("actual native edge is not opposed/closed"))?;
            if !((owns(first, &actual.first) && owns(second, opposite))
                || (owns(second, &actual.first) && owns(first, opposite)))
                || first.edgeuse == second.edgeuse
                || first.source.source_edge != second.source.source_edge
                || first.spatial_forward != second.spatial_forward
                || first.native_interval != second.native_interval
                || first.endpoints != actual.vertices || second.endpoints != actual.vertices
                || first.source_vertex_ids != second.source_vertex_ids
                || first.source_vertex_points_mm != second.source_vertex_points_mm
                || first.material_forward == second.material_forward {
                return Err(unavailable("opposed original coedges disagree in native owner, source edge, gauge or endpoint star"));
            }
            groups += 1;
            index += 2;
        }
        let mut actual_edges = 0usize;
        for _ in incidence.values() { work.charge(1, cancel)?; actual_edges += 1; }
        if groups != actual_edges { return Err(unavailable("original source assignment omits a native physical edge")); }

        // Each endpoint occurrence participates before in-place deduplication:
        // equality is exact source identity/coefficient equality, never welding.
        let count = self.bindings.len().checked_mul(2).ok_or_else(resource)?;
        if self.retained_bytes().checked_add(bytes::<SourceVertexRecord>(count)?)
            .is_none_or(|required| required > self.allowance) { return Err(resource()); }
        let mut vertices = Vec::new();
        vertices.try_reserve_exact(count).map_err(|_| resource())?;
        if self.retained_bytes().checked_add(bytes::<SourceVertexRecord>(vertices.capacity())?)
            .is_none_or(|required| required > self.allowance) { return Err(resource()); }
        for binding in &self.bindings {
            work.charge(2, cancel)?;
            for endpoint in 0..2 {
                vertices.push(SourceVertexRecord { native: binding.endpoints[endpoint],
                    source: binding.source_vertex_ids[endpoint], point: binding.source_vertex_points_mm[endpoint] });
            }
        }
        sort_work(vertices.len(), work, cancel)?;
        vertices.sort_unstable_by_key(|vertex| (vertex.source.0, vertex.native));
        for pair in vertices.windows(2) {
            work.charge(1, cancel)?;
            if pair[0].source == pair[1].source && (pair[0].native != pair[1].native || pair[0].point != pair[1].point) {
                return Err(unavailable("one authored source vertex has inconsistent native-star ownership or coefficients"));
            }
        }
        sort_work(vertices.len(), work, cancel)?;
        vertices.sort_unstable_by_key(|vertex| vertex.native);
        for pair in vertices.windows(2) {
            work.charge(1, cancel)?;
            if pair[0].native == pair[1].native && (pair[0].source != pair[1].source || pair[0].point != pair[1].point) {
                return Err(unavailable("whole native vertex star has inconsistent authored source identities or coefficients"));
            }
        }
        vertices.dedup_by_key(|vertex| vertex.native);
        let mut native_vertices = 0usize;
        for id in self.definition.brep().vertices().ids() {
            work.charge(usize::BITS as u64 + 1, cancel)?;
            if vertices.binary_search_by_key(&id, |vertex| vertex.native).is_err() {
                return Err(unavailable("native arena vertex is absent from the complete authored source star census"));
            }
            native_vertices += 1;
        }
        if native_vertices != vertices.len() { return Err(unavailable("authored vertex census contains foreign native vertices")); }
        Ok(vertices)
    }
}
