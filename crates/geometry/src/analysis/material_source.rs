// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{GeometryWorkBudget, PlacedSlabRequest, SlabAnalysis, SlabRequest};
use crate::section::{OriginalTrimSource, SectionQuery};
use crate::{Definition, GeometryError, NativeSection, PreparedSectionSource};
use spiling_contracts::geometry::GeometryErrorCode;
use std::sync::atomic::AtomicBool;

use super::PreparedSourceVertexPlans;
use super::{AnalysisPolicy, BoundsPolicy, HeightAnalysis, PlacedMaterialProjectedBounds};
use spiling_contracts::geometry::RigidPoseMm;

/// Immutable original-domain and material-plan owner, without native Boolean seeds.
/// Query allowances exclude this owner's retained capacity, including pinned proof.
pub struct PreparedGeometrySource<'d> {
    definition: &'d Definition,
    originals: crate::parametric::PreparedSectionOriginals<'d>,
    plans: PreparedSourceVertexPlans<'d>,
}

fn geometry_header_bytes() -> usize {
    std::mem::size_of::<PreparedGeometrySource<'_>>()
        - std::mem::size_of::<crate::parametric::PreparedSectionOriginals<'_>>()
        - std::mem::size_of::<PreparedSourceVertexPlans<'_>>()
}

/// Prepare original trim domains and canonical plans once, without native seeds.
pub fn prepare_geometry_source<'d>(
    definition: &'d Definition,
    remaining_prepared_bytes: usize,
    cancel: &AtomicBool,
    work: &mut GeometryWorkBudget,
) -> Result<PreparedGeometrySource<'d>, GeometryError> {
    work.charge(1, cancel)?;
    let limit = remaining_prepared_bytes.min(crate::MAX_PREPARED_SECTION_SOURCE_BYTES);
    let original_limit = limit.checked_sub(geometry_header_bytes())
        .and_then(|bytes| bytes.checked_sub(definition.retained_query_proof_bytes()))
        .ok_or_else(capacity_error)?;
    let originals = crate::parametric::PreparedSectionOriginals::prepare_trim_domains(
        definition, original_limit, work, cancel,
    )?;
    let plan_limit = original_limit.checked_sub(originals.retained_bytes()).ok_or_else(capacity_error)?;
    let plans = PreparedSourceVertexPlans::prepare(
        OriginalTrimSource::standalone(definition).borrowing_domains(Some(&originals)),
        plan_limit, work, cancel,
    )?;
    let source = PreparedGeometrySource { definition, originals, plans };
    if source.retained_bytes() > limit { return Err(capacity_error()); }
    Ok(source)
}

impl<'d> PreparedGeometrySource<'d> {
    /// Original domains, plan capacities, wrapper and pinned proof, each once.
    /// Borrowed BREP and allocator bookkeeping are excluded.
    pub fn retained_bytes(&self) -> usize {
        geometry_header_bytes()
            .saturating_add(self.definition.retained_query_proof_bytes())
            .saturating_add(self.originals.retained_bytes())
            .saturating_add(self.plans.retained_bytes())
    }

    fn trim_source(&self) -> OriginalTrimSource<'_, 'd> {
        OriginalTrimSource::standalone(self.definition)
            .borrowing_domains(Some(&self.originals))
            .borrowing_vertex_plans(&self.plans)
    }

    pub fn placed_material_bounds(
        &self, pose: RigidPoseMm, world_direction: [f64; 3], policy: BoundsPolicy,
        query_byte_limit: usize, work: &mut GeometryWorkBudget, cancel: &AtomicBool,
    ) -> Result<PlacedMaterialProjectedBounds, GeometryError> {
        super::placed::placed_material_bounds_with_scratch_limit(
            self.trim_source(), pose, world_direction, policy, query_byte_limit, work, cancel,
        )
    }

    pub fn analyze_placed_height_domain(
        &self, pose: RigidPoseMm, world_height_interval_mm: [f64; 2], policy: AnalysisPolicy,
        query_byte_limit: usize, work: &mut GeometryWorkBudget, cancel: &AtomicBool,
    ) -> Result<HeightAnalysis, GeometryError> {
        super::placed::analyze_placed_height_domain_with_scratch_limit(
            self.trim_source(), pose, world_height_interval_mm, policy, query_byte_limit, work, cancel,
        )
    }

    /// Consume the original owner; add boundary inventory and native seeds only now.
    /// The allowance is the total replacement-source allowance, not extra capacity.
    pub fn prepare_sections(
        self, remaining_prepared_bytes: usize, cancel: &AtomicBool, work: &mut GeometryWorkBudget,
    ) -> Result<PreparedMaterialSource<'d>, GeometryError> {
        work.charge(1, cancel)?;
        let limit = remaining_prepared_bytes.min(crate::MAX_PREPARED_SECTION_SOURCE_BYTES);
        let section_limit = limit.checked_sub(header_bytes())
            .and_then(|bytes| bytes.checked_sub(self.plans.retained_bytes()))
            .ok_or_else(capacity_error)?;
        // Conservatively reserve the complete section wrapper while extending originals.
        let original_limit = section_limit.checked_sub(self.definition.retained_query_proof_bytes())
            .and_then(|bytes| bytes.checked_sub(std::mem::size_of::<PreparedSectionSource<'_>>()))
            .ok_or_else(capacity_error)?;
        let originals = self.originals.prepare_boundaries(original_limit, work, cancel)?;
        let section = crate::section::prepare_section_source_from_originals(
            self.definition, Some(originals), section_limit, cancel, work,
        )?;
        let source = PreparedMaterialSource { section, plans: self.plans };
        if source.retained_bytes() > limit { return Err(capacity_error()); }
        Ok(source)
    }
}

/// Job-local native section preparation and canonical source-material plans.
///
/// Geometry selection is immutable and shared by occurrences and adaptive
/// retries. Slab, metric, incidence and posed proof authority is never retained
/// here. Raw section inspection does not require this material admission.
pub struct PreparedMaterialSource<'d> {
    section: PreparedSectionSource<'d>,
    plans: PreparedSourceVertexPlans<'d>,
}

fn header_bytes() -> usize {
    std::mem::size_of::<PreparedMaterialSource<'_>>()
        - std::mem::size_of::<PreparedSectionSource<'_>>()
        - std::mem::size_of::<PreparedSourceVertexPlans<'_>>()
}

/// Prepare one immutable definition's material queries within the existing
/// shared source allowance. Publish only a complete successful preparation.
pub fn prepare_material_source<'d>(
    definition: &'d Definition,
    remaining_prepared_bytes: usize,
    cancel: &AtomicBool,
    work: &mut GeometryWorkBudget,
) -> Result<PreparedMaterialSource<'d>, GeometryError> {
    prepare_geometry_source(definition, remaining_prepared_bytes, cancel, work)?
        .prepare_sections(remaining_prepared_bytes, cancel, work)
}

fn capacity_error() -> GeometryError {
    GeometryError::new(
        GeometryErrorCode::ResourceLimit,
        "prepared material source exceeds retained source capacity",
    )
}

impl<'d> PreparedMaterialSource<'d> {
    /// All retained seeds, original proofs and selected source plans, once.
    /// Borrowed BREP and transient process RSS are not owned allocations here.
    pub fn retained_bytes(&self) -> usize {
        self.section
            .retained_bytes()
            .saturating_add(self.plans.retained_bytes())
            .saturating_add(header_bytes())
    }

    /// Borrow raw section services without changing their admission contract.
    pub fn section_source(&self) -> &PreparedSectionSource<'d> {
        &self.section
    }

    pub(crate) fn definition(&self) -> &'d Definition {
        self.section.trim_source().definition()
    }

    pub(crate) fn trim_source(&self) -> OriginalTrimSource<'_, 'd> {
        self.section
            .trim_source()
            .borrowing_vertex_plans(&self.plans)
    }

    pub(crate) fn section_for_query(
        &self,
        query: &SectionQuery,
        cancel: &AtomicBool,
        byte_limit: u32,
        work: &mut GeometryWorkBudget,
    ) -> Result<NativeSection, GeometryError> {
        self.section
            .section_for_query(query, cancel, byte_limit, work)
    }

    /// Analyze the complete native slab, including its occupied projection.
    /// A sampled section alone never authorizes material reuse or support.
    /// `query_byte_limit` excludes all retained prepared sources, prior results
    /// and other coexisting buffers from the existing shared 64 MiB pool.
    pub fn analyze_slab(
        &self,
        request: SlabRequest,
        query_byte_limit: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<SlabAnalysis, GeometryError> {
        super::analyze_slab(self, request, query_byte_limit, work, cancel)
    }

    /// Certify a world-height slab of the actual placed occurrence. The query
    /// allowance excludes the same coexisting ownership as `analyze_slab`.
    pub fn analyze_placed_slab(
        &self,
        request: &PlacedSlabRequest,
        query_byte_limit: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<SlabAnalysis, GeometryError> {
        super::analyze_placed_slab(self, request, query_byte_limit, work, cancel)
    }
}
