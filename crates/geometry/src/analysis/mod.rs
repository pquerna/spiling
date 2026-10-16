// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

mod bounds;
mod collar;
mod critical;
mod joint;
mod placed;
mod planar;
mod slab;
mod source_incidence;
mod voids;

use crate::{Definition, GeometryError, NativeSection, PlaneFrameMm, PlaneMm, check_cancel};
use spiling_contracts::geometry::{DefinitionId, GeometryErrorCode};
use std::sync::atomic::AtomicBool;

pub use bounds::projected_bounds;
pub use collar::{
    NativeCollarHeightSpace, NativeCollarPort, NativeCriticalCollar, NativeCriticalCollarCell,
    NativeCriticalCollarMaterialTarget, NativeCriticalCollarPolicy, NativeCriticalCollarTarget,
    NativeCriticalCollarTargetSide, certify_native_critical_collar,
};
pub use critical::CriticalEventNeighborhood;
pub use joint::{
    CompletedNativeUnionProofSet, EndpointOwnershipRequest, NativeCriticalCollarIncidence,
    NativeInterfaceAttachment, NativeOwnedEndpoint, NativeSourceLeafProofKind,
    NativeSourceLeafReceipt, NativeUnionBindingLedger, NativeUnionBoundaryEdge,
    NativeUnionBoundarySource, NativeUnionCoverage, NativeUnionOccurrence,
    NativeUnionOccurrenceEvidence, NativeUnionPolicy, NativeUnionProofKind,
    NativeUnionSourceCapture, PendingNativeUnionProof, SourceLeafRequest,
    certify_endpoint_ownership, certify_endpoint_stratum_ownership,
    certify_native_collar_incidence, certify_native_one_sided_source_leaf,
    certify_native_source_leaf, certify_native_union,
};
pub use placed::{analyze_placed_height_domain, placed_bounds};
pub(crate) use placed::{analyze_placed_slab, section_placed_with_work};
pub use planar::{
    PlanarCapsuleClearance, PlanarCapsuleClearancePolicy, PlanarCapsuleMm,
    certify_planar_capsule_clearance, certify_planar_capsule_pair_clearance,
};
pub use slab::analyze_height_domain;
pub(crate) use slab::{HeightFunctional, analyze_height_domain_enclosed, analyze_slab};
pub use voids::{
    NativeProjectedVoids, NativeProtectedVoidEnvelope, NativeVoidProjectionCapture,
    ProtectedVoidHeightScope, ProtectedVoidPolicy, capture_projected_voids,
    projected_protected_void_envelopes,
};

/// Cumulative numerical work across a captured geometry job. Reusing a cache or
/// subdividing an interval never resets this counter.
#[derive(Debug)]
pub struct GeometryWorkBudget {
    remaining: u64,
    consumed: u64,
}
impl GeometryWorkBudget {
    pub fn new(maximum_work: u64) -> Self {
        Self {
            remaining: maximum_work,
            consumed: 0,
        }
    }
    pub fn consumed(&self) -> u64 {
        self.consumed
    }
    pub(crate) fn remaining(&self) -> u64 {
        self.remaining
    }
    pub(crate) fn charge(&mut self, amount: u64, cancel: &AtomicBool) -> Result<(), GeometryError> {
        if amount > self.remaining {
            check_cancel(cancel)?;
            return Err(GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "cumulative geometry work exhausted",
            ));
        }
        self.remaining -= amount;
        self.consumed += amount;
        // Debit completed native work even when cancellation arrived inside it.
        check_cancel(cancel)
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
    frame: PlaneFrameMm,
    height_interval_mm: [f64; 2],
    pose: Option<spiling_contracts::geometry::RigidPoseMm>,
}
impl InvariantHeightDomain {
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

#[derive(Debug)]
struct HeightCapture {
    definition_id: DefinitionId,
    frame: PlaneFrameMm,
    height_interval_mm: [f64; 2],
    policy: AnalysisPolicy,
    pose: Option<spiling_contracts::geometry::RigidPoseMm>,
}
impl HeightAnalysis {
    pub fn event_brackets_mm(&self) -> &[[f64; 2]] {
        &self.event_brackets_mm
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
    capture: SlabCapture,
}

#[derive(Debug)]
struct SlabCapture {
    definition_id: DefinitionId,
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
                && a.frame == b.frame
                && a.height_interval_mm == b.height_interval_mm
                && a.sample_height_mm.to_bits() == b.sample_height_mm.to_bits()
                && a.native_shape_error_mm.to_bits() == b.native_shape_error_mm.to_bits()
                && proof.has_native_convex_section_topology_proof()
        })
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

pub(crate) enum SectionRegularity {
    Empty,
    Regular,
}

/// Completeness is established independently of native Boolean search seeds.
pub(crate) fn section_regularity(
    source: crate::section::OriginalTrimSource<'_, '_>,
    plane: &PlaneMm,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SectionRegularity, GeometryError> {
    check_cancel(cancel)?;
    let frame = PlaneFrameMm::from_plane(*plane)?;
    let support = bounds::projected_bounds_with_scratch_limit(
        source,
        plane.normal,
        BoundsPolicy {
            maximum_endpoint_uncertainty_mm:
                spiling_contracts::geometry::SECTION_BOOLEAN_TOLERANCE_MM,
        },
        remaining_bytes,
        work,
        cancel,
    )
    .map_err(|mut error| {
        error.message = format!("section regularity / projected bounds: {}", error.message);
        error
    })?;
    let offset = crate::parametric::dot_point(plane.origin_mm, plane.normal);
    let tolerance = spiling_contracts::geometry::SECTION_BOOLEAN_TOLERANCE_MM;
    if support.minimum[0] > offset.hi + tolerance || support.maximum[1] < offset.lo - tolerance {
        return Ok(SectionRegularity::Empty);
    }
    let height = slab::analyze_height_domain_with_scratch_limit(
        source,
        frame,
        [-tolerance, tolerance],
        AnalysisPolicy {
            maximum_height_uncertainty_mm: tolerance,
            maximum_shape_error_mm: spiling_contracts::geometry::SECTION_SAMPLING_TOLERANCE_MM,
        },
        remaining_bytes,
        work,
        cancel,
    )
    .map_err(|mut error| {
        error.message = format!(
            "section regularity / height-event census: {}",
            error.message
        );
        error
    })?;
    if height
        .proved_event_brackets_mm
        .iter()
        .any(|event| event[0] >= -tolerance && event[1] <= tolerance)
    {
        return Err(GeometryError::new(
            GeometryErrorCode::DegenerateSection,
            "section plane meets a proved native critical stratum within native tolerance",
        ));
    }
    if height
        .event_brackets_mm
        .iter()
        .any(|event| event[0] <= tolerance && event[1] >= -tolerance)
    {
        return Err(GeometryError::new(
            GeometryErrorCode::AnalysisIndeterminate,
            "section intersects an unresolved or critical height bracket",
        ));
    }
    Ok(SectionRegularity::Regular)
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
}
