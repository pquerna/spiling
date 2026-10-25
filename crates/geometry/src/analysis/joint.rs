// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Joint source-bound union evidence. Uncertainty-separated native components
//! are certified independently; every unresolved pair stays in one bounded
//! interaction component. Interacting components require exact native
//! orthogonal products, while isolated ordinary native families retain their
//! actual frozen section graph. Polygon proximity or independent occurrence
//! invariance never establishes a crossing of native curves.

use super::collar::{
    NativeCollarComplex, NativeCollarHeightSpace, NativeCriticalCollar, NativeCriticalCollarTarget,
    NativeCriticalCollarTargetSide,
};
use super::{
    BoundsPolicy, GeometryWorkBudget, NativeEndpointStratum, ProjectedBounds, SlabAnalysis,
    placed_bounds,
};
use crate::parametric::{Interval, down, up};
use crate::{Definition, GeometryError, SectionLoopMm};
use sha2::{Digest, Sha256};
use spiling_contracts::geometry::{
    DefinitionId, FaceId, GeometryErrorCode, OccurrenceId, RigidPoseMm,
};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

mod planar;
use planar::{exact_cycle_mapping, feature_clearance};
mod radial_region;
use radial_region::certify_actual_polygon_radial_region;
mod radial_metric;
pub(super) use radial_metric::{
    ClosedRadialSlab, certify_closed_radial_slab, seal_closed_radial_slab_from_complete_metric,
};
mod inner_prism;
pub use inner_prism::NativeInnerMaterialPrism;

const MAX_SCRATCH: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct NativeUnionPolicy {
    /// Requested coordinate budget, independently checked against actual
    /// candidate geometry. Overlay work/rounding estimates are NOT evidence.
    pub candidate_coordinate_error_mm: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum NativeSourceLeafProofKind {
    ClosedSlab,
    MaterialCore,
    SampleToClosedSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeUnionCoverage {
    DisjointSources,
    InvariantUnion,
    NativeInterfaceComplex,
}

pub enum NativeUnionOccurrenceEvidence<'a> {
    Slab {
        analysis: &'a SlabAnalysis,
        leaf: &'a NativeSourceLeafReceipt,
        endpoint_ownerships: &'a [NativeEndpointOwnership],
    },
    SlabWithCollar {
        analysis: &'a SlabAnalysis,
        leaf: &'a NativeSourceLeafReceipt,
        endpoint_ownerships: &'a [NativeEndpointOwnership],
        collar: &'a NativeCriticalCollarIncidence,
    },
    EndpointOwned(&'a NativeEndpointOwnership),
}
pub struct NativeUnionOccurrence<'a, 'd> {
    pub occurrence_id: OccurrenceId,
    pub source: &'a crate::PreparedSolid<'d>,
    pub pose: RigidPoseMm,
    pub evidence: NativeUnionOccurrenceEvidence<'a>,
}
#[derive(Clone, Copy)]
struct SlabOccurrence<'a, 'd> {
    occurrence_index: usize,
    source: &'a crate::PreparedSolid<'d>,
    pose: RigidPoseMm,
    analysis: &'a SlabAnalysis,
    source_metric: NativeSourceLeafProofKind,
    leaf: Option<&'a NativeSourceLeafReceipt>,
}

#[derive(Debug)]
struct NativeUnionSourceIdentity {
    occurrence_id: OccurrenceId,
    definition_id: DefinitionId,
    material_realization: crate::kernel::MaterialRealizationIdentity,
    pose: RigidPoseMm,
}
#[derive(Debug, Clone)]
pub struct NativeUnionSourceCapture {
    identity: Arc<NativeUnionSourceIdentity>,
    native_shape_error_policy_mm: f64,
}
impl NativeUnionSourceCapture {
    pub(crate) fn material_realization_identity(
        &self,
    ) -> crate::kernel::MaterialRealizationIdentity {
        self.identity.material_realization
    }
    fn matches_original_material(&self, definition: &Definition) -> bool {
        self.definition_id() == definition.id()
            && self.identity.material_realization
                == crate::kernel::MaterialRealizationIdentity::new(definition)
    }
    fn same_material_recipe(&self, other: &Self) -> bool {
        self.definition_id() == other.definition_id()
            && self.identity.material_realization == other.identity.material_realization
    }
    pub fn occurrence_id(&self) -> OccurrenceId {
        self.identity.occurrence_id
    }
    pub fn definition_id(&self) -> &DefinitionId {
        &self.identity.definition_id
    }
    pub fn pose(&self) -> RigidPoseMm {
        self.identity.pose
    }
    pub fn native_shape_error_policy_mm(&self) -> f64 {
        self.native_shape_error_policy_mm
    }
}

/// Compact final-candidate authority; no source, matrix or contour is retained.
#[derive(Debug, Clone, Copy, PartialEq)]
struct NativeRadialLeaf {
    product_sha256: [u8; 32],
    axis_sha256: [u8; 32],
    kind: super::critical::RadialRegionKind,
    center: [[f64; 2]; 2],
    outer_kernel_radius_mm: f64,
    inner_cover_radius_mm: f64,
    inner_displacement_mm: Option<f64>,
}

/// Independent CLOSED continuation authority for one regular endpoint. Its
/// domain need not reach the opposite (possibly critical) coface of the leaf.
#[derive(Debug, Clone, Copy, PartialEq)]
struct NativeEndpointRadialLeaf {
    radial: NativeRadialLeaf,
    world_height_interval_mm: [f64; 2],
    source_height_interval_mm: [f64; 2],
}

/// An independently proved source-M material CORE contributor. With no extra
/// outgoing endpoint strata this also covers the CLOSED slab; otherwise only
/// the core closure is claimed. No contour coordinates are retained.
#[derive(Debug, Clone)]
pub struct NativeSourceLeafReceipt {
    source: NativeUnionSourceCapture,
    world_height_interval_mm: [f64; 2],
    sample_world_z_mm: f64,
    candidate_geometry_sha256: [u8; 32],
    candidate_xy_bounds_mm: Option<[[f64; 2]; 2]>,
    native_sample_sha256: [u8; 32],
    achieved_shape_error_mm: f64,
    policy: NativeUnionPolicy,
    positive: bool,
    has_owned_transition_material: bool,
    proof_kind: NativeSourceLeafProofKind,
    sample_region_error_mm: f64,
    sample_coordinate_error_mm: Option<f64>,
    critical_interfaces: [bool; 2],
    native_endpoint_closure: [bool; 2],
    regular_boundary_tube_mm: Option<f64>,
    native_world_z_invariant: bool,
    native_convex_section_topology: bool,
    radial_product: Option<NativeRadialLeaf>,
    endpoint_radial_products: [Option<NativeEndpointRadialLeaf>; 2],
    sample_loops: Arc<Vec<NativeSourceLoopCapture>>,
    /// Whole distinct-cycle boundary clearance of the registered actual sample,
    /// a LOWER bound, not a kernel radius or a source-motion estimate.
    sample_loop_clearance_mm: Option<f64>,
    rectangle: Option<Arc<NativeRectangleLeafWitness>>,
}
#[derive(Debug)]
struct NativeSourceLoopCapture {
    hole: bool,
    source_face_ordinals: Vec<usize>,
    shape: NativeLoopShape,
    registration: NativeLoopRegistration,
}
/// Derived algebraic witnesses of actual M, never a replacement source curve.
/// Each kernel is checked against EVERY oriented edge supporting halfplane.
#[derive(Debug)]
struct NativeLoopShape {
    xy_sha256: [u8; 32],
    bounds: [[f64; 2]; 2],
    center: [f64; 2],
    cover_radius_mm: f64,
    kernel_radius_mm: f64,
    kernel_box: Option<[[f64; 2]; 2]>,
}

/// Candidate-order metadata issued only with complete oriented native/candidate
/// cycle correspondence. The enclosing private leaf binds material, pose,
/// exact native sample and actual candidate hashes; no coordinates are retained.
#[derive(Debug, Clone, Copy, PartialEq)]
struct NativeLoopRegistration {
    native_index: usize,
    parent: Option<usize>,
}

#[derive(Default)]
struct SourceLoopRecording {
    loops: Vec<NativeLoopRegistration>,
    clearance_mm: Option<f64>,
}

/// Sealed ACTUAL footprint-pair topology, never a native homotopy inferred from
/// scalar distance. Leaf identities include source, material, pose, native
/// sample, actual ordered candidate and parent/native-index registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeRegularInterfacePairReceipt {
    lower: [u8; 32],
    upper: [u8; 32],
    cut_bits: u64,
    matching: [u8; 32],
    geometry: [u8; 32],
}
impl NativeSourceLeafReceipt {
    pub(crate) fn material_realization_identity(
        &self,
    ) -> crate::kernel::MaterialRealizationIdentity {
        self.source.material_realization_identity()
    }
    pub(crate) fn matches_original_material(&self, definition: &Definition) -> bool {
        self.source.matches_original_material(definition)
    }
    pub fn occurrence_id(&self) -> OccurrenceId {
        self.source.occurrence_id()
    }
    pub fn definition_id(&self) -> &DefinitionId {
        self.source.definition_id()
    }
    pub fn pose(&self) -> RigidPoseMm {
        self.source.pose()
    }
    pub fn world_height_interval_mm(&self) -> [f64; 2] {
        self.world_height_interval_mm
    }
    pub fn sample_world_z_mm(&self) -> f64 {
        self.sample_world_z_mm
    }
    pub fn native_sample_sha256(&self) -> &[u8; 32] {
        &self.native_sample_sha256
    }
    pub fn is_positive(&self) -> bool {
        self.positive
    }
    pub fn candidate_geometry_sha256(&self) -> &[u8; 32] {
        &self.candidate_geometry_sha256
    }
    pub fn proof_kind(&self) -> NativeSourceLeafProofKind {
        self.proof_kind
    }
    pub fn directed_sample_to_closed_source_error_mm(&self) -> f64 {
        self.achieved_shape_error_mm
    }
    pub fn material_core_shape_error_mm(&self) -> Option<f64> {
        (self.proof_kind != NativeSourceLeafProofKind::SampleToClosedSource)
            .then_some(self.achieved_shape_error_mm)
    }
    pub fn closed_shape_error_mm(&self) -> Option<f64> {
        (self.proof_kind == NativeSourceLeafProofKind::ClosedSlab)
            .then_some(self.achieved_shape_error_mm)
    }
    /// Symmetric filled-polygon correspondence to this leaf's frozen native
    /// sample polygons. This is not an exact-section or CLOSED-slab cover.
    /// The independently proved rectangle lane does not retain this authority.
    pub(super) fn sample_polygon_coordinate_error_mm(&self) -> Option<f64> {
        self.sample_coordinate_error_mm
    }
    pub fn native_shape_error_policy_mm(&self) -> f64 {
        self.source.native_shape_error_policy_mm()
    }
    pub fn policy(&self) -> NativeUnionPolicy {
        self.policy
    }
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(source_retained_bytes(&self.source))
            .saturating_add(
                std::mem::size_of::<Vec<NativeSourceLoopCapture>>()
                    + 2 * std::mem::size_of::<usize>(),
            )
            .saturating_add(
                self.sample_loops.capacity() * std::mem::size_of::<NativeSourceLoopCapture>(),
            )
            .saturating_add(
                self.sample_loops
                    .iter()
                    .map(|loop_| {
                        loop_.source_face_ordinals.capacity() * std::mem::size_of::<usize>()
                    })
                    .sum::<usize>(),
            )
            .saturating_add(
                self.rectangle
                    .as_ref()
                    .map_or(0, |rectangle| rectangle_retained_bytes(rectangle)),
            )
    }
    pub fn native_rectangle_source_faces(&self) -> Option<&[FaceId; 4]> {
        self.rectangle
            .as_ref()
            .map(|rectangle| &rectangle.source_faces)
    }
    pub fn native_rectangle_source_caps(&self) -> Option<&[FaceId; 2]> {
        self.rectangle
            .as_ref()
            .map(|rectangle| &rectangle.source_caps)
    }
}
/// Fixed-size algebraic evidence derived from the actual four-edge source-M
/// contributor. These are certified supporting halfplanes, not copied contours.
#[derive(Debug)]
struct NativeRectangleLeafWitness {
    bounds: [[f64; 2]; 2],
    /// Inward bracket in which the original native product contains material.
    world_material_height_interval_mm: [f64; 2],
    actual_material_left_halfplanes: [[Interval; 3]; 4],
    source_faces: [FaceId; 4],
    source_caps: [FaceId; 2],
    source_sides: [u8; 4],
    support_residuals: [[f64; 2]; 2],
    height_supports: [[f64; 2]; 2],
    coordinate_error_mm: f64,
}
fn rectangle_retained_bytes(rectangle: &NativeRectangleLeafWitness) -> usize {
    std::mem::size_of::<NativeRectangleLeafWitness>()
        + 2 * std::mem::size_of::<usize>()
        + rectangle
            .source_faces
            .iter()
            .chain(&rectangle.source_caps)
            .map(|face| face.as_str().len())
            .sum::<usize>()
}
#[derive(Debug)]
pub struct NativeInterfaceAttachment {
    endpoint_occurrence_id: OccurrenceId,
    current_occurrence_id: OccurrenceId,
    endpoint_leaf_sha256: [u8; 32],
    current_leaf_sha256: [u8; 32],
    native_axis_events: [[f64; 4]; 2],
    native_axis_event_counts: [usize; 2],
    native_cell_incidence: [u8; 9],
    endpoint_product: Arc<NativeRectangleLeafWitness>,
    endpoint_cap_index: usize,
}
impl NativeInterfaceAttachment {
    pub fn endpoint_occurrence_id(&self) -> OccurrenceId {
        self.endpoint_occurrence_id
    }
    pub fn current_occurrence_id(&self) -> OccurrenceId {
        self.current_occurrence_id
    }
    pub fn endpoint_leaf_sha256(&self) -> &[u8; 32] {
        &self.endpoint_leaf_sha256
    }
    pub fn current_leaf_sha256(&self) -> &[u8; 32] {
        &self.current_leaf_sha256
    }
    /// Rounded representatives of exact dyadic native supports. The sealed
    /// residuals prove strict ordering/coincidence; achieved error bounds the
    /// representative grid's displacement from the true native grid.
    pub fn native_axis_events(&self) -> [[f64; 4]; 2] {
        self.native_axis_events
    }
    pub fn native_axis_event_counts(&self) -> [usize; 2] {
        self.native_axis_event_counts
    }
    /// Bits 1/2 are exact source occupancy, 3 their positive-area attachment.
    pub fn native_cell_incidence(&self) -> [u8; 9] {
        self.native_cell_incidence
    }
    pub fn endpoint_source_cap_face_id(&self) -> &FaceId {
        &self.endpoint_product.source_caps[self.endpoint_cap_index]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeOwnedEndpoint {
    Lower,
    Upper,
}

/// Possible boundary-only material owned by a genuinely certified adjacent
/// closed slab. This is neither an empty SlabAnalysis nor a finite local
/// Hausdorff(empty, contact) claim. Contact existence itself is not asserted.
#[derive(Debug, Clone)]
pub struct NativeEndpointOwnership {
    source: NativeUnionSourceCapture,
    world_height_interval_mm: [f64; 2],
    sample_world_z_mm: f64,
    endpoint: NativeOwnedEndpoint,
    source_world_z_support: Option<ProjectedBounds>,
    occupied_xy_bounds_mm: [[f64; 2]; 2],
    adjacent: NativeSourceLeafReceipt,
    stratum: Option<Arc<NativeStratumSourceCapture>>,
}
impl NativeEndpointOwnership {
    pub(crate) fn material_realization_identity(
        &self,
    ) -> crate::kernel::MaterialRealizationIdentity {
        self.source.material_realization_identity()
    }
    pub fn occurrence_id(&self) -> OccurrenceId {
        self.source.occurrence_id()
    }
    pub fn definition_id(&self) -> &DefinitionId {
        self.source.definition_id()
    }
    pub fn pose(&self) -> RigidPoseMm {
        self.source.pose()
    }
    pub fn world_height_interval_mm(&self) -> [f64; 2] {
        self.world_height_interval_mm
    }
    pub fn sample_world_z_mm(&self) -> f64 {
        self.sample_world_z_mm
    }
    pub fn native_shape_error_policy_mm(&self) -> f64 {
        self.source.native_shape_error_policy_mm()
    }
    pub fn endpoint(&self) -> NativeOwnedEndpoint {
        self.endpoint
    }
    pub fn source_world_z_support(&self) -> Option<ProjectedBounds> {
        self.source_world_z_support
    }
    pub fn occupied_xy_bounds_mm(&self) -> [[f64; 2]; 2] {
        self.occupied_xy_bounds_mm
    }
    pub fn adjacent_receipt(&self) -> &NativeSourceLeafReceipt {
        &self.adjacent
    }
    pub fn stratum_face_id(&self) -> Option<&FaceId> {
        self.stratum.as_ref().map(|capture| &capture.face_id)
    }
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + source_retained_bytes(&self.source)
            + self.adjacent.retained_bytes()
            + self.stratum.as_ref().map_or(0, |capture| {
                std::mem::size_of::<NativeStratumSourceCapture>()
                    + 2 * std::mem::size_of::<usize>()
                    + capture.face_id.as_str().len()
            })
    }
}
#[derive(Debug)]
struct NativeStratumSourceCapture {
    face_id: FaceId,
    face_index: usize,
    world_height_mm: f64,
    outward_height_sign: i8,
}

/// Source-native critical cofaces AND independently proved actual-M glue cells.
/// The metric collar alone is deliberately not an incidence certificate.
#[derive(Debug, Clone)]
pub struct NativeCriticalCollarIncidence {
    inner: Arc<NativeCriticalCollarIncidenceCapture>,
}
#[derive(Debug)]
struct NativeCriticalCollarIncidenceCapture {
    current: NativeSourceLeafReceipt,
    collar: NativeCriticalCollar,
    dependencies: Vec<NativeSourceLeafReceipt>,
    empty_dependencies: Vec<NativeOwnedEndpoint>,
    occupied_xy_bounds_mm: [[f64; 2]; 2],
    proved_interfaces: [bool; 2],
    incidence_sha256: [u8; 32],
}
impl NativeCriticalCollarIncidence {
    pub fn current_leaf(&self) -> &NativeSourceLeafReceipt {
        &self.inner.current
    }
    pub fn native_collar(&self) -> &NativeCriticalCollar {
        &self.inner.collar
    }
    pub fn used_source_leaves(&self) -> &[NativeSourceLeafReceipt] {
        &self.inner.dependencies
    }
    pub fn incidence_sha256(&self) -> &[u8; 32] {
        &self.inner.incidence_sha256
    }
    pub fn occupied_xy_bounds_mm(&self) -> [[f64; 2]; 2] {
        self.inner.occupied_xy_bounds_mm
    }
    /// Consume exactly this incidence's frozen native slab for lazy support
    /// exclusion. The closed-source C0 projection protects every possible
    /// bounded complement component only inside the captured CLOSED world
    /// interval. It establishes no Hole lineage, absence, regularity, occupied
    /// material or stronger source-leaf proof kind.
    pub fn capture_projected_voids(
        &self,
        analysis: SlabAnalysis,
        byte_limit: u32,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<super::NativeVoidProjectionCapture>, GeometryError> {
        work.charge(1, cancel)?;
        let current = self.current_leaf();
        let collar = self.native_collar();
        let request = analysis.request();
        let captured = collar.source_request();
        let exact_request = request.height_interval_mm.map(f64::to_bits)
            == captured.height_interval_mm.map(f64::to_bits)
            && request.sample_height_mm.to_bits() == captured.sample_height_mm.to_bits()
            && request.native_shape_error_mm.to_bits() == captured.native_shape_error_mm.to_bits()
            && request.frame.origin_mm.map(f64::to_bits)
                == captured.frame.origin_mm.map(f64::to_bits)
            && request.frame.x_axis.map(f64::to_bits) == captured.frame.x_axis.map(f64::to_bits)
            && request.frame.y_axis.map(f64::to_bits) == captured.frame.y_axis.map(f64::to_bits)
            && request.frame.z_axis.map(f64::to_bits) == captured.frame.z_axis.map(f64::to_bits);
        let exact_pose = analysis.placed_pose().is_some_and(|pose| {
            pose.translation_mm.map(f64::to_bits) == current.pose().translation_mm.map(f64::to_bits)
                && pose.rotation_xyzw.map(f64::to_bits)
                    == current.pose().rotation_xyzw.map(f64::to_bits)
        });
        if !exact_request
            || !exact_pose
            || analysis.definition_id() != current.definition_id()
            || analysis.material_realization_identity() != current.material_realization_identity()
            || analysis
                .world_height_interval_mm()
                .map(|v| v.map(f64::to_bits))
                != Some(current.world_height_interval_mm().map(f64::to_bits))
            || analysis.sample_world_z_mm().map(f64::to_bits)
                != Some(current.sample_world_z_mm().to_bits())
            || analysis.world_shape_error_policy_mm().map(f64::to_bits)
                != Some(current.native_shape_error_policy_mm().to_bits())
            || current.native_sample_sha256() != &native_sample_hash(&analysis, work, cancel)?
            || !analysis.collar_complex.as_ref().is_some_and(|complex| {
                complex.height_space() == NativeCollarHeightSpace::World
                    && complex.definition_id() == current.definition_id()
                    && complex.material_realization_identity()
                        == current.material_realization_identity()
                    && collar.complex_sha256() == &complex.native_complex_sha256()
            })
            || (current.is_positive()
                && !collar.material_targets().iter().any(|target| {
                    target.side() == NativeCriticalCollarTargetSide::Current
                        && same_leaf(target.leaf(), current)
                }))
        {
            return Err(error(
                GeometryErrorCode::InvalidGeometry,
                "closed complement capture changed its accepted source/pose/slab/sample/collar/current-M binding",
            ));
        }
        super::voids::capture_closed_source_projected_voids(
            analysis,
            collar.closed_source_complement_xy_bounds_mm(),
            current.world_height_interval_mm(),
            byte_limit,
            work,
            cancel,
        )
    }
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + std::mem::size_of::<NativeCriticalCollarIncidenceCapture>()
            + 2 * std::mem::size_of::<usize>()
            + self.inner.current.retained_bytes()
            + self.inner.collar.retained_bytes()
            + self.inner.dependencies.capacity() * std::mem::size_of::<NativeSourceLeafReceipt>()
            + self
                .inner
                .dependencies
                .iter()
                .map(NativeSourceLeafReceipt::retained_bytes)
                .sum::<usize>()
            + self.inner.empty_dependencies.capacity() * std::mem::size_of::<NativeOwnedEndpoint>()
    }
}
struct CollarMaterial<'a> {
    side: NativeCriticalCollarTargetSide,
    leaf: &'a NativeSourceLeafReceipt,
    rings: Vec<Ring>,
    owners: Vec<usize>,
}
fn collar_side(
    leaf: &NativeSourceLeafReceipt,
    current: &NativeSourceLeafReceipt,
) -> Result<NativeCriticalCollarTargetSide, GeometryError> {
    let interval = current.world_height_interval_mm();
    if same_leaf(leaf, current) {
        Ok(NativeCriticalCollarTargetSide::Current)
    } else if leaf.world_height_interval_mm()[1] == interval[0] {
        Ok(NativeCriticalCollarTargetSide::Lower)
    } else if leaf.world_height_interval_mm()[0] == interval[1] {
        Ok(NativeCriticalCollarTargetSide::Upper)
    } else {
        Err(error(
            GeometryErrorCode::InvalidGeometry,
            "critical incidence target is neither current nor immediately adjacent actual M",
        ))
    }
}
fn point_in_ring(
    ring: &Ring,
    p: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let mut winding = 0_i64;
    for i in 0..ring.points.len() {
        work.charge(1, cancel)?;
        let a = ring.points[i];
        let b = ring.points[(i + 1) % ring.points.len()];
        let upward = a[1] <= p[1] && p[1] < b[1];
        let downward = b[1] <= p[1] && p[1] < a[1];
        if !upward && !downward {
            continue;
        }
        let determinant = cross(delta(b, a), delta(p, a));
        if determinant.contains_zero() {
            return Err(error(
                GeometryErrorCode::AnalysisIndeterminate,
                "actual-M winding has an unresolved dyadic ray/edge incidence",
            ));
        }
        if upward && determinant.lo > 0.0 {
            winding += 1;
        }
        if downward && determinant.hi < 0.0 {
            winding -= 1;
        }
    }
    Ok(winding != 0)
}
fn prepare_collar_material<'a>(
    side: NativeCriticalCollarTargetSide,
    leaf: &'a NativeSourceLeafReceipt,
    actual: &[SectionLoopMm],
    scratch: &mut Scratch,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<CollarMaterial<'a>, GeometryError> {
    verify_source_leaf_candidate(leaf, actual, work, cancel)?;
    if actual.len() != leaf.sample_loops.len() {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "critical actual-M loop lineage differs from its source leaf",
        ));
    }
    let mut rings = Vec::new();
    scratch.reserve(&mut rings, actual.len(), work, cancel)?;
    for loop_ in actual {
        rings.push(ring(loop_, scratch, work, cancel)?);
    }
    let mut owners = Vec::new();
    scratch.reserve(&mut owners, rings.len(), work, cancel)?;
    for (index, r) in rings.iter().enumerate() {
        let mut owner = None;
        for (candidate, outer) in rings.iter().enumerate() {
            if candidate == index || outer.hole {
                continue;
            }
            if point_in_ring(outer, r.points[0], work, cancel)? {
                if !r.hole || owner.is_some() {
                    return Err(error(
                        GeometryErrorCode::AnalysisIndeterminate,
                        "critical interface needs nonnested material components and uniquely owned native holes",
                    ));
                }
                owner = Some(candidate);
            }
        }
        owners.push(if r.hole {
            owner.ok_or_else(|| {
                error(
                    GeometryErrorCode::InvalidGeometry,
                    "critical source-M hole has no enclosing component",
                )
            })?
        } else {
            index
        });
    }
    Ok(CollarMaterial {
        side,
        leaf,
        rings,
        owners,
    })
}
fn loop_kernel_contains(
    ring: &Ring,
    p: [f64; 2],
    radius: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    for i in 0..ring.points.len() {
        work.charge(1, cancel)?;
        let a = ring.points[i];
        let b = ring.points[(i + 1) % ring.points.len()];
        let direction = delta(b, a);
        let length = direction[0].square().add(direction[1].square()).sqrt()?.hi;
        let determinant = cross(direction, delta(p, a));
        if !length.is_finite() || !determinant.lo.is_finite() || !determinant.hi.is_finite() {
            return Ok(false);
        }
        let signed = if ring.hole {
            -determinant.hi
        } else {
            determinant.lo
        };
        if signed <= Interval::point(radius).mul(Interval::point(length)).hi {
            return Ok(false);
        }
    }
    Ok(true)
}
fn common_kernel(
    a: &Ring,
    b: &Ring,
    radius: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    // Candidate points are heuristics only. ALL actual edge halfplanes are
    // subsequently checked with outward arithmetic; no sampled authority.
    for ring in [a, b] {
        let mut bounds = [[f64::INFINITY, f64::NEG_INFINITY]; 2];
        for p in &ring.points {
            work.charge(1, cancel)?;
            for axis in 0..2 {
                bounds[axis][0] = bounds[axis][0].min(p[axis]);
                bounds[axis][1] = bounds[axis][1].max(p[axis]);
            }
        }
        let center = std::array::from_fn(|axis| 0.5 * bounds[axis][0] + 0.5 * bounds[axis][1]);
        if loop_kernel_contains(a, center, radius, work, cancel)?
            && loop_kernel_contains(b, center, radius, work, cancel)?
        {
            return Ok(true);
        }
        for i in 0..ring.points.len() {
            let p = ring.points[i];
            let q = ring.points[(i + 1) % ring.points.len()];
            let dx = q[0] - p[0];
            let dy = q[1] - p[1];
            let length = dx.hypot(dy);
            if !length.is_finite() || length == 0.0 {
                continue;
            }
            let sign = if ring.hole { -1.0 } else { 1.0 };
            for distance in [2.0 * radius, 4.0 * radius, length * 0.125, length * 0.25] {
                work.charge(1, cancel)?;
                if distance <= 0.0 || !distance.is_finite() {
                    continue;
                }
                let point = [
                    0.5 * p[0] + 0.5 * q[0] - sign * dy / length * distance,
                    0.5 * p[1] + 0.5 * q[1] + sign * dx / length * distance,
                ];
                if loop_kernel_contains(a, point, radius, work, cancel)?
                    && loop_kernel_contains(b, point, radius, work, cancel)?
                {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}
fn ring_interiors_separated(
    a: &Ring,
    b: &Ring,
    radius: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    for i in 0..a.points.len() {
        for j in 0..b.points.len() {
            work.charge(1, cancel)?;
            if segment_gap(
                a.points[i],
                a.points[(i + 1) % a.points.len()],
                b.points[j],
                b.points[(j + 1) % b.points.len()],
            ) <= radius
            {
                return Ok(false);
            }
        }
    }
    Ok(!point_in_ring(a, b.points[0], work, cancel)?
        && !point_in_ring(b, a.points[0], work, cancel)?)
}
fn port_loop(
    material: &CollarMaterial<'_>,
    face: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<usize>, GeometryError> {
    let mut result = None;
    for (index, loop_) in material.leaf.sample_loops.iter().enumerate() {
        work.charge(1, cancel)?;
        if loop_.source_face_ordinals.binary_search(&face).is_ok() {
            if result.is_some() {
                return Err(error(
                    GeometryErrorCode::AnalysisIndeterminate,
                    "native port original neighbor face owns multiple branches; an exact native branch key is required",
                ));
            }
            result = Some(index);
        }
    }
    Ok(result)
}

/// Critical connection bands keep their original coface owner; their proved
/// heights never replace the original port/coface height.
fn native_collar_interfaces(
    complex: &NativeCollarComplex,
    interval: [f64; 2],
    sample: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[bool; 2], GeometryError> {
    let side = |height: [f64; 2]| {
        if height[1] < sample[0] {
            Some(0)
        } else if height[0] > sample[1] {
            Some(1)
        } else {
            None
        }
    };
    let mut interfaces = [false; 2];
    for (_, height, _) in complex.event_faces() {
        work.charge(1, cancel)?;
        let interface = side(height).ok_or_else(|| {
            error(
                GeometryErrorCode::AnalysisIndeterminate,
                "actual common native sample crosses a critical coface band; refine/resample the common slab",
            )
        })?;
        interfaces[interface] = true;
    }
    for band in complex.boundary_critical_bands() {
        work.charge(1, cancel)?;
        let height = band.height_interval_mm();
        if !height.into_iter().all(f64::is_finite)
            || height[0] > height[1]
            || height[0] < interval[0]
            || height[1] > interval[1]
        {
            return Err(error(
                GeometryErrorCode::InvalidGeometry,
                "native boundary-critical band is outside its closed request",
            ));
        }
        let mut owner = None;
        for (face, owner_height, sign) in complex.event_faces() {
            work.charge(1, cancel)?;
            if face == band.event_face() && sign == band.outward_height_sign() {
                owner = side(owner_height);
                break;
            }
        }
        let interface = side(height).ok_or_else(|| {
            error(
                GeometryErrorCode::AnalysisIndeterminate,
                "actual common native sample crosses a boundary-critical connection; refine/resample the common slab",
            )
        })?;
        if owner != Some(interface) {
            return Err(error(
                GeometryErrorCode::InvalidGeometry,
                "native boundary-critical connection lost its original same-side coface owner",
            ));
        }
        interfaces[interface] = true;
    }
    Ok(interfaces)
}

/// Prove actual-M glue, not merely Hausdorff closeness. The floor/ceiling lane
/// needs unchanged uniquely attributed outer branches and a forest of native
/// void-port correspondences. Common strict star-kernels prove contractible
/// intersections; hole containment and forbidden-pair separation prove the
/// entire incidence graph. Primitive whole-component birth/death is supported
/// against a genuinely absent accepted source-M contribution.
pub fn certify_native_collar_incidence(
    analysis: &SlabAnalysis,
    current: &NativeSourceLeafReceipt,
    collar: &NativeCriticalCollar,
    inputs: &[NativeCriticalCollarTarget<'_>],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<NativeCriticalCollarIncidence, GeometryError> {
    work.charge(1, cancel)?;
    let complex = analysis.collar_complex.as_ref().ok_or_else(|| {
        error(
            GeometryErrorCode::InvalidGeometry,
            "critical incidence requires an actual original native collar complex",
        )
    })?;
    if current.native_sample_sha256() != &native_sample_hash(analysis, work, cancel)?
        || current.definition_id() != collar.definition_id()
        || current.material_realization_identity() != collar.material_realization_identity()
        || current.material_realization_identity() != analysis.material_realization_identity()
        || current.material_realization_identity() != complex.material_realization_identity()
        || current.pose() != collar.placed_pose()
        || current.world_height_interval_mm() != collar.world_height_interval_mm()
        || current.sample_world_z_mm() != collar.sample_world_z_mm()
        || current.native_shape_error_policy_mm() != collar.world_shape_error_policy_mm()
        || collar.native_sample_sha256() != current.native_sample_sha256()
        || collar.complex_sha256() != &complex.native_complex_sha256()
        || complex.height_space() != NativeCollarHeightSpace::World
    {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "critical incidence source/pose/native-complex/sample/policy capture mismatch",
        ));
    }
    let mut scratch = Scratch {
        bytes: collar.retained_bytes() + current.retained_bytes(),
    };
    let mut materials = Vec::new();
    scratch.reserve(&mut materials, 3, work, cancel)?;
    if inputs.len() != collar.material_targets().len() {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "critical incidence needs actual M of every captured target, including metric-unused port alternatives",
        ));
    }
    for input in inputs {
        work.charge(1, cancel)?;
        if input.leaf.occurrence_id() != current.occurrence_id()
            || input.leaf.definition_id() != current.definition_id()
            || input.leaf.material_realization_identity() != current.material_realization_identity()
            || input.leaf.pose() != current.pose()
            || !input.leaf.is_positive()
        {
            return Err(error(
                GeometryErrorCode::InvalidGeometry,
                "critical incidence target is not this occurrence's positive source M",
            ));
        }
        let side = collar_side(input.leaf, current)?;
        if materials
            .iter()
            .any(|m: &CollarMaterial<'_>| m.side == side)
            || !collar
                .material_targets()
                .iter()
                .any(|target| same_leaf(target.leaf(), input.leaf))
        {
            return Err(error(
                GeometryErrorCode::InvalidGeometry,
                "critical target is duplicated or differs from native metric capture",
            ));
        }
        materials.push(prepare_collar_material(
            side,
            input.leaf,
            input.actual_material,
            &mut scratch,
            work,
            cancel,
        )?);
    }
    if current.is_positive()
        && !materials
            .iter()
            .any(|m| m.side == NativeCriticalCollarTargetSide::Current)
    {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "critical incidence requires actual CURRENT positive source-M geometry",
        ));
    }
    if !current.is_positive() {
        verify_source_leaf_candidate(current, &[], work, cancel)?;
    }
    let mut dependencies = Vec::new();
    let mut empty_dependencies = Vec::new();
    for target in collar.used_material_targets() {
        scratch.push(&mut dependencies, target.leaf().clone(), work, cancel)?;
    }
    let mut digest = Sha256::new();
    digest.update(b"spiling-native-critical-source-m-interface-1\0");
    digest.update(collar.complex_sha256());
    hash_leaf(&mut digest, current);
    let sample = collar.actual_sample_world_height_interval_mm();
    let used_interfaces = native_collar_interfaces(
        complex,
        current.world_height_interval_mm(),
        sample,
        work,
        cancel,
    )?;
    for (interface, used) in used_interfaces.into_iter().enumerate() {
        if !used {
            continue;
        }
        let before_side = if interface == 0 {
            NativeCriticalCollarTargetSide::Lower
        } else {
            NativeCriticalCollarTargetSide::Current
        };
        let after_side = if interface == 0 {
            NativeCriticalCollarTargetSide::Current
        } else {
            NativeCriticalCollarTargetSide::Upper
        };
        let before = materials.iter().find(|m| m.side == before_side);
        let after = materials.iter().find(|m| m.side == after_side);
        let n = before.map_or(0, |m| m.rings.len());
        let m = after.map_or(0, |m| m.rings.len());
        let size = n.checked_mul(m).ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "critical incidence capacity overflow",
            )
        })?;
        let mut holes = Vec::new();
        scratch.reserve(&mut holes, size, work, cancel)?;
        holes.resize(size, false);
        let mut before_ported = Vec::new();
        scratch.reserve(&mut before_ported, n, work, cancel)?;
        before_ported.resize(n, false);
        let mut after_ported = Vec::new();
        scratch.reserve(&mut after_ported, m, work, cancel)?;
        after_ported.resize(m, false);
        if let (Some(a), Some(b)) = (before, after) {
            for (i, left) in a.leaf.sample_loops.iter().enumerate() {
                for (j, right) in b.leaf.sample_loops.iter().enumerate() {
                    work.charge(1, cancel)?;
                    if left.hole
                        && right.hole
                        && !left.source_face_ordinals.is_empty()
                        && left.source_face_ordinals == right.source_face_ordinals
                    {
                        holes[i * m + j] = true;
                    }
                }
            }
        }
        for (face, height, sign) in complex.event_faces() {
            let this_interface = if height[1] < sample[0] { 0 } else { 1 };
            if this_interface != interface {
                continue;
            }
            let mut left = Vec::new();
            let mut right = Vec::new();
            let mut ports = 0_usize;
            for port in collar
                .native_event_ports()
                .iter()
                .filter(|port| port.event_face_ordinal() == face)
            {
                work.charge(1, cancel)?;
                ports += 1;
                if port.height_space() != NativeCollarHeightSpace::World {
                    return Err(error(
                        GeometryErrorCode::InvalidGeometry,
                        "critical port was not recertified in actual world height",
                    ));
                }
                let a = before
                    .map(|value| port_loop(value, port.neighbor_face_ordinal(), work, cancel))
                    .transpose()?
                    .flatten();
                let b = after
                    .map(|value| port_loop(value, port.neighbor_face_ordinal(), work, cancel))
                    .transpose()?
                    .flatten();
                if a.is_some() && b.is_some() {
                    return Err(error(
                        GeometryErrorCode::AnalysisIndeterminate,
                        "original critical port neighbor appears on both sides; exact branch continuation is ambiguous",
                    ));
                }
                if a.is_none() && b.is_none() {
                    return Err(error(
                        GeometryErrorCode::AnalysisIndeterminate,
                        "critical native incidence port needs an actual lower/upper positive leaf on its original source branch",
                    ));
                }
                if let Some(index) = a {
                    before_ported[index] = true;
                    if !left.contains(&index) {
                        scratch.push(&mut left, index, work, cancel)?;
                    }
                }
                if let Some(index) = b {
                    after_ported[index] = true;
                    if !right.contains(&index) {
                        scratch.push(&mut right, index, work, cancel)?;
                    }
                }
                digest.update((port.event_face_ordinal() as u64).to_le_bytes());
                for ordinal in port
                    .event_coedge_ordinal()
                    .into_iter()
                    .chain([port.neighbor_face_ordinal()])
                    .chain(port.neighbor_coedge_ordinal())
                {
                    digest.update((ordinal as u64).to_le_bytes());
                }
                digest.update([u8::from(port.original_orientation())]);
                digest.update(port.outward_height_sign().to_le_bytes());
                for value in port.height_interval_mm() {
                    digest.update(value.to_bits().to_le_bytes());
                }
                digest.update((a.unwrap_or(usize::MAX) as u64).to_le_bytes());
                digest.update((b.unwrap_or(usize::MAX) as u64).to_le_bytes());
            }
            if ports == 0 {
                return Err(error(
                    GeometryErrorCode::AnalysisIndeterminate,
                    "original critical coface has no native coedge port graph",
                ));
            }
            if let (Some(a), Some(b)) = (before, after) {
                if left.iter().any(|&index| !a.rings[index].hole)
                    || right.iter().any(|&index| !b.rings[index].hole)
                {
                    return Err(error(
                        GeometryErrorCode::AnalysisIndeterminate,
                        "critical material-component attachment needs native arrangement beyond the floor/ceiling hole lane",
                    ));
                }
                if left.is_empty() && sign != 1 || right.is_empty() && sign != -1 {
                    return Err(error(
                        GeometryErrorCode::AnalysisIndeterminate,
                        "critical coface material/void-side orientation disagrees with actual source M",
                    ));
                }
                for &i in &left {
                    for &j in &right {
                        holes[i * m + j] = true;
                    }
                }
            } else {
                let birth = before.is_none() && after.is_some() && sign == -1;
                let death = after.is_none() && before.is_some() && sign == 1;
                if !birth && !death {
                    return Err(error(
                        GeometryErrorCode::AnalysisIndeterminate,
                        "void birth/death needs a positive material-facing leaf; metric cover alone cannot realize a native floor",
                    ));
                }
            }
        }
        if let (Some(a), Some(b)) = (before, after) {
            let radius = Interval::point(a.leaf.sample_region_error_mm)
                .add(Interval::point(b.leaf.sample_region_error_mm))
                .add(Interval::point(a.leaf.policy.candidate_coordinate_error_mm))
                .add(Interval::point(b.leaf.policy.candidate_coordinate_error_mm))
                .hi;
            for (i, outer_a) in a.rings.iter().enumerate().filter(|(_, r)| !r.hole) {
                let source_a = &a.leaf.sample_loops[i].source_face_ordinals;
                let mut matches = 0;
                for (j, outer_b) in b.rings.iter().enumerate().filter(|(_, r)| !r.hole) {
                    work.charge(1, cancel)?;
                    let source_b = &b.leaf.sample_loops[j].source_face_ordinals;
                    if !source_a.is_empty() && source_a == source_b {
                        matches += 1;
                        if !common_kernel(outer_a, outer_b, radius, work, cancel)? {
                            return Err(error(
                                GeometryErrorCode::AnalysisIndeterminate,
                                "actual native-M outer intersection has no strict common star-kernel",
                            ));
                        }
                        for (_, hole) in a
                            .rings
                            .iter()
                            .enumerate()
                            .filter(|(k, r)| r.hole && a.owners[*k] == i)
                        {
                            for &point in &hole.points {
                                if !loop_kernel_contains(outer_b, point, radius, work, cancel)? {
                                    return Err(error(
                                        GeometryErrorCode::AnalysisIndeterminate,
                                        "actual source-M hole crosses/approaches the adjacent outer boundary",
                                    ));
                                }
                            }
                        }
                        for (_, hole) in b
                            .rings
                            .iter()
                            .enumerate()
                            .filter(|(k, r)| r.hole && b.owners[*k] == j)
                        {
                            for &point in &hole.points {
                                if !loop_kernel_contains(outer_a, point, radius, work, cancel)? {
                                    return Err(error(
                                        GeometryErrorCode::AnalysisIndeterminate,
                                        "actual source-M hole crosses/approaches the previous outer boundary",
                                    ));
                                }
                            }
                        }
                    } else if !ring_interiors_separated(outer_a, outer_b, radius, work, cancel)? {
                        return Err(error(
                            GeometryErrorCode::AnalysisIndeterminate,
                            "actual critical interface joins distinct original material branches",
                        ));
                    }
                }
                if matches != 1 {
                    return Err(error(
                        GeometryErrorCode::AnalysisIndeterminate,
                        "native outer branch correspondence is missing or ambiguous",
                    ));
                }
            }
            for (j, _) in b.rings.iter().enumerate().filter(|(_, r)| !r.hole) {
                let mut matches = 0;
                for (i, _) in a.rings.iter().enumerate().filter(|(_, r)| !r.hole) {
                    work.charge(1, cancel)?;
                    matches += usize::from(
                        a.leaf.sample_loops[i].source_face_ordinals
                            == b.leaf.sample_loops[j].source_face_ordinals,
                    );
                }
                if matches != 1 {
                    return Err(error(
                        GeometryErrorCode::AnalysisIndeterminate,
                        "critical interface changes unsupported material component branches",
                    ));
                }
            }
            let mut parents = Vec::new();
            scratch.reserve(&mut parents, n + m, work, cancel)?;
            parents.extend(0..n + m);
            for (i, left) in a.rings.iter().enumerate().filter(|(_, r)| r.hole) {
                let mut matched = false;
                for (j, right) in b.rings.iter().enumerate().filter(|(_, r)| r.hole) {
                    if holes[i * m + j] {
                        matched = true;
                        if !common_kernel(left, right, radius, work, cancel)? {
                            return Err(error(
                                GeometryErrorCode::AnalysisIndeterminate,
                                "actual native void-port intersection has no strict common star-kernel",
                            ));
                        }
                        let mut root_a = i;
                        while parents[root_a] != root_a {
                            work.charge(1, cancel)?;
                            root_a = parents[root_a];
                        }
                        let mut root_b = n + j;
                        while parents[root_b] != root_b {
                            work.charge(1, cancel)?;
                            root_b = parents[root_b];
                        }
                        if root_a == root_b {
                            return Err(error(
                                GeometryErrorCode::AnalysisIndeterminate,
                                "void-port incidence cycle needs full native handle evidence",
                            ));
                        }
                        parents[root_b] = root_a;
                    } else if !ring_interiors_separated(left, right, radius, work, cancel)? {
                        return Err(error(
                            GeometryErrorCode::AnalysisIndeterminate,
                            "actual source-M interface joins unrelated native void ports",
                        ));
                    }
                }
                if !matched && !before_ported[i] {
                    return Err(error(
                        GeometryErrorCode::AnalysisIndeterminate,
                        "disappearing void lacks original critical coface ownership",
                    ));
                }
            }
            for (j, _) in b.rings.iter().enumerate().filter(|(_, r)| r.hole) {
                if !(0..n).any(|i| holes[i * m + j]) && !after_ported[j] {
                    return Err(error(
                        GeometryErrorCode::AnalysisIndeterminate,
                        "appearing void lacks original critical coface ownership",
                    ));
                }
            }
            for value in &holes {
                digest.update([u8::from(*value)]);
            }
        } else {
            let positive = before.or(after).ok_or_else(|| {
                error(
                    GeometryErrorCode::AnalysisIndeterminate,
                    "critical material is not realized by any positive source-M leaf",
                )
            })?;
            let ported = if before.is_some() {
                &before_ported
            } else {
                &after_ported
            };
            if ported.iter().any(|value| !*value) {
                return Err(error(
                    GeometryErrorCode::AnalysisIndeterminate,
                    "missing adjacent positive leaf for a continuing native material/void branch",
                ));
            }
            let absent = if before.is_none() {
                before_side
            } else {
                after_side
            };
            if absent != NativeCriticalCollarTargetSide::Current {
                let endpoint = if absent == NativeCriticalCollarTargetSide::Lower {
                    NativeOwnedEndpoint::Lower
                } else {
                    NativeOwnedEndpoint::Upper
                };
                if !empty_dependencies.contains(&endpoint) {
                    scratch.push(&mut empty_dependencies, endpoint, work, cancel)?;
                }
            } else if current.is_positive() {
                return Err(error(
                    GeometryErrorCode::InvalidGeometry,
                    "positive CURRENT M was omitted",
                ));
            }
            hash_leaf(&mut digest, positive.leaf);
        }
        for material in [before, after].into_iter().flatten() {
            if !dependencies
                .iter()
                .any(|leaf| same_leaf(leaf, material.leaf))
            {
                scratch.push(&mut dependencies, material.leaf.clone(), work, cancel)?;
            }
            hash_leaf(&mut digest, material.leaf);
        }
    }
    if !used_interfaces.into_iter().any(|value| value) {
        return Err(error(
            GeometryErrorCode::AnalysisIndeterminate,
            "collar has no native coface incidence",
        ));
    }
    let mut occupied_xy_bounds_mm = collar.occupied_xy_bounds_mm();
    for material in &materials {
        if dependencies
            .iter()
            .any(|leaf| same_leaf(leaf, material.leaf))
        {
            for ring in &material.rings {
                for point in &ring.points {
                    work.charge(1, cancel)?;
                    for axis in 0..2 {
                        occupied_xy_bounds_mm[axis][0] =
                            occupied_xy_bounds_mm[axis][0].min(point[axis]);
                        occupied_xy_bounds_mm[axis][1] =
                            occupied_xy_bounds_mm[axis][1].max(point[axis]);
                    }
                }
            }
        }
    }
    for side in &empty_dependencies {
        digest.update([match side {
            NativeOwnedEndpoint::Lower => 0,
            NativeOwnedEndpoint::Upper => 1,
        }]);
    }
    let policy = collar.policy();
    for value in [
        policy.maximum_xy_cover_error_mm,
        policy.maximum_distance_mm,
        collar.sample_to_closed_source_error_mm(),
        collar.closed_source_to_material_error_mm(),
        collar.achieved_xy_cover_error_mm(),
    ] {
        digest.update(value.to_bits().to_le_bytes());
    }
    digest.update(policy.byte_limit.to_le_bytes());
    for cell in collar.projected_cover() {
        work.charge(1, cancel)?;
        digest.update((cell.source_face_ordinal().unwrap_or(usize::MAX) as u64).to_le_bytes());
        digest.update((cell.material_target_index() as u64).to_le_bytes());
        for axis in cell.world_xy_bounds_mm() {
            for value in axis {
                digest.update(value.to_bits().to_le_bytes());
            }
        }
        for value in cell
            .world_height_interval_mm()
            .into_iter()
            .chain([cell.xy_distance_mm(), cell.material_distance_mm()])
        {
            digest.update(value.to_bits().to_le_bytes());
        }
        if let Some(uv) = cell.source_uv() {
            for axis in uv {
                for value in axis {
                    digest.update(value.to_bits().to_le_bytes());
                }
            }
        }
        if let Some((port, range)) = cell.native_port_parameter() {
            digest.update((port as u64).to_le_bytes());
            for value in range {
                digest.update(value.to_bits().to_le_bytes());
            }
        }
    }
    scratch.owned(
        std::mem::size_of::<NativeCriticalCollarIncidenceCapture>()
            + 2 * std::mem::size_of::<usize>(),
        work,
        cancel,
    )?;
    Ok(NativeCriticalCollarIncidence {
        inner: Arc::new(NativeCriticalCollarIncidenceCapture {
            current: current.clone(),
            collar: collar.clone(),
            dependencies,
            empty_dependencies,
            occupied_xy_bounds_mm,
            proved_interfaces: used_interfaces,
            incidence_sha256: digest.finalize().into(),
        }),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeUnionProofKind {
    SeparatedNativeSlabTubes,
    /// Exact native orthogonal interaction components, possibly accompanied by
    /// separately certified native components. Coverage reports invariance only
    /// when the complete actual source union has that sealed authority.
    ExactNativeRectangularArrangement,
}

/// Attribution is obtained from the original source face occurrence, never from
/// nearest sampled edges. Several exactly coincident native supports can own an
/// arrangement edge. Indices refer to this certificate's source capture order.
#[derive(Debug)]
pub struct NativeUnionBoundarySource {
    pub boundary_edge_index: usize,
    pub occurrence_index: usize,
    pub face_id: FaceId,
}

/// A genuine joint proof whose adjacent source-leaf obligations still must be
/// bound to accepted positive contributors by NativeUnionBindingLedger. It is
/// NOT a completed stream certificate, even when its metric getters succeed.
#[derive(Debug)]
pub struct PendingNativeUnionProof {
    sources: Vec<NativeUnionSourceCapture>,
    world_height_interval_mm: [f64; 2],
    sample_world_z_mm: f64,
    candidate_geometry_sha256: [u8; 32],
    achieved_shape_error_mm: f64,
    material_core_shape_error_mm: Option<f64>,
    raw_leaf_to_candidate_error_mm: f64,
    preserved_source_occurrences: Vec<OccurrenceId>,
    target_envelopes: Vec<NativeTargetEnvelope>,
    material_component_count: usize,
    bounded_void_count: usize,
    proof_kind: NativeUnionProofKind,
    policy: NativeUnionPolicy,
    boundary_edges: Vec<NativeUnionBoundaryEdge>,
    boundary_sources: Vec<NativeUnionBoundarySource>,
    endpoint_dependencies: Vec<NativeEndpointOwnership>,
    positive_leaves: Vec<NativeSourceLeafReceipt>,
    interface_attachments: Vec<NativeInterfaceAttachment>,
    collar_dependencies: Vec<NativeCriticalCollarIncidence>,
    regular_interfaces: Vec<NativeRegularInterfacePairReceipt>,
    owned_critical_slab_count: usize,
    requires_interface_complex: bool,
}
impl PendingNativeUnionProof {
    /// Qualify the entire ACTUAL lower/current footprint overlap while both
    /// immutable buffers are borrowed. No contours are retained. `byte_limit`
    /// is TOTAL pending ownership plus temporary proof storage; borrowed caller
    /// storage is external. False issues no authority and retains legacy gates.
    pub fn certify_regular_interface_pair(
        &mut self,
        lower: NativeCriticalCollarTarget<'_>,
        upper: NativeCriticalCollarTarget<'_>,
        byte_limit: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        work.charge(1, cancel)?;
        let (before, after) = (lower.leaf, upper.leaf);
        let cut = before.world_height_interval_mm[1];
        if !before.is_positive()
            || !after.is_positive()
            || before.occurrence_id() != after.occurrence_id()
            || !before.source.same_material_recipe(&after.source)
            || before.pose() != after.pose()
            || cut.to_bits() != after.world_height_interval_mm[0].to_bits()
            || self.world_height_interval_mm != after.world_height_interval_mm
        {
            return Err(error(
                GeometryErrorCode::InvalidGeometry,
                "actual regular pair does not bind the same source/pose and exact adjacent closed cut",
            ));
        }
        let mut current = false;
        for leaf in &self.positive_leaves {
            work.charge(1, cancel)?;
            current |= same_leaf(leaf, after);
        }
        if !current {
            return Err(error(
                GeometryErrorCode::InvalidGeometry,
                "actual regular pair current leaf is foreign to its pending source proof",
            ));
        }
        verify_source_leaf_candidate(before, lower.actual_material, work, cancel)?;
        verify_source_leaf_candidate(after, upper.actual_material, work, cancel)?;
        // The independent original critical/coface/collar route remains owner
        // at either critical shared side; a sample pair cannot promote it.
        if before.critical_interfaces[1] || after.critical_interfaces[0] {
            return Ok(false);
        }
        let limit = byte_limit.min(MAX_SCRATCH);
        let resident = self.retained_bytes();
        if resident > limit {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                "actual pair proof has no total pending/temporary byte allowance",
            ));
        }
        let mut scratch = Scratch {
            bytes: MAX_SCRATCH - limit + resident,
        };
        let Some(matching) = regular_loop_matching(before, after, &mut scratch, work, cancel)?
        else {
            return Ok(false);
        };
        work.charge(
            (before.sample_loops.len() + after.sample_loops.len() + 2) as u64,
            cancel,
        )?;
        let lower_hash = leaf_fingerprint(before);
        let upper_hash = leaf_fingerprint(after);
        let matching_hash = matching_fingerprint(&matching);
        for receipt in &self.regular_interfaces {
            work.charge(1, cancel)?;
            if receipt.lower == lower_hash
                && receipt.upper == upper_hash
                && receipt.cut_bits == cut.to_bits()
                && receipt.matching == matching_hash
            {
                return Ok(true);
            }
        }
        let Some(geometry) = planar::actual_pair_collar(
            lower.actual_material,
            upper.actual_material,
            &matching,
            &mut scratch,
            work,
            cancel,
        )?
        else {
            return Ok(false);
        };
        let receipt = NativeRegularInterfacePairReceipt {
            lower: lower_hash,
            upper: upper_hash,
            cut_bits: cut.to_bits(),
            matching: matching_hash,
            geometry,
        };
        if self.regular_interfaces.len() == self.regular_interfaces.capacity() {
            // Reserve admits growth. ALSO admit the old payload while a moved
            // new allocation may coexist with it; never refund numerical work.
            scratch.owned(
                self.regular_interfaces
                    .capacity()
                    .checked_mul(std::mem::size_of::<NativeRegularInterfacePairReceipt>())
                    .ok_or_else(|| {
                        error(
                            GeometryErrorCode::ResourceLimit,
                            "actual pair receipt size overflow",
                        )
                    })?,
                work,
                cancel,
            )?;
        }
        scratch.push(&mut self.regular_interfaces, receipt, work, cancel)?;
        Ok(true)
    }

    pub fn sources(&self) -> &[NativeUnionSourceCapture] {
        &self.sources
    }
    pub fn world_height_interval_mm(&self) -> [f64; 2] {
        self.world_height_interval_mm
    }
    pub fn sample_world_z_mm(&self) -> f64 {
        self.sample_world_z_mm
    }
    pub fn candidate_geometry_sha256(&self) -> &[u8; 32] {
        &self.candidate_geometry_sha256
    }
    /// Bound for the core and all adjacent-owned boundary obligations. This is
    /// not a local Hausdorff(Mcurrent, CLOSED Ncurrent) claim when strata exist.
    pub fn achieved_shape_error_mm(&self) -> f64 {
        self.achieved_shape_error_mm
    }
    pub fn material_core_shape_error_mm(&self) -> Option<f64> {
        self.material_core_shape_error_mm
    }
    /// Inclusion of every actual input source-M region in this actual output
    /// candidate's dilation, independently checked rather than overlay work.
    pub fn raw_leaf_to_candidate_error_mm(&self) -> f64 {
        self.raw_leaf_to_candidate_error_mm
    }
    pub fn closed_shape_error_mm(&self) -> Option<f64> {
        (self.endpoint_dependencies.is_empty()
            && self.collar_dependencies.is_empty()
            && self.material_core_shape_error_mm.is_some())
        .then_some(self.achieved_shape_error_mm)
    }
    pub fn material_component_count(&self) -> usize {
        self.material_component_count
    }
    pub fn bounded_void_count(&self) -> usize {
        self.bounded_void_count
    }
    pub fn proof_kind(&self) -> NativeUnionProofKind {
        self.proof_kind
    }
    pub fn boundary_sources(&self) -> &[NativeUnionBoundarySource] {
        &self.boundary_sources
    }
    /// Exposed native support-grid representatives; their exact dyadic native
    /// displacement is included in achieved error. Empty in the separated lane.
    pub fn boundary_edges(&self) -> &[NativeUnionBoundaryEdge] {
        &self.boundary_edges
    }
    pub fn policy(&self) -> NativeUnionPolicy {
        self.policy
    }
    pub fn endpoint_dependencies(&self) -> &[NativeEndpointOwnership] {
        &self.endpoint_dependencies
    }
    pub fn owned_critical_slab_count(&self) -> usize {
        self.owned_critical_slab_count
    }
    pub fn positive_source_leaves(&self) -> &[NativeSourceLeafReceipt] {
        &self.positive_leaves
    }
    pub fn interface_attachments(&self) -> &[NativeInterfaceAttachment] {
        &self.interface_attachments
    }
    pub fn critical_collars(&self) -> &[NativeCriticalCollarIncidence] {
        &self.collar_dependencies
    }
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(
                self.sources.capacity() * std::mem::size_of::<NativeUnionSourceCapture>(),
            )
            .saturating_add(
                self.sources
                    .iter()
                    .map(source_retained_bytes)
                    .sum::<usize>(),
            )
            .saturating_add(
                self.boundary_edges.capacity() * std::mem::size_of::<NativeUnionBoundaryEdge>(),
            )
            .saturating_add(
                self.boundary_sources.capacity() * std::mem::size_of::<NativeUnionBoundarySource>(),
            )
            .saturating_add(
                self.boundary_sources
                    .iter()
                    .map(|source| source.face_id.as_str().len())
                    .sum::<usize>(),
            )
            .saturating_add(
                self.endpoint_dependencies.capacity()
                    * std::mem::size_of::<NativeEndpointOwnership>(),
            )
            .saturating_add(
                self.endpoint_dependencies
                    .iter()
                    .map(NativeEndpointOwnership::retained_bytes)
                    .sum::<usize>(),
            )
            .saturating_add(
                self.positive_leaves.capacity() * std::mem::size_of::<NativeSourceLeafReceipt>(),
            )
            .saturating_add(
                self.preserved_source_occurrences.capacity() * std::mem::size_of::<OccurrenceId>(),
            )
            .saturating_add(
                self.target_envelopes.capacity() * std::mem::size_of::<NativeTargetEnvelope>(),
            )
            .saturating_add(
                self.positive_leaves
                    .iter()
                    .map(NativeSourceLeafReceipt::retained_bytes)
                    .sum::<usize>(),
            )
            .saturating_add(
                self.interface_attachments.capacity()
                    * std::mem::size_of::<NativeInterfaceAttachment>(),
            )
            .saturating_add(
                self.interface_attachments
                    .iter()
                    .map(|attachment| rectangle_retained_bytes(&attachment.endpoint_product))
                    .sum::<usize>(),
            )
            .saturating_add(
                self.collar_dependencies.capacity()
                    * std::mem::size_of::<NativeCriticalCollarIncidence>(),
            )
            .saturating_add(
                self.collar_dependencies
                    .iter()
                    .map(NativeCriticalCollarIncidence::retained_bytes)
                    .sum::<usize>(),
            )
            .saturating_add(
                self.regular_interfaces.capacity()
                    * std::mem::size_of::<NativeRegularInterfacePairReceipt>(),
            )
    }
    pub fn coverage(&self) -> NativeUnionCoverage {
        if self.requires_interface_complex {
            return NativeUnionCoverage::NativeInterfaceComplex;
        }
        match self.proof_kind {
            NativeUnionProofKind::SeparatedNativeSlabTubes => NativeUnionCoverage::DisjointSources,
            NativeUnionProofKind::ExactNativeRectangularArrangement => {
                NativeUnionCoverage::InvariantUnion
            }
        }
    }
}

fn source_retained_bytes(source: &NativeUnionSourceCapture) -> usize {
    std::mem::size_of::<NativeUnionSourceIdentity>()
        + 2 * std::mem::size_of::<usize>()
        + source.definition_id().as_str().len()
}

fn same_leaf(a: &NativeSourceLeafReceipt, b: &NativeSourceLeafReceipt) -> bool {
    a.occurrence_id() == b.occurrence_id()
        && a.source.same_material_recipe(&b.source)
        && a.pose() == b.pose()
        && a.world_height_interval_mm == b.world_height_interval_mm
        && a.sample_world_z_mm == b.sample_world_z_mm
        && a.candidate_geometry_sha256 == b.candidate_geometry_sha256
        && a.native_sample_sha256 == b.native_sample_sha256
        && a.native_shape_error_policy_mm() == b.native_shape_error_policy_mm()
        && a.achieved_shape_error_mm == b.achieved_shape_error_mm
        && a.policy.candidate_coordinate_error_mm == b.policy.candidate_coordinate_error_mm
        && a.positive == b.positive
        && a.has_owned_transition_material == b.has_owned_transition_material
        && a.proof_kind == b.proof_kind
        && a.sample_region_error_mm == b.sample_region_error_mm
        && a.sample_coordinate_error_mm == b.sample_coordinate_error_mm
        && a.critical_interfaces == b.critical_interfaces
        && a.native_endpoint_closure == b.native_endpoint_closure
        && a.regular_boundary_tube_mm == b.regular_boundary_tube_mm
        && a.native_world_z_invariant == b.native_world_z_invariant
        && a.native_convex_section_topology == b.native_convex_section_topology
        && a.radial_product == b.radial_product
        && a.endpoint_radial_products == b.endpoint_radial_products
        && a.sample_loop_clearance_mm == b.sample_loop_clearance_mm
        && a.sample_loops.len() == b.sample_loops.len()
        && a.sample_loops
            .iter()
            .zip(b.sample_loops.iter())
            .all(|(a, b)| a.registration == b.registration)
}
fn hash_source(hash: &mut Sha256, source: &NativeUnionSourceCapture) {
    hash.update(source.occurrence_id().get().to_le_bytes());
    hash.update((source.definition_id().as_str().len() as u64).to_le_bytes());
    hash.update(source.definition_id().as_str().as_bytes());
    hash.update(source.material_realization_identity().identity_bytes());
    for value in source
        .pose()
        .translation_mm
        .into_iter()
        .chain(source.pose().rotation_xyzw)
    {
        hash.update(value.to_bits().to_le_bytes());
    }
    hash.update(source.native_shape_error_policy_mm.to_bits().to_le_bytes());
}
fn hash_leaf(hash: &mut Sha256, leaf: &NativeSourceLeafReceipt) {
    hash_source(hash, &leaf.source);
    for value in leaf.world_height_interval_mm.into_iter().chain([
        leaf.sample_world_z_mm,
        leaf.achieved_shape_error_mm,
        leaf.policy.candidate_coordinate_error_mm,
    ]) {
        hash.update(value.to_bits().to_le_bytes());
    }
    hash.update([
        u8::from(leaf.positive),
        u8::from(leaf.has_owned_transition_material),
        leaf.proof_kind as u8,
    ]);
    hash.update(leaf.candidate_geometry_sha256);
    hash.update(leaf.native_sample_sha256);
    hash.update(leaf.sample_region_error_mm.to_bits().to_le_bytes());
    hash.update([u8::from(leaf.sample_coordinate_error_mm.is_some())]);
    if let Some(error) = leaf.sample_coordinate_error_mm {
        hash.update(error.to_bits().to_le_bytes());
    }
    hash.update(leaf.critical_interfaces.map(u8::from));
    hash.update(leaf.native_endpoint_closure.map(u8::from));
    hash.update([u8::from(leaf.native_world_z_invariant)]);
    hash.update([u8::from(leaf.native_convex_section_topology)]);
    hash.update([u8::from(leaf.regular_boundary_tube_mm.is_some())]);
    if let Some(bound) = leaf.regular_boundary_tube_mm {
        hash.update(bound.to_bits().to_le_bytes());
    }
    hash.update([u8::from(leaf.radial_product.is_some())]);
    if let Some(radial) = leaf.radial_product {
        hash_radial_leaf(hash, radial);
    }
    for endpoint in leaf.endpoint_radial_products {
        hash.update([u8::from(endpoint.is_some())]);
        if let Some(endpoint) = endpoint {
            hash_radial_leaf(hash, endpoint.radial);
            for value in endpoint
                .world_height_interval_mm
                .into_iter()
                .chain(endpoint.source_height_interval_mm)
            {
                hash.update(value.to_bits().to_le_bytes());
            }
        }
    }
    hash.update([u8::from(leaf.sample_loop_clearance_mm.is_some())]);
    if let Some(clearance) = leaf.sample_loop_clearance_mm {
        hash.update(clearance.to_bits().to_le_bytes());
    }
    hash.update((leaf.sample_loops.len() as u64).to_le_bytes());
    for loop_ in leaf.sample_loops.iter() {
        hash.update((loop_.registration.native_index as u64).to_le_bytes());
        hash.update([u8::from(loop_.registration.parent.is_some())]);
        if let Some(parent) = loop_.registration.parent {
            hash.update((parent as u64).to_le_bytes());
        }
    }
}

pub(super) fn leaf_fingerprint(leaf: &NativeSourceLeafReceipt) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash_leaf(&mut hash, leaf);
    hash.finalize().into()
}
fn matching_fingerprint(matching: &[usize]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update((matching.len() as u64).to_le_bytes());
    for &index in matching {
        hash.update((index as u64).to_le_bytes());
    }
    hash.finalize().into()
}

fn hash_radial_leaf(hash: &mut Sha256, radial: NativeRadialLeaf) {
    hash.update(radial.product_sha256);
    hash.update(radial.axis_sha256);
    hash.update([radial.kind as u8]);
    for value in radial
        .center
        .into_iter()
        .flatten()
        .chain([radial.outer_kernel_radius_mm, radial.inner_cover_radius_mm])
    {
        hash.update(value.to_bits().to_le_bytes());
    }
    hash.update([u8::from(radial.inner_displacement_mm.is_some())]);
    if let Some(displacement) = radial.inner_displacement_mm {
        hash.update(displacement.to_bits().to_le_bytes());
    }
}

fn radial_leaf_contact(
    previous: &NativeSourceLeafReceipt,
    current: &NativeSourceLeafReceipt,
    interface: f64,
) -> bool {
    let (Some(a), Some(b)) = (previous.radial_product, current.radial_product) else {
        return false;
    };
    previous.source.same_material_recipe(&current.source)
        && previous.pose() == current.pose()
        && previous.proof_kind == NativeSourceLeafProofKind::ClosedSlab
        && current.proof_kind == NativeSourceLeafProofKind::ClosedSlab
        && previous.world_height_interval_mm[1].to_bits() == interface.to_bits()
        && current.world_height_interval_mm[0].to_bits() == interface.to_bits()
        && a.axis_sha256 == b.axis_sha256
        && a.center == b.center
        && a.kind == b.kind
        // Same source G and exact cut bind the physical native interface;
        // this separate strict band proves positive ACTUAL polygon attachment.
        && a.inner_cover_radius_mm.max(b.inner_cover_radius_mm)
            < a.outer_kernel_radius_mm.min(b.outer_kernel_radius_mm)
}

struct NativeUnionLeafDependency {
    leaf: NativeSourceLeafReceipt,
    owner_distance_mm: f64,
    owner_policy_mm: f64,
    interface_proved: bool,
}
fn resolved_dependency_error(
    distance: f64,
    policy: f64,
    inclusion: f64,
) -> Result<f64, GeometryError> {
    let bound = Interval::point(distance).add(Interval::point(inclusion)).hi;
    if !bound.is_finite() || bound > policy {
        return Err(error(
            GeometryErrorCode::ShapeErrorExceeded,
            "native owned contribution plus independently proved actual union inclusion exceeds its source policy",
        ));
    }
    Ok(bound)
}

struct NativeBoundaryTube {
    occurrence_id: OccurrenceId,
    distance_mm: Option<f64>,
    stationary: bool,
}

fn boundary_tube_for<'a>(
    tubes: &'a [NativeBoundaryTube],
    id: OccurrenceId,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<&'a NativeBoundaryTube, GeometryError> {
    for tube in tubes {
        work.charge(1, cancel)?;
        if tube.occurrence_id == id {
            return Ok(tube);
        }
    }
    Err(error(
        GeometryErrorCode::InvalidGeometry,
        "captured original occurrence boundary-flow witness is missing",
    ))
}

fn source_event_world_height(
    analysis: &SlabAnalysis,
    source_xyz: [[f64; 2]; 3],
    row: [Interval; 3],
    translation: f64,
    source_height: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Interval, GeometryError> {
    work.charge(16, cancel)?;
    let frame = analysis.request().frame;
    let n = frame.z_axis;
    let axis = (0..3)
        .max_by(|&a, &b| n[a].abs().total_cmp(&n[b].abs()))
        .unwrap();
    let scale = row[axis].div(Interval::point(n[axis]))?;
    let mut height = scale
        .mul(Interval::new(source_height[0], source_height[1])?)
        .add(Interval::point(translation));
    for i in 0..3 {
        height = height.add(row[i].mul(Interval::point(frame.origin_mm[i])));
        if i != axis {
            let residual = row[i].sub(scale.mul(Interval::point(n[i])));
            height = height.add(
                residual.mul(
                    Interval::new(source_xyz[i][0], source_xyz[i][1])?
                        .sub(Interval::point(frame.origin_mm[i])),
                ),
            );
        }
    }
    Interval::new(height.lo, height.hi)
}
fn source_boundary_tube(
    leaf: &NativeSourceLeafReceipt,
    collars: &[NativeCriticalCollarIncidence],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<NativeBoundaryTube, GeometryError> {
    let mut tube = NativeBoundaryTube {
        occurrence_id: leaf.occurrence_id(),
        distance_mm: leaf.regular_boundary_tube_mm,
        stationary: leaf.native_world_z_invariant,
    };
    for incidence in collars {
        work.charge(1, cancel)?;
        if !same_leaf(incidence.current_leaf(), leaf) {
            continue;
        }
        let collar = incidence.native_collar();
        let Some(speed) = collar.world_regular_boundary_xy_speed_mm_per_mm() else {
            return Ok(tube);
        };
        // The actual native sample is an almost-horizontal PLANE, not an
        // assumed world-Z level. Prove transversality to the actual world-height
        // flow before using its true original regular-complement speed.
        let matrix = super::placed::rotation(leaf.pose())?;
        let columns: [[Interval; 3]; 3] =
            std::array::from_fn(|i| std::array::from_fn(|j| matrix[j][i]));
        let cross3 = |a: [Interval; 3], b: [Interval; 3]| {
            [
                a[1].mul(b[2]).sub(a[2].mul(b[1])),
                a[2].mul(b[0]).sub(a[0].mul(b[2])),
                a[0].mul(b[1]).sub(a[1].mul(b[0])),
            ]
        };
        let cofactors = [
            cross3(columns[1], columns[2]),
            cross3(columns[2], columns[0]),
            cross3(columns[0], columns[1]),
        ];
        let n = collar.source_sample_plane().normal;
        let normal: [Interval; 3] = std::array::from_fn(|axis| {
            (0..3).fold(Interval::point(0.0), |sum, i| {
                sum.add(cofactors[i][axis].mul(Interval::point(n[i])))
            })
        });
        work.charge(32, cancel)?;
        if normal[2].lo <= 0.0 {
            return Ok(tube);
        }
        let slope = normal[0]
            .square()
            .add(normal[1].square())
            .sqrt()?
            .div(normal[2])?;
        if slope.mul(Interval::point(speed)).hi >= 1.0 {
            return Ok(tube);
        }
        let sample = collar.actual_sample_world_height_interval_mm();
        let range = leaf.world_height_interval_mm();
        let height = Interval::point(range[0])
            .sub(Interval::point(sample[1]))
            .abs_upper()
            .max(
                Interval::point(range[1])
                    .sub(Interval::point(sample[0]))
                    .abs_upper(),
            );
        let bound = Interval::point(speed)
            .mul(Interval::point(height))
            .add(Interval::point(leaf.sample_region_error_mm))
            .add(Interval::point(leaf.policy.candidate_coordinate_error_mm))
            .hi;
        if bound.is_finite() {
            tube.distance_mm = Some(bound);
            tube.stationary |= speed == 0.0;
        }
        return Ok(tube);
    }
    Ok(tube)
}
fn shape_kernel_contains(
    outer: &NativeLoopShape,
    inner: &NativeLoopShape,
    margin: f64,
) -> Result<bool, GeometryError> {
    if outer.kernel_radius_mm > 0.0
        && Interval::point(distance_upper(outer.center, inner.center)?)
            .add(Interval::point(inner.cover_radius_mm))
            .add(Interval::point(margin))
            .hi
            < outer.kernel_radius_mm
    {
        return Ok(true);
    }
    Ok(outer.kernel_box.is_some_and(|kernel| {
        (0..2).all(|axis| {
            Interval::point(kernel[axis][0])
                .add(Interval::point(margin))
                .hi
                < inner.bounds[axis][0]
                && Interval::point(kernel[axis][1])
                    .sub(Interval::point(margin))
                    .lo
                    > inner.bounds[axis][1]
        })
    }))
}
fn shape_kernels_overlap(
    a: &NativeLoopShape,
    b: &NativeLoopShape,
    margin: f64,
) -> Result<bool, GeometryError> {
    let left = Interval::point(a.kernel_radius_mm)
        .sub(Interval::point(margin))
        .lo;
    let right = Interval::point(b.kernel_radius_mm)
        .sub(Interval::point(margin))
        .lo;
    if left > 0.0
        && right > 0.0
        && distance_upper(a.center, b.center)?
            < Interval::point(left).add(Interval::point(right)).lo
    {
        return Ok(true);
    }
    Ok(match (a.kernel_box, b.kernel_box) {
        (Some(a), Some(b)) => (0..2).all(|axis| {
            Interval::point(a[axis][0])
                .add(Interval::point(margin))
                .hi
                .max(Interval::point(b[axis][0]).add(Interval::point(margin)).hi)
                < Interval::point(a[axis][1])
                    .sub(Interval::point(margin))
                    .lo
                    .min(Interval::point(b[axis][1]).sub(Interval::point(margin)).lo)
        }),
        _ => false,
    })
}
fn shape_regions_separated(
    a: &NativeLoopShape,
    b: &NativeLoopShape,
    margin: f64,
) -> Result<bool, GeometryError> {
    if bounds_separated(a.bounds, b.bounds, margin) {
        return Ok(true);
    }
    let d = delta(a.center, b.center);
    let lower = d[0].square().add(d[1].square()).sqrt()?.lo;
    Ok(lower
        > Interval::point(a.cover_radius_mm)
            .add(Interval::point(b.cover_radius_mm))
            .add(Interval::point(margin))
            .hi)
}
fn boundary_loop_confinement(
    leaf: &NativeSourceLeafReceipt,
    tube: &NativeBoundaryTube,
    side: usize,
    interface: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    // This is the native all-height graph/ordering/census theorem, calibrated
    // against this exact actual M. It is not a filled-distance motion tube.
    let whole_product = leaf.proof_kind == NativeSourceLeafProofKind::ClosedSlab
        && leaf.radial_product.is_some()
        && leaf.world_height_interval_mm[side].to_bits() == interface.to_bits();
    let endpoint_product = leaf.endpoint_radial_products[side].is_some_and(|endpoint| {
        let range = endpoint.world_height_interval_mm;
        leaf.world_height_interval_mm[side].to_bits() == interface.to_bits()
            && range[0] <= interface
            && interface <= range[1]
            && range[0] < leaf.sample_world_z_mm
            && leaf.sample_world_z_mm < range[1]
    });
    if whole_product || endpoint_product {
        return Ok(());
    }
    if tube.stationary || leaf.sample_loops.len() == 1 {
        return Ok(());
    }
    let distance = tube.distance_mm.ok_or_else(|| {
        error(
            GeometryErrorCode::AnalysisIndeterminate,
            "native regular multiple-boundary continuation lacks a true boundary-motion tube",
        )
    })?;
    let margin = Interval::point(distance).mul(Interval::point(2.0)).hi;
    work.charge(1, cancel)?;
    if leaf
        .sample_loop_clearance_mm
        .is_some_and(|lower| lower > margin)
    {
        // The complete Jordan/parent census was calibrated against the original
        // native cycles before coordinates were discarded. The SAME existing
        // two-tube margin keeps EVERY boundary pair apart throughout this
        // source-bound continuation, so its nesting cannot change. The caller
        // still owns exact closed-cut and critical/collar admission; this is
        // neither a full-slab promotion nor authority from a point/polygon.
        return Ok(());
    }
    for (i, a) in leaf.sample_loops.iter().enumerate() {
        if a.hole {
            let mut owners = 0;
            for b in leaf.sample_loops.iter() {
                work.charge(1, cancel)?;
                if !b.hole && shape_kernel_contains(&b.shape, &a.shape, margin)? {
                    owners += 1;
                }
            }
            if owners != 1 {
                return Err(error(
                    GeometryErrorCode::AnalysisIndeterminate,
                    "native hole motion is not confined within one original material branch kernel",
                ));
            }
        }
        for b in &leaf.sample_loops[i + 1..] {
            work.charge(1, cancel)?;
            if a.hole == b.hole && !shape_regions_separated(&a.shape, &b.shape, margin)? {
                return Err(error(
                    GeometryErrorCode::AnalysisIndeterminate,
                    "native distinct branch tubes do not exclude material/void braiding",
                ));
            }
        }
    }
    Ok(())
}
fn regular_loop_matching(
    before: &NativeSourceLeafReceipt,
    after: &NativeSourceLeafReceipt,
    scratch: &mut Scratch,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Vec<usize>>, GeometryError> {
    if before.sample_loops.len() != after.sample_loops.len() {
        return Ok(None);
    }
    let single = before.sample_loops.len() == 1
        && !before.sample_loops[0].hole
        && !after.sample_loops[0].hole;
    let mut matching = Vec::new();
    scratch.reserve(&mut matching, before.sample_loops.len(), work, cancel)?;
    for a in before.sample_loops.iter() {
        let mut found = None;
        for (j, b) in after.sample_loops.iter().enumerate() {
            work.charge(1, cancel)?;
            if a.hole == b.hole
                && (single
                    || !a.source_face_ordinals.is_empty()
                        && a.source_face_ordinals == b.source_face_ordinals)
            {
                if found.is_some() {
                    return Ok(None);
                }
                found = Some(j);
            }
        }
        let Some(index) = found else {
            return Ok(None);
        };
        for &previous in &matching {
            work.charge(1, cancel)?;
            if previous == index {
                return Ok(None);
            }
        }
        matching.push(index);
    }
    for (i, loop_) in before.sample_loops.iter().enumerate() {
        work.charge(1, cancel)?;
        if loop_.registration.parent.map(|parent| matching[parent])
            != after.sample_loops[matching[i]].registration.parent
        {
            return Ok(None);
        }
    }
    Ok(Some(matching))
}

fn certify_regular_material_interface(
    before: &NativeSourceLeafReceipt,
    after: &NativeSourceLeafReceipt,
    interface: f64,
    before_tube: &NativeBoundaryTube,
    after_tube: &NativeBoundaryTube,
    pairs: &[NativeRegularInterfacePairReceipt],
    scratch: &mut Scratch,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    let unresolved = || {
        error(
            GeometryErrorCode::AnalysisIndeterminate,
            format!(
                "native occurrence {} has unresolved actual-M regular interface incidence/clearance",
                before.occurrence_id().get()
            ),
        )
    };
    if !before.source.same_material_recipe(&after.source)
        || before.pose() != after.pose()
        || before.world_height_interval_mm[1].to_bits() != interface.to_bits()
        || after.world_height_interval_mm[0].to_bits() != interface.to_bits()
        || before.sample_loops.len() != after.sample_loops.len()
    {
        return Err(unresolved());
    }
    let Some(matching) = regular_loop_matching(before, after, scratch, work, cancel)? else {
        return Err(unresolved());
    };
    let mut identical = true;
    for (i, &j) in matching.iter().enumerate() {
        work.charge(1, cancel)?;
        identical &=
            before.sample_loops[i].shape.xy_sha256 == after.sample_loops[j].shape.xy_sha256;
    }
    boundary_loop_confinement(before, before_tube, 1, interface, work, cancel)?;
    boundary_loop_confinement(after, after_tube, 0, interface, work, cancel)?;
    // Exact actual geometry identity proves the printed interface, not native
    // incidence: the latter came from original branch flow and confinement.
    if identical {
        return Ok(());
    }
    let (lower, upper) = (leaf_fingerprint(before), leaf_fingerprint(after));
    let matching_hash = matching_fingerprint(&matching);
    for receipt in pairs {
        work.charge(1, cancel)?;
        if receipt.lower == lower
            && receipt.upper == upper
            && receipt.cut_bits == interface.to_bits()
            && receipt.matching == matching_hash
        {
            // Source/cut/pose, directed scope, confinement and full branch
            // matching ABOVE remain mandatory. Only ACTUAL overlap topology
            // below is discharged by this whole embedded-collar pair.
            return Ok(());
        }
    }
    let margin = Interval::point(before.sample_region_error_mm)
        .add(Interval::point(after.sample_region_error_mm))
        .hi;
    for (i, a) in before.sample_loops.iter().enumerate() {
        let b = &after.sample_loops[matching[i]];
        if !shape_kernels_overlap(&a.shape, &b.shape, margin)? {
            return Err(unresolved());
        }
        if a.hole {
            let mut owners = 0;
            for (k, outer) in before.sample_loops.iter().enumerate() {
                work.charge(1, cancel)?;
                if outer.hole {
                    continue;
                }
                let next = &after.sample_loops[matching[k]];
                if shape_kernel_contains(&outer.shape, &a.shape, margin)?
                    && shape_kernel_contains(&outer.shape, &b.shape, margin)?
                    && shape_kernel_contains(&next.shape, &a.shape, margin)?
                    && shape_kernel_contains(&next.shape, &b.shape, margin)?
                {
                    owners += 1;
                }
            }
            if owners != 1 {
                return Err(unresolved());
            }
        }
        for (j, other) in after.sample_loops.iter().enumerate() {
            work.charge(1, cancel)?;
            if j != matching[i]
                && a.hole == other.hole
                && !shape_regions_separated(&a.shape, &other.shape, margin)?
            {
                return Err(unresolved());
            }
        }
    }
    Ok(())
}

/// Ordered, bounded atomic resolver. It retains only source identities, the
/// preceding accepted positive leaves and dependencies on the next CLOSED slab;
/// no all-layer contour or proof-fact history is kept.
pub struct NativeUnionBindingLedger {
    expected: [f64; 2],
    maximum_retained_bytes: usize,
    last_upper: Option<f64>,
    sources: Vec<NativeUnionSourceCapture>,
    previous_positive: Vec<NativeSourceLeafReceipt>,
    next_upper_dependencies: Vec<NativeUnionLeafDependency>,
    next_upper_empty: Vec<OccurrenceId>,
    previous_raw_inclusion_error_mm: f64,
    previous_preserved_sources: Vec<OccurrenceId>,
    previous_target_envelopes: Vec<NativeTargetEnvelope>,
    previous_boundary_tubes: Vec<NativeBoundaryTube>,
    digest: Sha256,
    slab_count: usize,
    endpoint_count: usize,
    achieved_shape_error_mm: f64,
}
#[derive(Debug)]
pub struct CompletedNativeUnionProofSet {
    world_height_interval_mm: [f64; 2],
    sources: Vec<NativeUnionSourceCapture>,
    binding_sha256: [u8; 32],
    slab_count: usize,
    endpoint_count: usize,
    achieved_shape_error_mm: f64,
}
impl CompletedNativeUnionProofSet {
    pub fn world_height_interval_mm(&self) -> [f64; 2] {
        self.world_height_interval_mm
    }
    pub fn sources(&self) -> &[NativeUnionSourceCapture] {
        &self.sources
    }
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.sources.capacity() * std::mem::size_of::<NativeUnionSourceCapture>()
            + self
                .sources
                .iter()
                .map(source_retained_bytes)
                .sum::<usize>()
    }
    pub fn binding_sha256(&self) -> &[u8; 32] {
        &self.binding_sha256
    }
    pub fn slab_count(&self) -> usize {
        self.slab_count
    }
    pub fn owned_critical_slab_count(&self) -> usize {
        self.endpoint_count
    }
    pub fn achieved_shape_error_mm(&self) -> f64 {
        self.achieved_shape_error_mm
    }
}
impl NativeUnionBindingLedger {
    pub fn new(
        expected_closed_world_interval: [f64; 2],
        maximum_retained_bytes: usize,
    ) -> Result<Self, GeometryError> {
        if !expected_closed_world_interval.iter().all(|v| v.is_finite())
            || expected_closed_world_interval[0] >= expected_closed_world_interval[1]
        {
            return Err(error(
                GeometryErrorCode::InvalidGeometry,
                "native union ledger requires a finite nonempty CLOSED stream interval",
            ));
        }
        let maximum_retained_bytes = maximum_retained_bytes.min(MAX_SCRATCH);
        if maximum_retained_bytes < std::mem::size_of::<Self>() {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                "native union ledger capacity cannot hold its scalar binding state",
            ));
        }
        let mut digest = Sha256::new();
        digest.update(b"spiling-completed-native-union-proof-set-1\0");
        for value in expected_closed_world_interval {
            digest.update(value.to_bits().to_le_bytes());
        }
        Ok(Self {
            expected: expected_closed_world_interval,
            maximum_retained_bytes,
            last_upper: None,
            sources: Vec::new(),
            previous_positive: Vec::new(),
            next_upper_dependencies: Vec::new(),
            next_upper_empty: Vec::new(),
            previous_raw_inclusion_error_mm: 0.0,
            previous_preserved_sources: Vec::new(),
            previous_boundary_tubes: Vec::new(),
            previous_target_envelopes: Vec::new(),
            digest,
            slab_count: 0,
            endpoint_count: 0,
            achieved_shape_error_mm: 0.0,
        })
    }
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.sources.capacity() * std::mem::size_of::<NativeUnionSourceCapture>()
            + self
                .sources
                .iter()
                .map(source_retained_bytes)
                .sum::<usize>()
            + self.previous_positive.capacity() * std::mem::size_of::<NativeSourceLeafReceipt>()
            + self
                .previous_positive
                .iter()
                .map(NativeSourceLeafReceipt::retained_bytes)
                .sum::<usize>()
            + self.next_upper_dependencies.capacity()
                * std::mem::size_of::<NativeUnionLeafDependency>()
            + self
                .next_upper_dependencies
                .iter()
                .map(|dependency| dependency.leaf.retained_bytes())
                .sum::<usize>()
            + self.next_upper_empty.capacity() * std::mem::size_of::<OccurrenceId>()
            + self.previous_preserved_sources.capacity() * std::mem::size_of::<OccurrenceId>()
            + self.previous_target_envelopes.capacity()
                * std::mem::size_of::<NativeTargetEnvelope>()
            + self.previous_boundary_tubes.capacity() * std::mem::size_of::<NativeBoundaryTube>()
    }
    pub fn record(
        &mut self,
        mut pending: PendingNativeUnionProof,
        actual_final_m_loops: &[SectionLoopMm],
        actual_positive_source_leaves: &[&NativeSourceLeafReceipt],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        work.charge(1, cancel)?;
        let range = pending.world_height_interval_mm;
        if range[0] != self.last_upper.unwrap_or(self.expected[0]) || range[1] > self.expected[1] {
            return Err(error(
                GeometryErrorCode::InvalidGeometry,
                "native union stream is not ordered contiguous CLOSED coverage of its requested interval",
            ));
        }
        if candidate_hash(
            actual_final_m_loops,
            pending.sample_world_z_mm,
            work,
            cancel,
        )? != pending.candidate_geometry_sha256
        {
            return Err(error(
                GeometryErrorCode::InvalidGeometry,
                "final M geometry changed after native union proof/refinement; regenerate its pending proof",
            ));
        }
        if actual_positive_source_leaves.len() != pending.positive_leaves.len() {
            return Err(error(
                GeometryErrorCode::InvalidGeometry,
                "native union binding is missing actual positive source-M contributors",
            ));
        }
        for expected in &pending.positive_leaves {
            let mut matches = 0;
            for actual in actual_positive_source_leaves {
                work.charge(1, cancel)?;
                matches += usize::from(actual.is_positive() && same_leaf(expected, actual));
            }
            if matches != 1 {
                return Err(error(
                    GeometryErrorCode::InvalidGeometry,
                    "native union binding source-M leaf/hash/request is missing, duplicated, or stale after refinement",
                ));
            }
        }
        if self.last_upper.is_some() {
            if pending.sources.len() != self.sources.len() {
                return Err(error(
                    GeometryErrorCode::InvalidGeometry,
                    "native union stream source occurrence set changed",
                ));
            }
            for source in &self.sources {
                let mut matches = 0;
                for actual in &pending.sources {
                    work.charge(1, cancel)?;
                    matches += usize::from(
                        source.occurrence_id() == actual.occurrence_id()
                            && source.same_material_recipe(actual)
                            && source.pose() == actual.pose(),
                    );
                }
                if matches != 1 {
                    return Err(error(
                        GeometryErrorCode::InvalidGeometry,
                        "native union stream original source identity/pose changed",
                    ));
                }
            }
        }
        let mut resolved_shape_error = pending.achieved_shape_error_mm;
        for occurrence in &self.next_upper_empty {
            for actual in actual_positive_source_leaves {
                work.charge(1, cancel)?;
                if actual.occurrence_id() == *occurrence {
                    return Err(error(
                        GeometryErrorCode::AnalysisIndeterminate,
                        "critical native endpoint requires source-M absence, but the accepted successor uses positive material",
                    ));
                }
            }
        }
        for dependency in &self.next_upper_dependencies {
            let mut found = false;
            for actual in actual_positive_source_leaves {
                work.charge(1, cancel)?;
                found |= same_leaf(&dependency.leaf, actual);
            }
            if !found {
                return Err(error(
                    GeometryErrorCode::InvalidGeometry,
                    "future endpoint dependency was not actually used as a positive leaf in the accepted successor candidate",
                ));
            }
            resolved_shape_error = resolved_shape_error.max(resolved_dependency_error(
                dependency.owner_distance_mm,
                dependency.owner_policy_mm,
                pending.raw_leaf_to_candidate_error_mm,
            )?);
        }
        let coexistent = self
            .retained_bytes()
            .checked_add(pending.retained_bytes())
            .ok_or_else(|| {
                error(
                    GeometryErrorCode::ResourceLimit,
                    "native union ledger/proof retention overflow",
                )
            })?;
        if coexistent > self.maximum_retained_bytes {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                "native union ledger and pending proof exceed their coexistent retained-byte capacity",
            ));
        }
        // Reserve against the caller's smaller ledger cap BEFORE allocation;
        // Scratch's fixed ceiling is reduced by a virtual unavailable prefix.
        let mut scratch = Scratch {
            bytes: MAX_SCRATCH - self.maximum_retained_bytes + coexistent,
        };
        let mut next_upper = Vec::new();
        let mut next_upper_empty = Vec::new();
        let mut boundary_tubes = Vec::new();
        scratch.reserve(
            &mut boundary_tubes,
            pending.positive_leaves.len(),
            work,
            cancel,
        )?;
        for leaf in &pending.positive_leaves {
            boundary_tubes.push(source_boundary_tube(
                leaf,
                &pending.collar_dependencies,
                work,
                cancel,
            )?);
        }
        if pending.target_envelopes.len() != pending.positive_leaves.len()
            || self.previous_target_envelopes.len() != self.previous_positive.len()
        {
            return Err(error(
                GeometryErrorCode::InvalidGeometry,
                "native target/positive leaf capture cardinality mismatch",
            ));
        }
        for ((id, _), leaf) in pending
            .target_envelopes
            .iter()
            .zip(&pending.positive_leaves)
        {
            work.charge(1, cancel)?;
            if *id != leaf.occurrence_id() {
                return Err(error(
                    GeometryErrorCode::InvalidGeometry,
                    "native target enclosure is not attributed to its actual positive input",
                ));
            }
        }
        if let Some(interface) = self.last_upper {
            for (previous, (_, previous_envelope)) in self
                .previous_positive
                .iter()
                .zip(&self.previous_target_envelopes)
            {
                for (current, (_, current_envelope)) in pending
                    .positive_leaves
                    .iter()
                    .zip(&pending.target_envelopes)
                {
                    work.charge(1, cancel)?;
                    let allowance = Interval::point(self.previous_raw_inclusion_error_mm)
                        .add(Interval::point(pending.raw_leaf_to_candidate_error_mm))
                        .hi;
                    if previous.occurrence_id() != current.occurrence_id() {
                        // Whole CLOSED native targets on BOTH actual accepted
                        // intervals, enlarged by both complete candidate policies.
                        // Unlike raw-M boxes, this also proves diagonal clearance.
                        if let (Some(a), Some(b)) =
                            (previous_envelope.as_ref(), current_envelope.as_ref())
                            && separated_tubes(Some(a), Some(b))
                        {
                            continue;
                        }
                        let (Some(a), Some(b)) = (
                            previous.candidate_xy_bounds_mm,
                            current.candidate_xy_bounds_mm,
                        ) else {
                            continue;
                        };
                        let margin = Interval::point(allowance)
                            .add(Interval::point(previous.sample_region_error_mm))
                            .add(Interval::point(current.sample_region_error_mm))
                            .hi;
                        if !bounds_separated(a, b, margin)
                            && !rectangle_leaf_contact(
                                previous, current, interface, allowance, work, cancel,
                            )?
                        {
                            return Err(error(
                                GeometryErrorCode::AnalysisIndeterminate,
                                format!(
                                    "native occurrence pair {} / {} has unresolved cross-height actual-material contact",
                                    previous.occurrence_id().get(),
                                    current.occurrence_id().get()
                                ),
                            ));
                        }
                        continue;
                    }
                    if rectangle_leaf_contact(
                        previous, current, interface, allowance, work, cancel,
                    )? {
                        continue;
                    }
                    if !source_material_preserved(
                        &self.previous_preserved_sources,
                        previous.occurrence_id(),
                        work,
                        cancel,
                    )? || !source_material_preserved(
                        &pending.preserved_source_occurrences,
                        current.occurrence_id(),
                        work,
                        cancel,
                    )? {
                        return Err(error(
                            GeometryErrorCode::AnalysisIndeterminate,
                            "rounded nonorthogonal cross-height material needs an actual joint native interface proof",
                        ));
                    }
                    if radial_leaf_contact(previous, current, interface) {
                        continue;
                    }
                    let convex = (previous.native_convex_section_topology
                        || current.native_convex_section_topology)
                        && previous.sample_loops.len() == 1
                        && !previous.sample_loops[0].hole
                        && current.sample_loops.len() == 1
                        && !current.sample_loops[0].hole;
                    if (previous.critical_interfaces[1] || current.critical_interfaces[0])
                        && !convex
                    {
                        let mut proved = false;
                        for collar in &pending.collar_dependencies {
                            work.charge(1, cancel)?;
                            if same_leaf(collar.current_leaf(), current)
                                && collar.inner.proved_interfaces[0]
                            {
                                for dependency in collar.used_source_leaves() {
                                    work.charge(1, cancel)?;
                                    proved |= same_leaf(previous, dependency);
                                }
                            }
                        }
                        for dependency in &self.next_upper_dependencies {
                            work.charge(1, cancel)?;
                            proved |=
                                dependency.interface_proved && same_leaf(&dependency.leaf, current);
                        }
                        if !proved {
                            return Err(error(
                                GeometryErrorCode::AnalysisIndeterminate,
                                format!(
                                    "cross-height critical source interface lacks the actual native coface/port ownership proof; cut={interface:?}; previous={:?}/{:?}/radial={:?}; current={:?}/{:?}/radial={:?}; radial_pair={:?}",
                                    previous.world_height_interval_mm,
                                    previous.proof_kind,
                                    previous.radial_product.map(|p| p.kind),
                                    current.world_height_interval_mm,
                                    current.proof_kind,
                                    current.radial_product.map(|p| p.kind),
                                    previous.radial_product.zip(current.radial_product).map(
                                        |(a, b)| (
                                            previous.source.same_material_recipe(&current.source),
                                            previous.pose() == current.pose(),
                                            a.axis_sha256 == b.axis_sha256,
                                            a.center == b.center,
                                            a.kind == b.kind,
                                            a.inner_cover_radius_mm.max(b.inner_cover_radius_mm),
                                            a.outer_kernel_radius_mm.min(b.outer_kernel_radius_mm),
                                        )
                                    ),
                                ),
                            ));
                        }
                    } else if convex {
                        // A sealed WHOLE original source convexity/closure proof,
                        // not sampled section counts, proves every nonempty source
                        // section a disk. The ACTUAL accepted polygons must still
                        // share an open star kernel; native Hausdorff error alone
                        // could otherwise disconnect a thin moving solid.
                        if !shape_kernels_overlap(
                            &previous.sample_loops[0].shape,
                            &current.sample_loops[0].shape,
                            0.0,
                        )? {
                            return Err(error(
                                GeometryErrorCode::AnalysisIndeterminate,
                                "globally native-convex source has unresolved actual-M interface kernel",
                            ));
                        }
                    } else {
                        let before = boundary_tube_for(
                            &self.previous_boundary_tubes,
                            previous.occurrence_id(),
                            work,
                            cancel,
                        )?;
                        let after = boundary_tube_for(
                            &boundary_tubes,
                            current.occurrence_id(),
                            work,
                            cancel,
                        )?;
                        certify_regular_material_interface(
                            previous,
                            current,
                            interface,
                            before,
                            after,
                            &pending.regular_interfaces,
                            &mut scratch,
                            work,
                            cancel,
                        ).map_err(|failure| error(failure.code, format!(
                            "native regular interface cut={interface}; previous={:?}/{:?}/radial={:?}; current={:?}/{:?}/radial={:?}; boundary_tubes={:?}/{:?}: {}",
                            previous.world_height_interval_mm, previous.proof_kind,
                            previous.radial_product.map(|p| (p.kind, p.center)),
                            current.world_height_interval_mm, current.proof_kind,
                            current.radial_product.map(|p| (p.kind, p.center)),
                            before.distance_mm, after.distance_mm, failure.message,
                        )))?;
                    }
                }
            }
        }
        for dependency in &pending.endpoint_dependencies {
            work.charge(1, cancel)?;
            match dependency.endpoint {
                NativeOwnedEndpoint::Lower => {
                    let mut found = false;
                    for previous in &self.previous_positive {
                        work.charge(1, cancel)?;
                        found |= same_leaf(&dependency.adjacent, previous);
                    }
                    if !found {
                        return Err(error(
                            GeometryErrorCode::InvalidGeometry,
                            "lower endpoint dependency is not an actual positive leaf of the immediately preceding accepted candidate",
                        ));
                    }
                    resolved_shape_error = resolved_shape_error.max(resolved_dependency_error(
                        dependency.adjacent.achieved_shape_error_mm,
                        dependency.native_shape_error_policy_mm(),
                        self.previous_raw_inclusion_error_mm,
                    )?);
                }
                NativeOwnedEndpoint::Upper => scratch.push(
                    &mut next_upper,
                    NativeUnionLeafDependency {
                        leaf: dependency.adjacent.clone(),
                        owner_distance_mm: dependency.adjacent.achieved_shape_error_mm,
                        owner_policy_mm: dependency.native_shape_error_policy_mm(),
                        interface_proved: false,
                    },
                    work,
                    cancel,
                )?,
            }
        }
        for collar in &pending.collar_dependencies {
            work.charge(1, cancel)?;
            let current = collar.current_leaf();
            let distance = collar.native_collar().closed_source_to_material_error_mm();
            let native_policy = current.native_shape_error_policy_mm();
            for leaf in collar.used_source_leaves() {
                match collar_side(leaf, current)? {
                    NativeCriticalCollarTargetSide::Current => {
                        if !pending
                            .positive_leaves
                            .iter()
                            .any(|actual| same_leaf(leaf, actual))
                        {
                            return Err(error(
                                GeometryErrorCode::InvalidGeometry,
                                "critical CURRENT dependency is not an actual positive input of this candidate",
                            ));
                        }
                        resolved_shape_error = resolved_shape_error.max(resolved_dependency_error(
                            distance,
                            native_policy,
                            pending.raw_leaf_to_candidate_error_mm,
                        )?);
                    }
                    NativeCriticalCollarTargetSide::Lower => {
                        let mut found = false;
                        for previous in &self.previous_positive {
                            work.charge(1, cancel)?;
                            found |= same_leaf(leaf, previous);
                        }
                        if !found {
                            return Err(error(
                                GeometryErrorCode::InvalidGeometry,
                                "critical lower native port/cover leaf was not actually used by the preceding accepted candidate",
                            ));
                        }
                        resolved_shape_error = resolved_shape_error.max(resolved_dependency_error(
                            distance,
                            native_policy,
                            self.previous_raw_inclusion_error_mm,
                        )?);
                    }
                    NativeCriticalCollarTargetSide::Upper => scratch.push(
                        &mut next_upper,
                        NativeUnionLeafDependency {
                            leaf: leaf.clone(),
                            owner_distance_mm: distance,
                            owner_policy_mm: native_policy,
                            interface_proved: collar.inner.proved_interfaces[1],
                        },
                        work,
                        cancel,
                    )?,
                }
            }
            for endpoint in &collar.inner.empty_dependencies {
                match endpoint {
                    NativeOwnedEndpoint::Lower => {
                        for previous in &self.previous_positive {
                            work.charge(1, cancel)?;
                            if previous.occurrence_id() == current.occurrence_id() {
                                return Err(error(
                                    GeometryErrorCode::AnalysisIndeterminate,
                                    "native component birth needs preceding source-M absence, not overlapping positive material",
                                ));
                            }
                        }
                    }
                    NativeOwnedEndpoint::Upper => {
                        if !next_upper_empty.contains(&current.occurrence_id()) {
                            scratch.push(
                                &mut next_upper_empty,
                                current.occurrence_id(),
                                work,
                                cancel,
                            )?;
                        }
                    }
                }
            }
        }
        let mut digest = self.digest.clone();
        digest.update((self.slab_count as u64).to_le_bytes());
        for value in range.into_iter().chain([
            pending.sample_world_z_mm,
            pending.achieved_shape_error_mm,
            pending.policy.candidate_coordinate_error_mm,
            pending.raw_leaf_to_candidate_error_mm,
            resolved_shape_error,
        ]) {
            digest.update(value.to_bits().to_le_bytes());
        }
        digest.update(pending.candidate_geometry_sha256);
        digest.update((pending.sources.len() as u64).to_le_bytes());
        digest.update((pending.preserved_source_occurrences.len() as u64).to_le_bytes());
        for id in &pending.preserved_source_occurrences {
            work.charge(1, cancel)?;
            digest.update(id.get().to_le_bytes());
        }
        digest.update((pending.target_envelopes.len() as u64).to_le_bytes());
        for (id, envelope) in &pending.target_envelopes {
            work.charge(1, cancel)?;
            digest.update(id.get().to_le_bytes());
            digest.update([u8::from(envelope.is_some())]);
            if let Some(envelope) = envelope {
                for value in envelope
                    .bounds
                    .into_iter()
                    .flatten()
                    .chain(envelope.native_bounds.into_iter().flatten())
                    .chain(envelope.expanded.into_iter().flatten())
                    .chain([envelope.tube, envelope.policy])
                {
                    digest.update(value.to_bits().to_le_bytes());
                }
                digest.update([u8::from(envelope.ball.is_some())]);
                if let Some(ball) = envelope.ball {
                    for value in ball.center.into_iter().chain([ball.radius]) {
                        digest.update(value.to_bits().to_le_bytes());
                    }
                }
            }
        }
        for source in &pending.sources {
            work.charge(1, cancel)?;
            hash_source(&mut digest, source);
        }
        digest.update((pending.positive_leaves.len() as u64).to_le_bytes());
        for leaf in &pending.positive_leaves {
            work.charge(1, cancel)?;
            hash_leaf(&mut digest, leaf);
        }
        digest.update((pending.endpoint_dependencies.len() as u64).to_le_bytes());
        for dependency in &pending.endpoint_dependencies {
            work.charge(1, cancel)?;
            digest.update([match dependency.endpoint {
                NativeOwnedEndpoint::Lower => 0,
                NativeOwnedEndpoint::Upper => 1,
            }]);
            hash_leaf(&mut digest, &dependency.adjacent);
            digest.update([u8::from(dependency.stratum.is_some())]);
            if let Some(stratum) = &dependency.stratum {
                digest.update((stratum.face_id.as_str().len() as u64).to_le_bytes());
                digest.update(stratum.face_id.as_str().as_bytes());
                digest.update((stratum.face_index as u64).to_le_bytes());
                digest.update(stratum.world_height_mm.to_bits().to_le_bytes());
                digest.update(stratum.outward_height_sign.to_le_bytes());
            }
            for axis in dependency.occupied_xy_bounds_mm {
                for bound in axis {
                    digest.update(bound.to_bits().to_le_bytes());
                }
            }
        }
        digest.update((pending.interface_attachments.len() as u64).to_le_bytes());
        for attachment in &pending.interface_attachments {
            work.charge(1, cancel)?;
            digest.update(attachment.endpoint_occurrence_id.get().to_le_bytes());
            digest.update(attachment.current_occurrence_id.get().to_le_bytes());
            digest.update(attachment.endpoint_leaf_sha256);
            digest.update(attachment.current_leaf_sha256);
            let cap = attachment.endpoint_source_cap_face_id();
            digest.update((cap.as_str().len() as u64).to_le_bytes());
            digest.update(cap.as_str().as_bytes());
            for axis in attachment.native_axis_events {
                for event in axis {
                    digest.update(event.to_bits().to_le_bytes());
                }
            }
            for count in attachment.native_axis_event_counts {
                digest.update((count as u64).to_le_bytes());
            }
            digest.update(attachment.native_cell_incidence);
        }
        digest.update((pending.collar_dependencies.len() as u64).to_le_bytes());
        for collar in &pending.collar_dependencies {
            work.charge(1, cancel)?;
            digest.update(collar.incidence_sha256());
        }
        digest.update((pending.regular_interfaces.len() as u64).to_le_bytes());
        for receipt in &pending.regular_interfaces {
            work.charge(1, cancel)?;
            digest.update(receipt.lower);
            digest.update(receipt.upper);
            digest.update(receipt.cut_bits.to_le_bytes());
            digest.update(receipt.matching);
            digest.update(receipt.geometry);
        }
        let slab_count = self.slab_count.checked_add(1).ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "native union ledger slab count overflow",
            )
        })?;
        let owned_critical_slab_count = self
            .endpoint_count
            .checked_add(pending.owned_critical_slab_count())
            .ok_or_else(|| {
                error(
                    GeometryErrorCode::ResourceLimit,
                    "native union ledger endpoint count overflow",
                )
            })?;
        if self.last_upper.is_none() {
            self.sources = std::mem::take(&mut pending.sources);
        }
        self.previous_positive = std::mem::take(&mut pending.positive_leaves);
        self.next_upper_dependencies = next_upper;
        self.next_upper_empty = next_upper_empty;
        self.previous_raw_inclusion_error_mm = pending.raw_leaf_to_candidate_error_mm;
        self.previous_preserved_sources = std::mem::take(&mut pending.preserved_source_occurrences);
        self.previous_target_envelopes = std::mem::take(&mut pending.target_envelopes);
        self.previous_boundary_tubes = boundary_tubes;
        self.digest = digest;
        self.last_upper = Some(range[1]);
        self.slab_count = slab_count;
        self.endpoint_count = owned_critical_slab_count;
        self.achieved_shape_error_mm = self.achieved_shape_error_mm.max(resolved_shape_error);
        Ok(())
    }
    pub fn finalize(
        self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<CompletedNativeUnionProofSet, GeometryError> {
        work.charge(1, cancel)?;
        if self.last_upper != Some(self.expected[1]) || !self.next_upper_dependencies.is_empty() {
            return Err(error(
                GeometryErrorCode::AnalysisIndeterminate,
                "native union stream is incomplete or has unresolved future positive source-M dependencies",
            ));
        }
        Ok(CompletedNativeUnionProofSet {
            world_height_interval_mm: self.expected,
            sources: self.sources,
            binding_sha256: self.digest.finalize().into(),
            slab_count: self.slab_count,
            endpoint_count: self.endpoint_count,
            achieved_shape_error_mm: self.achieved_shape_error_mm,
        })
    }
}

fn error(code: GeometryErrorCode, message: impl AsRef<str>) -> GeometryError {
    GeometryError::new(code, message)
}
fn unresolved(a: usize, b: usize, reason: &str) -> GeometryError {
    error(
        GeometryErrorCode::AnalysisIndeterminate,
        format!("native union source index pair {a}/{b} unresolved: {reason}"),
    )
}
fn original_pair_error(
    occurrences: &[NativeUnionOccurrence<'_, '_>],
    a: usize,
    b: usize,
    failure: GeometryError,
) -> GeometryError {
    if matches!(
        failure.code,
        GeometryErrorCode::Cancelled | GeometryErrorCode::ResourceLimit
    ) {
        return failure;
    }
    error(
        failure.code,
        format!(
            "native union original occurrence pair {}/{} (source indices {a}/{b}): {}",
            occurrences[a].occurrence_id.get(),
            occurrences[b].occurrence_id.get(),
            failure.message
        ),
    )
}

/// Tracks all simultaneously retained allocations, including temporary native
/// trim preparations. Numerical work is always debited from the caller's job.
struct Scratch {
    bytes: usize,
}
impl Scratch {
    fn remaining(&self) -> usize {
        MAX_SCRATCH.saturating_sub(self.bytes)
    }
    fn owned(
        &mut self,
        bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        work.charge(bytes as u64, cancel)?;
        if bytes > self.remaining() {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                "joint native union owned identity storage exceeds scratch",
            ));
        }
        self.bytes += bytes;
        Ok(())
    }
    fn reserve<T>(
        &mut self,
        values: &mut Vec<T>,
        additional: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        work.charge(additional as u64, cancel)?;
        let required = values.len().checked_add(additional).ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "joint native union scratch count overflow",
            )
        })?;
        if required <= values.capacity() {
            return Ok(());
        }
        let growth = required
            .checked_sub(values.capacity())
            .and_then(|n| n.checked_mul(std::mem::size_of::<T>()))
            .ok_or_else(|| {
                error(
                    GeometryErrorCode::ResourceLimit,
                    "joint native union scratch byte overflow",
                )
            })?;
        if growth > self.remaining() {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                "joint native union coexistent scratch exhausted",
            ));
        }
        let old = values.capacity();
        values
            .try_reserve_exact(required - values.len())
            .map_err(|_| {
                error(
                    GeometryErrorCode::ResourceLimit,
                    "joint native union allocation refused",
                )
            })?;
        self.bytes = self
            .bytes
            .checked_add((values.capacity() - old) * std::mem::size_of::<T>())
            .ok_or_else(|| {
                error(
                    GeometryErrorCode::ResourceLimit,
                    "joint native union allocation overflow",
                )
            })?;
        if self.bytes > MAX_SCRATCH {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                "joint native union allocator capacity exceeds scratch",
            ));
        }
        Ok(())
    }
    fn push<T>(
        &mut self,
        values: &mut Vec<T>,
        value: T,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        self.reserve(values, 1, work, cancel)?;
        values.push(value);
        Ok(())
    }
}

fn loop_xy_hash(
    loop_: &SectionLoopMm,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[u8; 32], GeometryError> {
    let points = &loop_.points_mm[..loop_.points_mm.len() - 1];
    let mut first = 0;
    for i in 1..points.len() {
        work.charge(1, cancel)?;
        if points[i][0]
            .total_cmp(&points[first][0])
            .then(points[i][1].total_cmp(&points[first][1]))
            .is_lt()
        {
            first = i;
        }
    }
    let mut hash = Sha256::new();
    hash.update(b"spiling-actual-source-material-xy-cycle-1\0");
    hash.update([u8::from(loop_.is_hole)]);
    hash.update((points.len() as u64).to_le_bytes());
    for offset in 0..points.len() {
        work.charge(1, cancel)?;
        let p = points[(first + offset) % points.len()];
        hash.update(p[0].to_bits().to_le_bytes());
        hash.update(p[1].to_bits().to_le_bytes());
    }
    Ok(hash.finalize().into())
}
fn actual_loop_kernel_contains(
    loop_: &SectionLoopMm,
    p: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    for edge in loop_.points_mm.windows(2) {
        work.charge(1, cancel)?;
        let a = [edge[0][0], edge[0][1]];
        let b = [edge[1][0], edge[1][1]];
        if a == b {
            continue;
        }
        let determinant = cross(delta(b, a), delta(p, a));
        let inside = if loop_.is_hole {
            -determinant.hi
        } else {
            determinant.lo
        };
        if !inside.is_finite() || inside <= 0.0 {
            return Ok(false);
        }
    }
    Ok(true)
}
fn source_loop_shape(
    loop_: &SectionLoopMm,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<NativeLoopShape, GeometryError> {
    let mut bounds = [[f64::INFINITY, f64::NEG_INFINITY]; 2];
    for p in &loop_.points_mm {
        work.charge(1, cancel)?;
        for axis in 0..2 {
            bounds[axis][0] = bounds[axis][0].min(p[axis]);
            bounds[axis][1] = bounds[axis][1].max(p[axis]);
        }
    }
    let center = bounds.map(|axis| axis[0] * 0.5 + axis[1] * 0.5);
    let mut cover_radius_mm = 0.0_f64;
    let mut kernel_radius_mm = f64::INFINITY;
    for edge in loop_.points_mm.windows(2) {
        work.charge(1, cancel)?;
        let a = [edge[0][0], edge[0][1]];
        let b = [edge[1][0], edge[1][1]];
        cover_radius_mm = cover_radius_mm.max(distance_upper(center, a)?);
        if a == b {
            continue;
        }
        let direction = delta(b, a);
        let length = direction[0].square().add(direction[1].square()).sqrt()?;
        let determinant = cross(direction, delta(center, a));
        let signed = if loop_.is_hole {
            -determinant.hi
        } else {
            determinant.lo
        };
        if signed <= 0.0 || !signed.is_finite() || length.lo <= 0.0 || !length.hi.is_finite() {
            kernel_radius_mm = 0.0;
        } else {
            kernel_radius_mm = kernel_radius_mm.min(
                Interval::point(signed)
                    .div(Interval::point(length.hi))?
                    .lo
                    .max(0.0),
            );
        }
    }
    if !cover_radius_mm.is_finite() {
        return Err(error(
            GeometryErrorCode::ConditioningFailure,
            "actual-M loop enclosure is nonfinite",
        ));
    }
    if !kernel_radius_mm.is_finite() {
        kernel_radius_mm = 0.0;
    }
    let mut kernel_box = None;
    // Trial points only PROPOSE a witness. Every box corner is subsequently
    // proved inside EVERY oriented edge halfplane; no sample is authoritative.
    for fraction in [0.999999, 0.9, 0.5, 0.1] {
        work.charge(1, cancel)?;
        let trial: [[f64; 2]; 2] = std::array::from_fn(|axis| {
            bounds[axis].map(|value| center[axis] + (value - center[axis]) * fraction)
        });
        if trial
            .iter()
            .any(|axis| axis[0] >= axis[1] || axis.iter().any(|value| !value.is_finite()))
        {
            continue;
        }
        let mut proved = true;
        for x in trial[0] {
            for y in trial[1] {
                if !actual_loop_kernel_contains(loop_, [x, y], work, cancel)? {
                    proved = false;
                    break;
                }
            }
        }
        if proved {
            kernel_box = Some(trial);
            break;
        }
    }
    Ok(NativeLoopShape {
        xy_sha256: loop_xy_hash(loop_, work, cancel)?,
        bounds,
        center,
        cover_radius_mm,
        kernel_radius_mm,
        kernel_box,
    })
}
fn preserves_source_material(
    leaf: &NativeSourceLeafReceipt,
    candidate: CandidateLoops<'_>,
    scratch: &mut Scratch,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let count = leaf.sample_loops.len();
    if count != candidate.len() {
        return Ok(false);
    }
    let mut seen = Vec::new();
    scratch.reserve(&mut seen, count, work, cancel)?;
    seen.resize(count, false);
    for loop_ in candidate.iter() {
        let hash = loop_xy_hash(loop_, work, cancel)?;
        let mut found = None;
        for (ordinal, original) in leaf.sample_loops.iter().enumerate() {
            work.charge(1, cancel)?;
            if original.hole == loop_.is_hole && original.shape.xy_sha256 == hash {
                if found.is_some() {
                    return Ok(false);
                }
                found = Some(ordinal);
            }
        }
        let Some(index) = found else {
            return Ok(false);
        };
        if seen[index] {
            return Ok(false);
        }
        seen[index] = true;
    }
    Ok(true)
}
fn source_material_preserved(
    ids: &[OccurrenceId],
    id: OccurrenceId,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    for captured in ids {
        work.charge(1, cancel)?;
        if *captured == id {
            return Ok(true);
        }
    }
    Ok(false)
}

fn candidate_hash(
    loops: &[SectionLoopMm],
    z: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[u8; 32], GeometryError> {
    candidate_capture(loops, z, work, cancel).map(|capture| capture.geometry_sha256)
}
struct CandidateCapture {
    geometry_sha256: [u8; 32],
    xy_bounds_mm: Option<[[f64; 2]; 2]>,
}
fn candidate_capture(
    loops: &[SectionLoopMm],
    z: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<CandidateCapture, GeometryError> {
    work.charge(1, cancel)?;
    let mut hash = Sha256::new();
    hash.update(b"spiling-native-union-candidate-1\0");
    hash.update((loops.len() as u64).to_le_bytes());
    let mut bounds = [[f64::INFINITY, f64::NEG_INFINITY]; 2];
    let mut any = false;
    for loop_ in loops {
        work.charge(1, cancel)?;
        if loop_.points_mm.len() < 4 || loop_.points_mm.first() != loop_.points_mm.last() {
            return Err(error(
                GeometryErrorCode::InvalidGeometry,
                "native union candidate loops must be explicitly closed",
            ));
        }
        hash.update([u8::from(loop_.is_hole)]);
        hash.update((loop_.points_mm.len() as u64).to_le_bytes());
        for point in &loop_.points_mm {
            work.charge(1, cancel)?;
            if point.iter().any(|v| !v.is_finite()) || point[2] != z {
                return Err(error(
                    GeometryErrorCode::InvalidGeometry,
                    "native union candidate must contain finite world XY points on the common sample plane",
                ));
            }
            for coordinate in point {
                hash.update(coordinate.to_bits().to_le_bytes());
            }
            any = true;
            for axis in 0..2 {
                bounds[axis][0] = bounds[axis][0].min(point[axis]);
                bounds[axis][1] = bounds[axis][1].max(point[axis]);
            }
        }
    }
    Ok(CandidateCapture {
        geometry_sha256: hash.finalize().into(),
        xy_bounds_mm: any.then_some(bounds),
    })
}

pub(super) fn verify_source_leaf_candidate(
    receipt: &NativeSourceLeafReceipt,
    actual_material: &[SectionLoopMm],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    if candidate_hash(actual_material, receipt.sample_world_z_mm, work, cancel)?
        != receipt.candidate_geometry_sha256
        || receipt.is_positive() != !actual_material.is_empty()
    {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "actual source-M candidate does not match its immutable native leaf hash/request",
        ));
    }
    Ok(())
}

pub(super) fn native_sample_hash(
    analysis: &SlabAnalysis,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[u8; 32], GeometryError> {
    work.charge(1, cancel)?;
    let mut hash = Sha256::new();
    hash.update(b"spiling-native-frozen-slab-sample-1\0");
    hash.update(analysis.material_realization_identity().identity_bytes());
    hash.update((analysis.section.loops.len() as u64).to_le_bytes());
    let plane = analysis.source_sample_plane();
    for value in plane.origin_mm.into_iter().chain(plane.normal) {
        hash.update(value.to_bits().to_le_bytes());
    }
    for (index, loop_) in analysis.section.loops.iter().enumerate() {
        hash.update([u8::from(loop_.is_hole)]);
        hash.update((loop_.points_mm.len() as u64).to_le_bytes());
        let faces = analysis
            .section
            .loop_source_face_ordinals(index)
            .unwrap_or(&[]);
        hash.update((faces.len() as u64).to_le_bytes());
        for face in faces {
            work.charge(1, cancel)?;
            hash.update((*face as u64).to_le_bytes());
        }
        for point in &loop_.points_mm {
            work.charge(1, cancel)?;
            for coordinate in point {
                hash.update(coordinate.to_bits().to_le_bytes());
            }
        }
    }
    for value in [
        analysis.section.achieved_chord_error_mm(),
        analysis.section.achieved_plane_error_mm(),
        analysis.section.boolean_tolerance_mm,
        analysis.boundary_motion_bound_mm,
    ] {
        hash.update(value.to_bits().to_le_bytes());
    }
    if let Some(radial) = &analysis.radial_slab {
        hash.update(radial.product.identity_sha256());
        hash.update(radial.bound_mm.to_bits().to_le_bytes());
    }
    for product in &analysis.endpoint_radial_products {
        hash.update([u8::from(product.is_some())]);
        if let Some(product) = product {
            hash.update(product.identity_sha256());
        }
    }
    hash.update((analysis.endpoint_strata().len() as u64).to_le_bytes());
    for stratum in analysis.endpoint_strata() {
        work.charge(1, cancel)?;
        hash.update((stratum.face_id().as_str().len() as u64).to_le_bytes());
        hash.update(stratum.face_id().as_str().as_bytes());
        hash.update((stratum.face_index() as u64).to_le_bytes());
        hash.update([match stratum.endpoint() {
            NativeOwnedEndpoint::Lower => 0,
            NativeOwnedEndpoint::Upper => 1,
        }]);
        hash.update(stratum.outward_height_sign().to_le_bytes());
        hash.update([u8::from(stratum.world_height_mm().is_some())]);
        if let Some(height) = stratum.world_height_mm() {
            hash.update(height.to_bits().to_le_bytes());
        }
        hash.update([u8::from(stratum.world_xy_bounds_mm().is_some())]);
        if let Some(bounds) = stratum.world_xy_bounds_mm() {
            for axis in bounds {
                for value in axis {
                    hash.update(value.to_bits().to_le_bytes());
                }
            }
        }
    }
    hash.update([
        u8::from(analysis.has_transition_material()),
        u8::from(analysis.collar_complex.is_some()),
    ]);
    if let Some(complex) = &analysis.collar_complex {
        hash.update(complex.native_complex_sha256());
    }
    Ok(hash.finalize().into())
}

/// Original source, frozen slab and actual contributor geometry for a leaf proof.
pub struct SourceLeafRequest<'a, 'd> {
    pub source: &'a crate::PreparedSolid<'d>,
    pub occurrence_id: &'a OccurrenceId,
    pub pose: RigidPoseMm,
    pub analysis: &'a SlabAnalysis,
    pub actual_used_source_loops: &'a [SectionLoopMm],
    pub policy: NativeUnionPolicy,
}

/// Prove the actual source-M contributor against the immutable CLOSED native
/// slab. This native base fact does not depend on a global union receipt.
pub fn certify_native_source_leaf(
    request: &SourceLeafRequest<'_, '_>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<NativeSourceLeafReceipt, GeometryError> {
    certify_source_leaf(request, false, work, cancel)
}

/// A directed SAMPLE-prism -> original CLOSED source metric. This metric alone
/// establishes neither reverse coverage nor incidence; any regular endpoint
/// authority is issued separately from a genuine CLOSED material product.
pub fn certify_native_one_sided_source_leaf(
    request: &SourceLeafRequest<'_, '_>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<NativeSourceLeafReceipt, GeometryError> {
    certify_source_leaf(request, true, work, cancel)
}

fn certify_endpoint_radial_leaves(
    request: &SourceLeafRequest<'_, '_>,
    interval: [f64; 2],
    sample: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[Option<NativeEndpointRadialLeaf>; 2], GeometryError> {
    let analysis = request.analysis;
    let mut endpoints = [None; 2];
    if analysis
        .endpoint_radial_products
        .iter()
        .all(Option::is_none)
    {
        return Ok(endpoints);
    }
    let source_request = analysis.request();
    let source_sample_band = analysis.capture.source_sample_height_band_mm;
    let world_sample_band = super::collar::sample_plane_height_bounds(
        request.source.definition(),
        analysis,
        super::placed::rotation(request.pose)?,
        request.pose,
    )?;
    for (side, product) in analysis.endpoint_radial_products.iter().enumerate() {
        let Some(product) = product else {
            continue;
        };
        work.charge(1, cancel)?;
        let world_range = product.coordinates().world_interval();
        let source_range = product.source_request().height_interval_mm;
        let expected_request = super::SlabRequest {
            height_interval_mm: source_range,
            ..source_request
        };
        if !product.matches_source_world(
            request.source.definition(),
            request.pose,
            &expected_request,
            world_range,
            sample,
        ) || world_range[side].to_bits() != interval[side].to_bits()
            || world_range[0] < interval[0]
            || world_range[1] > interval[1]
            || !(world_range[0] < world_sample_band.lo && world_sample_band.hi < world_range[1])
            || source_range[0] < source_request.height_interval_mm[0]
            || source_range[1] > source_request.height_interval_mm[1]
            || !(source_range[0] < source_sample_band[0] && source_sample_band[1] < source_range[1])
        {
            return Err(error(
                GeometryErrorCode::InvalidGeometry,
                "endpoint radial product does not bind its own CLOSED source/world domain and actual native reference",
            ));
        }
        let Some(region) = certify_actual_polygon_radial_region(
            request.actual_used_source_loops,
            product.coordinates(),
            product.kind(),
            work,
            cancel,
        )?
        else {
            // No endpoint authority is issued. The existing true boundary-tube
            // path remains available; a filled metric is never substituted.
            continue;
        };
        endpoints[side] = Some(NativeEndpointRadialLeaf {
            radial: NativeRadialLeaf {
                product_sha256: product.identity_sha256(),
                axis_sha256: product.coordinates().axis_sha256(),
                kind: product.kind(),
                center: product.coordinates().world_center_enclosure(),
                outer_kernel_radius_mm: region.outer_kernel_radius_mm(),
                inner_cover_radius_mm: region.inner_cover_radius_mm(),
                inner_displacement_mm: None,
            },
            world_height_interval_mm: world_range,
            source_height_interval_mm: source_range,
        });
    }
    Ok(endpoints)
}

fn certify_source_leaf(
    request: &SourceLeafRequest<'_, '_>,
    sample_only: bool,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<NativeSourceLeafReceipt, GeometryError> {
    let &SourceLeafRequest {
        source,
        occurrence_id,
        pose,
        analysis,
        actual_used_source_loops,
        policy,
    } = request;
    let definition = source.definition();
    work.charge(1, cancel)?;
    source.source_view()?;
    pose.validate()?;
    let interval = analysis.world_height_interval_mm().ok_or_else(|| {
        error(
            GeometryErrorCode::InvalidGeometry,
            "source-M leaf requires an actual placed native CLOSED slab",
        )
    })?;
    let z = analysis.sample_world_z_mm().ok_or_else(|| {
        error(
            GeometryErrorCode::InvalidGeometry,
            "source-M leaf requires a captured native sample",
        )
    })?;
    let native_policy = analysis.world_shape_error_policy_mm().ok_or_else(|| {
        error(
            GeometryErrorCode::InvalidGeometry,
            "source-M leaf requires a captured native policy",
        )
    })?;
    if !analysis.matches_original_material(definition)
        || analysis.section.definition_id() != definition.id()
        || !analysis.section.is_native_intersection()
        || analysis.placed_pose() != Some(pose)
        || !native_policy.is_finite()
        || native_policy <= 0.0
        || !(interval[0] < z && z < interval[1])
        || !policy.candidate_coordinate_error_mm.is_finite()
        || policy.candidate_coordinate_error_mm < 0.0
    {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "source-M leaf source/pose/CLOSED interval/sample/policy capture mismatch",
        ));
    }
    if let Some(occupied) = &analysis.occupied {
        if occupied.definition_id() != definition.id()
            || !sample_only
                && !analysis.has_transition_material()
                && analysis.section.loops.is_empty()
                && !occupied.loops.is_empty()
        {
            return Err(error(
                GeometryErrorCode::AnalysisIndeterminate,
                "source-M leaf cannot replace nonempty CLOSED occupancy by an empty sample",
            ));
        }
    } else if analysis.invariance.is_none() {
        return Err(error(
            GeometryErrorCode::AnalysisIndeterminate,
            "source-M leaf has no whole CLOSED slab occupied or invariant proof",
        ));
    }
    if let Some(invariance) = &analysis.invariance {
        let range = invariance.height_interval_mm();
        if !invariance.matches_original_material(definition)
            || invariance.pose() != Some(pose)
            || range[0] > interval[0]
            || range[1] < interval[1]
        {
            return Err(error(
                GeometryErrorCode::InvalidGeometry,
                "source-M leaf invariance does not bind its CLOSED native request",
            ));
        }
    }
    let CandidateCapture {
        geometry_sha256: candidate_geometry_sha256,
        xy_bounds_mm: candidate_xy_bounds_mm,
    } = candidate_capture(actual_used_source_loops, z, work, cancel)?;
    let native_sample_sha256 = native_sample_hash(analysis, work, cancel)?;
    let radial_slab = analysis.radial_slab.as_ref();
    if let Some(radial) = radial_slab {
        if !radial.product.matches_world(definition, pose, interval, z)
            || !radial.matches_native(&analysis.section.loops, work, cancel)?
        {
            return Err(error(
                GeometryErrorCode::InvalidGeometry,
                "closed radial product does not bind the exact native source/pose/scope/sample",
            ));
        }
    }
    let mut scratch = Scratch { bytes: 0 };
    let mut proof_kind = if sample_only {
        NativeSourceLeafProofKind::SampleToClosedSource
    } else if analysis.closed_shape_error_mm().is_some() {
        NativeSourceLeafProofKind::ClosedSlab
    } else {
        NativeSourceLeafProofKind::MaterialCore
    };
    let occurrence = SlabOccurrence {
        occurrence_index: 0,
        source,
        pose,
        analysis,
        source_metric: proof_kind,
        leaf: None,
    };
    let mut loop_recording = SourceLoopRecording::default();
    let mut exact_rectangle_bound = None;
    // Radial fidelity does not supply the original cap/interface arrangement.
    // Retain an available affine product for later cross-source attachment.
    let profile = if analysis.section.loops.is_empty() {
        None
    } else {
        native_profile(&occurrence, &mut scratch, work, cancel)?
    };
    let rectangle = if let Some(profile) = profile {
        if let Some(bounds) = profile_rectangle(&profile) {
            if actual_used_source_loops.len() != 1 {
                return Err(error(
                    GeometryErrorCode::AnalysisIndeterminate,
                    "native rectangle leaf has more than one actual contributor boundary",
                ));
            }
            let actual = ring(&actual_used_source_loops[0], &mut scratch, work, cancel)?;
            if actual.hole || actual.points.len() != 4 {
                None
            } else {
                let mut halfplanes = [[Interval::point(0.0); 3]; 4];
                for (i, halfplane) in halfplanes.iter_mut().enumerate() {
                    work.charge(1, cancel)?;
                    let a = actual.points[i];
                    let b = actual.points[(i + 1) % 4];
                    let c = actual.points[(i + 2) % 4];
                    if cross(delta(b, a), delta(c, b)).lo <= 0.0 {
                        return Err(error(
                            GeometryErrorCode::AnalysisIndeterminate,
                            "actual rectangle source-M contributor is not strictly convex/material-left",
                        ));
                    }
                    let d = delta(b, a);
                    *halfplane = [
                        d[1].neg(),
                        d[0],
                        d[1].mul(Interval::point(a[0]))
                            .sub(d[0].mul(Interval::point(a[1]))),
                    ];
                    if halfplane
                        .iter()
                        .any(|v| !v.lo.is_finite() || !v.hi.is_finite())
                    {
                        return Err(error(
                            GeometryErrorCode::ConditioningFailure,
                            "actual source-M edge halfplane is nonfinite",
                        ));
                    }
                }
                // Original native coefficients/trim incidence prove a constant
                // rectangle on the WHOLE interval. Compare actual contributor
                // edges directly to those true straight supports, rather than
                // requiring a sampled SSI polygon to have identical vertices.
                let corners = [
                    [bounds[0][0], bounds[1][0]],
                    [bounds[0][1], bounds[1][0]],
                    [bounds[0][1], bounds[1][1]],
                    [bounds[0][0], bounds[1][1]],
                ];
                let mut deviation = f64::INFINITY;
                for offset in 0..4 {
                    let mut maximum: f64 = 0.0;
                    for (i, &corner) in corners.iter().enumerate() {
                        work.charge(1, cancel)?;
                        maximum =
                            maximum.max(distance_upper(corner, actual.points[(i + offset) % 4])?);
                    }
                    deviation = deviation.min(maximum);
                }
                let source_bound = Interval::point(deviation)
                    .add(Interval::point(profile.coordinate_error_mm))
                    .hi;
                let achieved = if policy.candidate_coordinate_error_mm == 0.0 {
                    source_bound
                } else {
                    Interval::point(source_bound)
                        .add(Interval::point(policy.candidate_coordinate_error_mm))
                        .hi
                };
                if !achieved.is_finite() || achieved > native_policy {
                    return Err(error(
                        GeometryErrorCode::ShapeErrorExceeded,
                        "actual source-M rectangle edges exceed native whole-interval fidelity policy",
                    ));
                }
                exact_rectangle_bound = Some(achieved);
                scratch.owned(
                    std::mem::size_of::<NativeRectangleLeafWitness>()
                        + 2 * std::mem::size_of::<usize>(),
                    work,
                    cancel,
                )?;
                for edge in &profile.edges {
                    scratch.owned(edge.face_id.as_str().len(), work, cancel)?;
                }
                let source_faces = std::array::from_fn(|i| (*profile.edges[i].face_id).clone());
                let mut support_residuals = [[0.0; 2]; 2];
                let mut found = [[false; 2]; 2];
                for edge in &profile.edges {
                    for (p, residual) in [(edge.a, edge.a_residual), (edge.b, edge.b_residual)] {
                        work.charge(1, cancel)?;
                        for axis in 0..2 {
                            for side in 0..2 {
                                if p[axis] == bounds[axis][side] {
                                    if found[axis][side]
                                        && support_residuals[axis][side] != residual[axis]
                                    {
                                        return Err(error(
                                            GeometryErrorCode::AnalysisIndeterminate,
                                            "native rectangle has unresolved rounded support coincidence",
                                        ));
                                    }
                                    found[axis][side] = true;
                                    support_residuals[axis][side] = residual[axis];
                                }
                            }
                        }
                    }
                }
                if found.into_iter().flatten().any(|value| !value) {
                    return Err(error(
                        GeometryErrorCode::KernelFailure,
                        "native rectangle support witness is incomplete",
                    ));
                }
                let source_sides = std::array::from_fn(|i| {
                    let edge = &profile.edges[i];
                    let axis = usize::from(edge.a[0] != edge.b[0]);
                    (2 * axis + usize::from(edge.a[axis] == bounds[axis][1])) as u8
                });
                for face in profile.source_caps {
                    scratch.owned(face.as_str().len(), work, cancel)?;
                }
                let source_caps = profile.source_caps.map(|face| (*face).clone());
                Some(Arc::new(NativeRectangleLeafWitness {
                    bounds,
                    world_material_height_interval_mm: profile.world_height_interval_mm,
                    actual_material_left_halfplanes: halfplanes,
                    source_faces,
                    source_caps,
                    source_sides,
                    support_residuals,
                    height_supports: profile.height_supports,
                    coordinate_error_mm: profile.coordinate_error_mm,
                }))
            }
        } else {
            None
        }
    } else {
        None
    };
    let achieved = match exact_rectangle_bound {
        Some(bound) => {
            // Whole native cap/side incidence establishes a true product over
            // this CLOSED request, independently of a directed input scope.
            proof_kind = NativeSourceLeafProofKind::ClosedSlab;
            if analysis.section.loops.len() != 1 || analysis.section.loops[0].is_hole {
                return Err(error(
                    GeometryErrorCode::AnalysisIndeterminate,
                    "native rectangle/source sample loop provenance is not a unique occupied component",
                ));
            }
            scratch.push(
                &mut loop_recording.loops,
                NativeLoopRegistration {
                    native_index: 0,
                    parent: None,
                },
                work,
                cancel,
            )?;
            bound
        }
        None => certify_separated_candidate_recording(
            std::slice::from_ref(&occurrence),
            actual_used_source_loops,
            policy,
            &mut loop_recording,
            &mut scratch,
            work,
            cancel,
        )?,
    };
    let radial_product = if let Some(radial) = radial_slab {
        let Some(region) = certify_actual_polygon_radial_region(
            actual_used_source_loops,
            radial.product.coordinates(),
            radial.product.kind(),
            work,
            cancel,
        )?
        else {
            return Err(error(
                GeometryErrorCode::AnalysisIndeterminate,
                "actual used source-M polygon has no matching closed radial region",
            ));
        };
        Some(NativeRadialLeaf {
            product_sha256: radial.product.identity_sha256(),
            axis_sha256: radial.product.coordinates().axis_sha256(),
            kind: radial.product.kind(),
            center: radial.product.coordinates().world_center_enclosure(),
            outer_kernel_radius_mm: region.outer_kernel_radius_mm(),
            inner_cover_radius_mm: region.inner_cover_radius_mm(),
            inner_displacement_mm: radial.inner_displacement_mm,
        })
    } else {
        None
    };
    let endpoint_radial_products =
        certify_endpoint_radial_leaves(request, interval, z, work, cancel)?;
    let sample_region_error_mm = Interval::point(analysis.section.achieved_chord_error_mm())
        .add(Interval::point(analysis.section.achieved_plane_error_mm()))
        .add(Interval::point(analysis.section.boolean_tolerance_mm))
        .hi;
    let mut sample_loops = Vec::new();
    scratch.reserve(&mut sample_loops, loop_recording.loops.len(), work, cancel)?;
    let sample_loop_clearance_mm = loop_recording.clearance_mm;
    for (candidate_index, registration) in loop_recording.loops.into_iter().enumerate() {
        let native_index = registration.native_index;
        let original = analysis
            .section
            .loop_source_face_ordinals(native_index)
            .unwrap_or(&[]);
        let mut source_face_ordinals = Vec::new();
        scratch.reserve(&mut source_face_ordinals, original.len(), work, cancel)?;
        source_face_ordinals.extend_from_slice(original);
        sample_loops.push(NativeSourceLoopCapture {
            hole: actual_used_source_loops[candidate_index].is_hole,
            shape: source_loop_shape(&actual_used_source_loops[candidate_index], work, cancel)?,
            source_face_ordinals,
            registration,
        });
    }
    scratch.owned(
        std::mem::size_of::<Vec<NativeSourceLoopCapture>>() + 2 * std::mem::size_of::<usize>(),
        work,
        cancel,
    )?;
    scratch.owned(
        definition.id().as_str().len()
            + std::mem::size_of::<NativeUnionSourceIdentity>()
            + 2 * std::mem::size_of::<usize>(),
        work,
        cancel,
    )?;
    let mut critical_interfaces = [false; 2];
    let mut unresolved_critical_interfaces = [false; 2];
    let sample_band = Interval::point(z).add(Interval {
        lo: -analysis.section.achieved_plane_error_mm(),
        hi: analysis.section.achieved_plane_error_mm(),
    });
    // The complete CLOSED native cylinder atlas rules out world-height events
    // on both cuts, including conservative source stationary-root overcovers.
    // It does not discharge the separate actual-coface collar fold below.
    if radial_product.is_none() && !analysis.event_neighborhoods().is_empty() {
        work.charge(32, cancel)?;
        let row = super::placed::rotation(pose)?[2];
        let bounds = definition.bounds_mm();
        let source_xyz = std::array::from_fn(|axis| [bounds.min[axis], bounds.max[axis]]);
        for event in analysis.event_neighborhoods() {
            work.charge(1, cancel)?;
            let height = source_event_world_height(
                analysis,
                source_xyz,
                row,
                pose.translation_mm[2],
                event.height_bracket_mm(),
                work,
                cancel,
            )?;
            let endpoint_closure = if let Some((face, endpoint)) =
                event.proved_incoming_endpoint_face(definition.id(), &analysis.request())
            {
                // A source endpoint or an outward-rounded height band is not
                // equality in the posed request. Reuse the complete original
                // carrier coefficient identity under the ACTUAL world row;
                // any nonzero/uncertain residual retains the critical gate.
                super::slab::constant_source_face_height(
                    definition,
                    face,
                    super::HeightFunctional {
                        direction: row,
                        offset: Interval::point(pose.translation_mm[2]),
                    },
                    work,
                    cancel,
                )?
                .filter(|height| height.lo == interval[endpoint] && height.hi == interval[endpoint])
                .map(|_| endpoint)
            } else {
                None
            };
            if height.lo <= sample_band.hi {
                critical_interfaces[0] = true;
                unresolved_critical_interfaces[0] |= endpoint_closure != Some(0);
            }
            if height.hi >= sample_band.lo {
                critical_interfaces[1] = true;
                unresolved_critical_interfaces[1] |= endpoint_closure != Some(1);
            }
        }
    }
    if let Some(complex) = &analysis.collar_complex {
        if complex.height_space() != NativeCollarHeightSpace::World {
            return Err(error(
                GeometryErrorCode::AnalysisIndeterminate,
                "source leaf critical interfaces need the actual world-height complex",
            ));
        }
        let interfaces = native_collar_interfaces(
            complex,
            interval,
            [sample_band.lo, sample_band.hi],
            work,
            cancel,
        )?;
        for side in 0..2 {
            if interfaces[side] {
                critical_interfaces[side] = true;
                unresolved_critical_interfaces[side] = true;
            }
        }
    }
    let regular_boundary_tube_mm = (!analysis.has_transition_material()).then(|| {
        Interval::point(analysis.boundary_motion_bound_mm)
            .add(Interval::point(policy.candidate_coordinate_error_mm))
            .hi
    });
    Ok(NativeSourceLeafReceipt {
        source: NativeUnionSourceCapture {
            identity: Arc::new(NativeUnionSourceIdentity {
                occurrence_id: *occurrence_id,
                definition_id: definition.id().clone(),
                material_realization: analysis.material_realization_identity(),
                pose,
            }),
            native_shape_error_policy_mm: native_policy,
        },
        world_height_interval_mm: interval,
        sample_world_z_mm: z,
        candidate_geometry_sha256,
        candidate_xy_bounds_mm,
        native_sample_sha256,
        achieved_shape_error_mm: achieved,
        policy,
        positive: !actual_used_source_loops.is_empty(),
        has_owned_transition_material: analysis.has_transition_material(),
        proof_kind,
        sample_region_error_mm,
        critical_interfaces,
        native_endpoint_closure: std::array::from_fn(|side| {
            critical_interfaces[side] && !unresolved_critical_interfaces[side]
        }),
        regular_boundary_tube_mm,
        native_world_z_invariant: analysis.invariance.is_some() || rectangle.is_some(),
        rectangle,
        native_convex_section_topology: analysis.has_native_convex_section_topology_proof(),
        radial_product,
        sample_loop_clearance_mm,
        endpoint_radial_products,
        sample_coordinate_error_mm: exact_rectangle_bound
            .is_none()
            .then_some(policy.candidate_coordinate_error_mm),
        sample_loops: Arc::new(sample_loops),
    })
}

/// Current placed slab and the adjacent positive leaf claiming endpoint ownership.
pub struct EndpointOwnershipRequest<'a> {
    pub definition: &'a Definition,
    pub occurrence_id: &'a OccurrenceId,
    pub pose: RigidPoseMm,
    pub world_height_interval_mm: [f64; 2],
    pub sample_world_z_mm: f64,
    pub native_shape_error_mm: f64,
    pub adjacent: &'a NativeSourceLeafReceipt,
}

/// Prove that the source has no positive-volume material in the open current
/// slab, while a possible closed endpoint stratum has an actual native source-M
/// base fact in the adjacent CLOSED slab. This records a pending positive-leaf
/// obligation, not completed union success. Native directional support is
/// queried independently; an uncertain endpoint bracket cannot be rounded away.
pub fn certify_endpoint_ownership(
    request: &EndpointOwnershipRequest<'_>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<NativeEndpointOwnership, GeometryError> {
    let &EndpointOwnershipRequest {
        definition,
        occurrence_id,
        pose,
        world_height_interval_mm,
        sample_world_z_mm,
        native_shape_error_mm,
        adjacent,
    } = request;
    work.charge(1, cancel)?;
    pose.validate()?;
    let interval = world_height_interval_mm;
    if !interval.iter().all(|v| v.is_finite())
        || !(interval[0] < sample_world_z_mm && sample_world_z_mm < interval[1])
        || !native_shape_error_mm.is_finite()
        || native_shape_error_mm <= 0.0
    {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "endpoint ownership requires a finite closed slab, strict interior sample and positive native policy",
        ));
    }
    if adjacent.occurrence_id() != *occurrence_id
        || !adjacent.matches_original_material(definition)
        || adjacent.pose() != pose
    {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "endpoint receipt does not bind the same original occurrence/definition/pose",
        ));
    }
    if !adjacent.is_positive() {
        return Err(error(
            GeometryErrorCode::AnalysisIndeterminate,
            "endpoint ownership requires an actually used positive source-M leaf, not an empty base fact",
        ));
    }
    let adjacent_bound=adjacent.closed_shape_error_mm().ok_or_else(||error(GeometryErrorCode::AnalysisIndeterminate,
        "whole-source endpoint ownership needs genuine CLOSED adjacent coverage or an independent native cover proof"))?;
    if adjacent_bound > native_shape_error_mm {
        return Err(error(
            GeometryErrorCode::ShapeErrorExceeded,
            "adjacent CLOSED source-M leaf exceeds the current native policy",
        ));
    }
    let previous = adjacent.world_height_interval_mm();
    let endpoint = if previous[1] == interval[0] {
        NativeOwnedEndpoint::Lower
    } else if previous[0] == interval[1] {
        NativeOwnedEndpoint::Upper
    } else {
        return Err(error(
            GeometryErrorCode::AnalysisIndeterminate,
            format!(
                "original occurrence {} endpoint ownership requires exact adjacent CLOSED intervals",
                occurrence_id.get()
            ),
        ));
    };
    let bounds_policy = BoundsPolicy {
        maximum_endpoint_uncertainty_mm: native_shape_error_mm
            .min(spiling_contracts::geometry::SECTION_BOOLEAN_TOLERANCE_MM),
    };
    let support = placed_bounds(
        definition,
        pose,
        [0.0, 0.0, 1.0],
        bounds_policy,
        work,
        cancel,
    )?;
    let excluded = match endpoint {
        NativeOwnedEndpoint::Lower => support.maximum[1] <= interval[0],
        NativeOwnedEndpoint::Upper => support.minimum[0] >= interval[1],
    };
    if !excluded {
        return Err(error(
            GeometryErrorCode::AnalysisIndeterminate,
            format!(
                "original occurrence {} native support does not exclude open-slab material at the {:?} endpoint; support min {:?}, max {:?}, current {:?}",
                occurrence_id.get(),
                endpoint,
                support.minimum,
                support.maximum,
                interval
            ),
        ));
    }
    let x = placed_bounds(
        definition,
        pose,
        [1.0, 0.0, 0.0],
        bounds_policy,
        work,
        cancel,
    )?;
    let y = placed_bounds(
        definition,
        pose,
        [0.0, 1.0, 0.0],
        bounds_policy,
        work,
        cancel,
    )?;
    let mut source = adjacent.source.clone();
    source.native_shape_error_policy_mm = native_shape_error_mm;
    Ok(NativeEndpointOwnership {
        source,
        world_height_interval_mm: interval,
        sample_world_z_mm,
        endpoint,
        source_world_z_support: Some(support),
        occupied_xy_bounds_mm: [[x.minimum[0], x.maximum[1]], [y.minimum[0], y.maximum[1]]],
        adjacent: adjacent.clone(),
        stratum: None,
    })
}

/// Bind a genuinely proved original horizontal face stratum to the adjacent
/// source-M CORE on its material-facing side. Strict outward orientation and
/// original two-sided coedge incidence (the opaque stratum authority) imply
/// every face interior point belongs to that adjacent material-core closure;
/// closure includes its original coedges. No polygon coverage guess is used.
pub fn certify_endpoint_stratum_ownership(
    stratum: &NativeEndpointStratum,
    adjacent: &NativeSourceLeafReceipt,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<NativeEndpointOwnership, GeometryError> {
    work.charge(1, cancel)?;
    let unresolved = || {
        error(
            GeometryErrorCode::AnalysisIndeterminate,
            "native face stratum lacks an exact placed world-horizontal endpoint/incidence capture",
        )
    };
    let pose = stratum.placed_pose().ok_or_else(unresolved)?;
    let interval = stratum.world_height_interval_mm().ok_or_else(unresolved)?;
    let z = stratum.sample_world_z_mm().ok_or_else(unresolved)?;
    let native_policy = stratum
        .world_shape_error_policy_mm()
        .ok_or_else(unresolved)?;
    let height = stratum.world_height_mm().ok_or_else(unresolved)?;
    let bounds = stratum.world_xy_bounds_mm().ok_or_else(unresolved)?;
    let endpoint = stratum.endpoint();
    let (index, sign) = match endpoint {
        NativeOwnedEndpoint::Lower => (0, 1),
        NativeOwnedEndpoint::Upper => (1, -1),
    };
    if !interval.iter().all(|value| value.is_finite())
        || !(interval[0] < z && z < interval[1])
        || !height.is_finite()
        || height != interval[index]
        || stratum.outward_height_sign() != sign
        || !native_policy.is_finite()
        || native_policy <= 0.0
        || bounds
            .iter()
            .any(|range| !range.iter().all(|value| value.is_finite()) || range[0] >= range[1])
    {
        return Err(unresolved());
    }
    if adjacent.definition_id() != stratum.definition_id()
        || adjacent.material_realization_identity() != stratum.material_realization_identity()
        || adjacent.pose() != pose
    {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "native stratum and adjacent source-M core have different original source/pose",
        ));
    }
    let adjacent_interval = adjacent.world_height_interval_mm();
    let material_side = match endpoint {
        NativeOwnedEndpoint::Lower => {
            adjacent_interval[1] == interval[0] && adjacent_interval[0] < interval[0]
        }
        NativeOwnedEndpoint::Upper => {
            adjacent_interval[0] == interval[1] && interval[1] < adjacent_interval[1]
        }
    };
    if !material_side || !adjacent.is_positive() {
        return Err(error(
            GeometryErrorCode::AnalysisIndeterminate,
            "original native face stratum requires an actual positive adjacent core leaf on its exact material-facing CLOSED side",
        ));
    }
    let adjacent_bound=adjacent.closed_shape_error_mm().ok_or_else(||error(GeometryErrorCode::AnalysisIndeterminate,
        "native endpoint face needs genuine CLOSED adjacent coverage; restricted core/directed bounds are insufficient"))?;
    if adjacent_bound > native_policy {
        return Err(error(
            GeometryErrorCode::ShapeErrorExceeded,
            "native face stratum adjacent source-M coverage exceeds its native policy",
        ));
    }
    let mut scratch = Scratch { bytes: 0 };
    scratch.owned(
        stratum.face_id().as_str().len()
            + std::mem::size_of::<NativeStratumSourceCapture>()
            + 2 * std::mem::size_of::<usize>(),
        work,
        cancel,
    )?;
    let capture = Arc::new(NativeStratumSourceCapture {
        face_id: stratum.face_id().clone(),
        face_index: stratum.face_index(),
        world_height_mm: height,
        outward_height_sign: sign,
    });
    let mut source = adjacent.source.clone();
    source.native_shape_error_policy_mm = native_policy;
    Ok(NativeEndpointOwnership {
        source,
        world_height_interval_mm: interval,
        sample_world_z_mm: z,
        endpoint,
        source_world_z_support: None,
        occupied_xy_bounds_mm: bounds,
        adjacent: adjacent.clone(),
        stratum: Some(capture),
    })
}

fn owns_native_stratum(
    ownership: &NativeEndpointOwnership,
    stratum: &NativeEndpointStratum,
) -> bool {
    let Some(capture) = &ownership.stratum else {
        return false;
    };
    capture.face_id == *stratum.face_id()
        && capture.face_index == stratum.face_index()
        && ownership.definition_id() == stratum.definition_id()
        && ownership.material_realization_identity() == stratum.material_realization_identity()
        && Some(ownership.pose()) == stratum.placed_pose()
        && Some(ownership.world_height_interval_mm) == stratum.world_height_interval_mm()
        && Some(ownership.sample_world_z_mm) == stratum.sample_world_z_mm()
        && Some(ownership.native_shape_error_policy_mm()) == stratum.world_shape_error_policy_mm()
        && ownership.endpoint == stratum.endpoint()
        && Some(capture.world_height_mm) == stratum.world_height_mm()
        && capture.outward_height_sign == stratum.outward_height_sign()
        && Some(ownership.occupied_xy_bounds_mm) == stratum.world_xy_bounds_mm()
}

fn evidence_request(
    evidence: &NativeUnionOccurrenceEvidence<'_>,
) -> Result<([f64; 2], f64), GeometryError> {
    match evidence {
        NativeUnionOccurrenceEvidence::Slab { analysis, .. }
        | NativeUnionOccurrenceEvidence::SlabWithCollar { analysis, .. } => Ok((
            analysis.world_height_interval_mm().ok_or_else(|| {
                error(
                    GeometryErrorCode::InvalidGeometry,
                    "native union requires captured placed slab intervals",
                )
            })?,
            analysis.sample_world_z_mm().ok_or_else(|| {
                error(
                    GeometryErrorCode::InvalidGeometry,
                    "native union requires a captured world sample",
                )
            })?,
        )),
        NativeUnionOccurrenceEvidence::EndpointOwned(ownership) => Ok((
            ownership.world_height_interval_mm(),
            ownership.sample_world_z_mm(),
        )),
    }
}

/// Certify incidence and fidelity of the actual final M polygon union. Curved
/// cross-source interactions, non-invariant overlapping slabs and uncertain
/// contacts have no implicit fallback: they retain a typed pair refusal.
pub fn certify_native_union(
    occurrences: &[NativeUnionOccurrence<'_, '_>],
    candidate_union_loops: &[SectionLoopMm],
    policy: NativeUnionPolicy,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<PendingNativeUnionProof, GeometryError> {
    work.charge(1, cancel)?;
    if !policy.candidate_coordinate_error_mm.is_finite()
        || policy.candidate_coordinate_error_mm < 0.0
    {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "native union coordinate policy must be finite and nonnegative",
        ));
    }
    let first = occurrences.first().ok_or_else(|| {
        error(
            GeometryErrorCode::InvalidGeometry,
            "joint native union requires original source occurrences",
        )
    })?;
    let (interval, z) = evidence_request(&first.evidence)?;
    if !interval.iter().all(|v| v.is_finite()) || !(interval[0] < z && z < interval[1]) {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "native union sample must be strictly interior to the common CLOSED slab",
        ));
    }
    let mut scratch = Scratch { bytes: 0 };
    let mut sources = Vec::new();
    let mut active = Vec::new();
    let mut endpoint_dependencies = Vec::new();
    let mut positive_leaves = Vec::new();
    let mut owned_critical_slab_count = 0;
    let mut collar_dependencies = Vec::new();
    for (index, occurrence) in occurrences.iter().enumerate() {
        work.charge(1, cancel)?;
        occurrence.source.source_view()?;
        occurrence.pose.validate()?;
        for previous in &occurrences[..index] {
            work.charge(1, cancel)?;
            if previous.occurrence_id == occurrence.occurrence_id {
                return Err(error(
                    GeometryErrorCode::InvalidGeometry,
                    "native union repeats an original occurrence identity",
                ));
            }
        }
        if evidence_request(&occurrence.evidence)? != (interval, z) {
            return Err(error(
                GeometryErrorCode::InvalidGeometry,
                format!(
                    "original occurrence {} has a different CLOSED world slab/sample capture",
                    occurrence.occurrence_id.get()
                ),
            ));
        }
        let source = match &occurrence.evidence {
            NativeUnionOccurrenceEvidence::Slab {
                analysis,
                leaf,
                endpoint_ownerships,
            }
            | NativeUnionOccurrenceEvidence::SlabWithCollar {
                analysis,
                leaf,
                endpoint_ownerships,
                ..
            } => {
                let native_policy = analysis.world_shape_error_policy_mm().ok_or_else(|| {
                    error(
                        GeometryErrorCode::InvalidGeometry,
                        "native union source has no placed policy capture",
                    )
                })?;
                if !analysis.matches_original_material(occurrence.source.definition())
                    || analysis.section.definition_id() != occurrence.source.definition().id()
                    || !analysis.section.is_native_intersection()
                    || analysis.placed_pose() != Some(occurrence.pose)
                    || !native_policy.is_finite()
                    || native_policy <= 0.0
                    || !analysis.boundary_motion_bound_mm.is_finite()
                    || analysis.boundary_motion_bound_mm < 0.0
                {
                    return Err(error(
                        GeometryErrorCode::InvalidGeometry,
                        format!(
                            "original occurrence {} does not match its immutable native source/pose/policy",
                            occurrence.occurrence_id.get()
                        ),
                    ));
                }
                if let Some(occupied) = &analysis.occupied {
                    if occupied.definition_id() != occurrence.source.definition().id() {
                        return Err(error(
                            GeometryErrorCode::InvalidGeometry,
                            "native union occupied projection has a different original source",
                        ));
                    }
                    if !analysis.has_transition_material()
                        && analysis.section.loops.is_empty()
                        && !occupied.loops.is_empty()
                    {
                        return Err(original_pair_error(
                            occurrences,
                            index,
                            index,
                            unresolved(
                                index,
                                index,
                                "empty sample plus nonempty CLOSED occupancy requires an independently certified endpoint receipt",
                            ),
                        ));
                    }
                } else if analysis.invariance.is_none() {
                    return Err(original_pair_error(
                        occurrences,
                        index,
                        index,
                        unresolved(
                            index,
                            index,
                            "source lacks whole-slab occupied or invariant authority",
                        ),
                    ));
                }
                if let Some(invariance) = &analysis.invariance {
                    let range = invariance.height_interval_mm();
                    if !invariance.matches_original_material(occurrence.source.definition())
                        || invariance.pose() != Some(occurrence.pose)
                        || range[0] > interval[0]
                        || range[1] < interval[1]
                    {
                        return Err(error(
                            GeometryErrorCode::InvalidGeometry,
                            "source invariance does not cover the captured CLOSED world slab",
                        ));
                    }
                }
                scratch.push(
                    &mut active,
                    SlabOccurrence {
                        occurrence_index: index,
                        source: occurrence.source,
                        pose: occurrence.pose,
                        analysis,
                        source_metric: if leaf.proof_kind
                            == NativeSourceLeafProofKind::SampleToClosedSource
                            && analysis.closed_shape_error_mm().is_some()
                        {
                            NativeSourceLeafProofKind::ClosedSlab
                        } else {
                            leaf.proof_kind
                        },
                        leaf: Some(*leaf),
                    },
                    work,
                    cancel,
                )?;
                if leaf.occurrence_id() != occurrence.occurrence_id
                    || !leaf.matches_original_material(occurrence.source.definition())
                    || leaf.material_realization_identity()
                        != analysis.material_realization_identity()
                    || leaf.pose() != occurrence.pose
                    || leaf.world_height_interval_mm() != interval
                    || leaf.sample_world_z_mm() != z
                    || leaf.native_shape_error_policy_mm() != native_policy
                    || leaf.native_sample_sha256() != &native_sample_hash(analysis, work, cancel)?
                    || leaf.has_owned_transition_material != analysis.has_transition_material()
                    || leaf.is_positive() != !analysis.section.loops.is_empty()
                {
                    return Err(error(
                        GeometryErrorCode::InvalidGeometry,
                        "source-M contributor leaf does not bind the actual immutable native slab/source/request",
                    ));
                }
                let collar = match &occurrence.evidence {
                    NativeUnionOccurrenceEvidence::SlabWithCollar { collar, .. } => Some(*collar),
                    _ => None,
                };
                if analysis.collar_complex.is_some() != collar.is_some() {
                    return Err(error(
                        GeometryErrorCode::AnalysisIndeterminate,
                        "native critical transition requires its actual-M incidence+coverage collar",
                    ));
                }
                // This applies to the ENTIRE accepted slab, including the two
                // ends of a stream: checking events only between sample planes
                // would silently lose a birth/death before the first sample.
                // Incoming original cap incidence plus regular continuation
                // closes its own exact posed endpoint. It does NOT authorize
                // cross-height critical glue; the stream still checks that
                // independently. Whole-source convexity or stationary native
                // products separately discharge their complete event bands.
                if collar.is_none()
                    && (0..2).any(|side| {
                        leaf.critical_interfaces[side] && !leaf.native_endpoint_closure[side]
                    })
                    && !leaf.native_world_z_invariant
                    && !leaf.native_convex_section_topology
                    && !(occurrences.len() == 1 && leaf.radial_product.is_some())
                    && (leaf.is_positive() || analysis.closed_shape_error_mm().is_none())
                {
                    return Err(original_pair_error(
                        occurrences,
                        index,
                        index,
                        unresolved(
                            index,
                            index,
                            &format!(
                                "native critical band requires original coface/root incidence or a sealed whole-source convex proof; closed_world_interval_mm={interval:?} native_reference_z_mm={z} critical_sides={:?} native_endpoint_closures={:?}",
                                leaf.critical_interfaces, leaf.native_endpoint_closure,
                            ),
                        ),
                    ));
                }
                if let Some(collar) = collar {
                    if !same_leaf(leaf, collar.current_leaf()) {
                        return Err(error(
                            GeometryErrorCode::InvalidGeometry,
                            "critical collar is bound to another actual source-M leaf/refinement",
                        ));
                    }
                    let complex = analysis.collar_complex.as_ref().unwrap();
                    for stratum in analysis.endpoint_strata() {
                        if !complex.event_faces().any(|(face, _, sign)| {
                            face == stratum.face_index() && sign == stratum.outward_height_sign()
                        }) {
                            return Err(error(
                                GeometryErrorCode::AnalysisIndeterminate,
                                "native critical collar does not contain an outgoing original endpoint face",
                            ));
                        }
                    }
                    if !endpoint_ownerships.is_empty() {
                        return Err(error(
                            GeometryErrorCode::InvalidGeometry,
                            "critical collar already owns native endpoint strata; duplicate ownership is not accepted",
                        ));
                    }
                    scratch.push(&mut collar_dependencies, collar.clone(), work, cancel)?;
                }
                if collar.is_none() && endpoint_ownerships.len() != analysis.endpoint_strata().len()
                {
                    return Err(error(
                        GeometryErrorCode::InvalidGeometry,
                        "every outgoing native endpoint face requires its actual adjacent source-M ownership fact",
                    ));
                }
                for expected in analysis
                    .endpoint_strata()
                    .iter()
                    .filter(|_| collar.is_none())
                {
                    let mut matches = 0;
                    for ownership in *endpoint_ownerships {
                        work.charge(1, cancel)?;
                        matches += usize::from(
                            owns_native_stratum(ownership, expected)
                                && ownership.occurrence_id() == occurrence.occurrence_id,
                        );
                    }
                    if matches != 1 {
                        return Err(error(
                            GeometryErrorCode::InvalidGeometry,
                            "native endpoint face/pose/request ownership is missing, duplicated, or bound to another source",
                        ));
                    }
                }
                for ownership in *endpoint_ownerships {
                    scratch.push(&mut endpoint_dependencies, ownership.clone(), work, cancel)?;
                }
                owned_critical_slab_count +=
                    usize::from(collar.is_some() || !endpoint_ownerships.is_empty());
                if leaf.is_positive() {
                    scratch.push(&mut positive_leaves, (*leaf).clone(), work, cancel)?;
                }
                leaf.source.clone()
            }
            NativeUnionOccurrenceEvidence::EndpointOwned(ownership) => {
                if !ownership
                    .source
                    .matches_original_material(occurrence.source.definition())
                    || ownership.occurrence_id() != occurrence.occurrence_id
                    || ownership.pose() != occurrence.pose
                    || ownership.stratum.is_some()
                {
                    return Err(error(
                        GeometryErrorCode::InvalidGeometry,
                        "endpoint ownership has a different original occurrence/definition/pose",
                    ));
                }
                scratch.push(
                    &mut endpoint_dependencies,
                    (*ownership).clone(),
                    work,
                    cancel,
                )?;
                owned_critical_slab_count += 1;
                ownership.source.clone()
            }
        };
        scratch.push(&mut sources, source, work, cancel)?;
    }
    let target_envelopes =
        prepare_target_envelopes(occurrences, &active, &mut scratch, work, cancel)?;
    let collar_attachments = certify_collar_cross_incidence(
        &CollarCrossIncidenceQuery {
            occurrences,
            active: &active,
            target_envelopes: &target_envelopes,
            endpoints: &endpoint_dependencies,
            collars: &collar_dependencies,
            policy,
        },
        &mut scratch,
        work,
        cancel,
    )?;
    let hash = candidate_hash(candidate_union_loops, z, work, cancel)?;
    let mut interface_attachments = certify_contact_incidence(
        occurrences,
        &active,
        &endpoint_dependencies,
        policy,
        &mut scratch,
        work,
        cancel,
    )?;
    if interface_attachments.is_empty() {
        interface_attachments = collar_attachments;
    } else {
        for attachment in collar_attachments {
            scratch.push(&mut interface_attachments, attachment, work, cancel)?;
        }
    }
    let NativePartitionProof {
        mut achieved,
        raw_inclusion: raw_leaf_to_candidate_error_mm,
        boundary_edges,
        boundary_sources,
        kind,
        mixed_varying,
        preserved_source_occurrences,
        target_envelopes,
    } = certify_interaction_groups(
        &InteractionGroupsQuery {
            original: occurrences,
            active: &active,
            candidate: candidate_union_loops,
            policy,
        },
        target_envelopes,
        &mut scratch,
        work,
        cancel,
    )?;
    // This is aggregate CLOSED-stream coverage via real adjacent candidates,
    // not the false local claim Hausdorff(empty current prism, contact)=0.
    let material_core_shape_error_mm = active
        .iter()
        .all(|source| source.source_metric != NativeSourceLeafProofKind::SampleToClosedSource)
        .then_some(achieved);
    for collar in &collar_dependencies {
        achieved = achieved
            .max(collar.native_collar().sample_to_closed_source_error_mm())
            .max(collar.native_collar().closed_source_to_material_error_mm());
    }
    for dependency in &endpoint_dependencies {
        work.charge(1, cancel)?;
        achieved = achieved.max(
            dependency
                .adjacent
                .directed_sample_to_closed_source_error_mm(),
        );
    }
    let mut requires_interface_complex = mixed_varying || !interface_attachments.is_empty();
    for dependency in &endpoint_dependencies {
        work.charge(1, cancel)?;
        let mut same_current_product = false;
        for current in &positive_leaves {
            work.charge(1, cancel)?;
            if same_leaf(current, &dependency.adjacent)
                && current.rectangle.is_some()
                && current.proof_kind == NativeSourceLeafProofKind::ClosedSlab
            {
                same_current_product = true;
                break;
            }
        }
        requires_interface_complex |= !same_current_product;
    }
    for collar in &collar_dependencies {
        work.charge(1, cancel)?;
        let current = collar.current_leaf();
        // A label alone is not authority: this is the independently proved
        // original native cap/side/trim PRODUCT over this entire CLOSED I.
        let mut same_current_product = current.rectangle.is_some()
            && current.proof_kind == NativeSourceLeafProofKind::ClosedSlab
            && current.is_positive();
        let mut used_current = false;
        for dependency in collar.used_source_leaves() {
            work.charge(1, cancel)?;
            let same = same_leaf(dependency, current);
            used_current |= same;
            same_current_product &= same;
        }
        requires_interface_complex |= !same_current_product || !used_current;
    }
    let pending = PendingNativeUnionProof {
        sources,
        world_height_interval_mm: interval,
        sample_world_z_mm: z,
        candidate_geometry_sha256: hash,
        achieved_shape_error_mm: achieved,
        material_core_shape_error_mm,
        raw_leaf_to_candidate_error_mm,
        preserved_source_occurrences,
        target_envelopes,
        material_component_count: candidate_union_loops.iter().filter(|l| !l.is_hole).count(),
        bounded_void_count: candidate_union_loops.iter().filter(|l| l.is_hole).count(),
        proof_kind: kind,
        policy,
        boundary_edges,
        boundary_sources,
        endpoint_dependencies,
        positive_leaves,
        interface_attachments,
        owned_critical_slab_count,
        collar_dependencies,
        regular_interfaces: Vec::new(),
        requires_interface_complex,
    };
    if pending.retained_bytes() > MAX_SCRATCH {
        return Err(error(
            GeometryErrorCode::ResourceLimit,
            "native union retained source/leaf/interface evidence exceeds the bounded proof capacity",
        ));
    }
    Ok(pending)
}

fn bounds_separated(a: [[f64; 2]; 2], b: [[f64; 2]; 2], padding: f64) -> bool {
    (0..2).any(|axis| {
        down(b[axis][0] - a[axis][1]) > padding || down(a[axis][0] - b[axis][1]) > padding
    })
}

fn rectangle_supports_resolved(
    a: &NativeRectangleLeafWitness,
    b: &NativeRectangleLeafWitness,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    for axis in 0..2 {
        for i in 0..2 {
            for j in 0..2 {
                work.charge(1, cancel)?;
                if a.bounds[axis][i] == b.bounds[axis][j]
                    && a.support_residuals[axis][i] != b.support_residuals[axis][j]
                {
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
}
fn rectangle_leaf_contact(
    a: &NativeSourceLeafReceipt,
    b: &NativeSourceLeafReceipt,
    z: f64,
    coordinate: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let (Some(left), Some(right)) = (&a.rectangle, &b.rectangle) else {
        return Ok(false);
    };
    if [left, right].iter().any(|w| {
        w.world_material_height_interval_mm[0] > z || w.world_material_height_interval_mm[1] < z
    }) || !rectangle_supports_resolved(left, right, work, cancel)?
    {
        return Ok(false);
    }
    let overlap: [[f64; 2]; 2] = std::array::from_fn(|axis| {
        [
            left.bounds[axis][0].max(right.bounds[axis][0]),
            left.bounds[axis][1].min(right.bounds[axis][1]),
        ]
    });
    if overlap.iter().any(|interval| interval[0] >= interval[1]) {
        return Ok(false);
    }
    let point = overlap.map(|interval| 0.5 * interval[0] + 0.5 * interval[1]);
    let margin = Interval::point(a.native_shape_error_policy_mm())
        .add(Interval::point(b.native_shape_error_policy_mm()))
        .add(Interval::point(coordinate))
        .add(Interval::point(left.coordinate_error_mm))
        .add(Interval::point(right.coordinate_error_mm));
    for rectangle in [left, right] {
        for (axis, &coordinate) in point.iter().enumerate() {
            if Interval::point(coordinate)
                .sub(Interval::point(rectangle.bounds[axis][0]))
                .lo
                <= margin.hi
                || Interval::point(rectangle.bounds[axis][1])
                    .sub(Interval::point(coordinate))
                    .lo
                    <= margin.hi
            {
                return Ok(false);
            }
        }
        for h in rectangle.actual_material_left_halfplanes {
            work.charge(1, cancel)?;
            let normal = h[0].square().add(h[1].square()).sqrt()?;
            if h[0]
                .mul(Interval::point(point[0]))
                .add(h[1].mul(Interval::point(point[1])))
                .add(h[2])
                .lo
                <= normal.mul(margin).hi
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}
fn collar_endpoint_contact(
    collar: &NativeCriticalCollarIncidence,
) -> Option<NativeEndpointOwnership> {
    let current = collar.current_leaf();
    if current.is_positive() {
        return None;
    }
    let interval = current.world_height_interval_mm();
    for adjacent in collar.used_source_leaves() {
        let Some(rectangle) = adjacent.rectangle.as_ref() else {
            continue;
        };
        if adjacent.closed_shape_error_mm().is_none() {
            continue;
        }
        let physical = rectangle.height_supports;
        let endpoint = if adjacent.world_height_interval_mm()[1] == interval[0]
            && physical[1] == [interval[0], 0.0]
        {
            NativeOwnedEndpoint::Lower
        } else if adjacent.world_height_interval_mm()[0] == interval[1]
            && physical[0] == [interval[1], 0.0]
        {
            NativeOwnedEndpoint::Upper
        } else {
            continue;
        };
        let bottom = native_support_interval(physical[0]);
        let top = native_support_interval(physical[1]);
        return Some(NativeEndpointOwnership {
            source: current.source.clone(),
            world_height_interval_mm: interval,
            sample_world_z_mm: current.sample_world_z_mm(),
            endpoint,
            source_world_z_support: Some(ProjectedBounds {
                minimum: [bottom.lo, bottom.hi],
                maximum: [top.lo, top.hi],
                achieved_uncertainty_mm: (bottom.hi - bottom.lo).max(top.hi - top.lo),
            }),
            occupied_xy_bounds_mm: rectangle.bounds,
            adjacent: adjacent.clone(),
            stratum: None,
        });
    }
    None
}
struct CollarCrossIncidenceQuery<'a, 'source> {
    occurrences: &'a [NativeUnionOccurrence<'source, 'source>],
    active: &'a [SlabOccurrence<'source, 'source>],
    target_envelopes: &'a [NativeTargetEnvelope],
    endpoints: &'a [NativeEndpointOwnership],
    collars: &'a [NativeCriticalCollarIncidence],
    policy: NativeUnionPolicy,
}

fn certify_collar_cross_incidence(
    query: &CollarCrossIncidenceQuery<'_, '_>,
    scratch: &mut Scratch,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Vec<NativeInterfaceAttachment>, GeometryError> {
    let &CollarCrossIncidenceQuery {
        occurrences,
        active,
        target_envelopes,
        endpoints,
        collars,
        policy,
    } = query;
    let mut attachments = Vec::new();
    for collar in collars {
        let own = collar.current_leaf();
        let source = source_index(occurrences, own.occurrence_id(), work, cancel)?;
        let envelope = collar.occupied_xy_bounds_mm();
        let own_target = target_envelope(target_envelopes, own.occurrence_id(), work, cancel)?;
        for (other_index, other) in active.iter().enumerate() {
            if other.occurrence_index == source {
                continue;
            }
            let id = occurrences[other.occurrence_index].occurrence_id;
            if let (Some(a), Some(b)) = (own_target, target_envelopes[other_index].1.as_ref())
                && separated_tubes(Some(a), Some(b))
            {
                continue;
            }
            let other_collar = collars
                .iter()
                .find(|value| value.current_leaf().occurrence_id() == id);
            let other_bounds = if let Some(value) = other_collar {
                Some(value.occupied_xy_bounds_mm())
            } else {
                section_bounds(
                    &other
                        .analysis
                        .occupied
                        .as_ref()
                        .unwrap_or(&other.analysis.section)
                        .loops,
                    work,
                    cancel,
                )?
            };
            let Some(other_bounds) = other_bounds else {
                continue;
            };
            let padding = Interval::point(own.native_shape_error_policy_mm())
                .add(Interval::point(
                    other.analysis.world_shape_error_policy_mm().unwrap(),
                ))
                .add(Interval::point(policy.candidate_coordinate_error_mm))
                .hi;
            if bounds_separated(envelope, other_bounds, padding) {
                continue;
            }
            let other_leaf = match &occurrences[other.occurrence_index].evidence {
                NativeUnionOccurrenceEvidence::Slab { leaf, .. }
                | NativeUnionOccurrenceEvidence::SlabWithCollar { leaf, .. } => *leaf,
                NativeUnionOccurrenceEvidence::EndpointOwned(_) => unreachable!(),
            };
            // Both native products cover the entire request; the subsequent
            // original-support arrangement proves the actual joint union.
            if own.rectangle.is_some()
                && other_leaf.rectangle.is_some()
                && collar
                    .used_source_leaves()
                    .iter()
                    .all(|leaf| same_leaf(leaf, own))
                && other_collar.is_none_or(|value| {
                    value
                        .used_source_leaves()
                        .iter()
                        .all(|leaf| same_leaf(leaf, other_leaf))
                })
            {
                continue;
            }
            let own_contact = collar_endpoint_contact(collar);
            let other_contact = other_collar.and_then(collar_endpoint_contact);
            let proof = if let Some(contact) = &own_contact {
                rectangle_interface(contact, other_leaf, policy, work, cancel)?
            } else if let Some(contact) = &other_contact {
                rectangle_interface(contact, own, policy, work, cancel)?
            } else {
                None
            };
            if let Some(proof) = proof {
                if !attachments.iter().any(|old: &NativeInterfaceAttachment| {
                    old.endpoint_occurrence_id == proof.endpoint_occurrence_id
                        && old.current_occurrence_id == proof.current_occurrence_id
                        && old.endpoint_leaf_sha256 == proof.endpoint_leaf_sha256
                        && old.current_leaf_sha256 == proof.current_leaf_sha256
                }) {
                    scratch.push(&mut attachments, proof, work, cancel)?;
                }
                continue;
            }
            return Err(original_pair_error(
                occurrences,
                source,
                other.occurrence_index,
                unresolved(
                    source,
                    other.occurrence_index,
                    "critical cross-source contact needs exact native cap/interface arrangement or shared native pair continuation",
                ),
            ));
        }
        for endpoint in endpoints {
            if endpoint.occurrence_id() == own.occurrence_id() {
                continue;
            }
            let other = source_index(occurrences, endpoint.occurrence_id(), work, cancel)?;
            let padding = Interval::point(own.native_shape_error_policy_mm())
                .add(Interval::point(endpoint.native_shape_error_policy_mm()))
                .add(Interval::point(policy.candidate_coordinate_error_mm))
                .hi;
            if !bounds_separated(envelope, endpoint.occupied_xy_bounds_mm(), padding) {
                return Err(original_pair_error(
                    occurrences,
                    source,
                    other,
                    unresolved(
                        source,
                        other,
                        "critical collar/endpoint interaction has no native common interface proof",
                    ),
                ));
            }
        }
    }
    Ok(attachments)
}
fn profile_rectangle(profile: &NativeProfile<'_>) -> Option<[[f64; 2]; 2]> {
    if profile.edges.len() != 4 {
        return None;
    }
    let mut bounds = [[f64::INFINITY, f64::NEG_INFINITY]; 2];
    for edge in &profile.edges {
        for p in [edge.a, edge.b] {
            for axis in 0..2 {
                bounds[axis][0] = bounds[axis][0].min(p[axis]);
                bounds[axis][1] = bounds[axis][1].max(p[axis]);
            }
        }
    }
    let [[x0, x1], [y0, y1]] = bounds;
    if x0 >= x1 || y0 >= y1 {
        return None;
    }
    let expected = [
        ([x0, y0], [x1, y0]),
        ([x1, y0], [x1, y1]),
        ([x1, y1], [x0, y1]),
        ([x0, y1], [x0, y0]),
    ];
    expected
        .iter()
        .all(|(a, b)| {
            profile
                .edges
                .iter()
                .filter(|e| e.a == *a && e.b == *b)
                .count()
                == 1
        })
        .then_some(bounds)
}
fn source_index(
    occurrences: &[NativeUnionOccurrence<'_, '_>],
    id: OccurrenceId,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<usize, GeometryError> {
    for (index, occurrence) in occurrences.iter().enumerate() {
        work.charge(1, cancel)?;
        if occurrence.occurrence_id == id {
            return Ok(index);
        }
    }
    Err(error(
        GeometryErrorCode::InvalidGeometry,
        "endpoint ownership original occurrence is absent from the union",
    ))
}
fn rectangle_interface(
    contact: &NativeEndpointOwnership,
    current: &NativeSourceLeafReceipt,
    policy: NativeUnionPolicy,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<NativeInterfaceAttachment>, GeometryError> {
    work.charge(1, cancel)?;
    let (Some(a), Some(b)) = (&contact.adjacent.rectangle, &current.rectangle) else {
        return Ok(None);
    };
    let endpoint = match contact.endpoint {
        NativeOwnedEndpoint::Lower => contact.world_height_interval_mm[0],
        NativeOwnedEndpoint::Upper => contact.world_height_interval_mm[1],
    };
    let cap_index = match contact.endpoint {
        NativeOwnedEndpoint::Lower => 1,
        NativeOwnedEndpoint::Upper => 0,
    };
    if a.height_supports[cap_index] != [endpoint, 0.0]
        || b.world_material_height_interval_mm[0] > endpoint
        || b.world_material_height_interval_mm[1] < endpoint
        || !rectangle_supports_resolved(a, b, work, cancel)?
    {
        return Ok(None);
    }
    let overlap: [[f64; 2]; 2] = std::array::from_fn(|axis| {
        [
            a.bounds[axis][0].max(b.bounds[axis][0]),
            a.bounds[axis][1].min(b.bounds[axis][1]),
        ]
    });
    if overlap.iter().any(|range| range[0] >= range[1]) {
        return Ok(None);
    }
    let point = overlap.map(|range| range[0] * 0.5 + range[1] * 0.5);
    if (0..2).any(|axis| {
        !point[axis].is_finite()
            || !(overlap[axis][0] < point[axis] && point[axis] < overlap[axis][1])
    }) {
        return Ok(None);
    }
    // Strict convexity of both ACTUAL source-M polygons and a strictly common
    // interior point prove one actual attachment disk. Native attachment comes
    // independently from the exact cap support cell arrangement below.
    let margin = Interval::point(contact.native_shape_error_policy_mm())
        .add(Interval::point(current.native_shape_error_policy_mm()))
        .add(Interval::point(policy.candidate_coordinate_error_mm))
        .add(Interval::point(a.coordinate_error_mm))
        .add(Interval::point(b.coordinate_error_mm));
    for rectangle in [a, b] {
        for h in rectangle.actual_material_left_halfplanes {
            work.charge(1, cancel)?;
            let normal = h[0].square().add(h[1].square()).sqrt()?;
            if h[0]
                .mul(Interval::point(point[0]))
                .add(h[1].mul(Interval::point(point[1])))
                .add(h[2])
                .lo
                <= normal.mul(margin).hi
            {
                return Ok(None);
            }
        }
    }
    let mut events = [[0.0; 4]; 2];
    let mut counts = [0; 2];
    for axis in 0..2 {
        let mut values = [
            a.bounds[axis][0],
            a.bounds[axis][1],
            b.bounds[axis][0],
            b.bounds[axis][1],
        ];
        for i in 1..4 {
            for j in (1..=i).rev() {
                work.charge(1, cancel)?;
                if values[j] < values[j - 1] {
                    values.swap(j, j - 1);
                }
            }
        }
        for value in values {
            if counts[axis] == 0 || events[axis][counts[axis] - 1] != value {
                events[axis][counts[axis]] = value;
                counts[axis] += 1;
            }
        }
    }
    let mut cells = [0; 9];
    let mut has_attachment = false;
    for x in 0..counts[0] - 1 {
        for y in 0..counts[1] - 1 {
            work.charge(1, cancel)?;
            let mut mask = 0;
            for (index, rectangle) in [a, b].into_iter().enumerate() {
                if rectangle.bounds[0][0] <= events[0][x]
                    && events[0][x + 1] <= rectangle.bounds[0][1]
                    && rectangle.bounds[1][0] <= events[1][y]
                    && events[1][y + 1] <= rectangle.bounds[1][1]
                {
                    mask |= 1 << index;
                }
            }
            cells[3 * x + y] = mask;
            has_attachment |= mask == 3;
        }
    }
    if !has_attachment {
        return Ok(None);
    }
    Ok(Some(NativeInterfaceAttachment {
        endpoint_occurrence_id: contact.occurrence_id(),
        current_occurrence_id: current.occurrence_id(),
        endpoint_leaf_sha256: *contact.adjacent.candidate_geometry_sha256(),
        current_leaf_sha256: *current.candidate_geometry_sha256(),
        native_axis_events: events,
        native_axis_event_counts: counts,
        native_cell_incidence: cells,
        endpoint_product: a.clone(),
        endpoint_cap_index: cap_index,
    }))
}

fn certify_contact_incidence(
    occurrences: &[NativeUnionOccurrence<'_, '_>],
    active: &[SlabOccurrence<'_, '_>],
    contacts: &[NativeEndpointOwnership],
    policy: NativeUnionPolicy,
    scratch: &mut Scratch,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Vec<NativeInterfaceAttachment>, GeometryError> {
    let mut attachments = Vec::new();
    for (i, contact) in contacts.iter().enumerate() {
        let source = source_index(occurrences, contact.occurrence_id(), work, cancel)?;
        let contact_radius =
            Interval::point(contact.adjacent.directed_sample_to_closed_source_error_mm())
                .add(Interval::point(policy.candidate_coordinate_error_mm));
        for component in active {
            work.charge(1, cancel)?;
            // The actual source coedge/face core-stratum incidence is already
            // proved by the opaque native source analysis. Its own floor need
            // not (and generally cannot) be disjoint from its ring core.
            if contact.stratum.is_some() && component.occurrence_index == source {
                continue;
            }
            let Some(bounds) = section_bounds(
                &component
                    .analysis
                    .occupied
                    .as_ref()
                    .unwrap_or(&component.analysis.section)
                    .loops,
                work,
                cancel,
            )?
            else {
                continue;
            };
            let padding = contact_radius
                .add(Interval::point(native_tube(component)?))
                .add(Interval::point(policy.candidate_coordinate_error_mm))
                .hi;
            if !bounds_separated(contact.occupied_xy_bounds_mm, bounds, padding) {
                let mut positive_count = 0;
                for source in active {
                    work.charge(1, cancel)?;
                    positive_count += usize::from(!source.analysis.section.loops.is_empty());
                }
                let current_leaf = match &occurrences[component.occurrence_index].evidence {
                    NativeUnionOccurrenceEvidence::Slab { leaf, .. }
                    | NativeUnionOccurrenceEvidence::SlabWithCollar { leaf, .. } => *leaf,
                    NativeUnionOccurrenceEvidence::EndpointOwned(_) => unreachable!(),
                };
                if contacts.len() == 1
                    && positive_count == 1
                    && let Some(attachment) =
                        rectangle_interface(contact, current_leaf, policy, work, cancel)?
                {
                    scratch.push(&mut attachments, attachment, work, cancel)?;
                    continue;
                }
                return Err(original_pair_error(
                    occurrences,
                    source,
                    component.occurrence_index,
                    unresolved(
                        source,
                        component.occurrence_index,
                        "CLOSED endpoint interaction has no exact native cap/actual contributor attachment proof or separated tubes",
                    ),
                ));
            }
        }
        for other in &contacts[i + 1..] {
            work.charge(1, cancel)?;
            if contact.stratum.is_some()
                && other.stratum.is_some()
                && contact.occurrence_id() == other.occurrence_id()
            {
                continue;
            }
            let padding = contact_radius
                .add(Interval::point(
                    other.adjacent.directed_sample_to_closed_source_error_mm(),
                ))
                .add(Interval::point(policy.candidate_coordinate_error_mm))
                .hi;
            if !bounds_separated(
                contact.occupied_xy_bounds_mm,
                other.occupied_xy_bounds_mm,
                padding,
            ) {
                let target = source_index(occurrences, other.occurrence_id(), work, cancel)?;
                return Err(original_pair_error(
                    occurrences,
                    source,
                    target,
                    unresolved(
                        source,
                        target,
                        "two adjacent-owned CLOSED endpoint strata have unresolved native incidence",
                    ),
                ));
            }
        }
    }
    Ok(attachments)
}

fn native_tube(occurrence: &SlabOccurrence<'_, '_>) -> Result<f64, GeometryError> {
    let section = &occurrence.analysis.section;
    let captured = if occurrence.source_metric == NativeSourceLeafProofKind::SampleToClosedSource {
        occurrence.analysis.sample_to_closed_source_shape_error_mm()
    } else {
        occurrence.analysis.material_core_shape_error_mm()
    };
    let values = [
        section.achieved_chord_error_mm(),
        section.achieved_plane_error_mm(),
        captured,
    ];
    if values.iter().any(|v| !v.is_finite() || *v < 0.0) {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "native source boundary tube is invalid",
        ));
    }
    // Slab motion already includes sample/chord/plane/Boolean error.
    Ok(captured.max(
        Interval::point(section.achieved_chord_error_mm())
            .add(Interval::point(section.achieved_plane_error_mm()))
            .add(Interval::point(section.boolean_tolerance_mm))
            .hi,
    ))
}
fn section_bounds(
    loops: &[SectionLoopMm],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[[f64; 2]; 2]>, GeometryError> {
    let mut bounds = [[f64::INFINITY, f64::NEG_INFINITY]; 2];
    let mut any = false;
    for loop_ in loops {
        for point in &loop_.points_mm {
            work.charge(1, cancel)?;
            if point.iter().any(|v| !v.is_finite()) {
                return Err(error(
                    GeometryErrorCode::InvalidGeometry,
                    "native section contains a nonfinite source point",
                ));
            }
            any = true;
            for axis in 0..2 {
                bounds[axis][0] = bounds[axis][0].min(point[axis]);
                bounds[axis][1] = bounds[axis][1].max(point[axis]);
            }
        }
    }
    Ok(any.then_some(bounds))
}
#[derive(Debug, Clone, Copy)]
struct NativeProjectionBall {
    center: [f64; 2],
    radius: f64,
}
#[derive(Debug, Clone, Copy)]
struct NativeTubeEnvelope {
    bounds: [[f64; 2]; 2],
    native_bounds: [[f64; 2]; 2],
    expanded: [[f64; 2]; 2],
    tube: f64,
    policy: f64,
    ball: Option<NativeProjectionBall>,
}
type NativeTargetEnvelope = (OccurrenceId, Option<NativeTubeEnvelope>);
fn prepare_target_envelopes(
    original: &[NativeUnionOccurrence<'_, '_>],
    active: &[SlabOccurrence<'_, '_>],
    scratch: &mut Scratch,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Vec<NativeTargetEnvelope>, GeometryError> {
    let mut envelopes = Vec::new();
    for source in active {
        let original = &original[source.occurrence_index];
        scratch.push(
            &mut envelopes,
            (
                original.occurrence_id,
                native_tube_envelope(source, original, work, cancel)?,
            ),
            work,
            cancel,
        )?;
    }
    Ok(envelopes)
}
fn target_envelope<'a>(
    envelopes: &'a [NativeTargetEnvelope],
    id: OccurrenceId,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<&'a NativeTubeEnvelope>, GeometryError> {
    for (captured, envelope) in envelopes {
        work.charge(1, cancel)?;
        if *captured == id {
            return Ok(envelope.as_ref());
        }
    }
    Err(error(
        GeometryErrorCode::InvalidGeometry,
        "missing sealed whole-interval native target enclosure",
    ))
}
fn projection_materials<'a>(
    leaf: &'a NativeSourceLeafReceipt,
    metric: Option<&'a NativeCriticalCollar>,
) -> impl Iterator<Item = &'a NativeSourceLeafReceipt> {
    std::iter::once(leaf)
        .filter(move |_| metric.is_none())
        .chain(
            metric
                .into_iter()
                .flat_map(|metric| metric.used_material_targets().map(|target| target.leaf())),
        )
}
fn native_projection_ball(
    source: &SlabOccurrence<'_, '_>,
    original: &NativeUnionOccurrence<'_, '_>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<NativeProjectionBall>, GeometryError> {
    let leaf = source.leaf.unwrap();
    let (metric, cover) = if let Some(cover) = leaf.closed_shape_error_mm() {
        (None, cover)
    } else if let (Some(closed), Some(coordinate)) = (
        source.analysis.closed_shape_error_mm(),
        leaf.sample_coordinate_error_mm,
    ) {
        // Independent FULL closed native-analysis authority plus the
        // factory's verified filled-polygon correspondence; never infer
        // reverse coverage from a directed/core receipt alone.
        (
            None,
            Interval::point(closed).add(Interval::point(coordinate)).hi,
        )
    } else if let NativeUnionOccurrenceEvidence::SlabWithCollar { collar, .. } = &original.evidence
    {
        let metric = collar.native_collar();
        (Some(metric), metric.closed_source_to_material_error_mm())
    } else {
        return Ok(None);
    };
    let mut bounds = [[f64::INFINITY, f64::NEG_INFINITY]; 2];
    let mut any = false;
    for material in projection_materials(leaf, metric) {
        work.charge(1, cancel)?;
        if let Some(box_) = material.candidate_xy_bounds_mm {
            any = true;
            for axis in 0..2 {
                bounds[axis][0] = bounds[axis][0].min(box_[axis][0]);
                bounds[axis][1] = bounds[axis][1].max(box_[axis][1]);
            }
        }
    }
    if !any {
        return Ok(None);
    }
    let center = bounds.map(|axis| 0.5 * axis[0] + 0.5 * axis[1]);
    let mut radius = 0.0_f64;
    for material in projection_materials(leaf, metric) {
        for loop_ in material.sample_loops.iter() {
            work.charge(1, cancel)?;
            if loop_.hole {
                continue;
            }
            radius = radius.max(
                Interval::point(distance_upper(center, loop_.shape.center)?)
                    .add(Interval::point(loop_.shape.cover_radius_mm))
                    .hi,
            );
        }
    }
    radius = Interval::point(radius).add(Interval::point(cover)).hi;
    // The disk encloses ENTIRE ACTUAL filled polygons by convexity, not samples
    // of native curves. Independent WHOLE-CLOSED-SLAB reverse authority then
    // encloses the TRUE native target T at EVERY captured height. Projection
    // is nonexpansive; the conservative occupied mask C need NOT lie in it.
    Ok(radius
        .is_finite()
        .then_some(NativeProjectionBall { center, radius }))
}
fn native_tube_envelope(
    source: &SlabOccurrence<'_, '_>,
    original: &NativeUnionOccurrence<'_, '_>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<NativeTubeEnvelope>, GeometryError> {
    let ball = native_projection_ball(source, original, work, cancel)?;
    let bounds = if let Some(bounds) = section_bounds(
        &source
            .analysis
            .occupied
            .as_ref()
            .unwrap_or(&source.analysis.section)
            .loops,
        work,
        cancel,
    )? {
        bounds
    } else if let Some(ball) = ball {
        ball.center.map(|coordinate| {
            [
                Interval::point(coordinate)
                    .sub(Interval::point(ball.radius))
                    .lo,
                Interval::point(coordinate)
                    .add(Interval::point(ball.radius))
                    .hi,
            ]
        })
    } else if source.analysis.has_transition_material() {
        return Err(error(
            GeometryErrorCode::AnalysisIndeterminate,
            "critical source has no sealed whole-slab native target projection enclosure",
        ));
    } else {
        return Ok(None);
    };
    let tube = native_tube(source)?;
    let policy = source.analysis.world_shape_error_policy_mm().unwrap();
    let native_bounds =
        if let Some(rectangle) = source.leaf.and_then(|leaf| leaf.rectangle.as_deref()) {
            std::array::from_fn(|axis| {
                [
                    native_support_interval([
                        rectangle.bounds[axis][0],
                        rectangle.support_residuals[axis][0],
                    ])
                    .lo,
                    native_support_interval([
                        rectangle.bounds[axis][1],
                        rectangle.support_residuals[axis][1],
                    ])
                    .hi,
                ]
            })
        } else {
            bounds.map(|axis| {
                [
                    Interval::point(axis[0]).sub(Interval::point(tube)).lo,
                    Interval::point(axis[1]).add(Interval::point(tube)).hi,
                ]
            })
        };
    let expanded = native_bounds.map(|axis| {
        [
            Interval::point(axis[0]).sub(Interval::point(policy)).lo,
            Interval::point(axis[1]).add(Interval::point(policy)).hi,
        ]
    });
    Ok(Some(NativeTubeEnvelope {
        bounds,
        native_bounds,
        expanded,
        tube,
        policy,
        ball,
    }))
}
fn ball_box_separated(ball: NativeProjectionBall, box_: [[f64; 2]; 2], reserve: Interval) -> bool {
    let mut squared = Interval::point(0.0);
    for (axis, interval) in box_.iter().enumerate() {
        let distance = if ball.center[axis] < interval[0] {
            Interval::point(interval[0])
                .sub(Interval::point(ball.center[axis]))
                .lo
                .max(0.0)
        } else if ball.center[axis] > interval[1] {
            Interval::point(ball.center[axis])
                .sub(Interval::point(interval[1]))
                .lo
                .max(0.0)
        } else {
            0.0
        };
        squared = squared.add(Interval::point(distance).square());
    }
    squared.lo > Interval::point(ball.radius).add(reserve).square().hi
}
fn envelope_contains(envelope: &NativeTubeEnvelope, point: [f64; 3]) -> bool {
    if !(0..2).all(|axis| {
        envelope.expanded[axis][0] <= point[axis] && point[axis] <= envelope.expanded[axis][1]
    }) {
        return false;
    }
    if let Some(ball) = envelope.ball {
        let distance = delta([point[0], point[1]], ball.center);
        if distance[0].square().add(distance[1].square()).lo
            > Interval::point(ball.radius)
                .add(Interval::point(envelope.policy))
                .square()
                .hi
        {
            return false;
        }
    }
    true
}
fn separated_tubes(a: Option<&NativeTubeEnvelope>, b: Option<&NativeTubeEnvelope>) -> bool {
    let (Some(a), Some(b)) = (a, b) else {
        return true;
    };
    // Preserve the original conservative native-target enclosure/tube predicate.
    let padding = Interval::point(a.tube)
        .add(Interval::point(b.tube))
        .add(Interval::point(a.policy))
        .add(Interval::point(b.policy))
        .hi;
    if (0..2).any(|axis| {
        down(b.bounds[axis][0] - a.bounds[axis][1]) > padding
            || down(a.bounds[axis][0] - b.bounds[axis][1]) > padding
    }) {
        return true;
    }
    // These independently proved WHOLE-SLAB target-T projections already
    // contain native boundary/slab uncertainty. Both full candidate policies
    // are additionally reserved. This makes NO assertion about the complete
    // conservative occupied mask C, nor any source family or raw-M gap.
    let reserve = Interval::point(a.policy).add(Interval::point(b.policy));
    a.ball
        .is_some_and(|ball| ball_box_separated(ball, b.native_bounds, reserve))
        || b.ball
            .is_some_and(|ball| ball_box_separated(ball, a.native_bounds, reserve))
}

fn partition_root(
    parents: &mut [usize],
    mut index: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<usize, GeometryError> {
    let start = index;
    while parents[index] != index {
        work.charge(1, cancel)?;
        index = parents[index];
    }
    let root = index;
    index = start;
    while parents[index] != index {
        work.charge(1, cancel)?;
        let next = parents[index];
        parents[index] = root;
        index = next;
    }
    Ok(root)
}

struct NativePartitionProof {
    achieved: f64,
    raw_inclusion: f64,
    boundary_edges: Vec<NativeUnionBoundaryEdge>,
    boundary_sources: Vec<NativeUnionBoundarySource>,
    kind: NativeUnionProofKind,
    mixed_varying: bool,
    preserved_source_occurrences: Vec<OccurrenceId>,
    target_envelopes: Vec<NativeTargetEnvelope>,
}
struct InteractionGroupsQuery<'a, 'source> {
    original: &'a [NativeUnionOccurrence<'source, 'source>],
    active: &'a [SlabOccurrence<'source, 'source>],
    candidate: &'a [SectionLoopMm],
    policy: NativeUnionPolicy,
}

fn certify_interaction_groups(
    query: &InteractionGroupsQuery<'_, '_>,
    mut envelopes: Vec<NativeTargetEnvelope>,
    scratch: &mut Scratch,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<NativePartitionProof, GeometryError> {
    let &InteractionGroupsQuery {
        original,
        active,
        candidate,
        policy,
    } = query;
    let mut parents = Vec::new();
    let mut sizes = Vec::new();
    let mut neighbors = Vec::new();
    for index in 0..active.len() {
        scratch.push(&mut parents, index, work, cancel)?;
        scratch.push(&mut sizes, 1_usize, work, cancel)?;
        scratch.push(&mut neighbors, usize::MAX, work, cancel)?;
    }
    for a in 0..active.len() {
        for b in a + 1..active.len() {
            work.charge(1, cancel)?;
            if separated_tubes(envelopes[a].1.as_ref(), envelopes[b].1.as_ref()) {
                continue;
            }
            // Retain one ACTUALLY unresolved incident pair per occurrence for
            // precise diagnostics, without another quadratic pair-proof pass.
            if neighbors[a] == usize::MAX {
                neighbors[a] = b;
            }
            if neighbors[b] == usize::MAX {
                neighbors[b] = a;
            }
            let mut a = partition_root(&mut parents, a, work, cancel)?;
            let mut b = partition_root(&mut parents, b, work, cancel)?;
            if a == b {
                continue;
            }
            if sizes[a] < sizes[b] {
                std::mem::swap(&mut a, &mut b);
            }
            parents[b] = a;
            sizes[a] += sizes[b];
        }
    }
    for i in 0..parents.len() {
        let root = partition_root(&mut parents, i, work, cancel)?;
        parents[i] = root;
    }
    let mut assignment = Vec::new();
    scratch.reserve(&mut assignment, candidate.len(), work, cancel)?;
    for loop_ in candidate {
        let mut group = None;
        for point in &loop_.points_mm {
            work.charge(1, cancel)?;
            let mut found = None;
            for (i, (_, envelope)) in envelopes.iter().enumerate() {
                work.charge(1, cancel)?;
                let Some(envelope) = envelope else {
                    continue;
                };
                if envelope_contains(envelope, *point) {
                    if found.is_some_and(|previous| previous != parents[i]) {
                        return Err(error(
                            GeometryErrorCode::AnalysisIndeterminate,
                            "actual final union loop has ambiguous native interaction-group association",
                        ));
                    }
                    found = Some(parents[i]);
                }
            }
            let found=found.ok_or_else(||error(GeometryErrorCode::ShapeErrorExceeded,
                "actual final union vertex is outside every captured native interaction-group tube"))?;
            if group.is_some_and(|previous| previous != found) {
                return Err(error(
                    GeometryErrorCode::AnalysisIndeterminate,
                    "actual final union loop bridges uncertainty-separated native interaction groups",
                ));
            }
            group = Some(found);
        }
        assignment.push(group.ok_or_else(|| {
            error(
                GeometryErrorCode::InvalidGeometry,
                "actual final union has an empty loop",
            )
        })?);
    }
    // Envelopes above only PROPOSE an assignment. Every ACTUAL loop is now
    // consumed exactly once by a COMPLETE native graph/fidelity verifier;
    // no extraneous contour, lost hole, or invented bridge can pass on a bbox.
    let mut achieved = 0.0_f64;
    let mut input_inclusion = 0.0_f64;
    let mut boundary_edges = Vec::new();
    let mut boundary_sources = Vec::new();
    let mut interacting = false;
    let mut stationary = true;
    let mut preserved_source_occurrences = Vec::new();
    for root in 0..parents.len() {
        work.charge(1, cancel)?;
        if parents[root] != root {
            continue;
        }
        let checkpoint = scratch.bytes;
        let result: Result<_, GeometryError> = (|| {
            let mut members = Vec::new();
            let mut indices = Vec::new();
            for (i, source) in active.iter().enumerate() {
                work.charge(1, cancel)?;
                if parents[i] == root {
                    scratch.push(&mut members, *source, work, cancel)?;
                }
            }
            for (i, &group) in assignment.iter().enumerate() {
                work.charge(1, cancel)?;
                if group == root {
                    scratch.push(&mut indices, i, work, cancel)?;
                }
            }
            let view = CandidateLoops {
                all: candidate,
                indices: Some(&indices),
            };
            let mut profiles = Vec::new();
            let multi = members.len() > 1;
            let leaf = members[0].leaf.unwrap();
            let source_material_preserved = !multi
                && leaf.is_positive()
                && preserves_source_material(leaf, view, scratch, work, cancel)?;
            let mut group_stationary = true;
            let (bound, edges, sources) = if multi {
                for (i, source) in active.iter().enumerate() {
                    work.charge(1, cancel)?;
                    if parents[i] != root {
                        continue;
                    }
                    let leaf = source.leaf.unwrap();
                    if source.analysis.section.loops.is_empty() {
                        group_stationary &= leaf.native_world_z_invariant;
                        continue;
                    }
                    let other = neighbors[i];
                    let a = source.occurrence_index;
                    let b = active[other].occurrence_index;
                    let profile=native_profile(source,scratch,work,cancel)
                        .map_err(|failure|original_pair_error(original,a,b,failure))?
                        .ok_or_else(||original_pair_error(original,a,b,unresolved(a,b,
                            "uncertain native pair requires exact orthogonal face/trim products; general shared continuation is unavailable")))?;
                    scratch.push(&mut profiles, profile, work, cancel)?;
                }
                let other = neighbors[root];
                let a = active[root].occurrence_index;
                let b = active[other].occurrence_index;
                rectangular_arrangement(
                    &profiles,
                    view,
                    members
                        .iter()
                        .map(|source| source.analysis.world_shape_error_policy_mm().unwrap())
                        .fold(f64::INFINITY, f64::min),
                    policy,
                    scratch,
                    work,
                    cancel,
                )
                .map_err(|failure| original_pair_error(original, a, b, failure))?
            } else {
                group_stationary = leaf.native_world_z_invariant;
                let bound = if source_material_preserved
                    && members[0].source_metric == leaf.proof_kind
                    && policy.candidate_coordinate_error_mm
                        == leaf.policy.candidate_coordinate_error_mm
                {
                    // The original request/pose/native-sample validation and
                    // incidence checks already ran. Exact actual-M identity
                    // reuses its sealed metric, never a stronger proof scope.
                    leaf.achieved_shape_error_mm
                } else {
                    certify_separated_candidate_impl(
                        &members, view, policy, None, scratch, work, cancel,
                    )?
                };
                (bound, Vec::new(), Vec::new())
            };
            let mut inclusion = 0.0_f64;
            for source in &members {
                work.charge(1, cancel)?;
                let leaf = source.leaf.unwrap();
                if !leaf.is_positive() {
                    continue;
                }
                let input = if multi {
                    // A native product makes the true section constant in Z.
                    // The frozen whole-curve error therefore bounds raw M's
                    // distance to that true footprint, without a slab-height
                    // padding falsely used as an overlay theorem.
                    leaf.sample_coordinate_error_mm
                        .map(|coordinate| {
                            Interval::point(coordinate)
                                .add(Interval::point(leaf.sample_region_error_mm))
                                .hi
                        })
                        .unwrap_or(leaf.achieved_shape_error_mm)
                } else if let Some(coordinate) = leaf.sample_coordinate_error_mm {
                    inclusion = inclusion.max(
                        Interval::point(coordinate)
                            .add(Interval::point(policy.candidate_coordinate_error_mm))
                            .hi,
                    );
                    continue;
                } else {
                    leaf.achieved_shape_error_mm
                };
                inclusion = inclusion.max(Interval::point(input).add(Interval::point(bound)).hi);
            }
            let preserved = source_material_preserved.then_some(leaf.occurrence_id());
            Ok((
                bound,
                inclusion,
                edges,
                sources,
                multi,
                group_stationary,
                preserved,
            ))
        })();
        let (bound, inclusion, edges, sources, multi, group_stationary, preserved) = result?;
        let vector_bytes = edges.capacity() * std::mem::size_of::<NativeUnionBoundaryEdge>()
            + sources.capacity() * std::mem::size_of::<NativeUnionBoundarySource>();
        let mut string_bytes = 0;
        for source in &sources {
            work.charge(1, cancel)?;
            string_bytes += source.face_id.as_str().len();
        }
        // Group-local source/candidate rings, profiles and UF views have been
        // dropped. Retain only actual native graph evidence while appending.
        scratch.bytes = checkpoint + vector_bytes + string_bytes;
        if let Some(id) = preserved {
            scratch.push(&mut preserved_source_occurrences, id, work, cancel)?;
        }
        scratch.reserve(&mut boundary_edges, edges.len(), work, cancel)?;
        scratch.reserve(&mut boundary_sources, sources.len(), work, cancel)?;
        let base = boundary_edges.len();
        for edge in edges {
            work.charge(1, cancel)?;
            boundary_edges.push(edge);
        }
        for mut source in sources {
            work.charge(1, cancel)?;
            source.boundary_edge_index += base;
            boundary_sources.push(source);
        }
        scratch.bytes -= vector_bytes;
        achieved = achieved.max(bound);
        input_inclusion = input_inclusion.max(inclusion);
        interacting |= multi;
        stationary &= group_stationary;
    }
    // Only the immediately preceding POSITIVE inputs are needed for streaming
    // cross-height target separation. Compact the existing scalar cache in
    // source order; no second coordinate/metadata vector or layer history.
    let mut positive = 0;
    for (index, source) in active.iter().enumerate() {
        work.charge(1, cancel)?;
        if source.leaf.unwrap().is_positive() {
            if positive != index {
                envelopes.swap(positive, index);
            }
            positive += 1;
        }
    }
    envelopes.truncate(positive);
    let kind = if interacting {
        NativeUnionProofKind::ExactNativeRectangularArrangement
    } else {
        NativeUnionProofKind::SeparatedNativeSlabTubes
    };
    Ok(NativePartitionProof {
        achieved,
        raw_inclusion: input_inclusion,
        boundary_edges,
        boundary_sources,
        kind,
        mixed_varying: interacting && !stationary,
        preserved_source_occurrences,
        target_envelopes: envelopes,
    })
}

struct Ring {
    hole: bool,
    points: Vec<[f64; 2]>,
}
fn ring(
    loop_: &SectionLoopMm,
    scratch: &mut Scratch,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Ring, GeometryError> {
    if loop_.points_mm.len() < 4 || loop_.points_mm.first() != loop_.points_mm.last() {
        return Err(error(
            GeometryErrorCode::AnalysisIndeterminate,
            "native union boundary is not a closed source cycle",
        ));
    }
    let mut points = Vec::new();
    scratch.reserve(&mut points, loop_.points_mm.len() - 1, work, cancel)?;
    for point in &loop_.points_mm[..loop_.points_mm.len() - 1] {
        work.charge(1, cancel)?;
        let p = [point[0], point[1]];
        if points.last() != Some(&p) {
            points.push(p);
        }
    }
    // Remove only exactly orthogonal redundant vertices. No sampled epsilon
    // turns curved native sections into straight source curves here.
    let mut changed = true;
    while changed && points.len() >= 3 {
        changed = false;
        for i in 0..points.len() {
            work.charge(1, cancel)?;
            let a = points[(i + points.len() - 1) % points.len()];
            let b = points[i];
            let c = points[(i + 1) % points.len()];
            if (a[0] == b[0] && b[0] == c[0] && between(b[1], a[1], c[1]))
                || (a[1] == b[1] && b[1] == c[1] && between(b[0], a[0], c[0]))
            {
                work.charge(points.len() as u64, cancel)?;
                points.remove(i);
                changed = true;
                break;
            }
        }
    }
    if points.len() < 3 {
        return Err(error(
            GeometryErrorCode::AnalysisIndeterminate,
            "native union boundary collapsed",
        ));
    }
    let area = (0..points.len()).fold(Interval::point(0.0), |sum, i| {
        let a = points[i].map(Interval::point);
        let b = points[(i + 1) % points.len()].map(Interval::point);
        sum.add(a[0].mul(b[1]).sub(a[1].mul(b[0])))
    });
    if (!loop_.is_hole && area.lo <= 0.0) || (loop_.is_hole && area.hi >= 0.0) {
        return Err(error(
            GeometryErrorCode::AnalysisIndeterminate,
            "native union requires robust material-left outer/hole orientation",
        ));
    }
    Ok(Ring {
        hole: loop_.is_hole,
        points,
    })
}
fn between(x: f64, a: f64, b: f64) -> bool {
    a.min(b) <= x && x <= a.max(b)
}
fn delta(a: [f64; 2], b: [f64; 2]) -> [Interval; 2] {
    std::array::from_fn(|i| Interval::point(a[i]).sub(Interval::point(b[i])))
}
fn cross(a: [Interval; 2], b: [Interval; 2]) -> Interval {
    a[0].mul(b[1]).sub(a[1].mul(b[0]))
}
fn distance_upper(a: [f64; 2], b: [f64; 2]) -> Result<f64, GeometryError> {
    if a == b {
        return Ok(0.0);
    }
    let d = delta(a, b);
    Ok(d[0].square().add(d[1].square()).sqrt()?.hi)
}
fn segment_gap(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> f64 {
    let mut gap = 0.0_f64;
    for axis in 0..2 {
        gap = gap
            .max(down(c[axis].min(d[axis]) - a[axis].max(b[axis])))
            .max(down(a[axis].min(b[axis]) - c[axis].max(d[axis])));
    }
    for (p, q, r, s) in [(a, b, c, d), (c, d, a, b)] {
        let direction = delta(q, p);
        let length = direction[0]
            .square()
            .add(direction[1].square())
            .sqrt()
            .map_or(f64::INFINITY, |v| v.hi);
        let u = cross(direction, delta(r, p));
        let v = cross(direction, delta(s, p));
        let numerator = if u.lo > 0.0 && v.lo > 0.0 {
            u.lo.min(v.lo)
        } else if u.hi < 0.0 && v.hi < 0.0 {
            (-u.hi).min(-v.hi)
        } else {
            0.0
        };
        if length > 0.0 {
            gap = gap.max(down(numerator / length));
        }
    }
    gap.max(0.0)
}
/// An allocation-free view of ACTUAL global output loops. Selected indices are
/// internal partition proposals; the complete graph verifier remains authority.
#[derive(Clone, Copy)]
struct CandidateLoops<'a> {
    all: &'a [SectionLoopMm],
    indices: Option<&'a [usize]>,
}
impl<'a> CandidateLoops<'a> {
    fn whole(all: &'a [SectionLoopMm]) -> Self {
        Self { all, indices: None }
    }
    fn len(self) -> usize {
        self.indices.map_or(self.all.len(), <[usize]>::len)
    }
    fn iter(self) -> impl Iterator<Item = &'a SectionLoopMm> {
        (0..self.len()).map(move |i| &self.all[self.indices.map_or(i, |indices| indices[i])])
    }
}
fn certify_separated_candidate_recording(
    occurrences: &[SlabOccurrence<'_, '_>],
    candidate: &[SectionLoopMm],
    policy: NativeUnionPolicy,
    recording: &mut SourceLoopRecording,
    scratch: &mut Scratch,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<f64, GeometryError> {
    certify_separated_candidate_impl(
        occurrences,
        CandidateLoops::whole(candidate),
        policy,
        Some(recording),
        scratch,
        work,
        cancel,
    )
}
fn certify_separated_candidate_impl(
    occurrences: &[SlabOccurrence<'_, '_>],
    candidate: CandidateLoops<'_>,
    policy: NativeUnionPolicy,
    mut recording: Option<&mut SourceLoopRecording>,
    scratch: &mut Scratch,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<f64, GeometryError> {
    let count = occurrences
        .iter()
        .try_fold(0_usize, |sum, o| {
            sum.checked_add(o.analysis.section.loops.len())
        })
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "native union source loop count overflow",
            )
        })?;
    if count != candidate.len() {
        return Err(error(
            GeometryErrorCode::AnalysisIndeterminate,
            "separated native union candidate changes the source boundary-cycle incidence",
        ));
    }
    let mut native = Vec::new();
    let mut budgets = Vec::new();
    for occurrence in occurrences {
        let tube = if let Some(leaf) = occurrence.leaf.filter(|leaf| leaf.rectangle.is_some()) {
            // The opaque full native product closes C, independently of a
            // weaker source critical/core estimate. Its frozen section error
            // is valid at every height of this actual closed product interval.
            leaf.sample_region_error_mm
        } else {
            native_tube(occurrence)?
        };
        let remaining = policy.candidate_coordinate_error_mm;
        if Interval::point(tube).add(Interval::point(remaining)).hi
            > occurrence.analysis.world_shape_error_policy_mm().unwrap()
        {
            return Err(error(
                GeometryErrorCode::ShapeErrorExceeded,
                "native union source tube plus actual overlay-coordinate error exceeds captured native policy",
            ));
        }
        for loop_ in &occurrence.analysis.section.loops {
            let r = ring(loop_, scratch, work, cancel)?;
            scratch.push(&mut native, r, work, cancel)?;
            scratch.push(&mut budgets, (tube, remaining), work, cancel)?;
        }
    }
    let mut candidate_rings = Vec::new();
    for loop_ in candidate.iter() {
        let r = ring(loop_, scratch, work, cancel)?;
        scratch.push(&mut candidate_rings, r, work, cancel)?;
    }
    if let Some(exact) = exact_cycle_mapping(&native, &candidate_rings, scratch, work, cancel)? {
        let mut achieved = 0.0_f64;
        for &index in &exact {
            work.charge(1, cancel)?;
            if let Some(recording) = recording.as_mut() {
                scratch.push(
                    &mut recording.loops,
                    NativeLoopRegistration {
                        native_index: index,
                        parent: None,
                    },
                    work,
                    cancel,
                )?;
            }
            achieved = achieved.max(
                Interval::point(budgets[index].0)
                    .add(Interval::point(0.0))
                    .hi,
            );
        }
        scratch.bytes -= exact.capacity() * std::mem::size_of::<usize>();
        drop(exact);
        // Complete oriented polygon identity makes the correspondence exactly
        // zero, not the native/source error. The same native clearance at the
        // actual maximum rounding (zero here) also proves candidate clearance.
        if let Some(recording) = recording.as_mut() {
            recording.clearance_mm = planar::capture_loop_registration(
                &native,
                &candidate_rings,
                &mut recording.loops,
                0.0,
                true,
                scratch,
                work,
                cancel,
            )?;
        } else {
            feature_clearance(&native, 0.0, scratch, work, cancel)?;
        }
        return Ok(achieved);
    }
    let mut used = Vec::new();
    scratch.reserve(&mut used, count, work, cancel)?;
    used.resize(count, false);
    let mut achieved = 0.0_f64;
    let mut max_rounding = 0.0_f64;
    for r in &candidate_rings {
        let mut matched = None;
        for (index, s) in native.iter().enumerate() {
            if used[index] || r.hole != s.hole || r.points.len() != s.points.len() {
                continue;
            }
            for shift in 0..s.points.len() {
                let mut bound = 0.0_f64;
                for i in 0..r.points.len() {
                    work.charge(1, cancel)?;
                    bound = bound.max(distance_upper(
                        r.points[i],
                        s.points[(i + shift) % s.points.len()],
                    )?);
                    if bound > budgets[index].1 {
                        break;
                    }
                }
                if bound <= budgets[index].1 {
                    if matched.is_some() {
                        return Err(error(
                            GeometryErrorCode::AnalysisIndeterminate,
                            format!(
                                "native union candidate cycle correspondence to source cycle {index} is not unique"
                            ),
                        ));
                    }
                    matched = Some((index, bound));
                }
            }
        }
        let (index,bound)=matched.ok_or_else(|| error(GeometryErrorCode::ShapeErrorExceeded,
            "final union candidate has no orientation-preserving source-cycle correspondence within policy"))?;
        used[index] = true;
        if let Some(recording) = recording.as_mut() {
            scratch.push(
                &mut recording.loops,
                NativeLoopRegistration {
                    native_index: index,
                    parent: None,
                },
                work,
                cancel,
            )?;
        }
        achieved = achieved.max(
            Interval::point(budgets[index].0)
                .add(Interval::point(bound))
                .hi,
        );
        max_rounding = max_rounding.max(bound);
    }
    // This is a graph embedding check in addition to metric matching. Close
    // Hausdorff boundaries alone could erase a tiny hole or bridge.
    if let Some(recording) = recording.as_mut() {
        recording.clearance_mm = planar::capture_loop_registration(
            &native,
            &candidate_rings,
            &mut recording.loops,
            max_rounding,
            false,
            scratch,
            work,
            cancel,
        )?;
    } else {
        feature_clearance(&native, max_rounding, scratch, work, cancel)?;
        feature_clearance(&candidate_rings, 0.0, scratch, work, cancel)?;
    }
    Ok(achieved)
}

/// Complete oriented-cycle correspondence, including unequal sampling
/// partitions. A common-parameter boundary homotopy with displacement <= d
/// proves FILLED Hausdorff <= d by winding degree: a point farther than d from
/// the opposite filled set cannot be crossed by the reverse homotopy.
/// Intermediate homotopies need not be embeddings. Independent simple-cycle,
/// feature, role and complete inventory checks remain mandatory.
#[cfg(test)]
pub(super) fn filled_projection_distance(
    original: &[SectionLoopMm],
    candidate: &[SectionLoopMm],
    original_tube: f64,
    maximum_distance: f64,
    byte_limit: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<f64, GeometryError> {
    if original.is_empty() || original.len() != candidate.len() {
        return Err(error(
            GeometryErrorCode::AnalysisIndeterminate,
            "filled projection changes the complete original cycle inventory",
        ));
    }
    let mut scratch = Scratch {
        bytes: MAX_SCRATCH - byte_limit.min(MAX_SCRATCH),
    };
    let mut native = Vec::new();
    let mut output = Vec::new();
    scratch.reserve(&mut native, original.len(), work, cancel)?;
    scratch.reserve(&mut output, candidate.len(), work, cancel)?;
    for loop_ in original {
        let r = ring(loop_, &mut scratch, work, cancel)?;
        scratch.push(&mut native, r, work, cancel)?;
    }
    for loop_ in CandidateLoops::whole(candidate).iter() {
        let r = ring(loop_, &mut scratch, work, cancel)?;
        scratch.push(&mut output, r, work, cancel)?;
    }
    for r in native.iter().chain(&output) {
        for i in 0..r.points.len() {
            work.charge(1, cancel)?;
            let vertex = r.points[i];
            let incoming = delta(r.points[(i + r.points.len() - 1) % r.points.len()], vertex);
            let outgoing = delta(r.points[(i + 1) % r.points.len()], vertex);
            if cross(incoming, outgoing).contains_zero()
                && incoming[0]
                    .mul(outgoing[0])
                    .add(incoming[1].mul(outgoing[1]))
                    .hi
                    >= 0.0
            {
                return Err(error(
                    GeometryErrorCode::AnalysisIndeterminate,
                    "filled projection has unresolved adjacent-edge overlap",
                ));
            }
        }
    }
    feature_clearance(&native, original_tube, &mut scratch, work, cancel)?;
    if maximum_distance >= 0.0
        && let Some(exact) = exact_cycle_mapping(&native, &output, &mut scratch, work, cancel)?
    {
        drop(exact);
        // Native clearance retained its full source tube. Exact oriented
        // filled-set identity reuses that stronger proof for output.
        return Ok(0.0);
    }
    feature_clearance(&output, 0.0, &mut scratch, work, cancel)?;
    let mut used = Vec::new();
    scratch.reserve(&mut used, output.len(), work, cancel)?;
    used.resize(output.len(), false);
    let mut achieved = 0.0_f64;
    for source in &native {
        let mut matched = None;
        for (index, target) in output.iter().enumerate() {
            work.charge(1, cancel)?;
            if used[index] || source.hole != target.hole {
                continue;
            }
            let saved = scratch.bytes;
            let distance = cyclic_path_distance(source, target, &mut scratch, work, cancel)?;
            scratch.bytes = saved;
            if distance <= maximum_distance {
                if matched.is_some() {
                    return Err(error(
                        GeometryErrorCode::AnalysisIndeterminate,
                        "filled projection cycle correspondence is not unique",
                    ));
                }
                matched = Some((index, distance));
            }
        }
        let (index, distance) = matched.ok_or_else(|| {
            error(
                GeometryErrorCode::AnalysisIndeterminate,
                "filled projection has no complete oriented cycle correspondence within policy",
            )
        })?;
        used[index] = true;
        achieved = achieved.max(distance);
    }
    Ok(achieved)
}

struct CyclePath {
    points: Vec<[Interval; 2]>,
    knots: Vec<f64>,
}
impl CyclePath {
    fn push(
        &mut self,
        point: [Interval; 2],
        scratch: &mut Scratch,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        scratch.push(&mut self.points, point, work, cancel)
    }
    fn parameterize(
        &mut self,
        scratch: &mut Scratch,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        scratch.reserve(&mut self.knots, self.points.len(), work, cancel)?;
        scratch.push(&mut self.knots, 0.0, work, cancel)?;
        let mut total = 0.0;
        for pair in self.points.windows(2) {
            work.charge(1, cancel)?;
            // Positive numerical lengths propose a parameterization only.
            // Exact binary64 knots and interval interpolation below prove it.
            let center = |p: [Interval; 2]| p.map(|v| v.lo * 0.5 + v.hi * 0.5);
            let a = center(pair[0]);
            let b = center(pair[1]);
            let length = (a[0] - b[0]).hypot(a[1] - b[1]);
            let next = total + length;
            if !next.is_finite() || next <= total {
                return Err(error(
                    GeometryErrorCode::AnalysisIndeterminate,
                    "filled projection cyclic parameterization is unresolved",
                ));
            }
            total = next;
            scratch.push(&mut self.knots, total, work, cancel)?;
        }
        for knot in &mut self.knots {
            *knot /= total;
        }
        if self.knots.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(error(
                GeometryErrorCode::AnalysisIndeterminate,
                "filled projection cyclic knots collapse at binary64 conditioning",
            ));
        }
        Ok(())
    }
    fn at(&self, segment: usize, t: f64) -> Result<[Interval; 2], GeometryError> {
        if t == self.knots[segment] {
            return Ok(self.points[segment]);
        }
        if t == self.knots[segment + 1] {
            return Ok(self.points[segment + 1]);
        }
        let fraction = Interval::point(t)
            .sub(Interval::point(self.knots[segment]))
            .div(
                Interval::point(self.knots[segment + 1]).sub(Interval::point(self.knots[segment])),
            )?;
        Ok(std::array::from_fn(|axis| {
            self.points[segment][axis]
                .add(fraction.mul(self.points[segment + 1][axis].sub(self.points[segment][axis])))
        }))
    }
}

fn cyclic_path_distance(
    source: &Ring,
    target: &Ring,
    scratch: &mut Scratch,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<f64, GeometryError> {
    // Closest-segment phase is only a search proposal. The complete
    // orientation-preserving map, including this segment's two remaining
    // pieces, is verified independently below.
    let p = source.points[0];
    let mut phase = (0, 0.0, f64::INFINITY);
    for i in 0..target.points.len() {
        work.charge(1, cancel)?;
        let a = target.points[i];
        let b = target.points[(i + 1) % target.points.len()];
        let d = [b[0] - a[0], b[1] - a[1]];
        let length = d[0] * d[0] + d[1] * d[1];
        if length <= 0.0 || !length.is_finite() {
            continue;
        }
        let t = (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / length).clamp(0.0, 1.0);
        let distance = (p[0] - (a[0] + t * d[0])).hypot(p[1] - (a[1] + t * d[1]));
        if distance < phase.2 {
            phase = (i, t, distance);
        }
    }
    if !phase.2.is_finite() {
        return Err(error(
            GeometryErrorCode::AnalysisIndeterminate,
            "filled projection has no finite cyclic phase",
        ));
    }
    let mut a = CyclePath {
        points: Vec::new(),
        knots: Vec::new(),
    };
    let mut b = CyclePath {
        points: Vec::new(),
        knots: Vec::new(),
    };
    scratch.reserve(&mut a.points, source.points.len() + 1, work, cancel)?;
    scratch.reserve(&mut b.points, target.points.len() + 2, work, cancel)?;
    for point in source.points.iter().chain(source.points.first()) {
        a.push(point.map(Interval::point), scratch, work, cancel)?;
    }
    let start = target.points[phase.0];
    let end = target.points[(phase.0 + 1) % target.points.len()];
    let phase_point = if phase.1 == 0.0 {
        start.map(Interval::point)
    } else if phase.1 == 1.0 {
        end.map(Interval::point)
    } else {
        std::array::from_fn(|axis| {
            Interval::point(start[axis]).add(
                Interval::point(phase.1)
                    .mul(Interval::point(end[axis]).sub(Interval::point(start[axis]))),
            )
        })
    };
    b.push(phase_point, scratch, work, cancel)?;
    for offset in 1..=target.points.len() {
        if offset == 1 && phase.1 == 1.0 || offset == target.points.len() && phase.1 == 0.0 {
            continue;
        }
        b.push(
            target.points[(phase.0 + offset) % target.points.len()].map(Interval::point),
            scratch,
            work,
            cancel,
        )?;
    }
    b.push(phase_point, scratch, work, cancel)?;
    a.parameterize(scratch, work, cancel)?;
    b.parameterize(scratch, work, cancel)?;
    let mut indices = [0, 0];
    let mut t = 0.0;
    let mut bound = 0.0_f64;
    loop {
        work.charge(1, cancel)?;
        let x = a.at(indices[0], t)?;
        let y = b.at(indices[1], t)?;
        let d = [x[0].sub(y[0]), x[1].sub(y[1])];
        let squared = d[0].square().add(d[1].square());
        bound = bound.max(
            Interval {
                lo: squared.lo.max(0.0),
                hi: squared.hi,
            }
            .sqrt()?
            .hi,
        );
        if t == 1.0 {
            break;
        }
        t = a.knots[indices[0] + 1].min(b.knots[indices[1] + 1]);
        for (index, path) in indices.iter_mut().zip([&a, &b]) {
            if path.knots[*index + 1] == t && *index + 2 < path.knots.len() {
                *index += 1;
            }
        }
    }
    Ok(bound)
}

// Exact singleton arithmetic is used solely to admit a narrower native source
// class. Failure to establish an identity is refusal, never epsilon snapping.
fn native_sum(a: f64, b: f64) -> Option<[f64; 2]> {
    if !a.is_finite() || !b.is_finite() {
        return None;
    }
    let sum = a + b;
    if !sum.is_finite() {
        return None;
    }
    let z = sum - a;
    let residual = (a - (sum - z)) + (b - z);
    if !residual.is_finite() {
        return None;
    }
    Some([
        if sum == 0.0 { 0.0 } else { sum },
        if residual == 0.0 { 0.0 } else { residual },
    ])
}
fn native_support_interval(value: [f64; 2]) -> Interval {
    if value[1] > 0.0 {
        Interval {
            lo: value[0],
            hi: up(value[0]),
        }
    } else if value[1] < 0.0 {
        Interval {
            lo: down(value[0]),
            hi: value[0],
        }
    } else {
        Interval::point(value[0])
    }
}
fn exact_add(a: f64, b: f64) -> Option<f64> {
    let value = native_sum(a, b)?;
    (value[1] == 0.0).then_some(value[0])
}
fn exact_mul(a: f64, b: f64) -> Option<f64> {
    if !a.is_finite() || !b.is_finite() {
        return None;
    }
    if a == 0.0 || b == 0.0 {
        return Some(0.0);
    }
    if a == 1.0 {
        return Some(b);
    }
    if b == 1.0 {
        return Some(a);
    }
    if a == -1.0 {
        return Some(-b);
    }
    if b == -1.0 {
        return Some(-a);
    }
    let p = a * b;
    (p.is_finite() && p.abs() >= 2.0 * f64::MIN_POSITIVE / f64::EPSILON && a.mul_add(b, -p) == 0.0)
        .then_some(p)
}
fn exact_div(a: f64, b: f64) -> Option<f64> {
    if !a.is_finite() || !b.is_finite() || b == 0.0 {
        return None;
    }
    let q = a / b;
    (q.is_finite() && exact_mul(q, b) == Some(a)).then_some(q)
}
fn native_topology_error(cause: solidcore::SolidError) -> GeometryError {
    error(
        GeometryErrorCode::ParameterCorrespondenceUnavailable,
        format!("native union source topology: {cause}"),
    )
}

#[derive(Clone, Copy)]
struct AffineFace {
    origin: [f64; 3],
    u: [f64; 3],
    v: [f64; 3],
}
fn native_affine_face(surface: &solidcore::surface::surface::Surface) -> Option<AffineFace> {
    use solidcore::surface::surface::p2::SurfaceKind;
    let mut source = surface;
    for depth in 0..=32 {
        match source.kind() {
            SurfaceKind::CurveBounded(bounded) => source = bounded.basis(),
            SurfaceKind::Step(step) => source = step.surface()?,
            _ => break,
        }
        if depth == 32 {
            return None;
        }
    }
    if let SurfaceKind::Plane(plane) = source.kind() {
        let position = plane.position();
        let origin = position.origin();
        let u = position.x_axis();
        let v = position.y_axis();
        let scale = plane.uv_scale();
        let face = AffineFace {
            origin: [origin.x, origin.y, origin.z],
            u: [
                exact_mul(u.x, scale.x)?,
                exact_mul(u.y, scale.x)?,
                exact_mul(u.z, scale.x)?,
            ],
            v: [
                exact_mul(v.x, scale.y)?,
                exact_mul(v.y, scale.y)?,
                exact_mul(v.z, scale.y)?,
            ],
        };
        return face
            .origin
            .iter()
            .chain(&face.u)
            .chain(&face.v)
            .all(|value| value.is_finite())
            .then_some(face);
    }
    let surface = crate::parametric::native_surface_nurbs(surface).ok()??;
    if surface.degrees() != (1, 1) || surface.n() != 1 || surface.m() != 1 {
        return None;
    }
    let ku = surface.u_knots();
    let kv = surface.v_knots();
    if ku.len() != 4
        || kv.len() != 4
        || ku[0] != ku[1]
        || ku[2] != ku[3]
        || kv[0] != kv[1]
        || kv[2] != kv[3]
    {
        return None;
    }
    let first = surface.cpt(0, 0).ok()?;
    let weight = if surface.is_rational() { first.w } else { 1.0 };
    if !weight.is_finite() || weight == 0.0 {
        return None;
    }
    let point = |u, v| -> Option<[f64; 3]> {
        let point = surface.cpt(u, v).ok()?;
        if surface.is_rational() && point.w != weight {
            return None;
        }
        Some([
            exact_div(point.x, weight)?,
            exact_div(point.y, weight)?,
            exact_div(point.z, weight)?,
        ])
    };
    let points = [[point(0, 0)?, point(0, 1)?], [point(1, 0)?, point(1, 1)?]];
    let domain = [[ku[1], ku[2]], [kv[1], kv[2]]];
    affine_tensor(points, domain)
}
fn affine_tensor(points: [[[f64; 3]; 2]; 2], domain: [[f64; 2]; 2]) -> Option<AffineFace> {
    if domain
        .iter()
        .any(|range| !range[0].is_finite() || !range[1].is_finite() || range[0] >= range[1])
    {
        return None;
    }
    let origin = points[0][0];
    let mut u = [0.0; 3];
    let mut v = [0.0; 3];
    for axis in 0..3 {
        u[axis] = exact_add(points[1][0][axis], -origin[axis])?;
        v[axis] = exact_add(points[0][1][axis], -origin[axis])?;
        if exact_add(exact_add(origin[axis], u[axis])?, v[axis])? != points[1][1][axis] {
            return None;
        }
    }
    Some(AffineFace { origin, u, v })
}
/// A source evaluator is inspected in its actual coefficient law. In
/// particular an original-radian operator is never cast as a native surface.
fn affine_face(surface: &dyn crate::parametric::SurfaceEvaluator) -> Option<AffineFace> {
    if let Some(native) = surface.native_surface() {
        return native_affine_face(native);
    }
    match surface.original_surface()? {
        solidcraft_step::geom::Surface::Plane { position } => {
            let a = position.axis;
            let b = position.ref_direction;
            let v = [
                exact_add(exact_mul(a[1], b[2])?, -exact_mul(a[2], b[1])?)?,
                exact_add(exact_mul(a[2], b[0])?, -exact_mul(a[0], b[2])?)?,
                exact_add(exact_mul(a[0], b[1])?, -exact_mul(a[1], b[0])?)?,
            ];
            position
                .origin
                .iter()
                .chain(&b)
                .chain(&v)
                .all(|value| value.is_finite())
                .then_some(AffineFace {
                    origin: position.origin,
                    u: b,
                    v,
                })
        }
        solidcraft_step::geom::Surface::BSpline(source) => {
            if source.u_degree != 1
                || source.v_degree != 1
                || source.points.len() != 2
                || source.points.iter().any(|row| row.len() != 2)
                || source.u_knots.len() != 2
                || source.v_knots.len() != 2
                || source.u_multiplicities != [2, 2]
                || source.v_multiplicities != [2, 2]
            {
                return None;
            }
            if let Some(weights) = &source.weights {
                if weights.len() != 2 || weights.iter().any(|row| row.len() != 2) {
                    return None;
                }
                let weight = weights[0][0];
                if !weight.is_finite()
                    || weight <= 0.0
                    || weights.iter().flatten().any(|&value| value != weight)
                {
                    return None;
                }
            }
            affine_tensor(
                [
                    [source.points[0][0], source.points[0][1]],
                    [source.points[1][0], source.points[1][1]],
                ],
                [
                    [source.u_knots[0], source.u_knots[1]],
                    [source.v_knots[0], source.v_knots[1]],
                ],
            )
        }
        _ => None,
    }
}
#[derive(Debug)]
struct NativeProfileEdge<'a> {
    a: [f64; 2],
    b: [f64; 2],
    a_residual: [f64; 2],
    b_residual: [f64; 2],
    face_id: &'a FaceId,
}
#[derive(Debug)]
struct NativeProfile<'a> {
    occurrence_index: usize,
    world_height_interval_mm: [f64; 2],
    height_supports: [[f64; 2]; 2],
    coordinate_error_mm: f64,
    source_caps: [&'a FaceId; 2],
    edges: Vec<NativeProfileEdge<'a>>,
}

fn cached_rectangle_profile<'a>(
    index: usize,
    rectangle: &'a NativeRectangleLeafWitness,
    scratch: &mut Scratch,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<NativeProfile<'a>, GeometryError> {
    let [[x0, x1], [y0, y1]] = rectangle.bounds;
    let corners = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
    let [[rx0, rx1], [ry0, ry1]] = rectangle.support_residuals;
    let residuals = [[rx0, ry0], [rx1, ry0], [rx1, ry1], [rx0, ry1]];
    let mut edges = Vec::new();
    scratch.reserve(&mut edges, 4, work, cancel)?;
    for (face, &side) in rectangle.source_faces.iter().zip(&rectangle.source_sides) {
        work.charge(1, cancel)?;
        let [a, b] = match side {
            0 => [3, 0],
            1 => [1, 2],
            2 => [0, 1],
            3 => [2, 3],
            _ => {
                return Err(error(
                    GeometryErrorCode::KernelFailure,
                    "sealed original native rectangle side is invalid",
                ));
            }
        };
        edges.push(NativeProfileEdge {
            a: corners[a],
            b: corners[b],
            a_residual: residuals[a],
            b_residual: residuals[b],
            face_id: face,
        });
    }
    Ok(NativeProfile {
        occurrence_index: index,
        world_height_interval_mm: rectangle.world_material_height_interval_mm,
        height_supports: rectangle.height_supports,
        coordinate_error_mm: rectangle.coordinate_error_mm,
        source_caps: [&rectangle.source_caps[0], &rectangle.source_caps[1]],
        edges,
    })
}

fn signed_axis_pose(pose: RigidPoseMm) -> Option<([usize; 3], [f64; 3])> {
    // These quaternion coefficients give exactly representable proper signed
    // permutations under the actual stored polynomial, not a normalized pose.
    let q = pose.rotation_xyzw;
    if !(q.iter().all(|v| *v == 0.0 || v.abs() == 1.0) || q.iter().all(|v| v.abs() == 0.5)) {
        return None;
    }
    let [x, y, z, w] = q;
    let m = [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - z * w),
            2.0 * (x * z + y * w),
        ],
        [
            2.0 * (x * y + z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - x * w),
        ],
        [
            2.0 * (x * z - y * w),
            2.0 * (y * z + x * w),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ];
    let mut axes = [0; 3];
    let mut signs = [0.0; 3];
    for row in 0..3 {
        let column = (0..3).find(|&c| m[row][c].abs() == 1.0)?;
        if (0..3).any(|c| c != column && m[row][c] != 0.0) {
            return None;
        }
        axes[row] = column;
        signs[row] = m[row][column];
    }
    if axes[0] == axes[1] || axes[0] == axes[2] || axes[1] == axes[2] {
        return None;
    }
    Some((axes, signs))
}

fn native_profile<'a>(
    occurrence: &SlabOccurrence<'a, '_>,
    scratch: &mut Scratch,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<NativeProfile<'a>>, GeometryError> {
    let checkpoint = scratch.bytes;
    let result: Result<Option<NativeProfile<'a>>, GeometryError> = (|| {
        if let Some(rectangle) = occurrence.leaf.and_then(|leaf| leaf.rectangle.as_deref()) {
            // Reuse the opaque six-original-face proof made by the leaf factory:
            // EVERY source face's native trim was already prepared and charged.
            return cached_rectangle_profile(
                occurrence.occurrence_index,
                rectangle,
                scratch,
                work,
                cancel,
            )
            .map(Some);
        }
        let definition = occurrence.source.definition();
        let source_view = occurrence.source.source_view()?;
        let brep = definition.brep();
        let Some(&first_face) = definition.face_ids().first() else {
            return Ok(None);
        };
        let physical_shell = brep.face(first_face).map_err(native_topology_error)?.shell;
        for &face in definition.face_ids() {
            work.charge(1, cancel)?;
            if brep.face(face).map_err(native_topology_error)?.shell != physical_shell {
                return Ok(None);
            }
        }
        let Some((axes, signs)) = signed_axis_pose(occurrence.pose) else {
            return Ok(None);
        };
        let vertical = axes[2];
        let mut heights = [f64::INFINITY, f64::NEG_INFINITY];
        for &face_id in definition.face_ids() {
            let face = brep.face(face_id).map_err(native_topology_error)?;
            for &wire_id in &face.loops {
                let wire = brep.loop_(wire_id).map_err(native_topology_error)?;
                for entry in &wire.edges {
                    work.charge(1, cancel)?;
                    let edge = brep.edge(entry.edge).map_err(native_topology_error)?;
                    let p = source_view.trim_source().source_vertex_point(
                        edge.vertices[usize::from(!entry.forward)],
                        work,
                        cancel,
                    )?;
                    if p.iter().any(|value| !value.is_finite()) {
                        return Ok(None);
                    }
                    heights[0] = heights[0].min(p[vertical]);
                    heights[1] = heights[1].max(p[vertical]);
                }
            }
        }
        if heights[0] >= heights[1] {
            return Ok(None);
        }
        let mut caps = [None; 2];
        let mut edges = Vec::new();
        let mut coordinate_error = [0.0_f64; 2];
        for (face_index, &face_id) in definition.face_ids().iter().enumerate() {
            work.charge(1, cancel)?;
            let face = brep.face(face_id).map_err(native_topology_error)?;
            // Borrow the SAME selected original domains used for this source's
            // slab. Raw native carrier metadata is not the affine material law.
            let domain = source_view.face_domain(face_index, scratch.remaining(), work, cancel)?;
            let Some(affine) = affine_face(domain.surface_evaluator()) else {
                return Ok(None);
            };
            if domain.source_chart_error_mm() != 0.0 {
                return Ok(None);
            }
            let normal = [
                Interval::point(affine.u[1])
                    .mul(Interval::point(affine.v[2]))
                    .sub(Interval::point(affine.u[2]).mul(Interval::point(affine.v[1]))),
                Interval::point(affine.u[2])
                    .mul(Interval::point(affine.v[0]))
                    .sub(Interval::point(affine.u[0]).mul(Interval::point(affine.v[2]))),
                Interval::point(affine.u[0])
                    .mul(Interval::point(affine.v[1]))
                    .sub(Interval::point(affine.u[1]).mul(Interval::point(affine.v[0]))),
            ];
            let Some(axis) = (0..3).find(|&a| {
                affine.u[a] == 0.0
                    && affine.v[a] == 0.0
                    && normal[a].lo.is_finite()
                    && normal[a].hi.is_finite()
                    && !normal[a].contains_zero()
            }) else {
                return Ok(None);
            };
            let constant = affine.origin[axis];
            let outward = if domain.orientation() {
                normal[axis]
            } else {
                normal[axis].neg()
            };
            let mut local = [[f64::INFINITY, f64::NEG_INFINITY]; 3];
            for &wire_id in &face.loops {
                let wire = brep.loop_(wire_id).map_err(native_topology_error)?;
                for entry in &wire.edges {
                    work.charge(1, cancel)?;
                    let edge = brep.edge(entry.edge).map_err(native_topology_error)?;
                    let p = source_view.trim_source().source_vertex_point(
                        edge.vertices[usize::from(!entry.forward)],
                        work,
                        cancel,
                    )?;
                    for a in 0..3 {
                        local[a][0] = local[a][0].min(p[a]);
                        local[a][1] = local[a][1].max(p[a]);
                    }
                }
            }
            if axis == vertical {
                let side = if constant == heights[0] {
                    0
                } else if constant == heights[1] {
                    1
                } else {
                    return Ok(None);
                };
                if caps[side].is_some()
                    || (side == 0 && outward.hi >= 0.0)
                    || (side == 1 && outward.lo <= 0.0)
                {
                    return Ok(None);
                }
                caps[side] = Some(
                    &definition
                        .faces()
                        .get(face_index)
                        .ok_or_else(|| {
                            error(
                                GeometryErrorCode::KernelFailure,
                                "native cap original face attribution is missing",
                            )
                        })?
                        .face_id,
                );
            } else if face.loops.len() != 1
                || brep
                    .loop_(face.loops[0])
                    .map_err(native_topology_error)?
                    .edges
                    .len()
                    != 4
                || local[vertical] != heights
            {
                return Ok(None);
            }
            let other = [(axis + 1) % 3, (axis + 2) % 3];
            let mut corners = 0_u8;
            let mut ordinal = 0;
            for (wire_index, &wire_id) in face.loops.iter().enumerate() {
                let wire = brep.loop_(wire_id).map_err(native_topology_error)?;
                for (coedge_index, entry) in wire.edges.iter().enumerate() {
                    work.charge(1, cancel)?;
                    let edge = brep.edge(entry.edge).map_err(native_topology_error)?;
                    let canonical = [
                        source_view.trim_source().source_vertex_point(
                            edge.vertices[0],
                            work,
                            cancel,
                        )?,
                        source_view.trim_source().source_vertex_point(
                            edge.vertices[1],
                            work,
                            cancel,
                        )?,
                    ];
                    let a = canonical[usize::from(!entry.forward)];
                    let b = canonical[usize::from(entry.forward)];
                    if a[axis] != constant
                        || b[axis] != constant
                        || (0..3).filter(|&j| a[j] != b[j]).count() != 1
                    {
                        return Ok(None);
                    }
                    if axis != vertical {
                        let Some(corner) = (0..4)
                            .find(|c| (0..2).all(|j| a[other[j]] == local[other[j]][(c >> j) & 1]))
                        else {
                            return Ok(None);
                        };
                        corners |= 1 << corner;
                    }
                    let index = ordinal;
                    ordinal += 1;
                    let correspondence = domain.original_coedge_correspondence(index)?;
                    let binding = correspondence.binding().ok_or_else(|| {
                        error(
                            GeometryErrorCode::ParameterCorrespondenceUnavailable,
                            "original affine profile has no sealed source binding",
                        )
                    })?;
                    if binding.face != face_id
                        || binding.edge != entry.edge
                        || binding.endpoints != edge.vertices
                        || binding.native_interval != [edge.interval.min(), edge.interval.max()]
                        || binding.owner != [face_index, wire_index, coedge_index]
                    {
                        return Err(error(
                            GeometryErrorCode::ParameterCorrespondenceUnavailable,
                            "original affine profile changed its native incidence binding",
                        ));
                    }
                    let straight = domain.with_original_spatial_curve(
                        index,
                        scratch.remaining(),
                        work,
                        cancel,
                        |curve, work, cancel| {
                            let Some(affine) = curve.exact_affine_coefficients(work, cancel)?
                            else {
                                return Ok(false);
                            };
                            if affine.denominator.sign() == 0 {
                                return Err(error(
                                    GeometryErrorCode::ConditioningFailure,
                                    "original affine profile has zero homogeneous denominator",
                                ));
                            }
                            for endpoint in 0..2 {
                                let parameter = correspondence.spatial_endpoint(endpoint == 1);
                                for axis in 0..3 {
                                    if !affine.endpoint_coordinate_equal(
                                        parameter,
                                        axis,
                                        canonical[endpoint][axis],
                                        work,
                                        cancel,
                                    )? {
                                        return Ok(false);
                                    }
                                }
                            }
                            Ok(true)
                        },
                    )?;
                    if !straight {
                        return Ok(None);
                    }
                    // Affine law plus both exact endpoint identities proves the
                    // complete selected source segment, not a sampled line fit.
                    let uv = domain.coedge_operand(index)?;
                    if !crate::kernel::certified_curve2d_is_straight_regular(
                        &uv,
                        correspondence.uv_range(),
                        work,
                        cancel,
                    )? {
                        return Ok(None);
                    }
                }
            }
            if axis != vertical {
                if corners != 15 {
                    return Ok(None);
                }
                let along = 3 - axis - vertical;
                if local[along][0] >= local[along][1] {
                    return Ok(None);
                }
                let mut p = [0.0; 3];
                p[axis] = constant;
                p[vertical] = heights[0];
                p[along] = local[along][0];
                let mut q = p;
                q[along] = local[along][1];
                let transform = |p: [f64; 3]| -> Option<([f64; 2], [f64; 2])> {
                    let x = native_sum(signs[0] * p[axes[0]], occurrence.pose.translation_mm[0])?;
                    let y = native_sum(signs[1] * p[axes[1]], occurrence.pose.translation_mm[1])?;
                    Some(([x[0], y[0]], [x[1], y[1]]))
                };
                let (Some(mut a), Some(mut b)) = (transform(p), transform(q)) else {
                    return Ok(None);
                };
                let row = if axes[0] == axis { 0 } else { 1 };
                let world_positive = (outward.lo > 0.0) == (signs[row] > 0.0);
                let increasing = (row == 0 && world_positive) || (row == 1 && !world_positive);
                if (b.0[1 - row] > a.0[1 - row]) != increasing {
                    std::mem::swap(&mut a, &mut b);
                }
                for (axis, error) in coordinate_error.iter_mut().enumerate() {
                    *error = (*error).max(a.1[axis].abs()).max(b.1[axis].abs());
                }
                let face_id = &definition
                    .faces()
                    .get(face_index)
                    .ok_or_else(|| {
                        error(
                            GeometryErrorCode::KernelFailure,
                            "native orthogonal source face attribution is missing",
                        )
                    })?
                    .face_id;
                scratch.push(
                    &mut edges,
                    NativeProfileEdge {
                        a: a.0,
                        b: b.0,
                        a_residual: a.1,
                        b_residual: b.1,
                        face_id,
                    },
                    work,
                    cancel,
                )?;
            }
        }
        let [Some(mut lower_cap), Some(mut upper_cap)] = caps else {
            return Ok(None);
        };
        if edges.is_empty() {
            return Ok(None);
        }
        // Every original side is a full-height affine rectangle, both caps and
        // every original pcurve/edge have been proved above. This opaque native
        // product is itself a world-Z invariance proof, not polygon invariance.
        let height = occurrence.analysis.world_height_interval_mm().unwrap();
        let Some(mut bottom) = native_sum(signs[2] * heights[0], occurrence.pose.translation_mm[2])
        else {
            return Ok(None);
        };
        let Some(mut top) = native_sum(signs[2] * heights[1], occurrence.pose.translation_mm[2])
        else {
            return Ok(None);
        };
        if signs[2] < 0.0 {
            std::mem::swap(&mut bottom, &mut top);
            std::mem::swap(&mut lower_cap, &mut upper_cap);
        }
        let material_height = [
            native_support_interval(bottom).hi,
            native_support_interval(top).lo,
        ];
        if material_height[0] > height[0] || material_height[1] < height[1] {
            return Ok(None);
        }
        // This closes original native side-face incidence, not a nearest-lineage
        // stitching of sampled sections. Hole cycles retain their inward winding.
        for edge in &edges {
            let mut incoming = 0;
            let mut outgoing = 0;
            for other in &edges {
                work.charge(1, cancel)?;
                incoming += usize::from(other.b == edge.a && other.b_residual == edge.a_residual);
                outgoing += usize::from(other.a == edge.a && other.a_residual == edge.a_residual);
            }
            if incoming != 1 || outgoing != 1 {
                return Ok(None);
            }
        }
        let coordinate_error_mm = coordinate_error.map(Interval::point);
        let coordinate_error_mm = coordinate_error_mm[0]
            .square()
            .add(coordinate_error_mm[1].square())
            .sqrt()?
            .hi;
        Ok(Some(NativeProfile {
            occurrence_index: occurrence.occurrence_index,
            world_height_interval_mm: material_height,
            height_supports: [bottom, top],
            coordinate_error_mm,
            source_caps: [lower_cap, upper_cap],
            edges,
        }))
    })();
    // A refused optional product lane retains no allocations. Its native work
    // remains cumulative, but dropped trim/profile scratch is no longer live.
    if !matches!(&result, Ok(Some(_))) {
        scratch.bytes = checkpoint;
    }
    result
}

#[derive(Debug, Clone, Copy, PartialEq)]
/// Representative native-grid edge; the opaque proof retains exact support
/// identity and the certificate bounds representative-to-native displacement.
pub struct NativeUnionBoundaryEdge {
    a: [f64; 2],
    b: [f64; 2],
}
impl NativeUnionBoundaryEdge {
    pub fn start_world_xy_mm(&self) -> [f64; 2] {
        self.a
    }
    pub fn end_world_xy_mm(&self) -> [f64; 2] {
        self.b
    }
}
fn side_occupied(
    profiles: &[NativeProfile<'_>],
    axis: usize,
    coordinate: f64,
    interval: [f64; 2],
    positive: bool,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let mut occupied = false;
    for profile in profiles {
        let mut winding = 0_i64;
        for edge in &profile.edges {
            work.charge(1, cancel)?;
            if edge.a[0] != edge.b[0] {
                continue;
            }
            let lo = edge.a[1].min(edge.b[1]);
            let hi = edge.a[1].max(edge.b[1]);
            // Exact open-cell side ordering: no midpoint, epsilon or root merge.
            let crosses = if axis == 0 {
                lo <= interval[0]
                    && interval[1] <= hi
                    && (edge.a[0] > coordinate || (!positive && edge.a[0] == coordinate))
            } else {
                let y = if positive {
                    lo <= coordinate && coordinate < hi
                } else {
                    lo < coordinate && coordinate <= hi
                };
                y && edge.a[0] >= interval[1]
            };
            if crosses {
                winding += if edge.b[1] > edge.a[1] { 1 } else { -1 };
            }
        }
        if !(0..=1).contains(&winding) {
            return Err(unresolved(
                0,
                profiles.len() - 1,
                "original native orthogonal profile has an unresolved oriented material degree",
            ));
        }
        occupied |= winding == 1;
    }
    Ok(occupied)
}
fn sort_native_supports(
    values: &mut Vec<[f64; 2]>,
    pair: [usize; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    fn sift(
        values: &mut [[f64; 2]],
        mut root: usize,
        end: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        while root < end / 2 {
            work.charge(3, cancel)?;
            let mut child = 2 * root + 1;
            if child + 1 < end && values[child][0] < values[child + 1][0] {
                child += 1;
            }
            if values[root][0] >= values[child][0] {
                break;
            }
            values.swap(root, child);
            root = child;
        }
        Ok(())
    }
    let count = values.len();
    for root in (0..count / 2).rev() {
        sift(values, root, count, work, cancel)?;
    }
    for end in (1..count).rev() {
        work.charge(1, cancel)?;
        values.swap(0, end);
        sift(values, 0, end, work, cancel)?;
    }
    let mut length = 0;
    for i in 0..values.len() {
        work.charge(1, cancel)?;
        if length != 0 && values[i][0] == values[length - 1][0] {
            if values[i][1] != values[length - 1][1] {
                return Err(unresolved(
                    pair[0],
                    pair[1],
                    "distinct exact native supports collapse to one floating coordinate; incidence is unresolved",
                ));
            }
        } else {
            values[length] = values[i];
            length += 1;
        }
    }
    values.truncate(length);
    Ok(())
}

fn rectangular_arrangement(
    boxes: &[NativeProfile<'_>],
    candidate: CandidateLoops<'_>,
    shape_policy: f64,
    policy: NativeUnionPolicy,
    scratch: &mut Scratch,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<
    (
        f64,
        Vec<NativeUnionBoundaryEdge>,
        Vec<NativeUnionBoundarySource>,
    ),
    GeometryError,
> {
    let mut coordinates: [Vec<[f64; 2]>; 2] = [Vec::new(), Vec::new()];
    let mut support_error = [0.0_f64; 2];
    for box_ in boxes {
        for edge in &box_.edges {
            for (point, residual) in [(edge.a, edge.a_residual), (edge.b, edge.b_residual)] {
                for axis in 0..2 {
                    scratch.push(
                        &mut coordinates[axis],
                        [point[axis], residual[axis]],
                        work,
                        cancel,
                    )?;
                    support_error[axis] = support_error[axis].max(residual[axis].abs());
                }
            }
        }
    }
    let pair = [
        boxes.first().map_or(0, |p| p.occurrence_index),
        boxes.last().map_or(0, |p| p.occurrence_index),
    ];
    for c in &mut coordinates {
        sort_native_supports(c, pair, work, cancel)?;
    }
    // Correct rounding is monotone. Distinct rounded representatives have
    // strictly ordered exact sums; equal representatives were accepted ONLY
    // with equal TwoSum residuals. Thus the coordinatewise piecewise-affine
    // map from this representative grid to the TRUE native support grid is
    // a homeomorphism, and its displacement is bounded in both directions.
    let support_error = Interval::point(support_error[0])
        .square()
        .add(Interval::point(support_error[1]).square())
        .sqrt()?
        .hi;
    let mut expected = Vec::new();
    let mut attribution = Vec::new();
    for axis in 0..2 {
        for &[coordinate, _] in &coordinates[axis] {
            for window in coordinates[1 - axis].windows(2) {
                work.charge(1, cancel)?;
                let interval = [window[0][0], window[1][0]];
                let negative =
                    side_occupied(boxes, axis, coordinate, interval, false, work, cancel)?;
                let positive =
                    side_occupied(boxes, axis, coordinate, interval, true, work, cancel)?;
                if negative == positive {
                    continue;
                }
                let mut a = [0.0; 2];
                let mut b = [0.0; 2];
                a[axis] = coordinate;
                b[axis] = coordinate;
                a[1 - axis] = interval[0];
                b[1 - axis] = interval[1];
                // Vertical up has material west; horizontal right has material north.
                if (axis == 0 && positive) || (axis == 1 && negative) {
                    std::mem::swap(&mut a, &mut b);
                }
                let index = expected.len();
                scratch.push(
                    &mut expected,
                    NativeUnionBoundaryEdge { a, b },
                    work,
                    cancel,
                )?;
                for box_ in boxes {
                    for source in &box_.edges {
                        work.charge(1, cancel)?;
                        if source.a[axis] == coordinate
                            && source.b[axis] == coordinate
                            && source.a[1 - axis].min(source.b[1 - axis]) <= interval[0]
                            && interval[1] <= source.a[1 - axis].max(source.b[1 - axis])
                            && (source.b[1 - axis] > source.a[1 - axis])
                                == (b[1 - axis] > a[1 - axis])
                        {
                            scratch.owned(source.face_id.as_str().len(), work, cancel)?;
                            scratch.push(
                                &mut attribution,
                                NativeUnionBoundarySource {
                                    boundary_edge_index: index,
                                    occurrence_index: box_.occurrence_index,
                                    face_id: (*source.face_id).clone(),
                                },
                                work,
                                cancel,
                            )?;
                        }
                    }
                }
            }
        }
    }
    // A checkerboard point contact has degree four, not a regular union cycle.
    // Exact coincidence is allowed only because the original native support
    // equations and occupied side were proved equal above.
    for edge in &expected {
        let mut incoming = 0;
        let mut outgoing = 0;
        for other in &expected {
            work.charge(1, cancel)?;
            incoming += usize::from(other.b == edge.a);
            outgoing += usize::from(other.a == edge.a);
        }
        if incoming != 1 || outgoing != 1 {
            return Err(unresolved(
                0,
                boxes.len() - 1,
                "exact native rectangular arrangement has a nonmanifold contact vertex",
            ));
        }
    }
    let allowance = policy.candidate_coordinate_error_mm;
    if allowance > shape_policy {
        return Err(error(
            GeometryErrorCode::ShapeErrorExceeded,
            "native union coordinate policy exceeds captured shape policy",
        ));
    }
    let mut matched = Vec::new();
    scratch.reserve(&mut matched, expected.len(), work, cancel)?;
    matched.resize(expected.len(), false);
    let mut achieved = 0.0_f64;
    let mut snapped_rings = Vec::new();
    let mut actual_rings = Vec::new();
    for loop_ in candidate.iter() {
        let actual = ring(loop_, scratch, work, cancel)?;
        let mut snapped = Ring {
            hole: actual.hole,
            points: Vec::new(),
        };
        scratch.reserve(&mut snapped.points, actual.points.len(), work, cancel)?;
        for &point in &actual.points {
            let mut p = [0.0; 2];
            for axis in 0..2 {
                let mut found = None;
                for &[coordinate, _] in &coordinates[axis] {
                    work.charge(1, cancel)?;
                    if point[axis] == coordinate
                        || Interval::point(point[axis])
                            .sub(Interval::point(coordinate))
                            .abs_upper()
                            <= allowance
                    {
                        if found.is_some() {
                            return Err(unresolved(
                                0,
                                boxes.len() - 1,
                                "native support/rounding endpoint association is ambiguous",
                            ));
                        }
                        found = Some(coordinate);
                    }
                }
                p[axis]=found.ok_or_else(||error(GeometryErrorCode::ShapeErrorExceeded,
                    "final union vertex does not match an exact native arrangement support within policy"))?;
            }
            achieved = achieved.max(distance_upper(point, p)?);
            snapped.points.push(p);
        }
        for i in 0..snapped.points.len() {
            let a = snapped.points[i];
            let b = snapped.points[(i + 1) % snapped.points.len()];
            let Some(axis) = (0..2).find(|&axis| a[axis] == b[axis] && a[1 - axis] != b[1 - axis])
            else {
                return Err(unresolved(
                    0,
                    boxes.len() - 1,
                    "candidate graph does not follow a native straight arrangement edge",
                ));
            };
            let forward = b[1 - axis] > a[1 - axis];
            let mut covered = 0;
            for window in coordinates[1 - axis].windows(2) {
                if a[1 - axis].min(b[1 - axis]) > window[0][0]
                    || window[1][0] > a[1 - axis].max(b[1 - axis])
                {
                    continue;
                }
                let mut p = a;
                let mut q = a;
                p[1 - axis] = if forward { window[0][0] } else { window[1][0] };
                q[1 - axis] = if forward { window[1][0] } else { window[0][0] };
                let mut found = None;
                for (index, e) in expected.iter().enumerate() {
                    work.charge(1, cancel)?;
                    if e.a == p && e.b == q {
                        found = Some(index);
                        break;
                    }
                }
                let index = found.ok_or_else(|| {
                    unresolved(
                        0,
                        boxes.len() - 1,
                        "candidate boundary has wrong native union occupied-side incidence",
                    )
                })?;
                if matched[index] {
                    return Err(unresolved(
                        0,
                        boxes.len() - 1,
                        "candidate repeats a native union arrangement edge",
                    ));
                }
                matched[index] = true;
                covered += 1;
            }
            if covered == 0 {
                return Err(unresolved(
                    0,
                    boxes.len() - 1,
                    "candidate arrangement edge has no native interval coverage",
                ));
            }
        }
        scratch.push(&mut snapped_rings, snapped, work, cancel)?;
        scratch.push(&mut actual_rings, actual, work, cancel)?;
    }
    if matched.iter().any(|m| !*m) {
        return Err(unresolved(
            0,
            boxes.len() - 1,
            "candidate omits an exposed native union arrangement edge",
        ));
    }
    achieved = Interval::point(achieved)
        .add(Interval::point(support_error))
        .hi;
    if achieved > shape_policy {
        return Err(error(
            GeometryErrorCode::ShapeErrorExceeded,
            "native union candidate rounding exceeds native shape policy",
        ));
    }
    feature_clearance(&snapped_rings, achieved, scratch, work, cancel)?;
    feature_clearance(&actual_rings, 0.0, scratch, work, cancel)?;
    Ok((achieved, expected, attribution))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rectangular_arrangement(
        boxes: &[NativeProfile<'_>],
        candidate: &[SectionLoopMm],
        shape_policy: f64,
        policy: NativeUnionPolicy,
        scratch: &mut Scratch,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<
        (
            f64,
            Vec<NativeUnionBoundaryEdge>,
            Vec<NativeUnionBoundarySource>,
        ),
        GeometryError,
    > {
        super::rectangular_arrangement(
            boxes,
            CandidateLoops::whole(candidate),
            shape_policy,
            policy,
            scratch,
            work,
            cancel,
        )
    }
    fn policy(error: f64) -> NativeUnionPolicy {
        NativeUnionPolicy {
            candidate_coordinate_error_mm: error,
        }
    }
    fn candidate(points: &[[f64; 2]], hole: bool) -> SectionLoopMm {
        let mut points_mm: Vec<_> = points.iter().map(|p| [p[0], p[1], 4.0]).collect();
        points_mm.push(points_mm[0]);
        SectionLoopMm {
            is_hole: hole,
            points_mm,
        }
    }
    fn rectangle(x0: f64, y0: f64, x1: f64, y1: f64) -> SectionLoopMm {
        candidate(&[[x0, y0], [x1, y0], [x1, y1], [x0, y1]], false)
    }
    fn copy_loops(loops: &[SectionLoopMm]) -> Vec<SectionLoopMm> {
        loops
            .iter()
            .map(|boundary| SectionLoopMm {
                is_hole: boundary.is_hole,
                points_mm: boundary.points_mm.clone(),
            })
            .collect()
    }
    fn profile<'a>(points: &[[f64; 2]], face: &'a FaceId) -> NativeProfile<'a> {
        NativeProfile {
            occurrence_index: 0,
            world_height_interval_mm: [0.0, 8.0],
            height_supports: [[0.0, 0.0], [8.0, 0.0]],
            coordinate_error_mm: 0.0,
            source_caps: [face; 2],
            edges: (0..points.len())
                .map(|i| NativeProfileEdge {
                    a: points[i],
                    b: points[(i + 1) % points.len()],
                    a_residual: [0.0; 2],
                    b_residual: [0.0; 2],
                    face_id: face,
                })
                .collect(),
        }
    }
    fn slab(
        source: &crate::PreparedSolid<'_>,
        pose: RigidPoseMm,
        retained: usize,
        work: &mut GeometryWorkBudget,
    ) -> SlabAnalysis {
        ranged_slab(source, pose, [3.99, 4.01], 4.0, retained, work)
    }
    fn ranged_slab(
        source: &crate::PreparedSolid<'_>,
        pose: RigidPoseMm,
        interval: [f64; 2],
        z: f64,
        retained: usize,
        work: &mut GeometryWorkBudget,
    ) -> SlabAnalysis {
        let cancel = AtomicBool::new(false);
        let remaining = crate::MAX_PREPARED_SECTION_BYTES.saturating_sub(retained);
        source
            .analyze_placed_slab(
                &crate::PlacedSlabRequest {
                    pose,
                    world_height_interval_mm: interval,
                    sample_world_z_mm: z,
                    native_shape_error_mm: 0.1,
                },
                remaining.saturating_sub(source.retained_bytes()),
                work,
                &cancel,
            )
            .unwrap_or_else(|error| {
                panic!(
                    "original slab {:?}, pose {pose:?}, interval {interval:?}, sample {z}: {error:?}",
                    source.definition().id()
                )
            })
    }
    fn original(name: &str) -> Definition {
        super::super::test_source::recipe(name)
    }

    #[test]
    fn exact_orthogonal_arrangement_proves_coincidence_occupied_side_and_candidate_graph() {
        let face = FaceId::from_step_entity(18).unwrap();
        let mut profiles = [
            profile(&[[0.0, 0.0], [20.0, 0.0], [20.0, 10.0], [0.0, 10.0]], &face),
            profile(
                &[[10.0, 0.0], [30.0, 0.0], [30.0, 10.0], [10.0, 10.0]],
                &face,
            ),
        ];
        profiles[1].occurrence_index = 1;
        let mut work = GeometryWorkBudget::new(100_000);
        let (bound, edges, attribution) = rectangular_arrangement(
            &profiles,
            &[rectangle(0.0, 0.0, 30.0, 10.0)],
            0.1,
            policy(0.0),
            &mut Scratch { bytes: 0 },
            &mut work,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(bound, 0.0);
        assert!(
            attribution
                .iter()
                .all(|a| a.boundary_edge_index < edges.len())
        );
        assert!(attribution.iter().any(|a| a.occurrence_index == 0));
        assert!(attribution.iter().any(|a| a.occurrence_index == 1));
        assert!(
            rectangular_arrangement(
                &profiles,
                &[rectangle(0.0, 0.0, 20.0, 10.0)],
                0.1,
                policy(0.0),
                &mut Scratch { bytes: 0 },
                &mut work,
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }

    #[test]
    fn exact_profile_winding_preserves_holes_and_refuses_filled_void() {
        let face = FaceId::from_step_entity(18).unwrap();
        let outer = [[0.0, 0.0], [20.0, 0.0], [20.0, 10.0], [0.0, 10.0]];
        let inner = [[5.0, 3.0], [5.0, 7.0], [15.0, 7.0], [15.0, 3.0]];
        let mut source = profile(&outer, &face);
        source.edges.extend(profile(&inner, &face).edges);
        let sources = [source];
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        rectangular_arrangement(
            &sources,
            &[candidate(&outer, false), candidate(&inner, true)],
            0.1,
            policy(0.0),
            &mut Scratch { bytes: 0 },
            &mut work,
            &cancel,
        )
        .unwrap();
        assert_eq!(
            rectangular_arrangement(
                &sources,
                &[candidate(&outer, false)],
                0.1,
                policy(0.0),
                &mut Scratch { bytes: 0 },
                &mut work,
                &cancel
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::AnalysisIndeterminate
        );
    }

    #[test]
    fn ambiguous_tiny_bridge_and_point_contact_are_not_epsilon_merged() {
        let face = FaceId::from_step_entity(18).unwrap();
        let touching = [
            profile(&[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]], &face),
            profile(&[[1.0, 1.0], [2.0, 1.0], [2.0, 2.0], [1.0, 2.0]], &face),
        ];
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        assert_eq!(
            rectangular_arrangement(
                &touching,
                &[rectangle(0.0, 0.0, 1.0, 1.0), rectangle(1.0, 1.0, 2.0, 2.0)],
                0.1,
                policy(0.0),
                &mut Scratch { bytes: 0 },
                &mut work,
                &cancel
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::AnalysisIndeterminate
        );
        let thin = [profile(
            &[[0.0, 0.0], [2.0, 0.0], [2.0, 1e-8], [0.0, 1e-8]],
            &face,
        )];
        assert_eq!(
            rectangular_arrangement(
                &thin,
                &[rectangle(0.0, 0.0, 2.0, 1e-8)],
                0.1,
                policy(1e-6),
                &mut Scratch { bytes: 0 },
                &mut work,
                &cancel
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::AnalysisIndeterminate
        );
    }

    #[test]
    fn source_native_plane_polynomial_and_rational_overlapping_boxes_bind_original_occurrences() {
        for name in [
            "box-mm.step",
            "native-parity-bspline-box.step",
            "native-parity-nurbs-box.step",
        ] {
            let definition = original(name);
            let cancel = AtomicBool::new(false);
            let mut work = GeometryWorkBudget::new(50_000_000);
            let source = crate::prepare_solid(
                &definition,
                crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
                &mut work,
                &cancel,
            )
            .unwrap();
            let pose = RigidPoseMm {
                translation_mm: [10.0, 0.0, 0.0],
                ..RigidPoseMm::IDENTITY
            };
            let a = slab(&source, RigidPoseMm::IDENTITY, 0, &mut work);
            let b = slab(&source, pose, a.retained_bytes(), &mut work);
            assert_eq!(
                a.material_realization_identity(),
                b.material_realization_identity()
            );
            let leaf_a = source_leaf(&source, 1, RigidPoseMm::IDENTITY, &a, &mut work);
            let leaf_b = source_leaf(&source, 2, pose, &b, &mut work);
            assert_eq!(
                leaf_a.material_realization_identity(),
                leaf_b.material_realization_identity()
            );
            assert_eq!(
                leaf_a.material_realization_identity(),
                crate::kernel::MaterialRealizationIdentity::new(&definition),
            );
            let occurrences = [
                NativeUnionOccurrence {
                    occurrence_id: OccurrenceId::new(1).unwrap(),
                    source: &source,
                    pose: RigidPoseMm::IDENTITY,
                    evidence: NativeUnionOccurrenceEvidence::Slab {
                        analysis: &a,
                        leaf: &leaf_a,
                        endpoint_ownerships: &[],
                    },
                },
                NativeUnionOccurrence {
                    occurrence_id: OccurrenceId::new(2).unwrap(),
                    source: &source,
                    pose,
                    evidence: NativeUnionOccurrenceEvidence::Slab {
                        analysis: &b,
                        leaf: &leaf_b,
                        endpoint_ownerships: &[],
                    },
                },
            ];
            let loops = [rectangle(0.0, 0.0, 30.0, 10.0)];
            let certificate=certify_native_union(&occurrences,&loops,policy(1e-8),&mut work,&cancel)
                .expect("actual native rectangular face/trim union proof, independent of source encoding labels");
            assert_eq!(certificate.coverage(), NativeUnionCoverage::InvariantUnion);
            assert_eq!(certificate.sources().len(), 2);
            assert_eq!(certificate.sources()[1].definition_id(), definition.id());
            assert_eq!(
                certificate.sources()[1].occurrence_id(),
                OccurrenceId::new(2).unwrap()
            );
            assert_eq!(certificate.sources()[1].pose(), pose);
            assert_eq!(certificate.world_height_interval_mm(), [3.99, 4.01]);
            assert_eq!(certificate.sample_world_z_mm(), 4.0);
            assert!(
                certificate
                    .boundary_sources()
                    .iter()
                    .all(|p| definition.faces().iter().any(|f| f.face_id == p.face_id))
            );
            let changed = [rectangle(0.0, 0.0, 29.0, 10.0)];
            assert_ne!(
                candidate_hash(&changed, 4.0, &mut work, &cancel).unwrap(),
                *certificate.candidate_geometry_sha256()
            );
            assert!(
                certify_native_union(&occurrences, &changed, policy(1e-8), &mut work, &cancel)
                    .is_err()
            );
        }
    }

    #[test]
    fn separated_varying_native_sources_use_full_occupied_tubes_not_invariance_assertions() {
        let definition = original("native-parity-cone-frustum.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(50_000_000);
        let source = crate::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let pose = RigidPoseMm {
            translation_mm: [100.0, 0.0, 0.0],
            ..RigidPoseMm::IDENTITY
        };
        let a = slab(&source, RigidPoseMm::IDENTITY, 0, &mut work);
        let b = slab(&source, pose, a.retained_bytes(), &mut work);
        assert!(a.invariance.is_none() && b.invariance.is_none());
        let loops: Vec<_> = a
            .section
            .loops
            .iter()
            .chain(&b.section.loops)
            .map(|l| SectionLoopMm {
                is_hole: l.is_hole,
                points_mm: l.points_mm.iter().map(|p| [p[0], p[1], 4.0]).collect(),
            })
            .collect();
        let leaf_a = source_leaf(&source, 1, RigidPoseMm::IDENTITY, &a, &mut work);
        let leaf_b = source_leaf(&source, 2, pose, &b, &mut work);
        let occurrences = [
            NativeUnionOccurrence {
                occurrence_id: OccurrenceId::new(1).unwrap(),
                source: &source,
                pose: RigidPoseMm::IDENTITY,
                evidence: NativeUnionOccurrenceEvidence::Slab {
                    analysis: &a,
                    leaf: &leaf_a,
                    endpoint_ownerships: &[],
                },
            },
            NativeUnionOccurrence {
                occurrence_id: OccurrenceId::new(2).unwrap(),
                source: &source,
                pose,
                evidence: NativeUnionOccurrenceEvidence::Slab {
                    analysis: &b,
                    leaf: &leaf_b,
                    endpoint_ownerships: &[],
                },
            },
        ];
        let proof =
            certify_native_union(&occurrences, &loops, policy(0.0), &mut work, &cancel).unwrap();
        assert_eq!(proof.coverage(), NativeUnionCoverage::DisjointSources);
        assert_eq!(proof.material_component_count(), 2);
        assert!(proof.achieved_shape_error_mm() <= 0.1);
    }

    #[test]
    fn curved_native_crossings_and_pose_capture_mismatch_have_typed_refusals() {
        let definition = original("native-parity-cone-frustum.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(50_000_000);
        let source = crate::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let pose = RigidPoseMm {
            translation_mm: [1.0, 0.0, 0.0],
            ..RigidPoseMm::IDENTITY
        };
        let a = slab(&source, RigidPoseMm::IDENTITY, 0, &mut work);
        let b = slab(&source, pose, a.retained_bytes(), &mut work);
        let leaf_a = source_leaf(&source, 1, RigidPoseMm::IDENTITY, &a, &mut work);
        let leaf_b = source_leaf(&source, 2, pose, &b, &mut work);
        let occurrences = [
            NativeUnionOccurrence {
                occurrence_id: OccurrenceId::new(1).unwrap(),
                source: &source,
                pose: RigidPoseMm::IDENTITY,
                evidence: NativeUnionOccurrenceEvidence::Slab {
                    analysis: &a,
                    leaf: &leaf_a,
                    endpoint_ownerships: &[],
                },
            },
            NativeUnionOccurrence {
                occurrence_id: OccurrenceId::new(2).unwrap(),
                source: &source,
                pose,
                evidence: NativeUnionOccurrenceEvidence::Slab {
                    analysis: &b,
                    leaf: &leaf_b,
                    endpoint_ownerships: &[],
                },
            },
        ];
        let failure =
            certify_native_union(&occurrences, &[], policy(0.0), &mut work, &cancel).unwrap_err();
        assert_eq!(failure.code, GeometryErrorCode::AnalysisIndeterminate);
        assert!(failure.message.contains("occurrence pair 1/2"));
        let stale = [NativeUnionOccurrence {
            occurrence_id: OccurrenceId::new(1).unwrap(),
            source: &source,
            pose,
            evidence: NativeUnionOccurrenceEvidence::Slab {
                analysis: &a,
                leaf: &leaf_a,
                endpoint_ownerships: &[],
            },
        }];
        assert_eq!(
            certify_native_union(&stale, &[], policy(0.0), &mut work, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::InvalidGeometry
        );
    }

    #[test]
    fn exact_arithmetic_does_not_claim_rounded_or_underflowed_identities() {
        assert_eq!(exact_add(20.0, 10.0), Some(30.0));
        assert_eq!(exact_add(1.0, f64::EPSILON * 0.5), None);
        assert_eq!(exact_mul(0.1, 0.1), None);
        assert_eq!(exact_mul(0.25, f64::from_bits(1)), None);
        assert_eq!(exact_div(10.0, 2.0), Some(5.0));
        assert_eq!(exact_div(1.0, 3.0), None);
    }

    #[test]
    fn joint_work_scratch_and_cancellation_are_cumulative() {
        let cancel = AtomicBool::new(false);
        let face = FaceId::from_step_entity(18).unwrap();
        let sources = [profile(
            &[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            &face,
        )];
        let mut work = GeometryWorkBudget::new(3);
        assert_eq!(
            rectangular_arrangement(
                &sources,
                &[rectangle(0.0, 0.0, 1.0, 1.0)],
                0.1,
                policy(0.0),
                &mut Scratch { bytes: 0 },
                &mut work,
                &cancel
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::ResourceLimit
        );
        assert!(work.consumed() > 0);
        let mut work = GeometryWorkBudget::new(100_000);
        assert_eq!(
            rectangular_arrangement(
                &sources,
                &[],
                0.1,
                policy(0.0),
                &mut Scratch { bytes: MAX_SCRATCH },
                &mut work,
                &cancel
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::ResourceLimit
        );
        cancel.store(true, std::sync::atomic::Ordering::Release);
        assert_eq!(
            candidate_hash(&[], 4.0, &mut work, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::Cancelled
        );
        assert_eq!(
            certify_native_union(&[], &[], policy(0.0), &mut work, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::Cancelled
        );
    }

    fn xy_candidate(analysis: &SlabAnalysis, z: f64) -> Vec<SectionLoopMm> {
        analysis
            .section
            .loops
            .iter()
            .map(|l| SectionLoopMm {
                is_hole: l.is_hole,
                points_mm: l.points_mm.iter().map(|p| [p[0], p[1], z]).collect(),
            })
            .collect()
    }
    fn source_leaf(
        source: &crate::PreparedSolid<'_>,
        id: u32,
        pose: RigidPoseMm,
        analysis: &SlabAnalysis,
        work: &mut GeometryWorkBudget,
    ) -> NativeSourceLeafReceipt {
        let z = analysis.sample_world_z_mm().unwrap();
        let certify = if analysis.has_critical_collar() {
            certify_native_one_sided_source_leaf
        } else {
            certify_native_source_leaf
        };
        certify(
            &SourceLeafRequest {
                source,
                occurrence_id: &OccurrenceId::new(id).unwrap(),
                pose,
                analysis,
                actual_used_source_loops: &xy_candidate(analysis, z),
                policy: policy(0.0),
            },
            work,
            &AtomicBool::new(false),
        )
        .expect("actual immutable native slab and actual source-M polygon proof")
    }

    #[test]
    fn registered_regular_pair_binds_source_buffers_receipt_and_closed_stream() {
        let cancel = AtomicBool::new(false);
        // Exercise the admitted operator source, not the deferred synthetic
        // source-corner/critical expansion. The planar test owns non-star cases.
        let definition = crate::import_step(
            include_bytes!(
                "../../../../fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step"
            ),
            &cancel,
        )
        .unwrap();
        let pose = RigidPoseMm {
            translation_mm: [100.0, 105.0, 0.0],
            rotation_xyzw: [0.0, 0.0, 0.0, 1.0],
        };
        let mut work = GeometryWorkBudget::new(200_000_000);
        let cut = 5.03;
        let before = ranged_slab(&definition, pose, [5.01, cut], 5.02, 0, &mut work);
        let after = ranged_slab(
            &definition,
            pose,
            [cut, 5.05],
            5.04,
            before.retained_bytes(),
            &mut work,
        );
        let before_m = xy_candidate(&before, before.sample_world_z_mm().unwrap());
        let after_m = xy_candidate(&after, after.sample_world_z_mm().unwrap());
        let id = OccurrenceId::new(1).unwrap();
        let before_leaf = certify_native_source_leaf(
            &SourceLeafRequest {
                definition: &definition,
                occurrence_id: &id,
                pose,
                analysis: &before,
                actual_used_source_loops: &before_m,
                policy: policy(0.0),
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        let after_leaf = certify_native_source_leaf(
            &SourceLeafRequest {
                definition: &definition,
                occurrence_id: &id,
                pose,
                analysis: &after,
                actual_used_source_loops: &after_m,
                policy: policy(0.0),
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        assert_eq!(
            before_leaf.proof_kind(),
            NativeSourceLeafProofKind::ClosedSlab
        );
        assert_eq!(
            after_leaf.proof_kind(),
            NativeSourceLeafProofKind::ClosedSlab
        );
        assert_eq!(
            after_leaf
                .sample_loops
                .iter()
                .filter(|cycle| !cycle.hole)
                .count(),
            1
        );
        assert!(after_leaf.sample_loops.iter().any(|cycle| cycle.hole));
        let before_tube = source_boundary_tube(&before_leaf, &[], &mut work, &cancel).unwrap();
        let after_tube = source_boundary_tube(&after_leaf, &[], &mut work, &cancel).unwrap();
        boundary_loop_confinement(&before_leaf, &before_tube, 1, cut, &mut work, &cancel).unwrap();
        boundary_loop_confinement(&after_leaf, &after_tube, 0, cut, &mut work, &cancel).unwrap();
        let occurrences = [NativeUnionOccurrence {
            occurrence_id: id,
            definition: &definition,
            pose,
            evidence: source_evidence(&after, &after_leaf, None),
        }];
        let mut pending =
            super::certify_native_union(&occurrences, &after_m, policy(0.0), &mut work, &cancel)
                .unwrap();
        let lower_target = NativeCriticalCollarTarget {
            leaf: &before_leaf,
            actual_material: &before_m,
        };
        let upper_target = NativeCriticalCollarTarget {
            leaf: &after_leaf,
            actual_material: &after_m,
        };
        let original_bytes = pending.retained_bytes();
        assert_eq!(
            pending
                .certify_regular_interface_pair(
                    NativeCriticalCollarTarget { ..lower_target },
                    NativeCriticalCollarTarget { ..upper_target },
                    original_bytes,
                    &mut work,
                    &cancel,
                )
                .unwrap_err()
                .code,
            GeometryErrorCode::ResourceLimit
        );
        assert!(
            pending
                .certify_regular_interface_pair(
                    NativeCriticalCollarTarget { ..lower_target },
                    NativeCriticalCollarTarget { ..upper_target },
                    MAX_SCRATCH,
                    &mut work,
                    &cancel,
                )
                .unwrap(),
            "complete actual registered buffers issue a sealed pair receipt"
        );
        assert_eq!(pending.regular_interfaces.len(), 1);
        assert!(
            pending.retained_bytes()
                >= original_bytes + std::mem::size_of::<NativeRegularInterfacePairReceipt>()
        );
        certify_regular_material_interface(
            &before_leaf,
            &after_leaf,
            cut,
            &before_tube,
            &after_tube,
            &pending.regular_interfaces,
            &mut Scratch { bytes: 0 },
            &mut work,
            &cancel,
        )
        .unwrap();
        let sealed = pending.regular_interfaces[0];
        let mut wrong_cut = before_leaf.clone();
        wrong_cut.world_height_interval_mm[1] = cut.next_up();
        assert_eq!(
            pending
                .certify_regular_interface_pair(
                    NativeCriticalCollarTarget {
                        leaf: &wrong_cut,
                        actual_material: &before_m
                    },
                    NativeCriticalCollarTarget { ..upper_target },
                    MAX_SCRATCH,
                    &mut work,
                    &cancel,
                )
                .unwrap_err()
                .code,
            GeometryErrorCode::InvalidGeometry
        );
        assert_eq!(pending.regular_interfaces[0], sealed);
        let mut critical_lower = before_leaf.clone();
        critical_lower.critical_interfaces[1] = true;
        assert!(
            !pending
                .certify_regular_interface_pair(
                    NativeCriticalCollarTarget {
                        leaf: &critical_lower,
                        actual_material: &before_m
                    },
                    NativeCriticalCollarTarget { ..upper_target },
                    MAX_SCRATCH,
                    &mut work,
                    &cancel,
                )
                .unwrap(),
            "a critical shared side remains owned by the independent collar route"
        );
        let mut exhausted = GeometryWorkBudget::new(0);
        assert_eq!(
            pending
                .certify_regular_interface_pair(
                    NativeCriticalCollarTarget { ..lower_target },
                    NativeCriticalCollarTarget { ..upper_target },
                    MAX_SCRATCH,
                    &mut exhausted,
                    &cancel,
                )
                .unwrap_err()
                .code,
            GeometryErrorCode::ResourceLimit
        );
        assert_eq!(
            pending
                .certify_regular_interface_pair(
                    NativeCriticalCollarTarget { ..lower_target },
                    NativeCriticalCollarTarget { ..upper_target },
                    MAX_SCRATCH,
                    &mut work,
                    &AtomicBool::new(true),
                )
                .unwrap_err()
                .code,
            GeometryErrorCode::Cancelled
        );
        assert_eq!(
            pending.regular_interfaces.as_slice(),
            &[sealed],
            "critical, stale, exhausted and cancelled attempts grant no additional authority"
        );
        let clearance = after_leaf.sample_loop_clearance_mm.unwrap();
        assert!(clearance > 0.0);

        // Exercise equality/hash binding without adding point storage to the
        // receipt. These deliberately poisoned private metadata values cannot
        // be constructed through the public source-leaf API.
        let mut changed = certify_native_source_leaf(
            &SourceLeafRequest {
                definition: &definition,
                occurrence_id: &id,
                pose,
                analysis: &after,
                actual_used_source_loops: &after_m,
                policy: policy(0.0),
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        let mut foreign_current = changed.clone();
        foreign_current.sample_region_error_mm = foreign_current.sample_region_error_mm.next_up();
        assert_eq!(
            pending
                .certify_regular_interface_pair(
                    NativeCriticalCollarTarget { ..lower_target },
                    NativeCriticalCollarTarget {
                        leaf: &foreign_current,
                        actual_material: &after_m
                    },
                    MAX_SCRATCH,
                    &mut work,
                    &cancel,
                )
                .unwrap_err()
                .code,
            GeometryErrorCode::InvalidGeometry,
            "the current leaf must belong to this pending proof"
        );
        drop(foreign_current);
        let mut foreign = after_m;
        foreign.swap(0, 1);
        assert_eq!(
            verify_source_leaf_candidate(&after_leaf, &foreign, &mut work, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::InvalidGeometry
        );
        assert_eq!(
            pending
                .certify_regular_interface_pair(
                    NativeCriticalCollarTarget {
                        leaf: &before_leaf,
                        actual_material: &before_m
                    },
                    NativeCriticalCollarTarget {
                        leaf: &after_leaf,
                        actual_material: &foreign
                    },
                    MAX_SCRATCH,
                    &mut work,
                    &cancel,
                )
                .unwrap_err()
                .code,
            GeometryErrorCode::InvalidGeometry,
            "an existing sealed receipt cannot be reused for foreign actual geometry"
        );
        foreign.swap(0, 1);
        let hole_index = foreign.iter().position(|cycle| cycle.is_hole).unwrap();
        foreign[hole_index].is_hole = false;
        assert_eq!(
            pending
                .certify_regular_interface_pair(
                    NativeCriticalCollarTarget { ..lower_target },
                    NativeCriticalCollarTarget {
                        leaf: &after_leaf,
                        actual_material: &foreign
                    },
                    MAX_SCRATCH,
                    &mut work,
                    &cancel,
                )
                .unwrap_err()
                .code,
            GeometryErrorCode::InvalidGeometry,
            "a sealed receipt cannot erase an actual protected hole"
        );
        foreign[hole_index].is_hole = true;
        assert_eq!(
            pending
                .certify_regular_interface_pair(
                    NativeCriticalCollarTarget {
                        leaf: &before_leaf,
                        actual_material: &foreign
                    },
                    NativeCriticalCollarTarget {
                        leaf: &after_leaf,
                        actual_material: &foreign
                    },
                    MAX_SCRATCH,
                    &mut work,
                    &cancel,
                )
                .unwrap_err()
                .code,
            GeometryErrorCode::InvalidGeometry,
            "both actual buffers must be independently verified, including the lower one"
        );
        let lower_occurrences = [NativeUnionOccurrence {
            occurrence_id: id,
            definition: &definition,
            pose,
            evidence: source_evidence(&before, &before_leaf, None),
        }];
        let lower_pending = super::certify_native_union(
            &lower_occurrences,
            &before_m,
            policy(0.0),
            &mut work,
            &cancel,
        )
        .unwrap();
        let mut ledger = NativeUnionBindingLedger::new([5.01, 5.05], MAX_SCRATCH).unwrap();
        ledger
            .record(
                lower_pending,
                &before_m,
                &[&before_leaf],
                &mut work,
                &cancel,
            )
            .unwrap();
        ledger
            .record(pending, &foreign, &[&after_leaf], &mut work, &cancel)
            .unwrap();
        let completed = ledger.finalize(&mut work, &cancel).unwrap();
        assert_eq!(completed.world_height_interval_mm(), [5.01, 5.05]);
        assert_eq!(
            completed.slab_count(),
            2,
            "the registered pair and adjacent source leaves retain closed stream binding"
        );
        let hash = |leaf: &NativeSourceLeafReceipt| {
            let mut hash = Sha256::new();
            hash_leaf(&mut hash, leaf);
            hash.finalize()
        };
        assert!(same_leaf(&after_leaf, &changed));
        changed.sample_loop_clearance_mm = Some(clearance * 0.5);
        assert!(!same_leaf(&after_leaf, &changed));
        assert_ne!(hash(&after_leaf), hash(&changed));
        changed.sample_loop_clearance_mm = Some(clearance);
        let original_index = changed.sample_loops[0].registration.native_index;
        Arc::get_mut(&mut changed.sample_loops).unwrap()[0]
            .registration
            .native_index = usize::MAX;
        assert!(!same_leaf(&after_leaf, &changed));
        assert_ne!(hash(&after_leaf), hash(&changed));
        Arc::get_mut(&mut changed.sample_loops).unwrap()[0]
            .registration
            .native_index = original_index;
        let hole = changed
            .sample_loops
            .iter()
            .position(|cycle| cycle.hole)
            .unwrap();
        let original_parent = changed.sample_loops[hole].registration.parent;
        Arc::get_mut(&mut changed.sample_loops).unwrap()[hole]
            .registration
            .parent = None;
        assert!(!same_leaf(&after_leaf, &changed));
        assert_ne!(hash(&after_leaf), hash(&changed));
        Arc::get_mut(&mut changed.sample_loops).unwrap()[hole]
            .registration
            .parent = original_parent;

        // Isolate the captured whole-boundary clearance consumer by discarding
        // alternative radial/kernel witnesses, never by inventing a source fact.
        changed.radial_product = None;
        changed.endpoint_radial_products = [None; 2];
        for cycle in Arc::get_mut(&mut changed.sample_loops).unwrap() {
            cycle.shape.kernel_radius_mm = 0.0;
            cycle.shape.kernel_box = None;
        }
        let moving = NativeBoundaryTube {
            occurrence_id: id,
            stationary: false,
            distance_mm: Some(clearance * 0.25),
        };
        boundary_loop_confinement(&changed, &moving, 0, cut, &mut work, &cancel).unwrap();
        let unavailable = NativeBoundaryTube {
            distance_mm: None,
            ..moving
        };
        assert_eq!(
            boundary_loop_confinement(&changed, &unavailable, 0, cut, &mut work, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::AnalysisIndeterminate,
        );
        let exhausted = NativeBoundaryTube {
            distance_mm: Some(clearance * 0.5),
            ..moving
        };
        assert_eq!(
            boundary_loop_confinement(&changed, &exhausted, 0, cut, &mut work, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::AnalysisIndeterminate,
            "nonpositive remaining whole-boundary clearance supplies no confinement",
        );
    }

    #[test]
    fn same_definition_leaf_with_different_material_recipe_is_not_reusable() {
        let definition = original("box-mm.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(50_000_000);
        let source = crate::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let pose = RigidPoseMm::IDENTITY;
        let analysis = slab(&source, pose, 0, &mut work);
        let leaf = source_leaf(&source, 1, pose, &analysis, &mut work);
        let mut changed = leaf.clone();
        let allowance = if definition.provenance().uncertainty_mm.is_some() {
            None
        } else {
            Some(0.0)
        };
        changed.source.identity = Arc::new(NativeUnionSourceIdentity {
            occurrence_id: leaf.occurrence_id(),
            definition_id: leaf.definition_id().clone(),
            pose,
            material_realization: crate::kernel::MaterialRealizationIdentity::with_allowance(
                allowance,
            ),
        });
        assert!(!same_leaf(&leaf, &changed));
        assert!(!changed.matches_original_material(&definition));
        let mut original_hash = Sha256::new();
        let mut changed_hash = Sha256::new();
        hash_source(&mut original_hash, &leaf.source);
        hash_source(&mut changed_hash, &changed.source);
        assert_ne!(original_hash.finalize(), changed_hash.finalize());
        let occurrences = [NativeUnionOccurrence {
            occurrence_id: leaf.occurrence_id(),
            source: &source,
            pose,
            evidence: NativeUnionOccurrenceEvidence::Slab {
                analysis: &analysis,
                leaf: &changed,
                endpoint_ownerships: &[],
            },
        }];
        let loops = xy_candidate(&analysis, analysis.sample_world_z_mm().unwrap());
        assert_eq!(
            certify_native_union(&occurrences, &loops, policy(0.0), &mut work, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::InvalidGeometry,
        );
    }

    #[test]
    fn exact_native_cycle_rotation_preserves_source_error_and_request_capture() {
        let definition = original("cylinder.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000_000);
        let source = crate::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let pose = RigidPoseMm::IDENTITY;
        let analysis = slab(&source, pose, 0, &mut work);
        let baseline = source_leaf(&source, 1, pose, &analysis, &mut work);
        let z = analysis.sample_world_z_mm().unwrap();
        let mut rotated = xy_candidate(&analysis, z);
        for cycle in &mut rotated {
            cycle.points_mm.pop();
            cycle.points_mm.rotate_left(1);
            cycle.points_mm.push(cycle.points_mm[0]);
        }
        rotated.reverse();
        let receipt = certify_native_source_leaf(
            &SourceLeafRequest {
                source: &source,
                occurrence_id: &OccurrenceId::new(1).unwrap(),
                pose,
                analysis: &analysis,
                actual_used_source_loops: &rotated,
                policy: policy(0.0),
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        assert_eq!(
            receipt.achieved_shape_error_mm,
            baseline.achieved_shape_error_mm
        );
        assert!(receipt.achieved_shape_error_mm > 0.0);
        assert_eq!(
            receipt.sample_region_error_mm,
            baseline.sample_region_error_mm
        );
        assert_eq!(
            receipt.sample_coordinate_error_mm,
            baseline.sample_coordinate_error_mm
        );
        assert_eq!(receipt.native_sample_sha256, baseline.native_sample_sha256);
        let stale_pose = RigidPoseMm {
            translation_mm: [1.0, 0.0, 0.0],
            ..pose
        };
        assert_eq!(
            certify_native_source_leaf(
                &SourceLeafRequest {
                    source: &source,
                    occurrence_id: &OccurrenceId::new(1).unwrap(),
                    pose: stale_pose,
                    analysis: &analysis,
                    actual_used_source_loops: &rotated,
                    policy: policy(0.0),
                },
                &mut work,
                &cancel,
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::InvalidGeometry
        );
    }

    #[test]
    fn incoming_original_caps_close_first_and_last_actual_m_without_convexity() {
        let cancel = AtomicBool::new(false);
        // A non-axis XY pose deliberately cannot use the orthogonal rectangle
        // specialization. Source Z is unchanged, not tolerance-snapped.
        let pose = RigidPoseMm {
            translation_mm: [100.0, 105.0, 0.0],
            rotation_xyzw: [0.0, 0.0, 0.6, 0.8],
        };
        for (name, holes) in [
            ("box-mm.step", 0),
            ("native-parity-bezier-extrusion-bore.step", 1),
            ("native-parity-cone-frustum.step", 0),
        ] {
            let definition = original(name);
            let mut work = GeometryWorkBudget::new(100_000_000);
            let source = crate::prepare_solid(
                &definition,
                crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
                &mut work,
                &cancel,
            )
            .unwrap();
            for (interval, z, endpoint) in [([0.0, 0.02], 0.01, 0), ([7.98, 8.0], 7.99, 1)] {
                let analysis = ranged_slab(&source, pose, interval, z, 0, &mut work);
                assert!(!analysis.has_transition_material(), "{name}");
                assert!(analysis.invariance.is_none(), "{name}");
                let actual = xy_candidate(&analysis, z);
                assert_eq!(actual.iter().filter(|loop_| loop_.is_hole).count(), holes);
                let leaf = source_leaf(&source, 11, pose, &analysis, &mut work);
                assert!(leaf.critical_interfaces[endpoint], "{name}");
                assert_eq!(
                    leaf.native_endpoint_closure,
                    std::array::from_fn(|side| side == endpoint),
                    "{name}"
                );
                let mut missing_incidence = leaf.clone();
                missing_incidence.native_endpoint_closure = [false; 2];
                assert!(!same_leaf(&leaf, &missing_incidence));
                let failure = super::certify_native_union(
                    &[sampled_occurrence(
                        &source,
                        &analysis,
                        &missing_incidence,
                        None,
                    )],
                    &actual,
                    policy(0.0),
                    &mut work,
                    &cancel,
                )
                .unwrap_err();
                assert_eq!(failure.code, GeometryErrorCode::AnalysisIndeterminate);

                let occurrence = sampled_occurrence(&source, &analysis, &leaf, None);
                let pending = super::certify_native_union(
                    std::slice::from_ref(&occurrence),
                    &actual,
                    policy(0.0),
                    &mut work,
                    &cancel,
                )
                .expect("original incoming coface and regular closure bind actual source M");
                assert!(pending.closed_shape_error_mm().is_some());
                assert_eq!(pending.material_component_count(), 1);
                assert_eq!(pending.sources()[0].definition_id(), definition.id());
                assert_eq!(pending.sources()[0].pose(), pose);
                let mut ledger = NativeUnionBindingLedger::new(interval, MAX_SCRATCH).unwrap();
                ledger
                    .record(pending, &actual, &[&leaf], &mut work, &cancel)
                    .unwrap();
                let completed = ledger.finalize(&mut work, &cancel).unwrap();
                assert_eq!(completed.world_height_interval_mm(), interval);
                assert_eq!(completed.slab_count(), 1);
                if holes != 0 {
                    let mut filled = copy_loops(&actual);
                    filled.retain(|loop_| !loop_.is_hole);
                    assert!(
                        super::certify_native_union(
                            std::slice::from_ref(&occurrence),
                            &filled,
                            policy(0.0),
                            &mut work,
                            &cancel,
                        )
                        .is_err(),
                        "incoming annular caps never authorize filling the original bore"
                    );
                }
            }
        }
    }

    #[test]
    fn nonendpoint_birth_and_outgoing_floor_keep_actual_critical_incidence_obligations() {
        let cancel = AtomicBool::new(false);
        for (name, pose, interval, z) in [
            (
                "box-mm.step",
                RigidPoseMm {
                    translation_mm: [100.0, 105.0, 5e-7],
                    ..RigidPoseMm::IDENTITY
                },
                [0.0, 0.02],
                0.01,
            ),
            (
                "native-parity-blind-pocket.step",
                RigidPoseMm::IDENTITY,
                [4.0, 4.02],
                4.01,
            ),
        ] {
            let definition = original(name);
            let mut work = GeometryWorkBudget::new(100_000_000);
            let source = crate::prepare_solid(
                &definition,
                crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
                &mut work,
                &cancel,
            )
            .unwrap();
            let analysis = ranged_slab(&source, pose, interval, z, 0, &mut work);
            assert!(analysis.has_critical_collar(), "{name}");
            let leaf = source_leaf(&source, 13, pose, &analysis, &mut work);
            assert_eq!(leaf.native_endpoint_closure, [false; 2], "{name}");
            let failure = super::certify_native_union(
                &[sampled_occurrence(&source, &analysis, &leaf, None)],
                &xy_candidate(&analysis, z),
                policy(0.0),
                &mut work,
                &cancel,
            )
            .unwrap_err();
            assert_eq!(failure.code, GeometryErrorCode::AnalysisIndeterminate);
        }
    }

    fn collar_for(
        source: &crate::PreparedSolid<'_>,
        analysis: &SlabAnalysis,
        leaf: &NativeSourceLeafReceipt,
        actual: &[SectionLoopMm],
        neighbors: &[NativeCriticalCollarTarget<'_>],
        work: &mut GeometryWorkBudget,
    ) -> Option<NativeCriticalCollarIncidence> {
        if !analysis.has_critical_collar() {
            return None;
        }
        let mut targets = Vec::new();
        if leaf.is_positive() {
            targets.push(NativeCriticalCollarTarget {
                leaf,
                actual_material: actual,
            });
        }
        for target in neighbors {
            targets.push(NativeCriticalCollarTarget {
                leaf: target.leaf,
                actual_material: target.actual_material,
            });
        }
        let cancel = AtomicBool::new(false);
        let metric = source
            .certify_native_critical_collar(
                analysis,
                &targets,
                super::super::collar::NativeCriticalCollarPolicy {
                    maximum_xy_cover_error_mm: 0.05,
                    maximum_distance_mm: leaf.native_shape_error_policy_mm(),
                    byte_limit: MAX_SCRATCH as u32,
                },
                work,
                &cancel,
            )
            .expect(
                "whole original native height-clipped boundary cover by actual positive source-M",
            );
        Some(certify_native_collar_incidence(analysis,leaf,&metric,&targets,work,&cancel)
            .expect("original source coface/coedge/neighbor loop incidence and actual source-M interface graph"))
    }
    fn source_evidence<'a>(
        analysis: &'a SlabAnalysis,
        leaf: &'a NativeSourceLeafReceipt,
        collar: Option<&'a NativeCriticalCollarIncidence>,
    ) -> NativeUnionOccurrenceEvidence<'a> {
        match collar {
            Some(collar) => NativeUnionOccurrenceEvidence::SlabWithCollar {
                analysis,
                leaf,
                endpoint_ownerships: &[],
                collar,
            },
            None => NativeUnionOccurrenceEvidence::Slab {
                analysis,
                leaf,
                endpoint_ownerships: &[],
            },
        }
    }
    // Legacy cap tests below still exercise their real endpoint API. Their
    // ACTIVE endpoint slabs now also require the new actual native coface proof;
    // build it from the SAME frozen source sample, never another section query.
    fn certify_native_union(
        occurrences: &[NativeUnionOccurrence<'_, '_>],
        candidate: &[SectionLoopMm],
        policy: NativeUnionPolicy,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<PendingNativeUnionProof, GeometryError> {
        work.charge(1, cancel)?;
        let mut collars = Vec::new();
        for occurrence in occurrences {
            let collar = match &occurrence.evidence {
                NativeUnionOccurrenceEvidence::Slab {
                    analysis,
                    leaf,
                    endpoint_ownerships,
                } if analysis.has_critical_collar()
                    && endpoint_ownerships.is_empty()
                    && leaf.is_positive() =>
                {
                    collar_for(
                        occurrence.source,
                        analysis,
                        leaf,
                        &xy_candidate(analysis, leaf.sample_world_z_mm()),
                        &[],
                        work,
                    )
                }
                _ => None,
            };
            collars.push(collar);
        }
        let adapted: Vec<_> = occurrences
            .iter()
            .zip(&collars)
            .map(|(original, collar)| NativeUnionOccurrence {
                occurrence_id: original.occurrence_id,
                source: original.source,
                pose: original.pose,
                evidence: match (&original.evidence, collar) {
                    (NativeUnionOccurrenceEvidence::Slab { analysis, leaf, .. }, Some(collar)) => {
                        NativeUnionOccurrenceEvidence::SlabWithCollar {
                            analysis,
                            leaf,
                            endpoint_ownerships: &[],
                            collar,
                        }
                    }
                    (
                        NativeUnionOccurrenceEvidence::Slab {
                            analysis,
                            leaf,
                            endpoint_ownerships,
                        },
                        None,
                    ) => NativeUnionOccurrenceEvidence::Slab {
                        analysis,
                        leaf,
                        endpoint_ownerships,
                    },
                    (
                        NativeUnionOccurrenceEvidence::SlabWithCollar {
                            analysis,
                            leaf,
                            endpoint_ownerships,
                            collar,
                        },
                        _,
                    ) => NativeUnionOccurrenceEvidence::SlabWithCollar {
                        analysis,
                        leaf,
                        endpoint_ownerships,
                        collar,
                    },
                    (NativeUnionOccurrenceEvidence::EndpointOwned(owner), _) => {
                        NativeUnionOccurrenceEvidence::EndpointOwned(owner)
                    }
                },
            })
            .collect();
        super::certify_native_union(&adapted, candidate, policy, work, cancel)
    }

    #[test]
    fn source_closed_receipt_owns_disjoint_box8_cap_while_cylinder12_remains_active() {
        let box_ = original("box-mm.step");
        let cylinder = original("cylinder.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000_000);
        let box_source = crate::prepare_solid(
            &box_,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let cylinder_source = crate::prepare_solid(
            &cylinder,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let box_id = OccurrenceId::new(11).unwrap();
        let cylinder_id = OccurrenceId::new(29).unwrap();
        let cylinder_pose = RigidPoseMm {
            translation_mm: [100.0, 0.0, 4.0],
            ..RigidPoseMm::IDENTITY
        };
        let old_box = ranged_slab(
            &box_source,
            RigidPoseMm::IDENTITY,
            [7.8, 8.0],
            7.9,
            0,
            &mut work,
        );
        let old_cylinder = ranged_slab(
            &cylinder_source,
            cylinder_pose,
            [7.8, 8.0],
            7.9,
            old_box.retained_bytes(),
            &mut work,
        );
        let receipt = source_leaf(&box_source, 11, RigidPoseMm::IDENTITY, &old_box, &mut work);
        let cylinder_leaf = source_leaf(
            &cylinder_source,
            29,
            cylinder_pose,
            &old_cylinder,
            &mut work,
        );
        let mut old_candidate = xy_candidate(&old_box, 7.9);
        old_candidate.extend(xy_candidate(&old_cylinder, 7.9));
        let old_occurrences = [
            NativeUnionOccurrence {
                occurrence_id: box_id,
                source: &box_source,
                pose: RigidPoseMm::IDENTITY,
                evidence: NativeUnionOccurrenceEvidence::Slab {
                    analysis: &old_box,
                    leaf: &receipt,
                    endpoint_ownerships: &[],
                },
            },
            NativeUnionOccurrence {
                occurrence_id: cylinder_id,
                source: &cylinder_source,
                pose: cylinder_pose,
                evidence: NativeUnionOccurrenceEvidence::Slab {
                    analysis: &old_cylinder,
                    leaf: &cylinder_leaf,
                    endpoint_ownerships: &[],
                },
            },
        ];
        let previous = certify_native_union(
            &old_occurrences,
            &old_candidate,
            policy(0.0),
            &mut work,
            &cancel,
        )
        .unwrap();
        let ownership = certify_endpoint_ownership(
            &EndpointOwnershipRequest {
                definition: &box_,
                occurrence_id: &box_id,
                pose: RigidPoseMm::IDENTITY,
                world_height_interval_mm: [8.0, 8.2],
                sample_world_z_mm: 8.1,
                native_shape_error_mm: 0.1,
                adjacent: &receipt,
            },
            &mut work,
            &cancel,
        )
        .expect("independent native support proves BOX has no open current material");
        assert_eq!(ownership.endpoint(), NativeOwnedEndpoint::Lower);
        assert!(ownership.source_world_z_support().unwrap().maximum[1] <= 8.0);
        let retained = old_box.retained_bytes()
            + old_cylinder.retained_bytes()
            + receipt.retained_bytes()
            + cylinder_leaf.retained_bytes()
            + previous.retained_bytes()
            + ownership.retained_bytes()
            + old_candidate.capacity() * std::mem::size_of::<SectionLoopMm>()
            + old_candidate
                .iter()
                .map(|ring| ring.points_mm.capacity() * std::mem::size_of::<[f64; 3]>())
                .sum::<usize>();
        let cylinder_now = ranged_slab(
            &cylinder_source,
            cylinder_pose,
            [8.0, 8.2],
            8.1,
            retained,
            &mut work,
        );
        let current_leaf = source_leaf(
            &cylinder_source,
            29,
            cylinder_pose,
            &cylinder_now,
            &mut work,
        );
        let candidate = xy_candidate(&cylinder_now, 8.1);
        let current = [
            NativeUnionOccurrence {
                occurrence_id: box_id,
                source: &box_source,
                pose: RigidPoseMm::IDENTITY,
                evidence: NativeUnionOccurrenceEvidence::EndpointOwned(&ownership),
            },
            NativeUnionOccurrence {
                occurrence_id: cylinder_id,
                source: &cylinder_source,
                pose: cylinder_pose,
                evidence: NativeUnionOccurrenceEvidence::Slab {
                    analysis: &cylinder_now,
                    leaf: &current_leaf,
                    endpoint_ownerships: &[],
                },
            },
        ];
        let proof =
            certify_native_union(&current, &candidate, policy(0.0), &mut work, &cancel).unwrap();
        assert_eq!(proof.sources().len(), 2);
        assert_eq!(proof.material_component_count(), 1);
        assert_eq!(proof.endpoint_dependencies().len(), 1);
        assert_eq!(
            proof.endpoint_dependencies()[0]
                .adjacent_receipt()
                .candidate_geometry_sha256(),
            receipt.candidate_geometry_sha256()
        );
        assert!(proof.achieved_shape_error_mm() >= receipt.material_core_shape_error_mm().unwrap());
        let mut ledger = NativeUnionBindingLedger::new([7.8, 8.2], MAX_SCRATCH).unwrap();
        ledger
            .record(
                previous,
                &old_candidate,
                &[&receipt, &cylinder_leaf],
                &mut work,
                &cancel,
            )
            .unwrap();
        ledger
            .record(proof, &candidate, &[&current_leaf], &mut work, &cancel)
            .unwrap();
        let completed = ledger.finalize(&mut work, &cancel).unwrap();
        assert_eq!(completed.slab_count(), 2);
        assert_eq!(
            completed.owned_critical_slab_count(),
            1 + usize::from(old_box.has_critical_collar())
        );
        assert_eq!(
            certify_endpoint_ownership(
                &EndpointOwnershipRequest {
                    definition: &box_,
                    occurrence_id: &cylinder_id,
                    pose: RigidPoseMm::IDENTITY,
                    world_height_interval_mm: [8.0, 8.2],
                    sample_world_z_mm: 8.1,
                    native_shape_error_mm: 0.1,
                    adjacent: &receipt,
                },
                &mut work,
                &cancel
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::InvalidGeometry
        );
        assert_eq!(
            certify_endpoint_ownership(
                &EndpointOwnershipRequest {
                    definition: &box_,
                    occurrence_id: &box_id,
                    pose: RigidPoseMm::IDENTITY,
                    world_height_interval_mm: [7.9, 8.1],
                    sample_world_z_mm: 8.0,
                    native_shape_error_mm: 0.1,
                    adjacent: &receipt,
                },
                &mut work,
                &cancel
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::AnalysisIndeterminate
        );
    }

    #[test]
    fn source_future_receipt_owns_upper_birth_without_fabricating_empty_hausdorff_error() {
        let definition = original("box-mm.step");
        let id = OccurrenceId::new(7).unwrap();
        let pose = RigidPoseMm {
            translation_mm: [0.0, 0.0, 8.0],
            ..RigidPoseMm::IDENTITY
        };
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(50_000_000);
        let source = crate::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let successor = ranged_slab(&source, pose, [8.0, 8.2], 8.1, 0, &mut work);
        let successor_candidate = xy_candidate(&successor, 8.1);
        let receipt = source_leaf(&source, 7, pose, &successor, &mut work);
        let occurrence = [NativeUnionOccurrence {
            occurrence_id: id,
            source: &source,
            pose,
            evidence: NativeUnionOccurrenceEvidence::Slab {
                analysis: &successor,
                leaf: &receipt,
                endpoint_ownerships: &[],
            },
        }];
        let future = certify_native_union(
            &occurrence,
            &successor_candidate,
            policy(0.0),
            &mut work,
            &cancel,
        )
        .unwrap();
        let endpoint = certify_endpoint_ownership(
            &EndpointOwnershipRequest {
                definition: &definition,
                occurrence_id: &id,
                pose,
                world_height_interval_mm: [7.8, 8.0],
                sample_world_z_mm: 7.9,
                native_shape_error_mm: 0.1,
                adjacent: &receipt,
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        assert_eq!(endpoint.endpoint(), NativeOwnedEndpoint::Upper);
        assert!(endpoint.source_world_z_support().unwrap().minimum[0] >= 8.0);
        let current = [NativeUnionOccurrence {
            occurrence_id: id,
            source: &source,
            pose,
            evidence: NativeUnionOccurrenceEvidence::EndpointOwned(&endpoint),
        }];
        let proof = certify_native_union(&current, &[], policy(0.0), &mut work, &cancel).unwrap();
        assert_eq!(proof.material_component_count(), 0);
        assert_eq!(
            proof.endpoint_dependencies()[0]
                .adjacent_receipt()
                .candidate_geometry_sha256(),
            receipt.candidate_geometry_sha256()
        );
        assert_eq!(
            proof.achieved_shape_error_mm(),
            receipt.material_core_shape_error_mm().unwrap()
        );
        let mut unresolved = NativeUnionBindingLedger::new([7.8, 8.0], MAX_SCRATCH).unwrap();
        unresolved
            .record(proof, &[], &[], &mut work, &cancel)
            .unwrap();
        assert_eq!(
            unresolved.finalize(&mut work, &cancel).unwrap_err().code,
            GeometryErrorCode::AnalysisIndeterminate
        );
        let proof = certify_native_union(&current, &[], policy(0.0), &mut work, &cancel).unwrap();
        let mut ledger = NativeUnionBindingLedger::new([7.8, 8.2], MAX_SCRATCH).unwrap();
        ledger.record(proof, &[], &[], &mut work, &cancel).unwrap();
        ledger
            .record(
                future,
                &successor_candidate,
                &[&receipt],
                &mut work,
                &cancel,
            )
            .unwrap();
        assert_eq!(
            ledger
                .finalize(&mut work, &cancel)
                .unwrap()
                .owned_critical_slab_count(),
            1 + usize::from(successor.has_critical_collar())
        );
    }

    #[test]
    fn actual_native_partial_cantilever_interfaces_resolve_birth_and_death_without_receipt_cycles()
    {
        let definition = original("box-mm.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(200_000_000);
        let source = crate::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let a_id = OccurrenceId::new(41).unwrap();
        let b_id = OccurrenceId::new(97).unwrap();
        let b_pose = RigidPoseMm {
            translation_mm: [10.0, 0.0, 8.0],
            ..RigidPoseMm::IDENTITY
        };
        let a = ranged_slab(
            &source,
            RigidPoseMm::IDENTITY,
            [7.8, 8.0],
            7.9,
            0,
            &mut work,
        );
        let b = ranged_slab(
            &source,
            b_pose,
            [8.0, 8.2],
            8.1,
            a.retained_bytes(),
            &mut work,
        );
        let a_candidate = xy_candidate(&a, 7.9);
        let b_candidate = xy_candidate(&b, 8.1);
        let a_leaf = source_leaf(&source, 41, RigidPoseMm::IDENTITY, &a, &mut work);
        let b_leaf = source_leaf(&source, 97, b_pose, &b, &mut work);
        assert!(
            a_leaf.native_rectangle_source_faces().is_some()
                && b_leaf.native_rectangle_source_faces().is_some()
        );
        let upper_birth = certify_endpoint_ownership(
            &EndpointOwnershipRequest {
                definition: &definition,
                occurrence_id: &b_id,
                pose: b_pose,
                world_height_interval_mm: [7.8, 8.0],
                sample_world_z_mm: 7.9,
                native_shape_error_mm: 0.1,
                adjacent: &b_leaf,
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        let lower_death = certify_endpoint_ownership(
            &EndpointOwnershipRequest {
                definition: &definition,
                occurrence_id: &a_id,
                pose: RigidPoseMm::IDENTITY,
                world_height_interval_mm: [8.0, 8.2],
                sample_world_z_mm: 8.1,
                native_shape_error_mm: 0.1,
                adjacent: &a_leaf,
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        let before = [
            NativeUnionOccurrence {
                occurrence_id: a_id,
                source: &source,
                pose: RigidPoseMm::IDENTITY,
                evidence: NativeUnionOccurrenceEvidence::Slab {
                    analysis: &a,
                    leaf: &a_leaf,
                    endpoint_ownerships: &[],
                },
            },
            NativeUnionOccurrence {
                occurrence_id: b_id,
                source: &source,
                pose: b_pose,
                evidence: NativeUnionOccurrenceEvidence::EndpointOwned(&upper_birth),
            },
        ];
        let after = [
            NativeUnionOccurrence {
                occurrence_id: a_id,
                source: &source,
                pose: RigidPoseMm::IDENTITY,
                evidence: NativeUnionOccurrenceEvidence::EndpointOwned(&lower_death),
            },
            NativeUnionOccurrence {
                occurrence_id: b_id,
                source: &source,
                pose: b_pose,
                evidence: NativeUnionOccurrenceEvidence::Slab {
                    analysis: &b,
                    leaf: &b_leaf,
                    endpoint_ownerships: &[],
                },
            },
        ];
        let first =
            certify_native_union(&before, &a_candidate, policy(0.0), &mut work, &cancel).unwrap();
        let second =
            certify_native_union(&after, &b_candidate, policy(0.0), &mut work, &cancel).unwrap();
        for proof in [&first, &second] {
            assert_eq!(proof.interface_attachments().len(), 1);
            assert!(
                proof.interface_attachments()[0]
                    .native_cell_incidence()
                    .contains(&3)
            );
            assert!(proof.achieved_shape_error_mm() <= 0.1);
        }
        let mut ledger = NativeUnionBindingLedger::new([7.8, 8.2], MAX_SCRATCH).unwrap();
        ledger
            .record(first, &a_candidate, &[&a_leaf], &mut work, &cancel)
            .unwrap();
        ledger
            .record(second, &b_candidate, &[&b_leaf], &mut work, &cancel)
            .unwrap();
        let proof = ledger.finalize(&mut work, &cancel).unwrap();
        assert_eq!(proof.slab_count(), 2);
        assert_eq!(
            proof.owned_critical_slab_count(),
            2 + usize::from(a.has_critical_collar()) + usize::from(b.has_critical_collar())
        );
        assert_eq!(proof.sources()[0].occurrence_id(), a_id);
        assert_eq!(proof.sources()[1].occurrence_id(), b_id);
    }

    #[test]
    fn tiny_native_cantilever_attachment_is_not_promoted_from_polygon_overlap() {
        let definition = original("box-mm.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000_000);
        let source = crate::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let id = OccurrenceId::new(1).unwrap();
        let a = ranged_slab(
            &source,
            RigidPoseMm::IDENTITY,
            [7.8, 8.0],
            7.9,
            0,
            &mut work,
        );
        let a_leaf = source_leaf(&source, 1, RigidPoseMm::IDENTITY, &a, &mut work);
        let contact = certify_endpoint_ownership(
            &EndpointOwnershipRequest {
                definition: &definition,
                occurrence_id: &id,
                pose: RigidPoseMm::IDENTITY,
                world_height_interval_mm: [8.0, 8.2],
                sample_world_z_mm: 8.1,
                native_shape_error_mm: 0.1,
                adjacent: &a_leaf,
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        let pose = RigidPoseMm {
            translation_mm: [20.0 - 1.0 / 1024.0, 0.0, 8.0],
            ..RigidPoseMm::IDENTITY
        };
        let b = ranged_slab(
            &source,
            pose,
            [8.0, 8.2],
            8.1,
            a.retained_bytes() + a_leaf.retained_bytes() + contact.retained_bytes(),
            &mut work,
        );
        let b_leaf = source_leaf(&source, 2, pose, &b, &mut work);
        let inputs = [
            NativeUnionOccurrence {
                occurrence_id: id,
                source: &source,
                pose: RigidPoseMm::IDENTITY,
                evidence: NativeUnionOccurrenceEvidence::EndpointOwned(&contact),
            },
            NativeUnionOccurrence {
                occurrence_id: OccurrenceId::new(2).unwrap(),
                source: &source,
                pose,
                evidence: NativeUnionOccurrenceEvidence::Slab {
                    analysis: &b,
                    leaf: &b_leaf,
                    endpoint_ownerships: &[],
                },
            },
        ];
        let failure = certify_native_union(
            &inputs,
            &xy_candidate(&b, 8.1),
            policy(0.0),
            &mut work,
            &cancel,
        )
        .unwrap_err();
        assert_eq!(failure.code, GeometryErrorCode::AnalysisIndeterminate);
        assert!(failure.message.contains("occurrence pair 1/2"));
    }

    #[test]
    fn exact_singleton_material_identity_cannot_borrow_a_looser_coordinate_policy() {
        let definition = original("box-mm.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(10_000_000);
        let source = crate::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let analysis = slab(&source, RigidPoseMm::IDENTITY, 0, &mut work);
        let mut actual = xy_candidate(&analysis, 4.0);
        for ring in &mut actual {
            for point in &mut ring.points_mm {
                point[0] += 0.005;
            }
        }
        let id = OccurrenceId::new(1).unwrap();
        let leaf = certify_native_source_leaf(
            &SourceLeafRequest {
                source: &source,
                occurrence_id: &id,
                pose: RigidPoseMm::IDENTITY,
                analysis: &analysis,
                actual_used_source_loops: &actual,
                policy: policy(0.01),
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        let occurrences = [NativeUnionOccurrence {
            occurrence_id: id,
            source: &source,
            pose: RigidPoseMm::IDENTITY,
            evidence: NativeUnionOccurrenceEvidence::Slab {
                analysis: &analysis,
                leaf: &leaf,
                endpoint_ownerships: &[],
            },
        }];
        let accepted =
            super::certify_native_union(&occurrences, &actual, policy(0.01), &mut work, &cancel)
                .unwrap();
        let mut ledger = NativeUnionBindingLedger::new([3.99, 4.01], MAX_SCRATCH).unwrap();
        ledger
            .record(accepted, &actual, &[&leaf], &mut work, &cancel)
            .unwrap();
        assert_eq!(ledger.finalize(&mut work, &cancel).unwrap().slab_count(), 1);
        assert_eq!(
            super::certify_native_union(&occurrences, &actual, policy(0.0), &mut work, &cancel,)
                .unwrap_err()
                .code,
            GeometryErrorCode::ShapeErrorExceeded,
            "identical actual M does not relax the new source-correspondence policy",
        );
    }

    #[test]
    fn final_binding_rejects_changed_final_geometry_stale_source_leaf_and_bounded_capacity() {
        let definition = original("box-mm.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000_000);
        let source = crate::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let a = slab(&source, RigidPoseMm::IDENTITY, 0, &mut work);
        let candidate = xy_candidate(&a, 4.0);
        let leaf = source_leaf(&source, 1, RigidPoseMm::IDENTITY, &a, &mut work);
        let changed_slab = ranged_slab(
            &source,
            RigidPoseMm::IDENTITY,
            [3.99, 4.01],
            4.005,
            a.retained_bytes()
                + leaf.retained_bytes()
                + candidate.capacity() * std::mem::size_of::<SectionLoopMm>()
                + candidate
                    .iter()
                    .map(|ring| ring.points_mm.capacity() * std::mem::size_of::<[f64; 3]>())
                    .sum::<usize>(),
            &mut work,
        );
        let stale = source_leaf(&source, 1, RigidPoseMm::IDENTITY, &changed_slab, &mut work);
        let occurrences = [NativeUnionOccurrence {
            occurrence_id: OccurrenceId::new(1).unwrap(),
            source: &source,
            pose: RigidPoseMm::IDENTITY,
            evidence: NativeUnionOccurrenceEvidence::Slab {
                analysis: &a,
                leaf: &leaf,
                endpoint_ownerships: &[],
            },
        }];
        let make = |work: &mut GeometryWorkBudget| {
            certify_native_union(&occurrences, &candidate, policy(0.0), work, &cancel).unwrap()
        };
        let mut changed = copy_loops(&candidate);
        changed[0].points_mm[0][0] += 0.0001;
        let last = changed[0].points_mm.len() - 1;
        let first = changed[0].points_mm[0];
        changed[0].points_mm[last] = first;
        let mut ledger = NativeUnionBindingLedger::new([3.99, 4.01], MAX_SCRATCH).unwrap();
        assert_eq!(
            ledger
                .record(make(&mut work), &changed, &[&leaf], &mut work, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::InvalidGeometry
        );
        assert_eq!(
            ledger
                .record(make(&mut work), &candidate, &[&stale], &mut work, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::InvalidGeometry
        );
        let mut bounded = NativeUnionBindingLedger::new(
            [3.99, 4.01],
            std::mem::size_of::<NativeUnionBindingLedger>() + 1,
        )
        .unwrap();
        assert_eq!(
            bounded
                .record(make(&mut work), &candidate, &[&leaf], &mut work, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::ResourceLimit
        );
        ledger
            .record(make(&mut work), &candidate, &[&leaf], &mut work, &cancel)
            .unwrap();
        assert_eq!(ledger.finalize(&mut work, &cancel).unwrap().slab_count(), 1);
        let incomplete = NativeUnionBindingLedger::new([3.99, 4.01], MAX_SCRATCH).unwrap();
        assert_eq!(
            incomplete.finalize(&mut work, &cancel).unwrap_err().code,
            GeometryErrorCode::AnalysisIndeterminate
        );
        let cancelled = NativeUnionBindingLedger::new([3.99, 4.01], MAX_SCRATCH).unwrap();
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(
            cancelled.finalize(&mut work, &cancel).unwrap_err().code,
            GeometryErrorCode::Cancelled
        );
    }

    #[test]
    fn refining_a_future_request_invalidates_its_old_dependency_even_for_identical_xy_geometry() {
        let definition = original("box-mm.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000_000);
        let source = crate::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let id = OccurrenceId::new(7).unwrap();
        let pose = RigidPoseMm {
            translation_mm: [0.0, 0.0, 8.0],
            ..RigidPoseMm::IDENTITY
        };
        let old = ranged_slab(&source, pose, [8.0, 8.2], 8.1, 0, &mut work);
        let old_leaf = source_leaf(&source, 7, pose, &old, &mut work);
        let old_owner = certify_endpoint_ownership(
            &EndpointOwnershipRequest {
                definition: &definition,
                occurrence_id: &id,
                pose,
                world_height_interval_mm: [7.8, 8.0],
                sample_world_z_mm: 7.9,
                native_shape_error_mm: 0.1,
                adjacent: &old_leaf,
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        let old_evidence = [NativeUnionOccurrence {
            occurrence_id: id,
            source: &source,
            pose,
            evidence: NativeUnionOccurrenceEvidence::EndpointOwned(&old_owner),
        }];
        let first =
            certify_native_union(&old_evidence, &[], policy(0.0), &mut work, &cancel).unwrap();
        let refined = ranged_slab(
            &source,
            pose,
            [8.0, 8.1],
            8.05,
            old.retained_bytes()
                + old_leaf.retained_bytes()
                + old_owner.retained_bytes()
                + first.retained_bytes(),
            &mut work,
        );
        let refined_leaf = source_leaf(&source, 7, pose, &refined, &mut work);
        let refined_m = xy_candidate(&refined, 8.05);
        let refined_evidence = [NativeUnionOccurrence {
            occurrence_id: id,
            source: &source,
            pose,
            evidence: NativeUnionOccurrenceEvidence::Slab {
                analysis: &refined,
                leaf: &refined_leaf,
                endpoint_ownerships: &[],
            },
        }];
        let successor = certify_native_union(
            &refined_evidence,
            &refined_m,
            policy(0.0),
            &mut work,
            &cancel,
        )
        .unwrap();
        let mut stale = NativeUnionBindingLedger::new([7.8, 8.2], MAX_SCRATCH).unwrap();
        stale.record(first, &[], &[], &mut work, &cancel).unwrap();
        assert_eq!(
            stale
                .record(successor, &refined_m, &[&refined_leaf], &mut work, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::InvalidGeometry
        );
        let owner = certify_endpoint_ownership(
            &EndpointOwnershipRequest {
                definition: &definition,
                occurrence_id: &id,
                pose,
                world_height_interval_mm: [7.8, 8.0],
                sample_world_z_mm: 7.9,
                native_shape_error_mm: 0.1,
                adjacent: &refined_leaf,
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        let evidence = [NativeUnionOccurrence {
            occurrence_id: id,
            source: &source,
            pose,
            evidence: NativeUnionOccurrenceEvidence::EndpointOwned(&owner),
        }];
        let first = certify_native_union(&evidence, &[], policy(0.0), &mut work, &cancel).unwrap();
        let successor = certify_native_union(
            &refined_evidence,
            &refined_m,
            policy(0.0),
            &mut work,
            &cancel,
        )
        .unwrap();
        let mut rebound = NativeUnionBindingLedger::new([7.8, 8.1], MAX_SCRATCH).unwrap();
        rebound.record(first, &[], &[], &mut work, &cancel).unwrap();
        rebound
            .record(successor, &refined_m, &[&refined_leaf], &mut work, &cancel)
            .unwrap();
        assert_eq!(
            rebound.finalize(&mut work, &cancel).unwrap().slab_count(),
            2
        );
    }

    #[test]
    fn genuine_closed_empty_native_source_leaf_is_not_a_positive_dependency() {
        let definition = original("box-mm.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000_000);
        let source = crate::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let pose = RigidPoseMm {
            translation_mm: [0.0, 0.0, 20.0],
            ..RigidPoseMm::IDENTITY
        };
        let empty = slab(&source, pose, 0, &mut work);
        let leaf = source_leaf(&source, 1, pose, &empty, &mut work);
        assert!(!leaf.is_positive());
        let inputs = [NativeUnionOccurrence {
            occurrence_id: OccurrenceId::new(1).unwrap(),
            source: &source,
            pose,
            evidence: NativeUnionOccurrenceEvidence::Slab {
                analysis: &empty,
                leaf: &leaf,
                endpoint_ownerships: &[],
            },
        }];
        let pending = certify_native_union(&inputs, &[], policy(0.0), &mut work, &cancel).unwrap();
        assert!(pending.positive_source_leaves().is_empty());
        let mut ledger = NativeUnionBindingLedger::new([3.99, 4.01], MAX_SCRATCH).unwrap();
        ledger
            .record(pending, &[], &[], &mut work, &cancel)
            .unwrap();
        assert_eq!(ledger.finalize(&mut work, &cancel).unwrap().slab_count(), 1);
    }

    #[test]
    fn sewn_original_planar_and_rational_disk_floors_share_real_core_stratum_binding() {
        for name in [
            "native-parity-blind-pocket.step",
            "native-parity-blind-pocket-nurbs-floor.step",
        ] {
            let definition = original(name);
            let cancel = AtomicBool::new(false);
            let mut work = GeometryWorkBudget::new(200_000_000);
            let source = crate::prepare_solid(
                &definition,
                crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
                &mut work,
                &cancel,
            )
            .unwrap();
            if name.contains("nurbs") {
                assert!(definition.face_ids().iter().any(|&face| {
                    definition.face_surface(face).is_some_and(|surface| {
                        crate::parametric::native_surface_nurbs(surface)
                            .unwrap()
                            .is_some_and(|coefficients| coefficients.is_rational())
                    })
                }));
            }
            let id = OccurrenceId::new(83).unwrap();
            let below = ranged_slab(
                &source,
                RigidPoseMm::IDENTITY,
                [3.8, 4.0],
                3.9,
                0,
                &mut work,
            );
            let above = ranged_slab(
                &source,
                RigidPoseMm::IDENTITY,
                [4.0, 4.2],
                4.1,
                below.retained_bytes(),
                &mut work,
            );
            let below_m = xy_candidate(&below, 3.9);
            let above_m = xy_candidate(&above, 4.1);
            assert_eq!(below_m.iter().filter(|loop_| loop_.is_hole).count(), 0);
            assert_eq!(above_m.iter().filter(|loop_| loop_.is_hole).count(), 1);
            let below_leaf = source_leaf(&source, 83, RigidPoseMm::IDENTITY, &below, &mut work);
            let above_leaf = source_leaf(&source, 83, RigidPoseMm::IDENTITY, &above, &mut work);
            assert!(above_leaf.closed_shape_error_mm().is_none());
            let below_collar = collar_for(
                &source,
                &below,
                &below_leaf,
                &below_m,
                &[NativeCriticalCollarTarget {
                    leaf: &above_leaf,
                    actual_material: &above_m,
                }],
                &mut work,
            );
            let above_collar = collar_for(
                &source,
                &above,
                &above_leaf,
                &above_m,
                &[NativeCriticalCollarTarget {
                    leaf: &below_leaf,
                    actual_material: &below_m,
                }],
                &mut work,
            );
            let first = [NativeUnionOccurrence {
                occurrence_id: id,
                source: &source,
                pose: RigidPoseMm::IDENTITY,
                evidence: source_evidence(&below, &below_leaf, below_collar.as_ref()),
            }];
            let second = [NativeUnionOccurrence {
                occurrence_id: id,
                source: &source,
                pose: RigidPoseMm::IDENTITY,
                evidence: source_evidence(&above, &above_leaf, above_collar.as_ref()),
            }];
            let before =
                certify_native_union(&first, &below_m, policy(0.0), &mut work, &cancel).unwrap();
            let after =
                certify_native_union(&second, &above_m, policy(0.0), &mut work, &cancel).unwrap();
            assert_eq!(after.owned_critical_slab_count(), 1);
            assert_eq!(
                after.coverage(),
                NativeUnionCoverage::NativeInterfaceComplex
            );
            let mut ledger = NativeUnionBindingLedger::new([3.8, 4.2], MAX_SCRATCH).unwrap();
            ledger
                .record(before, &below_m, &[&below_leaf], &mut work, &cancel)
                .unwrap();
            ledger
                .record(after, &above_m, &[&above_leaf], &mut work, &cancel)
                .unwrap();
            let expected =
                usize::from(below.has_critical_collar()) + usize::from(above.has_critical_collar());
            assert_eq!(
                ledger
                    .finalize(&mut work, &cancel)
                    .unwrap()
                    .owned_critical_slab_count(),
                expected
            );
        }
    }

    #[test]
    fn original_occt_pocket_floor_binds_active_ring_core_and_its_own_native_face_stratum() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(200_000_000);
        let definition = crate::from_step_bytes_with_cancel(
            include_bytes!(
                "../../../../fixtures/geometry/independent-exporters/occt-narrow-pocket-mm.step"
            ),
            &cancel,
        )
        .unwrap();
        let source = crate::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let id = OccurrenceId::new(63).unwrap();
        let below = ranged_slab(
            &source,
            RigidPoseMm::IDENTITY,
            [1.05, 1.07],
            1.06,
            0,
            &mut work,
        );
        let above = ranged_slab(
            &source,
            RigidPoseMm::IDENTITY,
            [1.07, 1.09],
            1.08,
            below.retained_bytes(),
            &mut work,
        );
        let below_m = xy_candidate(&below, 1.06);
        let above_m = xy_candidate(&above, 1.08);
        assert_eq!(below_m.iter().filter(|loop_| loop_.is_hole).count(), 0);
        assert_eq!(above_m.iter().filter(|loop_| loop_.is_hole).count(), 1);
        assert!(above.closed_shape_error_mm().is_none());
        assert!(!above.endpoint_strata().is_empty());
        let below_leaf = source_leaf(&source, 63, RigidPoseMm::IDENTITY, &below, &mut work);
        let above_leaf = source_leaf(&source, 63, RigidPoseMm::IDENTITY, &above, &mut work);
        assert_eq!(
            below_leaf.proof_kind(),
            if below.has_critical_collar() {
                NativeSourceLeafProofKind::SampleToClosedSource
            } else {
                NativeSourceLeafProofKind::ClosedSlab
            }
        );
        assert!(above_leaf.closed_shape_error_mm().is_none());
        for stratum in above.endpoint_strata() {
            assert_eq!(stratum.endpoint(), NativeOwnedEndpoint::Lower);
            assert_eq!(stratum.world_height_mm(), Some(1.07));
            assert!(
                definition
                    .faces()
                    .iter()
                    .any(|face| &face.face_id == stratum.face_id())
            );
            assert_eq!(
                certify_endpoint_stratum_ownership(stratum, &above_leaf, &mut work, &cancel)
                    .unwrap_err()
                    .code,
                GeometryErrorCode::AnalysisIndeterminate
            );
        }
        let below_collar = collar_for(
            &source,
            &below,
            &below_leaf,
            &below_m,
            &[NativeCriticalCollarTarget {
                leaf: &above_leaf,
                actual_material: &above_m,
            }],
            &mut work,
        );
        let above_collar = collar_for(
            &source,
            &above,
            &above_leaf,
            &above_m,
            &[NativeCriticalCollarTarget {
                leaf: &below_leaf,
                actual_material: &below_m,
            }],
            &mut work,
        );
        let previous = [NativeUnionOccurrence {
            occurrence_id: id,
            source: &source,
            pose: RigidPoseMm::IDENTITY,
            evidence: source_evidence(&below, &below_leaf, below_collar.as_ref()),
        }];
        let missing = [NativeUnionOccurrence {
            occurrence_id: id,
            source: &source,
            pose: RigidPoseMm::IDENTITY,
            evidence: NativeUnionOccurrenceEvidence::Slab {
                analysis: &above,
                leaf: &above_leaf,
                endpoint_ownerships: &[],
            },
        }];
        assert_eq!(
            super::certify_native_union(&missing, &above_m, policy(0.0), &mut work, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::AnalysisIndeterminate
        );
        let current = [NativeUnionOccurrence {
            occurrence_id: id,
            source: &source,
            pose: RigidPoseMm::IDENTITY,
            evidence: source_evidence(&above, &above_leaf, above_collar.as_ref()),
        }];
        let before =
            certify_native_union(&previous, &below_m, policy(0.0), &mut work, &cancel).unwrap();
        let after =
            certify_native_union(&current, &above_m, policy(0.0), &mut work, &cancel).unwrap();
        assert_eq!(
            after.coverage(),
            NativeUnionCoverage::NativeInterfaceComplex
        );
        assert_eq!(after.owned_critical_slab_count(), 1);
        assert_eq!(after.critical_collars().len(), 1);
        assert!(after.endpoint_dependencies().is_empty());
        assert!(after.closed_shape_error_mm().is_none());
        let mut ledger = NativeUnionBindingLedger::new([1.05, 1.09], MAX_SCRATCH).unwrap();
        ledger
            .record(before, &below_m, &[&below_leaf], &mut work, &cancel)
            .unwrap();
        ledger
            .record(after, &above_m, &[&above_leaf], &mut work, &cancel)
            .unwrap();
        let completed = ledger.finalize(&mut work, &cancel).unwrap();
        let expected =
            usize::from(below.has_critical_collar()) + usize::from(above.has_critical_collar());
        assert_eq!(completed.slab_count(), 2);
        assert_eq!(completed.owned_critical_slab_count(), expected);
        assert_eq!(completed.sources().len(), 1);
        assert_eq!(completed.sources()[0].definition_id(), definition.id());
        assert!(completed.achieved_shape_error_mm() <= 0.1);
    }

    #[test]
    fn endpoint_support_must_exclude_interior_and_cannot_ignore_unproved_contact_incidence() {
        let definition = original("box-mm.step");
        let id = OccurrenceId::new(1).unwrap();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000_000);
        let source = crate::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let previous = ranged_slab(
            &source,
            RigidPoseMm::IDENTITY,
            [7.3, 7.5],
            7.4,
            0,
            &mut work,
        );
        let cert = source_leaf(&source, 1, RigidPoseMm::IDENTITY, &previous, &mut work);
        assert_eq!(
            certify_endpoint_ownership(
                &EndpointOwnershipRequest {
                    definition: &definition,
                    occurrence_id: &id,
                    pose: RigidPoseMm::IDENTITY,
                    world_height_interval_mm: [7.5, 7.7],
                    sample_world_z_mm: 7.6,
                    native_shape_error_mm: 0.1,
                    adjacent: &cert,
                },
                &mut work,
                &cancel
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::AnalysisIndeterminate
        );
        drop((previous, cert));
        let previous_slab = ranged_slab(
            &source,
            RigidPoseMm::IDENTITY,
            [7.8, 8.0],
            7.9,
            0,
            &mut work,
        );
        let previous = source_leaf(&source, 1, RigidPoseMm::IDENTITY, &previous_slab, &mut work);
        let endpoint = certify_endpoint_ownership(
            &EndpointOwnershipRequest {
                definition: &definition,
                occurrence_id: &id,
                pose: RigidPoseMm::IDENTITY,
                world_height_interval_mm: [8.0, 8.2],
                sample_world_z_mm: 8.1,
                native_shape_error_mm: 0.1,
                adjacent: &previous,
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        let corner_pose = RigidPoseMm {
            translation_mm: [20.0, 10.0, 8.0],
            ..RigidPoseMm::IDENTITY
        };
        let corner = ranged_slab(
            &source,
            corner_pose,
            [8.0, 8.2],
            8.1,
            previous_slab.retained_bytes() + previous.retained_bytes() + endpoint.retained_bytes(),
            &mut work,
        );
        let corner_leaf = source_leaf(&source, 2, corner_pose, &corner, &mut work);
        let current = [
            NativeUnionOccurrence {
                occurrence_id: id,
                source: &source,
                pose: RigidPoseMm::IDENTITY,
                evidence: NativeUnionOccurrenceEvidence::EndpointOwned(&endpoint),
            },
            NativeUnionOccurrence {
                occurrence_id: OccurrenceId::new(2).unwrap(),
                source: &source,
                pose: corner_pose,
                evidence: NativeUnionOccurrenceEvidence::Slab {
                    analysis: &corner,
                    leaf: &corner_leaf,
                    endpoint_ownerships: &[],
                },
            },
        ];
        let failure = certify_native_union(
            &current,
            &xy_candidate(&corner, 8.1),
            policy(0.0),
            &mut work,
            &cancel,
        )
        .unwrap_err();
        assert_eq!(failure.code, GeometryErrorCode::AnalysisIndeterminate);
        assert!(failure.message.contains("occurrence pair 1/2"));
    }

    fn sampled_occurrence<'s, 'd>(
        source: &'s crate::PreparedSolid<'d>,
        analysis: &'s SlabAnalysis,
        leaf: &'s NativeSourceLeafReceipt,
        collar: Option<&'s NativeCriticalCollarIncidence>,
    ) -> NativeUnionOccurrence<'s, 'd> {
        NativeUnionOccurrence {
            occurrence_id: leaf.occurrence_id(),
            source,
            pose: leaf.pose(),
            evidence: source_evidence(analysis, leaf, collar),
        }
    }

    #[test]
    fn exact_native_support_residuals_are_not_epsilon_coincidence() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1000);
        let exact = native_sum(1.0, 0.0).unwrap();
        let distinct = native_sum(1.0, 2.0_f64.powi(-54)).unwrap();
        assert_eq!(exact[0], distinct[0]);
        assert_ne!(exact[1], distinct[1]);
        let mut collapsed = vec![exact, distinct];
        assert_eq!(
            sort_native_supports(&mut collapsed, [3, 7], &mut work, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::AnalysisIndeterminate
        );
        let mut identical = vec![distinct, distinct, native_sum(2.0, 0.0).unwrap()];
        sort_native_supports(&mut identical, [3, 7], &mut work, &cancel).unwrap();
        assert_eq!(identical.len(), 2);
        assert!(
            native_support_interval(distinct).lo <= 1.0
                && native_support_interval(distinct).hi > 1.0
        );
    }

    #[test]
    fn original_non_grid_translated_boxes_prove_true_dyadic_support_union() {
        let definition = original("box-mm.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(200_000_000);
        let source = crate::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let dx = 10.123456789;
        let dy = 0.234567891;
        let pose = RigidPoseMm {
            translation_mm: [dx, dy, 0.0],
            ..RigidPoseMm::IDENTITY
        };
        let mut ledger = NativeUnionBindingLedger::new([3.98, 4.02], MAX_SCRATCH).unwrap();
        for (interval, z) in [([3.98, 4.0], 3.99), ([4.0, 4.02], 4.01)] {
            let a = ranged_slab(
                &source,
                RigidPoseMm::IDENTITY,
                interval,
                z,
                ledger.retained_bytes(),
                &mut work,
            );
            let b = ranged_slab(
                &source,
                pose,
                interval,
                z,
                ledger.retained_bytes() + a.retained_bytes(),
                &mut work,
            );
            let a_leaf = source_leaf(&source, 1, RigidPoseMm::IDENTITY, &a, &mut work);
            let b_leaf = source_leaf(&source, 2, pose, &b, &mut work);
            assert!(b_leaf.rectangle.as_ref().unwrap().coordinate_error_mm > 0.0);
            let mut actual = candidate(
                &[
                    [0.0, 0.0],
                    [20.0, 0.0],
                    [20.0, dy],
                    [20.0 + dx, dy],
                    [20.0 + dx, 10.0 + dy],
                    [dx, 10.0 + dy],
                    [dx, 10.0],
                    [0.0, 10.0],
                ],
                false,
            );
            for point in &mut actual.points_mm {
                point[2] = z;
            }
            let actual = [actual];
            let occurrences = [
                sampled_occurrence(&source, &a, &a_leaf, None),
                sampled_occurrence(&source, &b, &b_leaf, None),
            ];
            let pending =
                super::certify_native_union(&occurrences, &actual, policy(0.0), &mut work, &cancel)
                    .unwrap();
            assert_eq!(pending.coverage(), NativeUnionCoverage::InvariantUnion);
            assert!(
                pending.achieved_shape_error_mm() > 0.0 && pending.achieved_shape_error_mm() < 0.1
            );
            for source in pending.boundary_sources() {
                assert!(
                    definition
                        .faces()
                        .iter()
                        .any(|face| face.face_id == source.face_id)
                );
            }
            ledger
                .record(pending, &actual, &[&a_leaf, &b_leaf], &mut work, &cancel)
                .unwrap();
        }
        assert_eq!(ledger.finalize(&mut work, &cancel).unwrap().slab_count(), 2);
    }

    #[test]
    fn actual_native_collar_cantilever_pipeline_binds_both_original_caps_and_empty_leaves() {
        let definition = original("box-mm.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(300_000_000);
        let source = crate::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let pose = RigidPoseMm {
            translation_mm: [10.0, 0.0, 8.0],
            ..RigidPoseMm::IDENTITY
        };
        let a0 = ranged_slab(
            &source,
            RigidPoseMm::IDENTITY,
            [7.8, 8.0],
            7.9,
            0,
            &mut work,
        );
        let b0 = ranged_slab(
            &source,
            pose,
            [7.8, 8.0],
            7.9,
            a0.retained_bytes(),
            &mut work,
        );
        let a1 = ranged_slab(
            &source,
            RigidPoseMm::IDENTITY,
            [8.0, 8.2],
            8.1,
            a0.retained_bytes() + b0.retained_bytes(),
            &mut work,
        );
        let b1 = ranged_slab(
            &source,
            pose,
            [8.0, 8.2],
            8.1,
            a0.retained_bytes() + b0.retained_bytes() + a1.retained_bytes(),
            &mut work,
        );
        let am0 = xy_candidate(&a0, 7.9);
        let bm0 = xy_candidate(&b0, 7.9);
        let am1 = xy_candidate(&a1, 8.1);
        let bm1 = xy_candidate(&b1, 8.1);
        assert!(!am0.is_empty() && bm0.is_empty() && am1.is_empty() && !bm1.is_empty());
        let al0 = source_leaf(&source, 41, RigidPoseMm::IDENTITY, &a0, &mut work);
        let bl0 = source_leaf(&source, 97, pose, &b0, &mut work);
        let al1 = source_leaf(&source, 41, RigidPoseMm::IDENTITY, &a1, &mut work);
        let bl1 = source_leaf(&source, 97, pose, &b1, &mut work);
        let ac0 = collar_for(&source, &a0, &al0, &am0, &[], &mut work);
        let bc0 = collar_for(
            &source,
            &b0,
            &bl0,
            &bm0,
            &[NativeCriticalCollarTarget {
                leaf: &bl1,
                actual_material: &bm1,
            }],
            &mut work,
        );
        let ac1 = collar_for(
            &source,
            &a1,
            &al1,
            &am1,
            &[NativeCriticalCollarTarget {
                leaf: &al0,
                actual_material: &am0,
            }],
            &mut work,
        );
        let bc1 = collar_for(&source, &b1, &bl1, &bm1, &[], &mut work);
        let before = [
            sampled_occurrence(&source, &a0, &al0, ac0.as_ref()),
            sampled_occurrence(&source, &b0, &bl0, bc0.as_ref()),
        ];
        let after = [
            sampled_occurrence(&source, &a1, &al1, ac1.as_ref()),
            sampled_occurrence(&source, &b1, &bl1, bc1.as_ref()),
        ];
        let first =
            super::certify_native_union(&before, &am0, policy(0.0), &mut work, &cancel).unwrap();
        let second =
            super::certify_native_union(&after, &bm1, policy(0.0), &mut work, &cancel).unwrap();
        for pending in [&first, &second] {
            assert_eq!(
                pending.coverage(),
                NativeUnionCoverage::NativeInterfaceComplex
            );
            assert_eq!(pending.interface_attachments().len(), 1);
            let attachment = &pending.interface_attachments()[0];
            assert!(attachment.native_cell_incidence().contains(&3));
            assert!(
                definition
                    .faces()
                    .iter()
                    .any(|face| &face.face_id == attachment.endpoint_source_cap_face_id())
            );
        }
        let mut ledger = NativeUnionBindingLedger::new([7.8, 8.2], MAX_SCRATCH).unwrap();
        ledger
            .record(first, &am0, &[&al0], &mut work, &cancel)
            .unwrap();
        ledger
            .record(second, &bm1, &[&bl1], &mut work, &cancel)
            .unwrap();
        let completed = ledger.finalize(&mut work, &cancel).unwrap();
        assert_eq!(completed.sources().len(), 2);
    }

    #[test]
    fn nonrepresentable_original_floor_binds_void_port_even_when_current_material_covers_every_cell()
     {
        let definition = original("native-parity-blind-pocket.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(300_000_000);
        let source = crate::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let pose = RigidPoseMm {
            translation_mm: [0.0, 0.0, 0.13],
            ..RigidPoseMm::IDENTITY
        };
        let lower = ranged_slab(&source, pose, [3.98, 4.08], 4.03, 0, &mut work);
        let current = ranged_slab(
            &source,
            pose,
            [4.08, 4.18],
            4.10,
            lower.retained_bytes(),
            &mut work,
        );
        let upper = ranged_slab(
            &source,
            pose,
            [4.18, 4.28],
            4.23,
            lower.retained_bytes() + current.retained_bytes(),
            &mut work,
        );
        let lm = xy_candidate(&lower, 4.03);
        let cm = xy_candidate(&current, 4.10);
        let um = xy_candidate(&upper, 4.23);
        let ll = source_leaf(&source, 67, pose, &lower, &mut work);
        let cl = source_leaf(&source, 67, pose, &current, &mut work);
        let ul = source_leaf(&source, 67, pose, &upper, &mut work);
        assert_eq!(cm.iter().filter(|loop_| loop_.is_hole).count(), 0);
        assert_eq!(um.iter().filter(|loop_| loop_.is_hole).count(), 1);
        let cc = collar_for(
            &source,
            &current,
            &cl,
            &cm,
            &[NativeCriticalCollarTarget {
                leaf: &ul,
                actual_material: &um,
            }],
            &mut work,
        )
        .unwrap();
        assert!(cc.native_collar().native_event_ports().iter().any(|port| {
            let height = port.height_interval_mm();
            height[0] < height[1] && height[0] > 4.10 && height[1] < 4.18
        }));
        assert!(
            cc.native_collar()
                .used_material_targets()
                .all(|target| target.side() == NativeCriticalCollarTargetSide::Current)
        );
        assert!(
            cc.used_source_leaves()
                .iter()
                .any(|leaf| same_leaf(leaf, &ul))
        );
        let lower_occ = [sampled_occurrence(&source, &lower, &ll, None)];
        let current_occ = [sampled_occurrence(&source, &current, &cl, Some(&cc))];
        let upper_occ = [sampled_occurrence(&source, &upper, &ul, None)];
        let first =
            super::certify_native_union(&lower_occ, &lm, policy(0.0), &mut work, &cancel).unwrap();
        let middle =
            super::certify_native_union(&current_occ, &cm, policy(0.0), &mut work, &cancel)
                .unwrap();
        let last =
            super::certify_native_union(&upper_occ, &um, policy(0.0), &mut work, &cancel).unwrap();
        let mut ledger = NativeUnionBindingLedger::new([3.98, 4.28], MAX_SCRATCH).unwrap();
        ledger
            .record(first, &lm, &[&ll], &mut work, &cancel)
            .unwrap();
        ledger
            .record(middle, &cm, &[&cl], &mut work, &cancel)
            .unwrap();
        ledger
            .record(last, &um, &[&ul], &mut work, &cancel)
            .unwrap();
        assert_eq!(ledger.finalize(&mut work, &cancel).unwrap().slab_count(), 3);
        // A coincident empty-Hole sample is not authority for another native
        // reference. The exclusion receipt must bind the accepted full capture.
        let stale = ranged_slab(&source, pose, [4.08, 4.18], 4.11, 0, &mut work);
        assert_eq!(
            cc.capture_projected_voids(stale, MAX_SCRATCH as u32, &mut work, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::InvalidGeometry,
        );
        let capture = cc
            .capture_projected_voids(current, MAX_SCRATCH as u32, &mut work, &cancel)
            .unwrap()
            .expect("off-sample pocket remains protected, not absent");
        let protected = super::super::projected_protected_void_envelopes(
            &definition,
            &capture,
            super::super::ProtectedVoidPolicy {
                maximum_xy_error_mm: 0.05,
                byte_limit: MAX_SCRATCH as u32,
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        assert_eq!(protected.envelopes().len(), 1);
        let envelope = &protected.envelopes()[0];
        assert_eq!(envelope.source_loop_index(), None);
        assert_eq!(
            envelope.height_scope(),
            super::super::ProtectedVoidHeightScope::Bounded {
                lower_mm: 4.08,
                upper_mm: 4.18,
            },
        );
        let rectangle = envelope.world_xy_outer();
        assert!(!rectangle.is_hole);
        assert_eq!(rectangle.points_mm.first(), rectangle.points_mm.last());
        assert!(rectangle.points_mm.iter().all(|point| point[2] == 0.0));
        for boundary in um.iter().filter(|boundary| boundary.is_hole) {
            for point in &boundary.points_mm {
                for axis in 0..2 {
                    assert!(rectangle.points_mm[0][axis] <= point[axis]);
                    assert!(point[axis] <= rectangle.points_mm[2][axis]);
                }
            }
        }
    }

    fn original_opposing_pockets() -> Definition {
        super::super::test_source::original_definition(super::super::test_source::opposing_pockets())
    }

    #[test]
    fn disjoint_void_death_and_birth_inside_one_original_slab_require_both_actual_adjacent_leaves()
    {
        let definition = original_opposing_pockets();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(500_000_000);
        let source = crate::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let lower = ranged_slab(
            &source,
            RigidPoseMm::IDENTITY,
            [3.85, 3.95],
            3.9,
            0,
            &mut work,
        );
        let current = ranged_slab(
            &source,
            RigidPoseMm::IDENTITY,
            [3.95, 4.05],
            4.0,
            lower.retained_bytes(),
            &mut work,
        );
        let upper = ranged_slab(
            &source,
            RigidPoseMm::IDENTITY,
            [4.05, 4.15],
            4.1,
            lower.retained_bytes() + current.retained_bytes(),
            &mut work,
        );
        let lm = xy_candidate(&lower, 3.9);
        let cm = xy_candidate(&current, 4.0);
        let um = xy_candidate(&upper, 4.1);
        assert_eq!(lm.iter().filter(|loop_| loop_.is_hole).count(), 1);
        assert_eq!(cm.iter().filter(|loop_| loop_.is_hole).count(), 0);
        assert_eq!(um.iter().filter(|loop_| loop_.is_hole).count(), 1);
        let ll = source_leaf(&source, 71, RigidPoseMm::IDENTITY, &lower, &mut work);
        let cl = source_leaf(&source, 71, RigidPoseMm::IDENTITY, &current, &mut work);
        let ul = source_leaf(&source, 71, RigidPoseMm::IDENTITY, &upper, &mut work);
        let cc = collar_for(
            &source,
            &current,
            &cl,
            &cm,
            &[
                NativeCriticalCollarTarget {
                    leaf: &ll,
                    actual_material: &lm,
                },
                NativeCriticalCollarTarget {
                    leaf: &ul,
                    actual_material: &um,
                },
            ],
            &mut work,
        )
        .unwrap();
        assert!(
            cc.native_collar()
                .used_material_targets()
                .all(|target| target.side() == NativeCriticalCollarTargetSide::Current)
        );
        for adjacent in [&ll, &ul] {
            assert!(
                cc.used_source_leaves()
                    .iter()
                    .any(|leaf| same_leaf(leaf, adjacent))
            );
        }
        for sign in [-1, 1] {
            assert!(
                cc.native_collar()
                    .native_event_ports()
                    .iter()
                    .any(|port| port.outward_height_sign() == sign)
            );
        }
        let mut ledger = NativeUnionBindingLedger::new([3.85, 4.15], MAX_SCRATCH).unwrap();
        for (analysis, leaf, actual, collar) in [
            (&lower, &ll, &lm, None),
            (&current, &cl, &cm, Some(&cc)),
            (&upper, &ul, &um, None),
        ] {
            let occurrences = [sampled_occurrence(&source, analysis, leaf, collar)];
            let pending =
                super::certify_native_union(&occurrences, actual, policy(0.0), &mut work, &cancel)
                    .unwrap();
            ledger
                .record(pending, actual, &[leaf], &mut work, &cancel)
                .unwrap();
        }
        let completed = ledger.finalize(&mut work, &cancel).unwrap();
        assert_eq!(completed.sources().len(), 1);
        assert_eq!(completed.slab_count(), 3);
        assert_eq!(completed.owned_critical_slab_count(), 1);
    }

    #[test]
    fn genuine_thin_sheared_native_rod_cannot_infer_actual_interface_from_individual_fidelity() {
        let definition = super::super::test_source::original_definition(
            super::super::test_source::thin_sheared_rod(),
        );
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(200_000_000);
        let source = crate::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let before = ranged_slab(
            &source,
            RigidPoseMm::IDENTITY,
            [4.0 - 1.0 / 64.0, 4.0],
            4.0 - 1.0 / 128.0,
            0,
            &mut work,
        );
        let after = ranged_slab(
            &source,
            RigidPoseMm::IDENTITY,
            [4.0, 4.0 + 1.0 / 64.0],
            4.0 + 1.0 / 128.0,
            before.retained_bytes(),
            &mut work,
        );
        let bm = xy_candidate(&before, before.sample_world_z_mm().unwrap());
        let am = xy_candidate(&after, after.sample_world_z_mm().unwrap());
        let bl = source_leaf(&source, 79, RigidPoseMm::IDENTITY, &before, &mut work);
        let al = source_leaf(&source, 79, RigidPoseMm::IDENTITY, &after, &mut work);
        assert!(
            bl.directed_sample_to_closed_source_error_mm() < 0.1
                && al.directed_sample_to_closed_source_error_mm() < 0.1
        );
        let first = super::certify_native_union(
            &[sampled_occurrence(&source, &before, &bl, None)],
            &bm,
            policy(0.0),
            &mut work,
            &cancel,
        )
        .unwrap();
        let second = super::certify_native_union(
            &[sampled_occurrence(&source, &after, &al, None)],
            &am,
            policy(0.0),
            &mut work,
            &cancel,
        )
        .unwrap();
        let mut ledger =
            NativeUnionBindingLedger::new([4.0 - 1.0 / 64.0, 4.0 + 1.0 / 64.0], MAX_SCRATCH)
                .unwrap();
        ledger
            .record(first, &bm, &[&bl], &mut work, &cancel)
            .unwrap();
        assert_eq!(
            ledger
                .record(second, &am, &[&al], &mut work, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::AnalysisIndeterminate
        );
    }

    fn actual_box_union(dx: f64, dy: f64, z: f64) -> Vec<SectionLoopMm> {
        let mut loop_ = candidate(
            &[
                [0.0, 0.0],
                [20.0, 0.0],
                [20.0, dy],
                [20.0 + dx, dy],
                [20.0 + dx, 10.0 + dy],
                [dx, 10.0 + dy],
                [dx, 10.0],
                [0.0, 10.0],
            ],
            false,
        );
        for point in &mut loop_.points_mm {
            point[2] = z;
        }
        vec![loop_]
    }

    #[test]
    fn overlapping_original_boxes_and_separated_varying_cone_bind_every_actual_group_in_either_order()
     {
        let boxes = original("box-mm.step");
        let cone = original("native-parity-cone-frustum.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(500_000_000);
        let boxes_source = crate::prepare_solid(
            &boxes,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let cone_source = crate::prepare_solid(
            &cone,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let dx = 10.123456789;
        let dy = 0.234567891;
        let box_pose = RigidPoseMm {
            translation_mm: [dx, dy, 0.0],
            ..RigidPoseMm::IDENTITY
        };
        for translation in [[60.0, 0.0, 0.0], [34.5, 13.5, 0.0]] {
            let cone_pose = RigidPoseMm {
                translation_mm: translation,
                ..RigidPoseMm::IDENTITY
            };
            // Include a REAL original cap collar, not only regular middle samples:
            // the diagonal projection must bind ALL native material in the slab.
            for (domain, layers, critical) in [
                (
                    [3.98, 4.02],
                    [([3.98, 4.0], 3.99), ([4.0, 4.02], 4.01)],
                    false,
                ),
                (
                    [0.0, 0.04],
                    [([0.0, 0.02], 0.01), ([0.02, 0.04], 0.03)],
                    true,
                ),
            ] {
                let mut expected_hashes: [Option<[u8; 32]>; 2] = [None; 2];
                // Also change ORIGINAL IDs so the curved singleton precedes both boxes;
                // reversing only a caller vector is insufficient for sorted consumers.
                for (ids, order) in [
                    ([1, 2, 3], [0, 1, 2]),
                    ([1, 2, 3], [2, 1, 0]),
                    ([2, 3, 1], [2, 0, 1]),
                ] {
                    let mut ledger = NativeUnionBindingLedger::new(domain, MAX_SCRATCH).unwrap();
                    for (layer, (interval, z)) in layers.into_iter().enumerate() {
                        let a = ranged_slab(
                            &boxes_source,
                            RigidPoseMm::IDENTITY,
                            interval,
                            z,
                            0,
                            &mut work,
                        );
                        let b = ranged_slab(
                            &boxes_source,
                            box_pose,
                            interval,
                            z,
                            a.retained_bytes(),
                            &mut work,
                        );
                        let c = ranged_slab(
                            &cone_source,
                            cone_pose,
                            interval,
                            z,
                            a.retained_bytes() + b.retained_bytes(),
                            &mut work,
                        );
                        let al = source_leaf(
                            &boxes_source,
                            ids[0],
                            RigidPoseMm::IDENTITY,
                            &a,
                            &mut work,
                        );
                        let bl = source_leaf(&boxes_source, ids[1], box_pose, &b, &mut work);
                        let cl = source_leaf(&cone_source, ids[2], cone_pose, &c, &mut work);
                        let ac = collar_for(
                            &boxes_source,
                            &a,
                            &al,
                            &xy_candidate(&a, z),
                            &[],
                            &mut work,
                        );
                        let bc = collar_for(
                            &boxes_source,
                            &b,
                            &bl,
                            &xy_candidate(&b, z),
                            &[],
                            &mut work,
                        );
                        let cc =
                            collar_for(&cone_source, &c, &cl, &xy_candidate(&c, z), &[], &mut work);
                        if critical && layer == 0 {
                            assert!(cc.is_some());
                        }
                        let mut actual = actual_box_union(dx, dy, z);
                        actual.extend(xy_candidate(&c, z));
                        let occurrences = order.map(|i| match i {
                            0 => sampled_occurrence(&boxes_source, &a, &al, ac.as_ref()),
                            1 => sampled_occurrence(&boxes_source, &b, &bl, bc.as_ref()),
                            _ => sampled_occurrence(&cone_source, &c, &cl, cc.as_ref()),
                        });
                        let pending = super::certify_native_union(
                            &occurrences,
                            &actual,
                            policy(0.0),
                            &mut work,
                            &cancel,
                        )
                        .unwrap();
                        if critical && layer == 0 {
                            // Current-plane/directed evidence is NOT the reverse
                            // authority: the actual native bottom cap has radius 5,
                            // larger than the strictly interior sampled cone profile.
                            assert!(cl.closed_shape_error_mm().is_none());
                            assert!(c.closed_shape_error_mm().is_none());
                            let cone_envelope = target_envelope(
                                &pending.target_envelopes,
                                cl.occurrence_id(),
                                &mut work,
                                &cancel,
                            )
                            .unwrap()
                            .unwrap();
                            let ball = cone_envelope.ball.unwrap();
                            assert!(ball.radius >= 5.0);
                            if translation[0] == 34.5 {
                                let box_envelope = target_envelope(
                                    &pending.target_envelopes,
                                    bl.occurrence_id(),
                                    &mut work,
                                    &cancel,
                                )
                                .unwrap()
                                .unwrap();
                                assert!(!bounds_separated(
                                    box_envelope.bounds,
                                    cone_envelope.bounds,
                                    0.0
                                ));
                                assert!(ball_box_separated(
                                    ball,
                                    box_envelope.native_bounds,
                                    Interval::point(box_envelope.policy)
                                        .add(Interval::point(cone_envelope.policy))
                                ));
                            }
                        }
                        assert_eq!(
                            pending.coverage(),
                            NativeUnionCoverage::NativeInterfaceComplex
                        );
                        assert_eq!(pending.sources().len(), 3);
                        assert!(
                            pending
                                .preserved_source_occurrences
                                .contains(&cl.occurrence_id())
                        );
                        assert!(!pending.boundary_sources().is_empty());
                        for source in pending.boundary_sources() {
                            let original = occurrences[source.occurrence_index].source.definition();
                            assert!(
                                original
                                    .faces()
                                    .iter()
                                    .any(|face| face.face_id == source.face_id)
                            );
                        }
                        let hash = *pending.candidate_geometry_sha256();
                        if let Some(expected) = expected_hashes[layer] {
                            assert_eq!(hash, expected);
                        } else {
                            expected_hashes[layer] = Some(hash);
                        }
                        ledger
                            .record(pending, &actual, &[&al, &bl, &cl], &mut work, &cancel)
                            .unwrap();
                    }
                    let completed = ledger.finalize(&mut work, &cancel).unwrap();
                    assert_eq!(completed.slab_count(), 2);
                    assert_eq!(completed.sources().len(), 3);
                    assert_eq!(completed.owned_critical_slab_count() > 0, critical);
                }
            }
        }
    }

    #[test]
    fn whole_target_projection_clearance_reserves_both_candidate_tubes_and_refuses_tangency() {
        let box_ = [[10.123456789, 30.123456789], [0.234567891, 10.234567891]];
        let ball = NativeProjectionBall {
            center: [34.5, 13.5],
            radius: 5.1,
        };
        assert!(ball_box_separated(
            ball,
            box_,
            Interval::point(0.1).add(Interval::point(0.1))
        ));
        assert!(!ball_box_separated(
            ball,
            box_,
            Interval::point(0.2).add(Interval::point(0.2))
        ));
        assert!(!ball_box_separated(
            NativeProjectionBall {
                center: [25.0, 10.0],
                radius: 5.0
            },
            [[0.0, 20.0], [0.0, 10.0]],
            Interval::point(0.0)
        ));
        assert!(!ball_box_separated(
            NativeProjectionBall {
                center: [f64::from_bits(2), 0.0],
                radius: 0.0
            },
            [[0.0, 0.0], [0.0, 0.0]],
            Interval::point(0.0)
        ));
    }

    #[test]
    fn uncertainty_separation_not_raw_polygon_bboxes_decides_native_interaction_groups() {
        let boxes = original("box-mm.step");
        let cone = original("native-parity-cone-frustum.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(200_000_000);
        let boxes_source = crate::prepare_solid(
            &boxes,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let cone_source = crate::prepare_solid(
            &cone,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let dx = 10.123456789;
        let dy = 0.234567891;
        let box_pose = RigidPoseMm {
            translation_mm: [dx, dy, 0.0],
            ..RigidPoseMm::IDENTITY
        };
        let cone_pose = RigidPoseMm {
            translation_mm: [34.2, 0.0, 0.0],
            ..RigidPoseMm::IDENTITY
        };
        let a = slab(&boxes_source, RigidPoseMm::IDENTITY, 0, &mut work);
        let b = slab(&boxes_source, box_pose, a.retained_bytes(), &mut work);
        let c = slab(
            &cone_source,
            cone_pose,
            a.retained_bytes() + b.retained_bytes(),
            &mut work,
        );
        let al = source_leaf(&boxes_source, 1, RigidPoseMm::IDENTITY, &a, &mut work);
        let bl = source_leaf(&boxes_source, 2, box_pose, &b, &mut work);
        let cl = source_leaf(&cone_source, 3, cone_pose, &c, &mut work);
        let cm = xy_candidate(&c, 4.0);
        assert!(
            cm.iter()
                .flat_map(|loop_| &loop_.points_mm)
                .all(|point| point[0] > 20.0 + dx)
        );
        let mut actual = actual_box_union(dx, dy, 4.0);
        actual.extend(cm);
        let occurrences = [
            sampled_occurrence(&boxes_source, &a, &al, None),
            sampled_occurrence(&boxes_source, &b, &bl, None),
            sampled_occurrence(&cone_source, &c, &cl, None),
        ];
        let failure =
            super::certify_native_union(&occurrences, &actual, policy(0.0), &mut work, &cancel)
                .unwrap_err();
        assert_eq!(failure.code, GeometryErrorCode::AnalysisIndeterminate);
        assert!(
            failure.message.contains("occurrence pair 3/2")
                || failure.message.contains("occurrence pair 2/3")
        );
    }

    #[test]
    fn complete_actual_global_partition_rejects_missing_sources_extraneous_loops_and_bridges() {
        let boxes = original("box-mm.step");
        let cone = original("native-parity-cone-frustum.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(200_000_000);
        let boxes_source = crate::prepare_solid(
            &boxes,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let cone_source = crate::prepare_solid(
            &cone,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let dx = 10.123456789;
        let dy = 0.234567891;
        let box_pose = RigidPoseMm {
            translation_mm: [dx, dy, 0.0],
            ..RigidPoseMm::IDENTITY
        };
        let cone_pose = RigidPoseMm {
            translation_mm: [60.0, 0.0, 0.0],
            ..RigidPoseMm::IDENTITY
        };
        let a = slab(&boxes_source, RigidPoseMm::IDENTITY, 0, &mut work);
        let b = slab(&boxes_source, box_pose, a.retained_bytes(), &mut work);
        let c = slab(
            &cone_source,
            cone_pose,
            a.retained_bytes() + b.retained_bytes(),
            &mut work,
        );
        let al = source_leaf(&boxes_source, 1, RigidPoseMm::IDENTITY, &a, &mut work);
        let bl = source_leaf(&boxes_source, 2, box_pose, &b, &mut work);
        let cl = source_leaf(&cone_source, 3, cone_pose, &c, &mut work);
        let occurrences = [
            sampled_occurrence(&boxes_source, &a, &al, None),
            sampled_occurrence(&boxes_source, &b, &bl, None),
            sampled_occurrence(&cone_source, &c, &cl, None),
        ];
        let missing = actual_box_union(dx, dy, 4.0);
        assert!(
            super::certify_native_union(&occurrences, &missing, policy(0.0), &mut work, &cancel)
                .is_err()
        );
        let mut extra = copy_loops(&missing);
        extra.extend(xy_candidate(&c, 4.0));
        extra.push(rectangle(60.0, 0.0, 61.0, 1.0));
        assert!(
            super::certify_native_union(&occurrences, &extra, policy(0.0), &mut work, &cancel)
                .is_err()
        );
        let bridge = [candidate(
            &[
                [0.0, 0.0],
                [20.0, 0.0],
                [60.0, -1.0],
                [61.0, 0.0],
                [60.0, 1.0],
                [0.0, 10.0],
            ],
            false,
        )];
        let failure =
            super::certify_native_union(&occurrences, &bridge, policy(0.0), &mut work, &cancel)
                .unwrap_err();
        assert_eq!(failure.code, GeometryErrorCode::AnalysisIndeterminate);
        assert!(failure.message.contains("bridges uncertainty-separated"));
    }

    #[test]
    fn isolated_original_curved_void_is_not_lost_beside_an_interacting_native_box_group() {
        let boxes = original("box-mm.step");
        let pocket = original("native-parity-blind-pocket.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(200_000_000);
        let boxes_source = crate::prepare_solid(
            &boxes,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let pocket_source = crate::prepare_solid(
            &pocket,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let dx = 10.123456789;
        let dy = 0.234567891;
        let box_pose = RigidPoseMm {
            translation_mm: [dx, dy, 0.0],
            ..RigidPoseMm::IDENTITY
        };
        let pocket_pose = RigidPoseMm {
            translation_mm: [80.0, 0.0, 0.0],
            ..RigidPoseMm::IDENTITY
        };
        let a = ranged_slab(
            &boxes_source,
            RigidPoseMm::IDENTITY,
            [4.99, 5.01],
            5.0,
            0,
            &mut work,
        );
        let b = ranged_slab(
            &boxes_source,
            box_pose,
            [4.99, 5.01],
            5.0,
            a.retained_bytes(),
            &mut work,
        );
        let c = ranged_slab(
            &pocket_source,
            pocket_pose,
            [4.99, 5.01],
            5.0,
            a.retained_bytes() + b.retained_bytes(),
            &mut work,
        );
        let al = source_leaf(&boxes_source, 1, RigidPoseMm::IDENTITY, &a, &mut work);
        let bl = source_leaf(&boxes_source, 2, box_pose, &b, &mut work);
        let cl = source_leaf(&pocket_source, 3, pocket_pose, &c, &mut work);
        let mut actual = actual_box_union(dx, dy, 5.0);
        actual.extend(xy_candidate(&c, 5.0));
        assert_eq!(actual.iter().filter(|loop_| loop_.is_hole).count(), 1);
        let occurrences = [
            sampled_occurrence(&boxes_source, &a, &al, None),
            sampled_occurrence(&boxes_source, &b, &bl, None),
            sampled_occurrence(&pocket_source, &c, &cl, None),
        ];
        super::certify_native_union(&occurrences, &actual, policy(0.0), &mut work, &cancel)
            .unwrap();
        actual.retain(|loop_| !loop_.is_hole);
        assert!(
            super::certify_native_union(&occurrences, &actual, policy(0.0), &mut work, &cancel)
                .is_err()
        );
    }

    #[test]
    fn registered_regular_pair_binds_source_buffers_receipt_and_closed_stream() {
        let cancel = AtomicBool::new(false);
        // Exercise the admitted operator source, not the deferred synthetic
        // source-corner/critical expansion. The planar test owns non-star cases.
        let definition = crate::from_step_bytes_with_cancel(
            include_bytes!(
                "../../../../fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step"
            ),
            &cancel,
        )
        .unwrap();
        let pose = RigidPoseMm {
            translation_mm: [100.0, 105.0, 0.0],
            rotation_xyzw: [0.0, 0.0, 0.0, 1.0],
        };
        let mut work = GeometryWorkBudget::new(200_000_000);
        let source = crate::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let cut = 5.03;
        let before = ranged_slab(&source, pose, [5.01, cut], 5.02, 0, &mut work);
        let after = ranged_slab(
            &source,
            pose,
            [cut, 5.05],
            5.04,
            before.retained_bytes(),
            &mut work,
        );
        let before_m = xy_candidate(&before, before.sample_world_z_mm().unwrap());
        let after_m = xy_candidate(&after, after.sample_world_z_mm().unwrap());
        let id = OccurrenceId::new(1).unwrap();
        let before_leaf = certify_native_source_leaf(
            &SourceLeafRequest {
                source: &source,
                occurrence_id: &id,
                pose,
                analysis: &before,
                actual_used_source_loops: &before_m,
                policy: policy(0.0),
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        let after_leaf = certify_native_source_leaf(
            &SourceLeafRequest {
                source: &source,
                occurrence_id: &id,
                pose,
                analysis: &after,
                actual_used_source_loops: &after_m,
                policy: policy(0.0),
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        assert_eq!(
            before_leaf.proof_kind(),
            NativeSourceLeafProofKind::ClosedSlab
        );
        assert_eq!(
            after_leaf.proof_kind(),
            NativeSourceLeafProofKind::ClosedSlab
        );
        assert_eq!(
            after_leaf
                .sample_loops
                .iter()
                .filter(|cycle| !cycle.hole)
                .count(),
            1
        );
        assert!(after_leaf.sample_loops.iter().any(|cycle| cycle.hole));
        let before_tube = source_boundary_tube(&before_leaf, &[], &mut work, &cancel).unwrap();
        let after_tube = source_boundary_tube(&after_leaf, &[], &mut work, &cancel).unwrap();
        boundary_loop_confinement(&before_leaf, &before_tube, 1, cut, &mut work, &cancel).unwrap();
        boundary_loop_confinement(&after_leaf, &after_tube, 0, cut, &mut work, &cancel).unwrap();
        let occurrences = [NativeUnionOccurrence {
            occurrence_id: id,
            source: &source,
            pose,
            evidence: source_evidence(&after, &after_leaf, None),
        }];
        let mut pending =
            super::certify_native_union(&occurrences, &after_m, policy(0.0), &mut work, &cancel)
                .unwrap();
        let lower_target = NativeCriticalCollarTarget {
            leaf: &before_leaf,
            actual_material: &before_m,
        };
        let upper_target = NativeCriticalCollarTarget {
            leaf: &after_leaf,
            actual_material: &after_m,
        };
        let original_bytes = pending.retained_bytes();
        assert_eq!(
            pending
                .certify_regular_interface_pair(
                    NativeCriticalCollarTarget { ..lower_target },
                    NativeCriticalCollarTarget { ..upper_target },
                    original_bytes,
                    &mut work,
                    &cancel,
                )
                .unwrap_err()
                .code,
            GeometryErrorCode::ResourceLimit
        );
        assert!(
            pending
                .certify_regular_interface_pair(
                    NativeCriticalCollarTarget { ..lower_target },
                    NativeCriticalCollarTarget { ..upper_target },
                    MAX_SCRATCH,
                    &mut work,
                    &cancel,
                )
                .unwrap(),
            "complete actual registered buffers issue a sealed pair receipt"
        );
        assert_eq!(pending.regular_interfaces.len(), 1);
        assert!(
            pending.retained_bytes()
                >= original_bytes + std::mem::size_of::<NativeRegularInterfacePairReceipt>()
        );
        certify_regular_material_interface(
            &before_leaf,
            &after_leaf,
            cut,
            &before_tube,
            &after_tube,
            &pending.regular_interfaces,
            &mut Scratch { bytes: 0 },
            &mut work,
            &cancel,
        )
        .unwrap();
        let sealed = pending.regular_interfaces[0];
        let mut wrong_cut = before_leaf.clone();
        wrong_cut.world_height_interval_mm[1] = cut.next_up();
        assert_eq!(
            pending
                .certify_regular_interface_pair(
                    NativeCriticalCollarTarget {
                        leaf: &wrong_cut,
                        actual_material: &before_m
                    },
                    NativeCriticalCollarTarget { ..upper_target },
                    MAX_SCRATCH,
                    &mut work,
                    &cancel,
                )
                .unwrap_err()
                .code,
            GeometryErrorCode::InvalidGeometry
        );
        assert_eq!(pending.regular_interfaces[0], sealed);
        let mut critical_lower = before_leaf.clone();
        critical_lower.critical_interfaces[1] = true;
        assert!(
            !pending
                .certify_regular_interface_pair(
                    NativeCriticalCollarTarget {
                        leaf: &critical_lower,
                        actual_material: &before_m
                    },
                    NativeCriticalCollarTarget { ..upper_target },
                    MAX_SCRATCH,
                    &mut work,
                    &cancel,
                )
                .unwrap(),
            "a critical shared side remains owned by the independent collar route"
        );
        let mut exhausted = GeometryWorkBudget::new(0);
        assert_eq!(
            pending
                .certify_regular_interface_pair(
                    NativeCriticalCollarTarget { ..lower_target },
                    NativeCriticalCollarTarget { ..upper_target },
                    MAX_SCRATCH,
                    &mut exhausted,
                    &cancel,
                )
                .unwrap_err()
                .code,
            GeometryErrorCode::ResourceLimit
        );
        assert_eq!(
            pending
                .certify_regular_interface_pair(
                    NativeCriticalCollarTarget { ..lower_target },
                    NativeCriticalCollarTarget { ..upper_target },
                    MAX_SCRATCH,
                    &mut work,
                    &AtomicBool::new(true),
                )
                .unwrap_err()
                .code,
            GeometryErrorCode::Cancelled
        );
        assert_eq!(
            pending.regular_interfaces.as_slice(),
            &[sealed],
            "critical, stale, exhausted and cancelled attempts grant no additional authority"
        );
        let clearance = after_leaf.sample_loop_clearance_mm.unwrap();
        assert!(clearance > 0.0);

        // Exercise equality/hash binding without adding point storage to the
        // receipt. These deliberately poisoned private metadata values cannot
        // be constructed through the public source-leaf API.
        let mut changed = certify_native_source_leaf(
            &SourceLeafRequest {
                source: &source,
                occurrence_id: &id,
                pose,
                analysis: &after,
                actual_used_source_loops: &after_m,
                policy: policy(0.0),
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        let mut foreign_current = changed.clone();
        foreign_current.sample_region_error_mm = foreign_current.sample_region_error_mm.next_up();
        assert_eq!(
            pending
                .certify_regular_interface_pair(
                    NativeCriticalCollarTarget { ..lower_target },
                    NativeCriticalCollarTarget {
                        leaf: &foreign_current,
                        actual_material: &after_m
                    },
                    MAX_SCRATCH,
                    &mut work,
                    &cancel,
                )
                .unwrap_err()
                .code,
            GeometryErrorCode::InvalidGeometry,
            "the current leaf must belong to this pending proof"
        );
        drop(foreign_current);
        let mut foreign = after_m;
        foreign.swap(0, 1);
        assert_eq!(
            verify_source_leaf_candidate(&after_leaf, &foreign, &mut work, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::InvalidGeometry
        );
        assert_eq!(
            pending
                .certify_regular_interface_pair(
                    NativeCriticalCollarTarget {
                        leaf: &before_leaf,
                        actual_material: &before_m
                    },
                    NativeCriticalCollarTarget {
                        leaf: &after_leaf,
                        actual_material: &foreign
                    },
                    MAX_SCRATCH,
                    &mut work,
                    &cancel,
                )
                .unwrap_err()
                .code,
            GeometryErrorCode::InvalidGeometry,
            "an existing sealed receipt cannot be reused for foreign actual geometry"
        );
        foreign.swap(0, 1);
        let hole_index = foreign.iter().position(|cycle| cycle.is_hole).unwrap();
        foreign[hole_index].is_hole = false;
        assert_eq!(
            pending
                .certify_regular_interface_pair(
                    NativeCriticalCollarTarget { ..lower_target },
                    NativeCriticalCollarTarget {
                        leaf: &after_leaf,
                        actual_material: &foreign
                    },
                    MAX_SCRATCH,
                    &mut work,
                    &cancel,
                )
                .unwrap_err()
                .code,
            GeometryErrorCode::InvalidGeometry,
            "a sealed receipt cannot erase an actual protected hole"
        );
        foreign[hole_index].is_hole = true;
        assert_eq!(
            pending
                .certify_regular_interface_pair(
                    NativeCriticalCollarTarget {
                        leaf: &before_leaf,
                        actual_material: &foreign
                    },
                    NativeCriticalCollarTarget {
                        leaf: &after_leaf,
                        actual_material: &foreign
                    },
                    MAX_SCRATCH,
                    &mut work,
                    &cancel,
                )
                .unwrap_err()
                .code,
            GeometryErrorCode::InvalidGeometry,
            "both actual buffers must be independently verified, including the lower one"
        );
        let lower_occurrences = [NativeUnionOccurrence {
            occurrence_id: id,
            source: &source,
            pose,
            evidence: source_evidence(&before, &before_leaf, None),
        }];
        let lower_pending = super::certify_native_union(
            &lower_occurrences,
            &before_m,
            policy(0.0),
            &mut work,
            &cancel,
        )
        .unwrap();
        let mut ledger = NativeUnionBindingLedger::new([5.01, 5.05], MAX_SCRATCH).unwrap();
        ledger
            .record(
                lower_pending,
                &before_m,
                &[&before_leaf],
                &mut work,
                &cancel,
            )
            .unwrap();
        ledger
            .record(pending, &foreign, &[&after_leaf], &mut work, &cancel)
            .unwrap();
        let completed = ledger.finalize(&mut work, &cancel).unwrap();
        assert_eq!(completed.world_height_interval_mm(), [5.01, 5.05]);
        assert_eq!(
            completed.slab_count(),
            2,
            "the registered pair and adjacent source leaves retain closed stream binding"
        );
        let hash = |leaf: &NativeSourceLeafReceipt| {
            let mut hash = Sha256::new();
            hash_leaf(&mut hash, leaf);
            hash.finalize()
        };
        assert!(same_leaf(&after_leaf, &changed));
        changed.sample_loop_clearance_mm = Some(clearance * 0.5);
        assert!(!same_leaf(&after_leaf, &changed));
        assert_ne!(hash(&after_leaf), hash(&changed));
        changed.sample_loop_clearance_mm = Some(clearance);
        let original_index = changed.sample_loops[0].registration.native_index;
        Arc::get_mut(&mut changed.sample_loops).unwrap()[0]
            .registration
            .native_index = usize::MAX;
        assert!(!same_leaf(&after_leaf, &changed));
        assert_ne!(hash(&after_leaf), hash(&changed));
        Arc::get_mut(&mut changed.sample_loops).unwrap()[0]
            .registration
            .native_index = original_index;
        let hole = changed
            .sample_loops
            .iter()
            .position(|cycle| cycle.hole)
            .unwrap();
        let original_parent = changed.sample_loops[hole].registration.parent;
        Arc::get_mut(&mut changed.sample_loops).unwrap()[hole]
            .registration
            .parent = None;
        assert!(!same_leaf(&after_leaf, &changed));
        assert_ne!(hash(&after_leaf), hash(&changed));
        Arc::get_mut(&mut changed.sample_loops).unwrap()[hole]
            .registration
            .parent = original_parent;

        // Isolate the captured whole-boundary clearance consumer by discarding
        // alternative radial/kernel witnesses, never by inventing a source fact.
        changed.radial_product = None;
        changed.endpoint_radial_products = [None; 2];
        for cycle in Arc::get_mut(&mut changed.sample_loops).unwrap() {
            cycle.shape.kernel_radius_mm = 0.0;
            cycle.shape.kernel_box = None;
        }
        let moving = NativeBoundaryTube {
            occurrence_id: id,
            stationary: false,
            distance_mm: Some(clearance * 0.25),
        };
        boundary_loop_confinement(&changed, &moving, 0, cut, &mut work, &cancel).unwrap();
        let unavailable = NativeBoundaryTube {
            distance_mm: None,
            ..moving
        };
        assert_eq!(
            boundary_loop_confinement(&changed, &unavailable, 0, cut, &mut work, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::AnalysisIndeterminate,
        );
        let exhausted = NativeBoundaryTube {
            distance_mm: Some(clearance * 0.5),
            ..moving
        };
        assert_eq!(
            boundary_loop_confinement(&changed, &exhausted, 0, cut, &mut work, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::AnalysisIndeterminate,
            "nonpositive remaining whole-boundary clearance supplies no confinement",
        );
    }
}
