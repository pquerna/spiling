// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Native geometry admission and inspection, independent of sessions and applications.

mod analysis;
mod exact_coefficient;
mod import;
mod kernel;
pub mod parametric;
mod section;
mod tessellation;

#[cfg(test)]
#[path = "../examples/corpus/ast.rs"]
pub(crate) mod corpus_fixtures;
#[cfg(test)]
#[path = "../examples/parity/mod.rs"]
pub(crate) mod parity_fixtures;
#[cfg(test)]
mod tests;

pub use analysis::{
    AnalysisPolicy, BoundsPolicy, CompletedNativeUnionProofSet, CriticalEventNeighborhood,
    EndpointOwnershipRequest, GeometryWorkBudget, HeightAnalysis, InvariantHeightDomain,
    MaterialProjectedBounds, NativeCollarHeightSpace, NativeCollarPort, NativeCriticalCollar,
    NativeCriticalCollarCell, NativeCriticalCollarIncidence, NativeCriticalCollarMaterialTarget,
    NativeCriticalCollarPolicy, NativeCriticalCollarTarget, NativeCriticalCollarTargetSide,
    NativeEndpointStratum, NativeInnerMaterialPrism, NativeInterfaceAttachment,
    NativeOwnedEndpoint, NativeProjectedVoids, NativeProtectedVoidEnvelope,
    NativeSourceLeafProofKind, NativeSourceLeafReceipt, NativeUnionBindingLedger,
    NativeUnionBoundaryEdge, NativeUnionBoundarySource, NativeUnionCoverage, NativeUnionOccurrence,
    NativeUnionOccurrenceEvidence, NativeUnionPolicy, NativeUnionProofKind,
    NativeUnionSourceCapture, NativeVoidProjectionCapture, PendingNativeUnionProof,
    PlacedMaterialProjectedBounds, PlacedSlabRequest, PlanarCapsuleClearance,
    PlanarCapsuleClearancePolicy, PlanarCapsuleMm, PreparedSolid, ProjectedBounds,
    ProtectedVoidHeightScope, ProtectedVoidPolicy, SlabAnalysis, SlabRequest, SourceLeafRequest,
    analyze_height_domain, analyze_placed_height_domain, capture_projected_voids,
    certify_endpoint_ownership, certify_endpoint_stratum_ownership,
    certify_native_collar_incidence, certify_native_critical_collar,
    certify_native_one_sided_source_leaf, certify_native_source_leaf, certify_native_union,
    certify_planar_capsule_clearance, certify_planar_capsule_pair_clearance,
    material_projected_bounds, placed_bounds, placed_material_bounds, prepare_solid,
    projected_bounds, projected_protected_void_envelopes,
};
pub use import::{
    from_step_bytes, from_step_bytes_with_cancel, from_step_bytes_with_limits, from_step_file,
};
pub use kernel::{BBox3Mm, Definition, SurfaceParameterMap, make_kernel_context};
pub use parametric::{Interval, IntervalVec2, IntervalVec3};
pub use section::{
    MAX_PREPARED_SECTION_BYTES, MAX_PREPARED_SECTION_SOURCE_BYTES, NativeSection,
    PreparedSectionSource, SectionCurve, SectionEndpoint, SectionLoopMm, SectionSpan,
    prepare_section_source,
};
pub(crate) use section::{NativeSectionCertificate, SectionAuthority};
pub use spiling_contracts::display::EncodedMesh as MeshArtifact;
pub use spiling_contracts::display::MeshBudget;
use spiling_contracts::geometry::GeometryErrorCode;
pub use spiling_contracts::geometry::{
    DisplayProfile, FaceId, FaceInfo, GeometryError, KERNEL_NAME, PlaneFrameMm, PlaneMm,
};
use std::sync::atomic::{AtomicBool, Ordering};

/// Native adapter dependency version captured in persisted provenance.
pub const KERNEL_VERSION: &str = "0.0.1";
/// Exact published native dependency revision; changed with the workspace pin.
pub const KERNEL_REVISION: &str = "ee4d9c3feefb3c702ca53b190b7fe4516714c841";

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

impl PreparedSectionSource<'_> {
    /// Intersect the actual placed native source with an arbitrary world plane.
    /// The query allowance excludes every retained source and other coexisting
    /// buffer. Direct native intersection owns its proof/output admission;
    /// the 64 MiB query and packed 16 MiB output caps remain independent.
    pub fn section_placed_with_work(
        &self,
        pose: spiling_contracts::geometry::RigidPoseMm,
        plane: PlaneMm,
        query_byte_limit: u32,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<NativeSection, GeometryError> {
        analysis::section_placed_with_work(
            self.definition(),
            pose,
            plane,
            query_byte_limit,
            work,
            cancel,
        )
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
    let mut work = GeometryWorkBudget::new(50_000_000);
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
