// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! One borrowed native BREP material owner. Original STEP inspection and finish
//! authoring are separate roles and confer no material correspondence claim.
use super::{
    AnalysisPolicy, BoundsPolicy, GeometryWorkBudget, HeightAnalysis,
    PlacedMaterialProjectedBounds, PlacedSlabRequest, SlabAnalysis, SlabRequest,
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
    material_position_bounds: Option<[Interval; 3]>,
    material_unavailable: Option<GeometryError>,
    sections: crate::section::PreparedSectionSource<'d>,
    native_position_maps_ready: std::cell::Cell<bool>,
}

/// Internal borrowed operands of the one native material owner.
#[derive(Clone, Copy)]
pub(crate) struct MaterialSourceView<'s, 'd> {
    definition: &'d Definition,
    trims: &'s PreparedNativeTrimSource<'d>,
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
    pub(crate) fn trim_source(self) -> &'s PreparedNativeTrimSource<'d> {
        self.trims
    }
    pub(crate) fn effective_tolerance_mm(self) -> f64 {
        self.trims.effective_tolerance_mm()
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

/// Prepare the immutable native material and section operands once. Geometric
/// unavailability is a typed refusal; cancellation and ownership exhaustion are
/// errors, never successful partial preparation.
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
    let charts = PreparedNativeTrimSource::prepare_material(definition, allowance, work, cancel);
    if let Err(error) = &charts {
        if propagate_preparation_failure(error) {
            return Err(error.clone());
        }
    }
    let mut solid = PreparedSolid {
        definition,
        charts,
        material_position_bounds: None,
        material_unavailable: None,
        sections,
        native_position_maps_ready: std::cell::Cell::new(false),
    };
    if solid.charts.is_ok() {
        // Cache the same native material's coefficient enclosure for posed
        // interval arithmetic. This does not assert original STEP coincidence.
        let remaining = limit
            .checked_sub(solid.retained_bytes())
            .ok_or_else(capacity_error)?;
        let before = work.consumed();
        match super::placed::material_position_bounds(solid.source_view()?, remaining, work, cancel)
            .map_err(|error| {
                work.annotate_error(error, before, "native material position enclosure")
            }) {
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

impl<'d> PreparedSolid<'d> {
    pub fn definition(&self) -> &'d Definition {
        self.definition
    }
    pub fn effective_tolerance_mm(&self) -> Result<f64, GeometryError> {
        Ok(self.source_view()?.effective_tolerance_mm())
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
            .saturating_add(
                self.material_unavailable
                    .as_ref()
                    .map_or(0, |error| error.message.capacity()),
            )
    }
    /// Warm immutable native positional query aids BEFORE taking shared-owner
    /// byte snapshots. `query_byte_limit` excludes every already live owner and
    /// output; the returned allowance subtracts only newly retained capacities.
    /// Actual capacities become part of `retained_bytes()` under the unchanged
    /// source cap. No allocation occurs on reuse or during nested map queries.
    /// This changes no material geometry, source/native parameter law or tangent
    /// authority. Placement/bounds do not pay this manufacturing preparation.
    pub fn prepare_native_position_maps(
        &self,
        query_byte_limit: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<usize, GeometryError> {
        work.charge(1, cancel)?;
        if self.native_position_maps_ready.get() {
            return Ok(query_byte_limit);
        }
        let trims = self.charts.as_ref().map_err(Clone::clone)?;
        if let Some(error) = &self.material_unavailable {
            return Err(error.clone());
        }
        let available = crate::MAX_PREPARED_SECTION_SOURCE_BYTES
            .checked_sub(self.retained_bytes())
            .ok_or_else(capacity_error)?
            .min(query_byte_limit);
        let growth = trims.prepare_native_position_maps(available, work, cancel)?;
        let remaining = query_byte_limit
            .checked_sub(growth)
            .ok_or_else(capacity_error)?;
        self.sections
            .seed_native_position_maps(trims, work, cancel)?;
        self.native_position_maps_ready.set(true);
        Ok(remaining)
    }

    pub(crate) fn source_view(&self) -> Result<MaterialSourceView<'_, 'd>, GeometryError> {
        let trims = self.charts.as_ref().map_err(Clone::clone)?;
        if let Some(error) = &self.material_unavailable {
            return Err(error.clone());
        }
        Ok(MaterialSourceView {
            definition: self.definition,
            trims,
            material_position_bounds: self.material_position_bounds.as_ref(),
            retained_owner_bytes: self.retained_bytes(),
        })
    }
    /// Unposed support of the same native material, without preparing it again.
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
        let section = self
            .sections
            .section_for_query(query, cancel, byte_limit, work)?;
        if !section.matches_native_material(self.definition)
            || section.certificate.pose != RigidPoseMm::IDENTITY
        {
            return Err(GeometryError::new(
                GeometryErrorCode::InvalidGeometry,
                "native section changed its prepared material or pose identity",
            ));
        }
        Ok(section)
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
