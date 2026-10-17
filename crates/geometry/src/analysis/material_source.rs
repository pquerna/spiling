// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{GeometryWorkBudget, PlacedSlabRequest, SlabAnalysis, SlabRequest};
use crate::section::{OriginalTrimSource, SectionQuery};
use crate::{Definition, GeometryError, NativeSection, PreparedSectionSource};
use spiling_contracts::geometry::GeometryErrorCode;
use std::sync::atomic::AtomicBool;

use super::PreparedSourceVertexPlans;

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
    work.charge(1, cancel)?;
    let limit = remaining_prepared_bytes.min(crate::MAX_PREPARED_SECTION_SOURCE_BYTES);
    let section_limit = limit
        .checked_sub(header_bytes())
        .ok_or_else(capacity_error)?;
    let section = crate::prepare_section_source(definition, section_limit, cancel, work)?;
    let plan_limit = section_limit
        .checked_sub(section.retained_bytes())
        .ok_or_else(capacity_error)?;
    let plans =
        PreparedSourceVertexPlans::prepare(section.trim_source(), plan_limit, work, cancel)?;
    let source = PreparedMaterialSource { section, plans };
    if source.retained_bytes() > limit {
        return Err(capacity_error());
    }
    Ok(source)
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
