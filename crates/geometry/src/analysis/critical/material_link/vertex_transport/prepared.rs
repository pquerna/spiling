// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Immutable source selections and successful source-star qualifications.
//! The fixed identity index borrows no closed-slab or posed query authority.

use super::*;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    mem::size_of,
};

pub(crate) struct PreparedSourceVertexPlans<'d> {
    definition: &'d Definition,
    plans: Vec<SourceVertexTransportPlan<'d>>,
    slots: Vec<Option<usize>>,
    retained_bytes: usize,
}

fn resource(message: &'static str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::ResourceLimit, message)
}

fn slot(vertex: VertexId<Point3>, count: usize) -> usize {
    let mut hash = DefaultHasher::new();
    vertex.hash(&mut hash);
    (hash.finish() % count as u64) as usize
}

impl<'d> PreparedSourceVertexPlans<'d> {
    pub(crate) fn prepare(
        source: OriginalTrimSource<'_, 'd>,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        work.charge(1, cancel)?;
        let mut result = Self {
            definition: source.definition(),
            plans: Vec::new(),
            slots: Vec::new(),
            retained_bytes: size_of::<Self>(),
        };
        let available = remaining_bytes
            .checked_sub(result.retained_bytes)
            .ok_or_else(|| {
                resource("prepared source vertex owner exceeds retained-byte allowance")
            })?;
        if source
            .definition()
            .material_transport_is_proved_zero(work, cancel)?
        {
            return Ok(result);
        }
        let mut incidence = SourceEdgeIncidence::new(source.definition(), available, work, cancel)?;
        incidence.prepare_vertices(available, work, cancel)?;
        let available = available
            .checked_sub(incidence.owned_bytes())
            .ok_or_else(|| {
                resource("prepared source vertex incidence exceeds coexistent allowance")
            })?;
        let vertices = incidence
            .vertices()
            .ok_or_else(|| refusal("prepared source vertex owner lacks original incidence"))?;
        let mut count = 0usize;
        for _ in vertices {
            work.charge(1, cancel)?;
            count = count
                .checked_add(1)
                .ok_or_else(|| resource("prepared source vertex count overflow"))?;
        }
        let slot_count = count
            .checked_mul(2)
            .ok_or_else(|| resource("prepared source vertex index count overflow"))?;
        let requested = count
            .checked_mul(size_of::<SourceVertexTransportPlan<'d>>())
            .and_then(|bytes| {
                slot_count
                    .checked_mul(size_of::<Option<usize>>())
                    .and_then(|slots| bytes.checked_add(slots))
            })
            .filter(|bytes| *bytes <= available)
            .ok_or_else(|| {
                resource("prepared source vertex inventory exceeds retained-byte allowance")
            })?;
        result
            .plans
            .try_reserve_exact(count)
            .map_err(|_| resource("prepared source vertex inventory allocation refused"))?;
        result
            .slots
            .try_reserve_exact(slot_count)
            .map_err(|_| resource("prepared source vertex identity index allocation refused"))?;
        let inventory_bytes = result
            .plans
            .capacity()
            .checked_mul(size_of::<SourceVertexTransportPlan<'d>>())
            .and_then(|bytes| {
                result
                    .slots
                    .capacity()
                    .checked_mul(size_of::<Option<usize>>())
                    .and_then(|slots| bytes.checked_add(slots))
            })
            .filter(|bytes| *bytes >= requested && *bytes <= available)
            .ok_or_else(|| {
                resource("prepared source vertex allocator exceeds retained-byte allowance")
            })?;
        result.retained_bytes = result
            .retained_bytes
            .checked_add(inventory_bytes)
            .ok_or_else(|| resource("prepared source vertex retained-byte count overflow"))?;
        work.charge(
            u64::try_from(slot_count).map_err(|_| {
                resource("prepared source vertex index initialization work overflow")
            })?,
            cancel,
        )?;
        result.slots.resize(slot_count, None);
        let mut available = available - inventory_bytes;
        for (ordinal, vertex) in incidence
            .vertices()
            .ok_or_else(|| refusal("prepared source vertex owner lost original incidence"))?
            .enumerate()
        {
            let before = work.consumed();
            let plan = SourceVertexTransportPlan::prepare(
                source, &incidence, vertex, available, work, cancel,
            )
            .map_err(|error| {
                work.annotate_error(
                    error,
                    before,
                    format_args!(
                        "prepared source-only corner selection; source_vertex_ordinal={ordinal}"
                    ),
                )
            })?
            .ok_or_else(|| {
                refusal("prepared source vertex has no evaluatable immutable conical recipe")
            })?;
            // Inline plan headers already occupy the reserved inventory. Only
            // their owned buffers remain to be charged after moving the plan.
            let heap = plan
                .retained_bytes()
                .checked_sub(size_of::<SourceVertexTransportPlan<'d>>())
                .ok_or_else(|| resource("prepared source vertex plan omitted inline storage"))?;
            available = available.checked_sub(heap).ok_or_else(|| {
                resource("prepared source vertex plans exceed retained-byte allowance")
            })?;
            result.retained_bytes = result
                .retained_bytes
                .checked_add(heap)
                .ok_or_else(|| resource("prepared source vertex retained-byte count overflow"))?;
            let mut index = slot(vertex, result.slots.len());
            let mut vacant = None;
            for _ in 0..result.slots.len() {
                work.charge(1, cancel)?;
                if result.slots[index].is_none() {
                    vacant = Some(index);
                    break;
                }
                index = (index + 1) % result.slots.len();
            }
            let index = vacant.ok_or_else(|| {
                refusal("prepared source vertex identity index is unexpectedly full")
            })?;
            result.slots[index] = Some(result.plans.len());
            result.plans.push(plan);
        }
        Ok(result)
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    pub(in crate::analysis::critical::material_link) fn validate_source(
        &self,
        definition: &Definition,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        work.charge(1, cancel)?;
        if !std::ptr::eq(self.definition, definition) {
            return Err(refusal(
                "prepared source vertex plans belong to another immutable definition",
            ));
        }
        Ok(())
    }

    pub(in crate::analysis::critical::material_link) fn plan(
        &self,
        definition: &Definition,
        vertex: VertexId<Point3>,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<&SourceVertexTransportPlan<'d>, GeometryError> {
        self.validate_source(definition, work, cancel)?;
        if !self.slots.is_empty() {
            let mut index = slot(vertex, self.slots.len());
            for _ in 0..self.slots.len() {
                work.charge(1, cancel)?;
                let Some(plan_index) = self.slots[index] else {
                    break;
                };
                let plan = &self.plans[plan_index];
                if plan.vertex == vertex && std::ptr::eq(plan.definition, definition) {
                    return Ok(plan);
                }
                index = (index + 1) % self.slots.len();
            }
        }
        Err(refusal(
            "prepared source vertex inventory has no matching original vertex",
        ))
    }
}

/// A retained plan has already been charged at its source owner; a standalone
/// selection must instead reserve its own complete storage in query scratch.
pub(in crate::analysis::critical::material_link) enum SourceVertexPlan<'s, 'd> {
    Retained(&'s SourceVertexTransportPlan<'d>),
    Local(SourceVertexTransportPlan<'d>),
}

impl<'s, 'd> SourceVertexPlan<'s, 'd> {
    pub(in crate::analysis::critical::material_link) fn prepare(
        source: OriginalTrimSource<'s, 'd>,
        incidence: &SourceEdgeIncidence,
        vertex: VertexId<Point3>,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        if let Some(plans) = source.vertex_plans() {
            return Ok(Some(Self::Retained(plans.plan(
                source.definition(),
                vertex,
                work,
                cancel,
            )?)));
        }
        SourceVertexTransportPlan::prepare(source, incidence, vertex, remaining_bytes, work, cancel)
            .map(|plan| plan.map(Self::Local))
    }

    pub(in crate::analysis::critical::material_link) fn plan(
        &self,
    ) -> &SourceVertexTransportPlan<'d> {
        match self {
            Self::Retained(plan) => plan,
            Self::Local(plan) => plan,
        }
    }

    pub(in crate::analysis::critical::material_link) fn owned_bytes(&self) -> usize {
        match self {
            Self::Retained(_) => 0,
            Self::Local(plan) => plan.retained_bytes(),
        }
    }
}
