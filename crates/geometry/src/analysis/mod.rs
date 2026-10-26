// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

mod bounds;
mod collar;
mod critical;
mod joint;
mod material_source;
mod placed;
mod planar;
mod slab;
pub(crate) mod source_incidence;
pub(crate) mod source_restriction;
#[cfg(test)]
pub(crate) mod test_source;
mod voids;

use crate::{Definition, GeometryError, NativeSection, PlaneFrameMm, PlaneMm, check_cancel};
use spiling_contracts::geometry::{DefinitionId, GeometryErrorCode};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub(crate) use bounds::MAX_SCRATCH_ALLOCATION_BYTES;
pub use bounds::{MaterialProjectedBounds, material_projected_bounds, projected_bounds};
pub use collar::{
    NativeCollarHeightSpace, NativeCollarPort, NativeCriticalCollar, NativeCriticalCollarCell,
    NativeCriticalCollarMaterialTarget, NativeCriticalCollarPolicy, NativeCriticalCollarTarget,
    NativeCriticalCollarTargetSide, certify_native_critical_collar,
};
pub use critical::CriticalEventNeighborhood;
pub(crate) use critical::PreparedSourceVertexPlans;
pub use joint::{
    CompletedNativeUnionProofSet, EndpointOwnershipRequest, NativeCriticalCollarIncidence,
    NativeInnerMaterialPrism, NativeInterfaceAttachment, NativeOwnedEndpoint,
    NativeSourceLeafProofKind, NativeSourceLeafReceipt, NativeUnionBindingLedger,
    NativeUnionBoundaryEdge, NativeUnionBoundarySource, NativeUnionCoverage, NativeUnionOccurrence,
    NativeUnionOccurrenceEvidence, NativeUnionPolicy, NativeUnionProofKind,
    NativeUnionSourceCapture, PendingNativeUnionProof, SourceLeafRequest,
    certify_endpoint_ownership, certify_endpoint_stratum_ownership,
    certify_native_collar_incidence, certify_native_one_sided_source_leaf,
    certify_native_source_leaf, certify_native_union,
};
pub(crate) use material_source::{MaterialSourceView, certify_zero_material_transport};
pub use material_source::{PreparedSolid, prepare_solid};
pub use placed::{
    PlacedMaterialProjectedBounds, analyze_placed_height_domain, placed_bounds,
    placed_material_bounds,
};
pub(crate) use placed::{analyze_placed_slab, section_placed_with_work};
pub use planar::{
    PlanarCapsuleClearance, PlanarCapsuleClearancePolicy, PlanarCapsuleMm,
    certify_planar_capsule_clearance, certify_planar_capsule_pair_clearance,
};
pub use slab::analyze_height_domain;
pub(crate) use slab::{HeightFunctional, analyze_slab};
pub use voids::{
    NativeProjectedVoids, NativeProtectedVoidEnvelope, NativeVoidProjectionCapture,
    ProtectedVoidHeightScope, ProtectedVoidPolicy, capture_projected_voids,
    projected_protected_void_envelopes,
};

/// Cumulative numerical work across a captured geometry job. Reusing a cache or
/// subdividing an interval never resets this counter.
#[derive(Debug)]
pub struct GeometryWorkBudget {
    remaining: AtomicU64,
    consumed: AtomicU64,
}
impl GeometryWorkBudget {
    pub fn new(maximum_work: u64) -> Self {
        Self {
            remaining: AtomicU64::new(maximum_work),
            consumed: AtomicU64::new(0),
        }
    }
    pub fn consumed(&self) -> u64 {
        self.consumed.load(Ordering::Acquire)
    }
    pub fn remaining(&self) -> u64 {
        self.remaining.load(Ordering::Acquire)
    }
    /// Add bounded, failure-only phase context without changing the error code or cause.
    pub(crate) fn annotate_error(
        &self,
        error: GeometryError,
        before: u64,
        context: impl std::fmt::Display,
    ) -> GeometryError {
        let consumed = self.consumed();
        let remaining = self.remaining();
        GeometryError::new(
            error.code,
            format!(
                "{context}: work_before={before} work_consumed={consumed} work_delta={} work_remaining={remaining}: {}",
                consumed.saturating_sub(before),
                error.message,
            ),
        )
    }
    pub fn charge(&self, amount: u64, cancel: &AtomicBool) -> Result<(), GeometryError> {
        let mut current = self.remaining.load(Ordering::Acquire);
        loop {
            if amount > current {
                check_cancel(cancel)?;
                return Err(GeometryError::new(
                    GeometryErrorCode::ResourceLimit,
                    "cumulative geometry work exhausted",
                ));
            }
            match self.remaining.compare_exchange_weak(
                current,
                current - amount,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.consumed.fetch_add(amount, Ordering::AcqRel);
                    // Debit completed native work even when cancellation arrived inside it.
                    return check_cancel(cancel);
                }
                Err(actual) => {
                    current = actual;
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BoundsPolicy {
    pub maximum_endpoint_uncertainty_mm: f64,
}
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct ProjectedBounds {
    pub minimum: [f64; 2],
    pub maximum: [f64; 2],
    pub achieved_uncertainty_mm: f64,
}
#[derive(Debug, Clone, Copy)]
pub struct AnalysisPolicy {
    pub maximum_height_uncertainty_mm: f64,
    pub maximum_shape_error_mm: f64,
}
/// Constructed only by native interval analysis. Invariance applies within each
/// regular partition, never across the critical event brackets separating them.
#[derive(Debug, Clone)]
pub struct InvariantHeightDomain {
    definition_id: DefinitionId,
    material_realization: crate::kernel::MaterialRealizationIdentity,
    frame: PlaneFrameMm,
    height_interval_mm: [f64; 2],
    pose: Option<spiling_contracts::geometry::RigidPoseMm>,
}
impl InvariantHeightDomain {
    pub(crate) fn material_realization_identity(
        &self,
    ) -> crate::kernel::MaterialRealizationIdentity {
        self.material_realization
    }
    pub(crate) fn matches_original_material(&self, definition: &Definition) -> bool {
        self.definition_id() == definition.id()
            && self.material_realization
                == crate::kernel::MaterialRealizationIdentity::new(definition)
    }
    pub fn definition_id(&self) -> &DefinitionId {
        &self.definition_id
    }
    pub fn frame(&self) -> PlaneFrameMm {
        self.frame
    }
    pub fn height_interval_mm(&self) -> [f64; 2] {
        self.height_interval_mm
    }
    pub fn pose(&self) -> Option<spiling_contracts::geometry::RigidPoseMm> {
        self.pose
    }
}
#[derive(Debug)]
#[non_exhaustive]
pub struct HeightAnalysis {
    pub(crate) event_brackets_mm: Vec<[f64; 2]>,
    pub(crate) invariance: Option<InvariantHeightDomain>,
    proved_event_brackets_mm: Vec<[f64; 2]>,
    capture: HeightCapture,
}

/// Conservative support of possible source-relative transport, not an isolated
/// critical height, an existence witness, or a physical layer boundary.
type SourceTransportCover = critical::material_link::SourceTransportSupport;

#[derive(Debug)]
struct HeightCapture {
    definition_id: DefinitionId,
    material_realization: crate::kernel::MaterialRealizationIdentity,
    transport_support: Vec<SourceTransportCover>,
    transport_support_complete: bool,
    frame: PlaneFrameMm,
    height_interval_mm: [f64; 2],
    policy: AnalysisPolicy,
    pose: Option<spiling_contracts::geometry::RigidPoseMm>,
}
impl HeightAnalysis {
    /// Ordinary event brackets describe literal native geometry. Overlapping
    /// transport support requires a complete realized-material closed capture,
    /// even when the ordinary native event list is empty.
    pub fn requires_closed_material_capture(&self, range: [f64; 2]) -> bool {
        !self.capture.transport_support_complete
            || !range.into_iter().all(f64::is_finite)
            || range[0] > range[1]
            || range[0] < self.capture.height_interval_mm[0]
            || range[1] > self.capture.height_interval_mm[1]
            || self.capture.transport_support.iter().any(|cover| {
                cover.base_interval[0].min(cover.corrected_interval[0]) <= range[1]
                    && range[0] <= cover.base_interval[1].max(cover.corrected_interval[1])
            })
    }
    pub fn retained_bytes(&self) -> usize {
        self.capture.definition_id.allocated_bytes()
            + (self.event_brackets_mm.capacity() + self.proved_event_brackets_mm.capacity())
                * std::mem::size_of::<[f64; 2]>()
            + self.capture.transport_support.capacity()
                * std::mem::size_of::<SourceTransportCover>()
            + self
                .invariance
                .as_ref()
                .map_or(0, |proof| proof.definition_id.allocated_bytes())
    }
    pub(crate) fn material_realization_identity(
        &self,
    ) -> crate::kernel::MaterialRealizationIdentity {
        self.capture.material_realization
    }
    /// Literal native critical candidates, not transport-support boundaries.
    /// Material callers must also honor `requires_closed_material_capture`.
    pub fn event_brackets_mm(&self) -> &[[f64; 2]] {
        &self.event_brackets_mm
    }
    /// Mandatory material-plane candidates are unchanged literal events proved
    /// outside the complete source-owned correction support. Covered native
    /// events remain deferred closed-capture obligations, not extra sliver
    /// planes. An empty iterator never proves an absence of material critical
    /// strata; global material terminal brackets come from material bounds.
    pub fn material_plane_event_brackets_mm(&self) -> impl Iterator<Item = [f64; 2]> + '_ {
        self.event_brackets_mm
            .iter()
            .filter(|band| !self.requires_closed_material_capture(**band))
            .copied()
    }
    pub fn invariance(&self) -> Option<&InvariantHeightDomain> {
        self.invariance.as_ref()
    }
    pub fn definition_id(&self) -> &DefinitionId {
        &self.capture.definition_id
    }
    pub fn frame(&self) -> PlaneFrameMm {
        self.capture.frame
    }
    pub fn height_interval_mm(&self) -> [f64; 2] {
        self.capture.height_interval_mm
    }
    pub fn policy(&self) -> AnalysisPolicy {
        self.capture.policy
    }
    pub fn pose(&self) -> Option<spiling_contracts::geometry::RigidPoseMm> {
        self.capture.pose
    }
}
#[derive(Debug, Clone, Copy)]
pub struct SlabRequest {
    pub frame: PlaneFrameMm,
    pub height_interval_mm: [f64; 2],
    pub sample_height_mm: f64,
    pub native_shape_error_mm: f64,
}
/// A source occurrence's actual world-height slab and native shape policy.
#[derive(Debug)]
pub struct PlacedSlabRequest {
    pub pose: spiling_contracts::geometry::RigidPoseMm,
    pub world_height_interval_mm: [f64; 2],
    pub sample_world_z_mm: f64,
    pub native_shape_error_mm: f64,
}

#[derive(Debug)]
pub struct SlabAnalysis {
    pub(crate) section: NativeSection,
    /// No stored outer enclosure does not authorize reuse of an inward sample.
    pub(crate) occupied: Option<NativeSection>,
    pub(crate) boundary_motion_bound_mm: f64,
    pub(crate) invariance: Option<InvariantHeightDomain>,
    endpoint_strata: Vec<NativeEndpointStratum>,
    critical: Option<critical::CriticalSlabCertificate>,
    collar_complex: Option<collar::NativeCollarComplex>,
    radial_slab: Option<joint::ClosedRadialSlab>,
    endpoint_radial_products: [Option<critical::ClosedRadialMaterialProduct>; 2],
    capture: SlabCapture,
}

#[derive(Debug)]
struct SlabCapture {
    definition_id: DefinitionId,
    material_realization: crate::kernel::MaterialRealizationIdentity,
    request: SlabRequest,
    source_sample_plane: PlaneMm,
    source_sample_height_band_mm: [f64; 2],
    placed: Option<PlacedSlabCapture>,
}
impl SlabCapture {
    fn new(
        definition: &Definition,
        request: SlabRequest,
        section: &NativeSection,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        let height =
            slab::native_sample_height_band(definition, request.frame, section, work, cancel)?;
        Ok(Self {
            definition_id: definition.id().clone(),
            material_realization: crate::kernel::MaterialRealizationIdentity::new(definition),
            request,
            source_sample_plane: section.plane,
            source_sample_height_band_mm: [height.lo, height.hi],
            placed: None,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct PlacedSlabCapture {
    pose: spiling_contracts::geometry::RigidPoseMm,
    world_height_interval_mm: [f64; 2],
    sample_world_z_mm: f64,
    native_shape_error_mm: f64,
}

/// Original native boundary material present only at a closed slab endpoint.
/// Its private constructor discharges the full face cover and paired original
/// coedge incidence; it never claims a finite empty-prism Hausdorff distance.
#[derive(Debug)]
pub struct NativeEndpointStratum {
    definition_id: DefinitionId,
    material_realization: crate::kernel::MaterialRealizationIdentity,
    face_id: spiling_contracts::geometry::FaceId,
    face_index: usize,
    request: SlabRequest,
    placed: Option<PlacedSlabCapture>,
    endpoint: NativeOwnedEndpoint,
    height_mm: f64,
    source_position_bounds_mm: [[f64; 2]; 3],
    world_xy_bounds_mm: Option<[[f64; 2]; 2]>,
    outward_height_sign: i8,
    patch_cover: Vec<[[f64; 2]; 2]>,
}
impl NativeEndpointStratum {
    pub(crate) fn material_realization_identity(
        &self,
    ) -> crate::kernel::MaterialRealizationIdentity {
        self.material_realization
    }
    pub fn definition_id(&self) -> &DefinitionId {
        &self.definition_id
    }
    pub fn face_id(&self) -> &spiling_contracts::geometry::FaceId {
        &self.face_id
    }
    pub fn face_index(&self) -> usize {
        self.face_index
    }
    pub fn endpoint(&self) -> NativeOwnedEndpoint {
        self.endpoint
    }
    pub fn outward_height_sign(&self) -> i8 {
        self.outward_height_sign
    }
    pub fn placed_pose(&self) -> Option<spiling_contracts::geometry::RigidPoseMm> {
        self.placed.map(|capture| capture.pose)
    }
    pub fn world_height_interval_mm(&self) -> Option<[f64; 2]> {
        self.placed.map(|capture| capture.world_height_interval_mm)
    }
    pub fn sample_world_z_mm(&self) -> Option<f64> {
        self.placed.map(|capture| capture.sample_world_z_mm)
    }
    pub fn world_shape_error_policy_mm(&self) -> Option<f64> {
        self.placed.map(|capture| capture.native_shape_error_mm)
    }
    pub fn world_height_mm(&self) -> Option<f64> {
        self.placed.map(|capture| {
            capture.world_height_interval_mm[match self.endpoint {
                NativeOwnedEndpoint::Lower => 0,
                NativeOwnedEndpoint::Upper => 1,
            }]
        })
    }
    pub fn world_xy_bounds_mm(&self) -> Option<[[f64; 2]; 2]> {
        self.world_xy_bounds_mm
    }
    pub fn retained_bytes(&self) -> usize {
        self.definition_id
            .allocated_bytes()
            .saturating_add(self.face_id.allocated_bytes())
            .saturating_add(
                self.patch_cover
                    .capacity()
                    .saturating_mul(std::mem::size_of::<[[f64; 2]; 2]>()),
            )
    }
}
impl SlabAnalysis {
    pub(crate) fn material_realization_identity(
        &self,
    ) -> crate::kernel::MaterialRealizationIdentity {
        self.capture.material_realization
    }
    pub(crate) fn matches_original_material(&self, definition: &Definition) -> bool {
        self.definition_id() == definition.id()
            && self.capture.material_realization
                == crate::kernel::MaterialRealizationIdentity::new(definition)
    }
    pub fn section(&self) -> &NativeSection {
        &self.section
    }
    pub fn occupied(&self) -> Option<&NativeSection> {
        self.occupied.as_ref()
    }
    pub fn source_sample_plane(&self) -> PlaneMm {
        self.capture.source_sample_plane
    }
    pub fn source_sample_height_band_mm(&self) -> [f64; 2] {
        self.capture.source_sample_height_band_mm
    }
    /// Symmetric native MATERIAL-CORE/prism error, not necessarily full closed
    /// material. A regular continuation certificate bounds the regular-core
    /// closure. The one-sided critical fallback instead uses C intersected with
    /// sweep(actual native filled sample). Presence of a collar alone does not
    /// identify these cores. Missing transition material/original endpoint
    /// strata require independent material-facing native incidence and coverage.
    pub fn material_core_shape_error_mm(&self) -> f64 {
        self.boundary_motion_bound_mm
    }
    pub fn closed_shape_error_mm(&self) -> Option<f64> {
        (!self.has_transition_material()).then_some(self.boundary_motion_bound_mm)
    }
    pub fn endpoint_strata(&self) -> &[NativeEndpointStratum] {
        &self.endpoint_strata
    }
    pub fn has_critical_collar(&self) -> bool {
        self.collar_complex.is_some()
    }
    /// Global native-source convexity, proved from the complete original
    /// boundary and exact chart gluing, not from sample-loop counts. An actual
    /// admitted affine pose preserves it; this immutable capture binds the
    /// source proof and the posed slab used by downstream joint analysis.
    pub fn has_native_convex_section_topology_proof(&self) -> bool {
        self.critical.as_ref().is_some_and(|proof| {
            let a = proof.request();
            let b = self.capture.request;
            proof.definition_id() == &self.capture.definition_id
                && proof.material_realization_identity() == self.capture.material_realization
                && a.frame == b.frame
                && a.height_interval_mm == b.height_interval_mm
                && a.sample_height_mm.to_bits() == b.sample_height_mm.to_bits()
                && a.native_shape_error_mm.to_bits() == b.native_shape_error_mm.to_bits()
                && proof.has_native_convex_section_topology_proof()
        })
    }
    /// A fixed sampled source M is certified over this entire CLOSED request
    /// by a complete native material cylinder, full world metric and exact
    /// polygon calibration. This does not assert equal source sections, extend
    /// the captured interval, or authorize reuse of realized D.
    pub fn has_closed_radial_material_product(&self) -> bool {
        self.radial_slab.is_some()
    }
    pub fn has_transition_material(&self) -> bool {
        self.collar_complex.is_some() || !self.endpoint_strata.is_empty()
    }
    /// One-sided candidate prism to the original CLOSED native source material.
    /// This alone never establishes the reverse direction or feature incidence.
    pub fn sample_to_closed_source_shape_error_mm(&self) -> f64 {
        self.boundary_motion_bound_mm
    }
    /// Conservatively account all retained result buffers, counting the
    /// critical proof's one internally shared capture only once.
    pub fn retained_bytes(&self) -> usize {
        let mut bytes = self
            .section
            .retained_bytes()
            .saturating_add(self.capture.definition_id.allocated_bytes());
        if let Some(occupied) = &self.occupied {
            bytes = bytes.saturating_add(occupied.retained_bytes());
        }
        if let Some(invariance) = &self.invariance {
            bytes = bytes.saturating_add(invariance.definition_id.allocated_bytes());
        }
        if let Some(critical) = &self.critical {
            bytes = bytes.saturating_add(critical.retained_bytes());
        }
        if let Some(complex) = &self.collar_complex {
            bytes = bytes.saturating_add(complex.retained_bytes());
        }
        if let Some(radial) = &self.radial_slab {
            bytes = bytes.saturating_add(std::mem::size_of_val(radial));
        }
        bytes = bytes.saturating_add(std::mem::size_of_val(&self.endpoint_radial_products));
        bytes = bytes.saturating_add(
            self.endpoint_strata
                .capacity()
                .saturating_mul(std::mem::size_of::<NativeEndpointStratum>()),
        );
        self.endpoint_strata.iter().fold(bytes, |bytes, stratum| {
            bytes.saturating_add(stratum.retained_bytes())
        })
    }
    pub fn invariance(&self) -> Option<&InvariantHeightDomain> {
        self.invariance.as_ref()
    }
    /// Consume the slab proof after a consumer has obtained any required joint
    /// certificate. Geometry remains immutable until each section is consumed.
    pub fn into_parts(self) -> (NativeSection, Option<NativeSection>) {
        (self.section, self.occupied)
    }
    /// Source-frame event neighborhoods discharged by the whole-slab proof.
    /// For posed analyses use `placed_pose` and the captured world request as
    /// well; these are never silently relabeled as world-height roots.
    pub fn event_neighborhoods(&self) -> &[CriticalEventNeighborhood] {
        self.critical
            .as_ref()
            .map_or(&[], |proof| proof.event_neighborhoods.as_slice())
    }
    pub fn critical_material_component_count(&self) -> Option<usize> {
        self.critical
            .as_ref()
            .map(|proof| proof.material_component_count())
    }
    pub fn critical_bounded_void_count(&self) -> Option<usize> {
        self.critical
            .as_ref()
            .map(|proof| proof.bounded_void_count())
    }
    pub fn critical_tunnel_cycle_count(&self) -> Option<usize> {
        self.critical
            .as_ref()
            .map(|proof| proof.tunnel_cycle_count())
    }
    pub fn definition_id(&self) -> &DefinitionId {
        &self.capture.definition_id
    }
    pub fn request(&self) -> SlabRequest {
        self.capture.request
    }
    pub fn placed_pose(&self) -> Option<spiling_contracts::geometry::RigidPoseMm> {
        self.capture.placed.map(|capture| capture.pose)
    }
    pub fn world_height_interval_mm(&self) -> Option<[f64; 2]> {
        self.capture
            .placed
            .map(|capture| capture.world_height_interval_mm)
    }
    pub fn sample_world_z_mm(&self) -> Option<f64> {
        self.capture.placed.map(|capture| capture.sample_world_z_mm)
    }
    pub fn world_shape_error_policy_mm(&self) -> Option<f64> {
        self.capture
            .placed
            .map(|capture| capture.native_shape_error_mm)
    }
}

#[cfg(test)]
mod work_tests {
    use super::*;
    #[test]
    fn cancellation_does_not_discard_completed_native_work() {
        let mut work = GeometryWorkBudget::new(10);
        assert_eq!(
            work.charge(6, &AtomicBool::new(true)).unwrap_err().code,
            GeometryErrorCode::Cancelled
        );
        assert_eq!(work.consumed(), 6);
        assert_eq!(work.remaining(), 4);
        assert_eq!(
            work.charge(5, &AtomicBool::new(true)).unwrap_err().code,
            GeometryErrorCode::Cancelled
        );
        assert_eq!(work.consumed(), 6);
        assert_eq!(
            work.charge(5, &AtomicBool::new(false)).unwrap_err().code,
            GeometryErrorCode::ResourceLimit
        );
    }

    #[test]
    fn concurrent_atomic_debiting_preserves_total_accounting() {
        use std::sync::Arc;
        let budget = Arc::new(GeometryWorkBudget::new(100_000));
        let cancel = Arc::new(AtomicBool::new(false));
        let mut handles = Vec::new();
        for _ in 0..10 {
            let b = Arc::clone(&budget);
            let c = Arc::clone(&cancel);
            handles.push(std::thread::spawn(move || {
                for _ in 0..1_000 {
                    b.charge(10, &c).unwrap();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(budget.consumed(), 100_000);
        assert_eq!(budget.remaining(), 0);
        assert_eq!(
            budget.charge(1, &cancel).unwrap_err().code,
            GeometryErrorCode::ResourceLimit
        );
    }
}
