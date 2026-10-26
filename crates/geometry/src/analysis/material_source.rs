// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! One borrowed native BREP, with independently scoped inspection and material
//! preparation. Unavailable source correspondence cannot disable raw inspection,
//! and an inspection result cannot silently acquire manufacturing authority.
use super::{
    AnalysisPolicy, BoundsPolicy, GeometryWorkBudget, HeightAnalysis,
    PlacedMaterialProjectedBounds, PlacedSlabRequest, PreparedSourceVertexPlans, SlabAnalysis,
    SlabRequest,
};
use crate::kernel::{PreparedFaceTrimDomain, PreparedNativeTrimSource};
use crate::parametric::Interval;
use crate::section::SectionQuery;
use crate::{Definition, GeometryError, NativeSection, PlaneMm};
use spiling_contracts::geometry::{GeometryErrorCode, RigidPoseMm};
use std::{mem::size_of, sync::atomic::AtomicBool};

pub struct PreparedSolid<'d> {
    definition: &'d Definition,
    charts: Result<PreparedNativeTrimSource<'d>, GeometryError>,
    plans: Option<PreparedSourceVertexPlans<'d>>,
    material_position_bounds: Option<[Interval; 3]>,
    material_unavailable: Option<GeometryError>,
    sections: crate::section::PreparedSectionSource<'d>,
}

/// Internal borrowed query operands. This is not another prepared-source owner:
/// domains and source-only plans always belong to the same `PreparedSolid`.
#[derive(Clone, Copy)]
pub(crate) struct MaterialSourceView<'s, 'd> {
    definition: &'d Definition,
    trims: &'s PreparedNativeTrimSource<'d>,
    plans: Option<&'s PreparedSourceVertexPlans<'d>>,
    material_position_bounds: Option<&'s [Interval; 3]>,
    retained_owner_bytes: usize,
}
impl<'s, 'd> MaterialSourceView<'s, 'd> {
    pub(crate) fn definition(self) -> &'d Definition {
        self.definition
    }
    pub(crate) fn retained_owner_bytes(self) -> usize {
        self.retained_owner_bytes
    }
    pub(crate) fn cached_position_bounds(self) -> Option<[Interval; 3]> {
        self.material_position_bounds.copied()
    }
    pub(crate) fn face_domain(
        self,
        ordinal: usize,
        _remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<&'s PreparedFaceTrimDomain<'d>, GeometryError> {
        work.charge(1, cancel)?;
        self.trims.face_domain(ordinal).ok_or_else(|| {
            GeometryError::new(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "prepared native source face ordinal is unavailable",
            )
        })
    }
    pub(crate) fn vertex_plans(self) -> Option<&'s PreparedSourceVertexPlans<'d>> {
        self.plans
    }
    pub(crate) fn trim_source(self) -> &'s PreparedNativeTrimSource<'d> {
        self.trims
    }
    pub(crate) fn material_transport_is_proved_zero(
        self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.trims.certify_zero_material_transport(work, cancel)
    }
    /// Deliberately permit foreign/unretained plans only in owner-validation
    /// tests. Production views are constructed solely by their prepared owner.
    #[cfg(test)]
    pub(crate) fn with_vertex_plans(
        self,
        plans: Option<&'s PreparedSourceVertexPlans<'d>>,
    ) -> Self {
        Self {
            plans,
            material_position_bounds: None,
            ..self
        }
    }
}

fn capacity_error() -> GeometryError {
    GeometryError::new(
        GeometryErrorCode::ResourceLimit,
        "prepared native solid exceeds retained source allowance",
    )
}
fn propagate_preparation_failure(error: &GeometryError) -> bool {
    matches!(
        error.code,
        GeometryErrorCode::Cancelled | GeometryErrorCode::ResourceLimit
    )
}

/// Prepare immutable source facts once. Geometry unavailability is retained as a
/// typed manufacturing refusal; cancellation or exhausted ownership/work is an
/// error, never an apparently successful partial preparation.
pub fn prepare_solid<'d>(
    definition: &'d Definition,
    byte_limit: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<PreparedSolid<'d>, GeometryError> {
    work.charge(1, cancel)?;
    let limit = byte_limit.min(crate::MAX_PREPARED_SECTION_SOURCE_BYTES);
    let base = size_of::<PreparedSolid<'d>>()
        .checked_add(definition.retained_query_proof_bytes())
        .ok_or_else(capacity_error)?;
    let sections = crate::section::prepare_section_source(
        definition,
        limit
            .checked_sub(base)
            .and_then(|bytes| {
                bytes.checked_add(size_of::<crate::section::PreparedSectionSource<'d>>())
            })
            .ok_or_else(capacity_error)?,
        cancel,
        work,
    )?;
    let allowance = limit
        .checked_sub(base)
        .and_then(|bytes| {
            bytes.checked_sub(
                sections.retained_bytes() - size_of::<crate::section::PreparedSectionSource<'d>>(),
            )
        })
        .and_then(|bytes| bytes.checked_add(size_of::<PreparedNativeTrimSource<'d>>()))
        .ok_or_else(capacity_error)?;
    let charts = PreparedNativeTrimSource::prepare_originals(definition, allowance, work, cancel);
    if let Err(error) = &charts {
        if propagate_preparation_failure(error) {
            return Err(error.clone());
        }
    }
    let mut solid = PreparedSolid {
        definition,
        charts,
        plans: None,
        material_position_bounds: None,
        material_unavailable: None,
        sections,
    };
    if let Ok(trims) = &solid.charts {
        // This validates selected ORIGINAL coedges, domains, joints and their
        // source-forward maps, not generated native trims relabeled by proximity.
        // Closed realization/topology/metric authority remains query-scoped.
        let preparation = (|| {
            trims.require_source_correspondence(work, cancel)?;
            let remaining = limit
                .checked_sub(solid.retained_bytes())
                .ok_or_else(capacity_error)?;
            PreparedSourceVertexPlans::prepare(
                MaterialSourceView {
                    definition,
                    trims,
                    plans: None,
                    material_position_bounds: None,
                    retained_owner_bytes: solid.retained_bytes(),
                },
                remaining
                    .checked_add(size_of::<PreparedSourceVertexPlans<'d>>())
                    .ok_or_else(capacity_error)?,
                work,
                cancel,
            )
        })();
        match preparation {
            Ok(plans) => solid.plans = Some(plans),
            Err(error) if propagate_preparation_failure(&error) => return Err(error),
            Err(error) => solid.material_unavailable = Some(error),
        }
    }
    if solid.plans.is_some() && solid.material_unavailable.is_none() {
        // Bound the actual selected F/C/V realization and retained cone G once.
        // Neither the native AABB nor a declared tolerance bounds that material.
        let remaining = limit
            .checked_sub(solid.retained_bytes())
            .ok_or_else(capacity_error)?;
        match super::placed::material_position_bounds(solid.source_view()?, remaining, work, cancel)
        {
            Ok(bounds) => solid.material_position_bounds = Some(bounds),
            Err(error) if propagate_preparation_failure(&error) => return Err(error),
            Err(error) => solid.material_unavailable = Some(error),
        }
    }
    if solid.retained_bytes() > limit {
        return Err(capacity_error());
    }
    Ok(solid)
}

/// Standalone coefficient/source proof, without preparing source-vertex plans.
/// The prepared-source path calls the borrowed view method instead, so source
/// construction never recurses through a Definition zero-transport query.
pub(crate) fn certify_zero_material_transport(
    definition: &Definition,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(1, cancel)?;
    let allowance = crate::MAX_PREPARED_SECTION_SOURCE_BYTES
        .checked_sub(definition.retained_query_proof_bytes())
        .ok_or_else(capacity_error)?;
    let source = PreparedNativeTrimSource::prepare_originals(definition, allowance, work, cancel)?;
    source.certify_zero_material_transport(work, cancel)
}

impl<'d> PreparedSolid<'d> {
    pub fn definition(&self) -> &'d Definition {
        self.definition
    }
    /// Inline fields, each owned capacity and pinned source proof exactly once.
    /// The immutable borrowed BREP is not another job-local allocation.
    pub fn retained_bytes(&self) -> usize {
        let charts = match &self.charts {
            Ok(source) => source
                .retained_bytes()
                .saturating_sub(size_of::<PreparedNativeTrimSource<'d>>()),
            Err(error) => error.message.capacity(),
        };
        size_of::<Self>()
            .saturating_add(self.definition.retained_query_proof_bytes())
            .saturating_add(charts)
            .saturating_add(
                self.sections
                    .retained_bytes()
                    .saturating_sub(size_of::<crate::section::PreparedSectionSource<'_>>()),
            )
            .saturating_add(self.plans.as_ref().map_or(0, |plans| {
                plans
                    .retained_bytes()
                    .saturating_sub(size_of::<PreparedSourceVertexPlans<'d>>())
            }))
            .saturating_add(
                self.material_unavailable
                    .as_ref()
                    .map_or(0, |error| error.message.capacity()),
            )
    }
    pub(crate) fn source_view(&self) -> Result<MaterialSourceView<'_, 'd>, GeometryError> {
        let trims = self.charts.as_ref().map_err(Clone::clone)?;
        if let Some(error) = &self.material_unavailable {
            return Err(error.clone());
        }
        Ok(MaterialSourceView {
            definition: self.definition,
            trims,
            plans: self.plans.as_ref(),
            material_position_bounds: self.material_position_bounds.as_ref(),
            retained_owner_bytes: self.retained_bytes(),
        })
    }
    /// Unposed support of the same selected source, without preparing it again.
    /// Retained owner and query scratch share the frozen directional byte cap.
    pub fn material_bounds(
        &self,
        direction: [f64; 3],
        policy: BoundsPolicy,
        query_byte_limit: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<super::MaterialProjectedBounds, GeometryError> {
        let remaining = super::bounds::MAX_SCRATCH_ALLOCATION_BYTES
            .checked_sub(self.retained_bytes())
            .ok_or_else(|| {
                GeometryError::new(
                    GeometryErrorCode::ResourceLimit,
                    "retained native material source exhausts directional allocation cap",
                )
            })?
            .min(query_byte_limit);
        super::bounds::material_projected_bounds_with_scratch_limit(
            self.source_view()?,
            direction,
            policy,
            remaining,
            work,
            cancel,
        )
    }
    pub fn placed_material_bounds(
        &self,
        pose: RigidPoseMm,
        world_direction: [f64; 3],
        policy: BoundsPolicy,
        query_byte_limit: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<PlacedMaterialProjectedBounds, GeometryError> {
        super::placed::placed_material_bounds_with_scratch_limit(
            self.source_view()?,
            pose,
            world_direction,
            policy,
            query_byte_limit,
            work,
            cancel,
        )
    }
    pub fn analyze_placed_height_domain(
        &self,
        pose: RigidPoseMm,
        world_height_interval_mm: [f64; 2],
        policy: AnalysisPolicy,
        query_byte_limit: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<HeightAnalysis, GeometryError> {
        super::placed::analyze_placed_height_domain_with_scratch_limit(
            self.source_view()?,
            pose,
            world_height_interval_mm,
            policy,
            query_byte_limit,
            work,
            cancel,
        )
    }
    pub(crate) fn section_for_query(
        &self,
        query: &SectionQuery,
        cancel: &AtomicBool,
        byte_limit: u32,
        work: &mut GeometryWorkBudget,
    ) -> Result<NativeSection, GeometryError> {
        self.sections
            .section_for_query(query, cancel, byte_limit, work)
    }
    /// Raw posed inspection uses the same geometry-owned affine pullback and
    /// world-space metric checks without borrowing manufacturing admission.
    pub fn section_placed_with_work(
        &self,
        pose: RigidPoseMm,
        world_plane: PlaneMm,
        query_byte_limit: u32,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<NativeSection, GeometryError> {
        self.sections
            .section_placed_with_work(pose, world_plane, query_byte_limit, work, cancel)
    }
    pub fn analyze_slab(
        &self,
        request: SlabRequest,
        query_byte_limit: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<SlabAnalysis, GeometryError> {
        super::analyze_slab(self, request, query_byte_limit, work, cancel)
    }
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
