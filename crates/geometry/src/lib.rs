// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Native geometry admission and inspection, independent of sessions and applications.

mod analysis;
mod import;
mod kernel;
mod parametric;
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
    GeometryWorkBudget, HeightAnalysis, InvariantHeightDomain, NativeCollarHeightSpace,
    NativeCollarPort, NativeCriticalCollar, NativeCriticalCollarCell,
    NativeCriticalCollarIncidence, NativeCriticalCollarMaterialTarget, NativeCriticalCollarPolicy,
    NativeCriticalCollarTarget, NativeCriticalCollarTargetSide, NativeEndpointStratum,
    NativeInterfaceAttachment, NativeOwnedEndpoint, NativeProjectedVoids,
    NativeProtectedVoidEnvelope, NativeSourceLeafProofKind, NativeSourceLeafReceipt,
    NativeUnionBindingLedger, NativeUnionBoundaryEdge, NativeUnionBoundarySource,
    NativeUnionCoverage, NativeUnionOccurrence, NativeUnionOccurrenceEvidence, NativeUnionPolicy,
    NativeUnionProofKind, NativeUnionSourceCapture, NativeVoidProjectionCapture,
    PendingNativeUnionProof, PlanarCapsuleClearance, PlanarCapsuleClearancePolicy, PlanarCapsuleMm,
    ProjectedBounds, ProtectedVoidHeightScope, ProtectedVoidPolicy, SlabAnalysis, SlabRequest,
    analyze_height_domain, analyze_placed_height_domain, capture_projected_voids,
    certify_endpoint_ownership, certify_endpoint_stratum_ownership,
    certify_native_collar_incidence, certify_native_critical_collar,
    certify_native_one_sided_source_leaf, certify_native_source_leaf, certify_native_union,
    certify_planar_capsule_clearance, certify_planar_capsule_pair_clearance, placed_bounds,
    projected_bounds, projected_protected_void_envelopes,
};
pub use kernel::Definition;
pub use section::{
    MAX_PREPARED_SECTION_BYTES, MAX_PREPARED_SECTION_SOURCE_BYTES, PreparedSectionSource,
    prepare_section_source,
};
pub use spiling_contracts::display::EncodedMesh as MeshArtifact;
pub use spiling_contracts::display::MeshBudget;
use spiling_contracts::geometry::GeometryErrorCode;
pub use spiling_contracts::geometry::{
    DisplayProfile, FaceId, FaceInfo, GeometryError, PlaneFrameMm, PlaneMm,
};
use std::sync::atomic::{AtomicBool, Ordering};

/// Native adapter dependency version captured in persisted provenance.
pub const KERNEL_VERSION: &str = "0.4.1";
/// Exact published native dependency revision; changed with the workspace pin.
pub const KERNEL_REVISION: &str = "fd38f9db4cd5c2b0e08e3d8110d9de9adac177db";

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

/// One native section boundary, closed and canonically ordered in the plane frame.
#[derive(Debug)]
pub struct SectionLoopMm {
    pub is_hole: bool,
    pub points_mm: Vec<[f64; 3]>,
}

/// Owned native section; loop identities are local to this result, never source faces.
#[derive(Debug)]
pub struct NativeSection {
    pub(crate) plane: PlaneMm,
    pub(crate) frame: PlaneFrameMm,
    pub(crate) loops: Vec<SectionLoopMm>,
    /// Exact original source-face ordinals for each local native loop, in the
    /// same canonical order. Derived conservative envelopes have no lineage.
    pub(crate) loop_source_faces: Vec<Vec<usize>>,
    pub(crate) sampling_tolerance_mm: f64,
    pub(crate) boolean_tolerance_mm: f64,
    certificate: NativeSectionCertificate,
}

#[derive(Debug, Clone)]
struct NativeSectionCertificate {
    definition_id: spiling_contracts::geometry::DefinitionId,
    authority: SectionAuthority,
    maximum_chord_error_mm: f64,
    maximum_plane_error_mm: f64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SectionAuthority {
    NativeIntersection,
    ConservativeSlabEnvelope,
}
impl NativeSection {
    pub fn plane(&self) -> PlaneMm {
        self.plane
    }
    pub fn frame(&self) -> PlaneFrameMm {
        self.frame
    }
    pub fn loops(&self) -> &[SectionLoopMm] {
        &self.loops
    }
    pub fn sampling_tolerance_mm(&self) -> f64 {
        self.sampling_tolerance_mm
    }
    pub fn boolean_tolerance_mm(&self) -> f64 {
        self.boolean_tolerance_mm
    }
    /// Consume the proof-bearing result when only untrusted geometric data is needed.
    pub fn into_loops(self) -> Vec<SectionLoopMm> {
        self.loops
    }
    pub fn definition_id(&self) -> &spiling_contracts::geometry::DefinitionId {
        &self.certificate.definition_id
    }
    /// Occupied slab envelopes are conservative projection bounds, not native
    /// intersections or material regions that can be promoted into layers.
    pub fn is_native_intersection(&self) -> bool {
        self.certificate.authority == SectionAuthority::NativeIntersection
    }
    pub fn achieved_chord_error_mm(&self) -> f64 {
        self.certificate.maximum_chord_error_mm
    }
    pub fn achieved_plane_error_mm(&self) -> f64 {
        self.certificate.maximum_plane_error_mm
    }
    pub(crate) fn loop_source_face_ordinals(&self, index: usize) -> Option<&[usize]> {
        if !self.is_native_intersection() || self.loop_source_faces.len() != self.loops.len() {
            return None;
        }
        self.loop_source_faces
            .get(index)
            .filter(|owners| !owners.is_empty())
            .map(Vec::as_slice)
    }
    /// Owned vector and identity-string capacities, excluding borrowed native
    /// geometry and allocator bookkeeping. Use this for cumulative job caches.
    pub fn retained_bytes(&self) -> usize {
        let lineage = self.loop_source_faces.iter().fold(
            self.loop_source_faces
                .capacity()
                .saturating_mul(std::mem::size_of::<Vec<usize>>()),
            |bytes, faces| {
                bytes.saturating_add(
                    faces
                        .capacity()
                        .saturating_mul(std::mem::size_of::<usize>()),
                )
            },
        );
        self.loops.iter().fold(
            self.loops
                .capacity()
                .saturating_mul(std::mem::size_of::<SectionLoopMm>())
                .saturating_add(self.certificate.definition_id.allocated_bytes())
                .saturating_add(lineage),
            |bytes, boundary| {
                bytes.saturating_add(
                    boundary
                        .points_mm
                        .capacity()
                        .saturating_mul(std::mem::size_of::<[f64; 3]>()),
                )
            },
        )
    }
}

impl PreparedSectionSource<'_> {
    /// Intersect the actual placed native source with an arbitrary world plane.
    /// The query allowance bounds combined adapter proof and output buffers;
    /// callers subtract every retained prepared source (including pinned
    /// proofs) and every other coexisting owned query/output buffer first.
    /// The shared 64 MiB admission and packed 16 MiB cap remain independent.
    /// Native Boolean intermediate objects retain separate bounded admissions
    /// and require an external host-memory limit; this is not an RSS cap.
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
    /// Analyze the complete native slab, including its occupied projection.
    /// A sampled section alone never authorizes material reuse or support.
    /// The query allowance is the shared 64 MiB pool remaining after **all**
    /// retained prepared sources (including pinned proofs), prior results and
    /// other coexisting owned query/output buffers. It is not a per-occurrence
    /// allowance; individual numerical scratch limits remain unchanged.
    pub fn analyze_slab(
        &self,
        request: SlabRequest,
        query_byte_limit: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<SlabAnalysis, GeometryError> {
        analysis::analyze_slab(self, request, query_byte_limit, work, cancel)
    }

    /// Capture the actual occurrence transform and certify a world-height slab.
    /// The caller supplies the remaining shared query allowance described by
    /// [`Self::analyze_slab`], including other occurrences' retained results.
    pub fn analyze_placed_slab(
        &self,
        pose: spiling_contracts::geometry::RigidPoseMm,
        world_height_interval_mm: [f64; 2],
        sample_world_z_mm: f64,
        native_shape_error_mm: f64,
        query_byte_limit: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<SlabAnalysis, GeometryError> {
        analysis::analyze_placed_slab(
            self,
            pose,
            world_height_interval_mm,
            sample_world_z_mm,
            native_shape_error_mm,
            query_byte_limit,
            work,
            cancel,
        )
    }
}

/// Admission limits only narrow the frozen native face and retained-query-proof
/// caps. Proof storage includes pinned curve payloads, continuation pieces and
/// their retained indices, not the complete BREP or process RSS.
#[derive(Debug, Clone, Copy)]
pub struct ImportLimits {
    pub max_faces: u32,
    pub max_query_proof_bytes: usize,
}
impl ImportLimits {
    pub const FROZEN: Self = Self {
        max_faces: spiling_contracts::geometry::MAX_NATIVE_FACES,
        max_query_proof_bytes: import::MAX_QUERY_PROOF_BYTES,
    };
}

pub fn import_step(bytes: &[u8], cancel: &AtomicBool) -> Result<Definition, GeometryError> {
    import_step_with_limits(bytes, cancel, ImportLimits::FROZEN)
}

/// Admit within the caller's remaining face and retained-query-proof capacity.
/// Both allowances are enforced before retaining native query-proof allocations.
pub fn import_step_with_limits(
    bytes: &[u8],
    cancel: &AtomicBool,
    limits: ImportLimits,
) -> Result<Definition, GeometryError> {
    import::import_step(
        bytes,
        cancel,
        limits.max_faces,
        limits.max_query_proof_bytes,
    )
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
