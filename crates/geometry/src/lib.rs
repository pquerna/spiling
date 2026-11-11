// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Native geometry admission and inspection, independent of sessions and applications.

mod analysis;
mod compute;
mod exact_coefficient;
mod import;
mod kernel;
pub mod parametric;
mod section;
mod tessellation;
pub mod tolerant;

pub use tessellation::{DisplayProgress, DisplayStage};

#[cfg(test)]
#[path = "../examples/corpus/ast.rs"]
pub(crate) mod corpus_fixtures;
#[cfg(test)]
pub(crate) mod test_sources;
#[cfg(test)]
mod tests;

pub use analysis::{
    AnalysisPolicy, BoundsPolicy, CompletedNativeUnionProofSet, CriticalEventNeighborhood,
    EXACT_BOUNDS_WORK_FUSE, EndpointOwnershipRequest, GeometryWorkBudget, HeightAnalysis,
    InvariantHeightDomain, MaterialProjectedBounds, NativeCollarHeightSpace, NativeCollarPort,
    NativeCriticalCollar, NativeCriticalCollarCell, NativeCriticalCollarIncidence,
    NativeCriticalCollarMaterialTarget, NativeCriticalCollarPolicy, NativeCriticalCollarTarget,
    NativeCriticalCollarTargetSide, NativeEndpointStratum, NativeInnerMaterialPrism,
    NativeInterfaceAttachment, NativeOwnedEndpoint, NativeProjectedVoids,
    NativeProtectedVoidEnvelope, NativeSourceLeafProofKind, NativeSourceLeafReceipt,
    NativeUnionBindingLedger, NativeUnionBoundaryEdge, NativeUnionBoundarySource,
    NativeUnionCoverage, NativeUnionOccurrence, NativeUnionOccurrenceEvidence, NativeUnionPolicy,
    NativeUnionProofKind, NativeUnionSourceCapture, NativeVoidProjectionCapture,
    PendingNativeUnionProof, PlacedMaterialProjectedBounds, PlacedSlabRequest,
    PlanarCapsuleClearance, PlanarCapsuleClearancePolicy, PlanarCapsuleMm, PreparedSolid,
    ProjectedBounds, ProtectedVoidHeightScope, ProtectedVoidPolicy, SlabAnalysis, SlabRequest,
    SourceLeafRequest, analyze_height_domain, analyze_placed_height_domain,
    capture_projected_voids, certify_endpoint_ownership, certify_endpoint_stratum_ownership,
    certify_native_collar_incidence, certify_native_critical_collar,
    certify_native_one_sided_source_leaf, certify_native_source_leaf, certify_native_union,
    certify_planar_capsule_clearance, certify_planar_capsule_pair_clearance,
    material_projected_bounds, placed_bounds, placed_material_bounds, prepare_solid,
    projected_bounds, projected_protected_void_envelopes,
};
pub use compute::{
    DISPLAY_PROOF_BYTES, MAX_DIRECTIONAL_SCRATCH_BYTES, MAX_PREPARED_SECTION_BYTES,
    MAX_PREPARED_SECTION_SOURCE_BYTES, MAX_QUERY_PROOF_BYTES,
};
pub use import::{
    from_step_bytes, from_step_bytes_with_cancel, from_step_bytes_with_limits, from_step_file,
    from_step_solid_with_cancel, from_step_solid_with_limits,
    structure::{SourceBody, source_bodies},
};
pub use kernel::{
    BBox3Mm, Definition, KERNEL_APPROXIMATION_TOLERANCE_MM, KERNEL_INTERSECTION_TOLERANCE_MM,
    KERNEL_ZONE_TOLERANCE_MM, SurfaceParameterMap, make_kernel_context,
};
pub use parametric::{Interval, IntervalVec2, IntervalVec3};
#[cfg(test)]
pub(crate) use section::NativeSectionCertificate;
pub(crate) use section::SectionAuthority;
pub use section::{
    NativeSection, PreparedSectionSource, SectionChartEndpoint, SectionCurve, SectionEndpoint,
    SectionGraphChart, SectionLoopMm, SectionSpan, SectionSpanAttachments, prepare_section_source,
};
pub use spiling_contracts::display::EncodedMesh as MeshArtifact;
pub use spiling_contracts::display::MeshBudget;
use spiling_contracts::geometry::GeometryErrorCode;
pub use spiling_contracts::geometry::{
    DisplayProfile, FaceId, FaceInfo, GeometryError, KERNEL_NAME, PlaneFrameMm, PlaneMm,
};
use std::sync::atomic::{AtomicBool, Ordering};

#[expect(
    clippy::too_many_arguments,
    reason = "Native section dispatch keeps material authority, prepared ownership and both independent byte quotas explicit"
)]
fn intersect_native_section(
    definition: &Definition,
    prepared: Option<&PreparedSectionSource<'_>>,
    material: Option<analysis::MaterialSourceView<'_, '_>>,
    query: &section::SectionQuery,
    byte_limit: usize,
    packed_byte_limit: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<NativeSection, GeometryError> {
    let query = query.normalized()?;
    if prepared.is_some_and(|source| !std::ptr::eq(source.definition(), definition))
        || material.is_some_and(|source| !std::ptr::eq(source.definition(), definition))
    {
        return Err(GeometryError::new(
            GeometryErrorCode::InvalidGeometry,
            "native section operands belong to another definition",
        ));
    }
    if analysis::boundary_level::has_candidate(definition, query.plane, work, cancel)? {
        let local;
        let (source, available) = if let Some(source) = material {
            (source, byte_limit)
        } else {
            local = kernel::PreparedNativeTrimSource::prepare_material(
                definition,
                byte_limit.min(MAX_PREPARED_SECTION_BYTES),
                work,
                cancel,
            );
            match &local {
                Ok(trims) => (
                    analysis::MaterialSourceView::from_native_trims(trims),
                    byte_limit
                        .min(MAX_PREPARED_SECTION_BYTES)
                        .checked_sub(trims.retained_bytes())
                        .ok_or_else(|| {
                            section::resource_error("native section trims exceed query allowance")
                        })?,
                ),
                Err(error)
                    if matches!(
                        error.code,
                        GeometryErrorCode::Cancelled | GeometryErrorCode::ResourceLimit
                    ) =>
                {
                    return Err(error.clone());
                }
                Err(_) => {
                    return section::intersect_direct(
                        definition,
                        prepared,
                        &query,
                        byte_limit,
                        packed_byte_limit,
                        work,
                        cancel,
                    );
                }
            }
        };
        if let Some(result) = analysis::boundary_level::intersect(
            source,
            &query,
            available,
            packed_byte_limit,
            work,
            cancel,
        )? {
            return Ok(result);
        }
    }
    section::intersect_direct(
        definition,
        prepared,
        &query,
        byte_limit,
        packed_byte_limit,
        work,
        cancel,
    )
}

/// Native adapter dependency version captured in persisted provenance.
pub const KERNEL_VERSION: &str = "1.0.1";
/// Exact published native dependency revision; changed with the workspace pin.
pub const KERNEL_REVISION: &str = "aad5640cc8e55d6490192aab329581353cc93b41";

/// Immutable native certificate for one full-height rectangular vertical wall.
/// Coordinates and phase direction are source-local millimetres; the outward
/// supporting normal comes from the oriented native face use. This certifies
/// neither an entire invariant extrusion nor a manufactured texture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VerticalWallFace {
    pub origin_mm: [f64; 3],
    pub horizontal_direction: [f64; 3],
    pub outward_normal: [f64; 3],
    pub width_mm: f64,
    pub min_z_mm: f64,
    pub max_z_mm: f64,
}

/// One trimmed vertical rectangular face in source-local millimetres.
/// This establishes neither whole-source height coverage nor global support.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VerticalRectangularFace {
    pub origin_mm: [f64; 3],
    pub horizontal_direction: [f64; 3],
    pub outward_normal: [f64; 3],
    pub width_mm: f64,
    pub min_z_mm: f64,
    pub max_z_mm: f64,
}

/// Exact full-turn exterior cylindrical source-face certificate. Angles use
/// source +X and right-handed +Z, not the native carrier's seam or UV chart.
/// Manufacturing separately certifies posed invariant-extrusion eligibility.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VerticalCylinderFace {
    pub center_mm: [f64; 2],
    pub radius_mm: f64,
    pub min_z_mm: f64,
    pub max_z_mm: f64,
}

/// Source +X/right-handed +Z angular ownership, in increasing radians.
/// Each interval may cross 2π; its length is the physical owned arc.
#[derive(Debug, Clone, PartialEq)]
pub struct BoreFaceAngles {
    pub face_id: FaceId,
    pub intervals_radians: Vec<[f64; 2]>,
}

/// Geometry-owned rectangular source-Z plate with exactly one circular through
/// bore. This value is constructed only by native coefficient/trim/incidence
/// qualification; it grants no mechanical, manufacturing or calibration claim.
#[derive(Debug, Clone, PartialEq)]
pub struct BoredPlate {
    center_mm: [f64; 2],
    radius_mm: f64,
    height_mm: [f64; 2],
    outer_xy_mm: [[f64; 2]; 2],
    bore_faces: Vec<FaceId>,
    cap_faces: [FaceId; 2],
    outer_faces: [FaceId; 4],
    bore_face_angles: Vec<BoreFaceAngles>,
}

impl BoredPlate {
    pub fn center_mm(&self) -> [f64; 2] {
        self.center_mm
    }
    pub fn radius_mm(&self) -> f64 {
        self.radius_mm
    }
    pub fn height_mm(&self) -> [f64; 2] {
        self.height_mm
    }
    /// [[minX, minY], [maxX, maxY]].
    pub fn outer_xy_mm(&self) -> [[f64; 2]; 2] {
        self.outer_xy_mm
    }
    pub fn bore_faces(&self) -> &[FaceId] {
        &self.bore_faces
    }
    /// Bottom, top.
    pub fn cap_faces(&self) -> &[FaceId; 2] {
        &self.cap_faces
    }
    /// minX, maxX, minY, maxY.
    pub fn outer_faces(&self) -> &[FaceId; 4] {
        &self.outer_faces
    }
    pub fn bore_face_angles(&self) -> &[BoreFaceAngles] {
        &self.bore_face_angles
    }
}

impl PreparedSectionSource<'_> {
    /// Intersect the actual placed native source with an arbitrary world plane.
    /// The query allowance excludes every retained source and other coexisting
    /// buffer. Direct native intersection owns its proof/output admission;
    /// `MAX_PREPARED_SECTION_BYTES` and the packed 16 MiB output cap remain independent.
    pub fn section_placed_with_work(
        &self,
        pose: spiling_contracts::geometry::RigidPoseMm,
        plane: PlaneMm,
        query_byte_limit: u32,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<NativeSection, GeometryError> {
        analysis::section_placed_with_work(self, pose, plane, query_byte_limit, work, cancel)
    }
}

pub fn tessellate(
    definition: &Definition,
    profile: DisplayProfile,
    cancel: &AtomicBool,
) -> Result<MeshArtifact, GeometryError> {
    tessellate_with_budget(definition, profile, cancel, MeshBudget::FROZEN)
}

/// Pack within remaining scene capacity; full-cap standalone consumers use `tessellate`.
pub fn tessellate_with_budget(
    definition: &Definition,
    profile: DisplayProfile,
    cancel: &AtomicBool,
    budget: MeshBudget,
) -> Result<MeshArtifact, GeometryError> {
    tessellation::tessellate(definition, profile, cancel, budget)
}

/// Borrow the caller's cumulative work allowance and report native display phases.
/// Face counts apply to the current refinement attempt; work consumption is not
/// percentage complete. Callbacks run on the native caller thread.
pub fn tessellate_with_work(
    definition: &Definition,
    profile: DisplayProfile,
    cancel: &AtomicBool,
    budget: MeshBudget,
    work: &mut GeometryWorkBudget,
    progress: &mut impl FnMut(DisplayProgress),
) -> Result<MeshArtifact, GeometryError> {
    tessellation::tessellate_with_work(definition, profile, cancel, budget, work, progress)
}

pub fn section(
    definition: &Definition,
    plane_in_definition_mm: PlaneMm,
    cancel: &AtomicBool,
) -> Result<NativeSection, GeometryError> {
    section_with_byte_limit(
        definition,
        plane_in_definition_mm,
        cancel,
        spiling_contracts::geometry::MAX_SECTION_BYTES,
    )
}

/// Bound packed sampled output before growth, including SPLS-equivalent overhead.
///
/// Native preparation, pinned query proofs and owned query/output scratch share
/// the independent frozen `MAX_PREPARED_SECTION_BYTES` pool. `byte_limit` only
/// narrows packed output (at most `MAX_SECTION_BYTES`); it is not a native source
/// admission cap. Prepared job callers instead pass their remaining shared-pool
/// allowance to `PreparedSectionSource::section_with_byte_limit`.
pub fn section_with_byte_limit(
    definition: &Definition,
    plane_in_definition_mm: PlaneMm,
    cancel: &AtomicBool,
    byte_limit: u32,
) -> Result<NativeSection, GeometryError> {
    let mut work = GeometryWorkBudget::query();
    section::section(
        definition,
        plane_in_definition_mm,
        cancel,
        byte_limit,
        &mut work,
    )
}

pub fn inspect_face(definition: &Definition, face: &FaceId) -> Result<FaceInfo, GeometryError> {
    definition
        .faces
        .iter()
        .find(|info| &info.face_id == face)
        .cloned()
        .ok_or_else(|| {
            GeometryError::new(
                GeometryErrorCode::UnknownHandle,
                "source face is not in this definition",
            )
        })
}

pub(crate) fn check_cancel(cancel: &AtomicBool) -> Result<(), GeometryError> {
    if cancel.load(Ordering::Acquire) {
        Err(GeometryError::new(
            GeometryErrorCode::Cancelled,
            "geometry operation cancelled",
        ))
    } else {
        Ok(())
    }
}
