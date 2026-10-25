// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! One-sided, original-source critical collars. Metric coverage does not prove
//! critical incidence: the latter must already be owned by the captured native
//! complex. In particular, neither endpoint equality nor sampled loop counts
//! can discharge an event.
//!
//! Native arena IDs bind every paired edge and endpoint occurrence; their
//! coefficients are already millimetres. All material queries borrow the same
//! qualified `PreparedSolid` trim/source owner. Generated native UV closure alone
//! is not original-source correspondence, a joint connector or a filled-face cut.
//! Coefficient queries use the selected original operator and its sealed spatial
//! parameter map; original radian charts are never mixed with native UV gauges.
//! Whole-face rectangle covers deliberately overcover holes; the complete
//! trim-cell proof remains authoritative when that optional cover is too broad.

use super::joint::NativeSourceLeafReceipt;
use super::slab::{HeightFunctional, clip_height_parameter_box};
use super::{GeometryWorkBudget, SlabAnalysis};
use super::material_source::{MaterialSourceView, PreparedSolid, prepare_solid};
use super::source_incidence::{SourceEdgeIncidence, boundary_loop, face_orientation, native_coedge, native_face};
use solidcore::{EdgeId, FaceId, VertexId};
use solidcore::topology::EdgeuseId;
use crate::kernel::{TrimClassification, TrimJointKind};
use crate::parametric::{
    Interval, SURFACE_FIRST_ENCLOSURE_SCRATCH_BYTES, SurfaceFirstEnclosure, surface_enclosure,
    surface_parameter_boxes, surface_piecewise_enclosure, surface_piecewise_first_enclosure,
};
use crate::{Definition, GeometryError, SectionLoopMm};
use sha2::{Digest, Sha256};
use spiling_contracts::geometry::{DefinitionId, GeometryErrorCode, RigidPoseMm};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

const MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_DEPTH: u8 = 64;
type Box2 = [[f64; 2]; 2];
type Matrix = [[Interval; 3]; 3];

// The retained first image coexists with a deferred full Hessian image and its
// midpoint first query. Keep their adapter/native temporaries unavailable to
// trim preparation, target images and the growing accepted-cell inventory.
const SURFACE_COVER_SCRATCH_BYTES: usize = super::slab::HEIGHT_CONTRACTION_SCRATCH_BYTES
    + SURFACE_FIRST_ENCLOSURE_SCRATCH_BYTES
    + 2 * std::mem::size_of::<SurfaceFirstEnclosure>()
    + 2 * std::mem::size_of::<crate::parametric::SurfaceEnclosure>()
    + std::mem::size_of::<crate::parametric::SurfaceEnclosure>()
    + std::mem::size_of::<SurfaceAffineImage>()
    + 8 * std::mem::size_of::<Interval>();

const ISOLATED_VERTEX_SCRATCH_BYTES: usize =
    2 * std::mem::size_of::<crate::parametric::SurfaceEnclosure>()
        + std::mem::size_of::<crate::parametric::SurfaceEnclosure>()
        + std::mem::size_of::<Option<super::critical::OriginalFaceCornerAttachments<'_, '_>>>()
        + super::critical::OriginalFaceCornerAttachments::CONNECTION_SCRATCH_BYTES;

struct PortFaceCoverQuery<'a, 'source> {
    face: FaceId,
    face_index: usize,
    domain: &'a crate::kernel::PreparedFaceTrimDomain<'source>,
    matrix: Matrix,
    pose: RigidPoseMm,
    ports: Option<&'a [NativeCollarPort]>,
    cells: &'a [NativeCriticalCollarCell],
    targets: &'a [NativeCriticalCollarMaterialTarget],
    source_column: [f64; 2],
    interval: [f64; 2],
    policy: NativeCriticalCollarPolicy,
    native_policy: f64,
    count: usize,
    byte_limit: usize,
}

struct FilledFaceCoverQuery<'a, 'source, 'target> {
    definition: &'a Definition,
    face: FaceId,
    face_index: usize,
    domain: &'a crate::kernel::PreparedFaceTrimDomain<'source>,
    matrix: Matrix,
    pose: RigidPoseMm,
    inputs: &'a [NativeCriticalCollarTarget<'target>],
    targets: &'a [NativeCriticalCollarMaterialTarget],
    source_column: [f64; 2],
    event: Option<&'a NativeCollarFaceEvent>,
    sample_band: [f64; 2],
    policy: NativeCriticalCollarPolicy,
    native_policy: f64,
}

struct SurfaceAffineImageQuery<'a> {
    surface: &'a dyn crate::parametric::SurfaceEvaluator,
    uv: Box2,
    matrix: Matrix,
    pose: RigidPoseMm,
    chart_error_mm: f64,
    maximum_error_mm: f64,
}


fn oriented_vertices(
    definition: &Definition,
    coedge: &solidcore::topology::LoopEdge,
) -> Result<[VertexId; 2], GeometryError> {
    let edge = definition.brep().edge(coedge.edge)
        .map_err(|e| unresolved(format!("native collar edge incidence: {e}")))?;
    Ok(if coedge.forward { edge.vertices } else { [edge.vertices[1], edge.vertices[0]] })
}

fn source_coedge_index(
    domain: &crate::kernel::PreparedFaceTrimDomain<'_>, wire: usize, coedge: usize,
    work: &mut GeometryWorkBudget, cancel: &AtomicBool,
) -> Result<usize, GeometryError> {
    for (index, use_) in domain.coedges().iter().enumerate() {
        work.charge(1, cancel)?;
        let identity = use_.identity();
        if identity.wire_index == wire && identity.coedge_index == coedge {
            return Ok(index);
        }
    }
    Err(unresolved("native collar lost its selected original coedge occurrence"))
}

/// Regularity of an actually declared ORIGINAL boundary's scalar query gauge,
/// not a global physical inverse of its carrier. The sealed trace receipt
/// supplies the whole position/relative-jet proof; the separate pole-graph lane
/// cannot be relabelled as this regular lane. All geometric attachment
/// obligations below still use their selected operators and interval enclosures.
fn has_regular_original_boundary_query(
    domain: &crate::kernel::PreparedFaceTrimDomain<'_>,
    coedge: usize,
) -> Result<bool, GeometryError> {
    if !domain.original_coedge_is_declared(coedge)?
        || !matches!(domain.coedge_operand(coedge)?, crate::kernel::PreparedTrimCurve::Original(_))
    {
        return Ok(false);
    }
    let receipt = domain.original_coedge_correspondence(coedge)?;
    if receipt.provenance() == crate::kernel::OriginalCorrespondenceProvenance::CompletedPoleGraph {
        return Ok(false);
    }
    let relative = receipt.relative_derivative_error_bound();
    let slope = receipt.uv_parameter_derivative();
    if !relative.is_finite() || relative >= 0.25
        || !slope.lo.is_finite() || !slope.hi.is_finite() || slope.contains_zero()
    {
        return Ok(false);
    }
    let Some(inverse) = receipt.affine_spatial_parameter_preimage(receipt.uv_range())? else {
        return Ok(false);
    };
    let range = receipt.spatial_range();
    Ok(inverse.lo.is_finite() && inverse.hi.is_finite()
        && inverse.lo <= range[0] && inverse.hi >= range[1])
}

struct CriticalFaceBoundaryStarQuery<'a, 'source> {
    definition: &'a Definition,
    face_index: usize,
    surface: &'a dyn crate::parametric::SurfaceEvaluator,
    domain: &'a crate::kernel::PreparedFaceTrimDomain<'source>,
    cell: Box2,
    jet: &'a crate::parametric::SurfaceEnclosure,
    attachments: Option<&'a super::critical::OriginalFaceCornerAttachments<'a, 'source>>,
    functional: super::HeightFunctional,
    request: &'a super::SlabRequest,
    events: &'a [NativeCollarFaceEvent],
    edges: &'a SourceEdgeIncidence<'source>,
}

struct CriticalCoedgeNormalQuery<'a, 'source> {
    source: MaterialSourceView<'a, 'source>,
    face_index: usize,
    wire: usize,
    coedge: usize,
    height: [f64; 2],
    functional: super::HeightFunctional,
    sign: i8,
    remaining_bytes: usize,
}

struct CriticalEdgeBoundaryStarQuery<'a, 'source> {
    source: MaterialSourceView<'a, 'source>,
    face_index: usize,
    wire_index: usize,
    coedge_index: usize,
    curve: &'a dyn crate::parametric::CurveEvaluator,
    full: [f64; 2],
    cell: [f64; 2],
    functional: super::HeightFunctional,
    request: &'a super::SlabRequest,
    events: &'a [NativeCollarFaceEvent],
    edges: &'a SourceEdgeIncidence<'source>,
    remaining_bytes: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct NativeCriticalCollarPolicy {
    pub maximum_xy_cover_error_mm: f64,
    pub maximum_distance_mm: f64,
    pub byte_limit: u32,
}

/// Untrusted query inputs. The factory independently checks each positive leaf
/// and hashes these exact used polygons; a side is never supplied by a caller.
pub struct NativeCriticalCollarTarget<'a> {
    pub leaf: &'a NativeSourceLeafReceipt,
    pub actual_material: &'a [SectionLoopMm],
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeCriticalCollarTargetSide {
    Current,
    Lower,
    Upper,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeCollarHeightSpace {
    Source,
    World,
}
#[derive(Debug)]
pub struct NativeCriticalCollarMaterialTarget {
    leaf: NativeSourceLeafReceipt,
    side: NativeCriticalCollarTargetSide,
    used: bool,
}
impl NativeCriticalCollarMaterialTarget {
    pub fn leaf(&self) -> &NativeSourceLeafReceipt {
        &self.leaf
    }
    pub fn side(&self) -> NativeCriticalCollarTargetSide {
        self.side
    }
    pub fn is_used(&self) -> bool {
        self.used
    }
}

/// An original native event-face coedge and its actual opposed use. This is
/// retained incidence, not reconstructed proximity or a count of sample loops.
#[derive(Debug)]
pub struct NativeCollarPort {
    edge_id: EdgeId,
    event_edgeuse: EdgeuseId,
    neighbor_edgeuse: EdgeuseId,
    event_face: usize,
    event_wire: usize,
    event_coedge: usize,
    neighbor_face: usize,
    neighbor_wire: usize,
    neighbor_coedge: usize,
    orientation: bool,
    height: [f64; 2],
    outward_height_sign: i8,
    height_space: NativeCollarHeightSpace,
}
impl NativeCollarPort {
    pub fn event_face_ordinal(&self) -> usize {
        self.event_face
    }
    pub fn event_coedge_ordinal(&self) -> [usize; 2] {
        [self.event_wire, self.event_coedge]
    }
    pub fn neighbor_face_ordinal(&self) -> usize {
        self.neighbor_face
    }
    pub fn neighbor_coedge_ordinal(&self) -> [usize; 2] {
        [self.neighbor_wire, self.neighbor_coedge]
    }
    pub fn original_orientation(&self) -> bool {
        self.orientation
    }
    pub fn height_interval_mm(&self) -> [f64; 2] {
        self.height
    }
    pub fn height_space(&self) -> NativeCollarHeightSpace {
        self.height_space
    }
    pub fn outward_height_sign(&self) -> i8 {
        self.outward_height_sign
    }
}

/// Native all-Z outer cover cells. A cell with a face ordinal comes from an
/// original trimmed UV patch, not a mesh triangle or a sampled point. Sample
/// cells enclose the exact *filled* native section using its polygon
/// approximation tube; their rectangles are envelopes, not material membership
/// across holes or between nonconvex/disconnected components.
#[derive(Debug)]
pub struct NativeCriticalCollarCell {
    source_face_ordinal: Option<usize>,
    source_uv: Option<Box2>,
    // Private whole-image authority; a union of rectangle cells is not this fact.
    whole_original_face: bool,
    world_xy: Box2,
    world_height: [f64; 2],
    material_target_index: usize,
    native_port_parameter: Option<(usize, [f64; 2])>,
    xy_distance_mm: f64,
    material_distance_mm: f64,
}
impl NativeCriticalCollarCell {
    pub fn source_face_ordinal(&self) -> Option<usize> {
        self.source_face_ordinal
    }
    pub fn source_uv(&self) -> Option<Box2> {
        self.source_uv
    }
    pub fn world_xy_bounds_mm(&self) -> Box2 {
        self.world_xy
    }
    /// Height range of the represented source-column shadow, not merely its
    /// native boundary witness. Sample and non-event cells retain the whole
    /// source-occupied CLOSED interval; certified world event faces/ports
    /// restrict only the shadow's non-material-facing end. Material distance
    /// covers this entire range. An XY envelope does not assert occupancy
    /// throughout its rectangle; the captured request remains unchanged.
    pub fn world_height_interval_mm(&self) -> [f64; 2] {
        self.world_height
    }
    pub fn material_target_index(&self) -> usize {
        self.material_target_index
    }
    /// Port ordinal and its closed interval in the selected ORIGINAL spatial
    /// evaluator's query gauge. Its sealed map binds the raw source parameters;
    /// this is not a native edge fraction or a generated UV trim parameter.
    pub fn native_port_parameter(&self) -> Option<(usize, [f64; 2])> {
        self.native_port_parameter
    }
    pub fn xy_distance_mm(&self) -> f64 {
        self.xy_distance_mm
    }
    pub fn material_distance_mm(&self) -> f64 {
        self.material_distance_mm
    }
}

/// Sealed original CLOSED C -> actual neighboring positive M proof. This is
/// deliberately not Hausdorff(Mcurrent, C): the current native sample has only
/// the separately exposed one-sided sample -> C fact.
#[derive(Debug, Clone)]
pub struct NativeCriticalCollar {
    inner: Arc<NativeCriticalCollarCapture>,
}
#[derive(Debug)]
struct NativeCriticalCollarCapture {
    definition_id: DefinitionId,
    material_realization: crate::kernel::MaterialRealizationIdentity,
    pose: RigidPoseMm,
    source_request: super::SlabRequest,
    source_sample_plane: crate::PlaneMm,
    actual_sample_world_height_interval: [f64; 2],
    source_world_z_support: super::ProjectedBounds,
    interval: [f64; 2],
    sample: f64,
    native_policy: f64,
    policy: NativeCriticalCollarPolicy,
    targets: Vec<NativeCriticalCollarMaterialTarget>,
    native_ports: Option<Arc<Vec<NativeCollarPort>>>,
    sample_to_source: f64,
    source_to_material: f64,
    xy_error: f64,
    world_regular_boundary_xy_speed: Option<f64>,
    complex_sha256: [u8; 32],
    native_sample_sha256: [u8; 32],
    source_occupied_xy: Box2,
    closed_source_complement_xy: Box2,
    occupied_xy: Box2,
    cells: Vec<NativeCriticalCollarCell>,
}
impl NativeCriticalCollar {
    pub(crate) fn material_realization_identity(
        &self,
    ) -> crate::kernel::MaterialRealizationIdentity {
        self.inner.material_realization
    }
    pub fn definition_id(&self) -> &DefinitionId {
        &self.inner.definition_id
    }
    pub fn placed_pose(&self) -> RigidPoseMm {
        self.inner.pose
    }
    pub fn source_request(&self) -> super::SlabRequest {
        self.inner.source_request
    }
    pub fn source_sample_plane(&self) -> crate::PlaneMm {
        self.inner.source_sample_plane
    }
    pub fn actual_sample_world_height_interval_mm(&self) -> [f64; 2] {
        self.inner.actual_sample_world_height_interval
    }
    /// Independent native outer support that excludes impossible source
    /// heights without narrowing the captured CLOSED request or its incidence.
    pub fn source_world_z_support(&self) -> super::ProjectedBounds {
        self.inner.source_world_z_support
    }
    pub fn world_height_interval_mm(&self) -> [f64; 2] {
        self.inner.interval
    }
    pub fn sample_world_z_mm(&self) -> f64 {
        self.inner.sample
    }
    pub fn world_shape_error_policy_mm(&self) -> f64 {
        self.inner.native_policy
    }
    pub fn policy(&self) -> NativeCriticalCollarPolicy {
        self.inner.policy
    }
    /// Forward bound for the verified actual current source-M, or vacuous zero
    /// when there is no current positive material and the native sample is empty.
    /// A current leaf's stronger closed-product proof is retained verbatim.
    pub fn sample_to_closed_source_error_mm(&self) -> f64 {
        self.inner.sample_to_source
    }
    pub fn closed_source_to_material_error_mm(&self) -> f64 {
        self.inner.source_to_material
    }
    pub fn material_targets(&self) -> &[NativeCriticalCollarMaterialTarget] {
        &self.inner.targets
    }
    pub fn used_material_targets(
        &self,
    ) -> impl Iterator<Item = &NativeCriticalCollarMaterialTarget> {
        self.inner.targets.iter().filter(|target| target.used)
    }
    pub fn native_event_ports(&self) -> &[NativeCollarPort] {
        self.inner
            .native_ports
            .as_ref()
            .map_or(&[], |ports| ports.as_slice())
    }
    pub fn achieved_xy_cover_error_mm(&self) -> f64 {
        self.inner.xy_error
    }
    /// Actual-world XY motion on the original regular complement only.
    /// Event-face/coedge transitions remain separately owned native ports.
    pub fn world_regular_boundary_xy_speed_mm_per_mm(&self) -> Option<f64> {
        self.inner.world_regular_boundary_xy_speed
    }
    pub fn complex_sha256(&self) -> &[u8; 32] {
        &self.inner.complex_sha256
    }
    pub fn native_sample_sha256(&self) -> &[u8; 32] {
        &self.inner.native_sample_sha256
    }
    /// Native C plus the actual adjacent source-M footprint. This conservative
    /// contact envelope is not a topology or material-correspondence proof.
    pub fn occupied_xy_bounds_mm(&self) -> Box2 {
        self.inner.occupied_xy
    }
    pub fn source_occupied_xy_bounds_mm(&self) -> Box2 {
        self.inner.source_occupied_xy
    }
    /// C0 outer projection of the CLOSED source-M boundary: the raw original-F
    /// cover overbounds its retained complement, and every added actual-G
    /// ribbon/cone is covered directly. This asserts neither material
    /// membership nor the existence, count or continuation of any void.
    pub(super) fn closed_source_complement_xy_bounds_mm(&self) -> Box2 {
        self.inner.closed_source_complement_xy
    }
    pub fn projected_cover(&self) -> &[NativeCriticalCollarCell] {
        &self.inner.cells
    }
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + std::mem::size_of::<NativeCriticalCollarCapture>()
            + 2 * std::mem::size_of::<usize>()
            + self.inner.definition_id.allocated_bytes()
            + self.inner.targets.capacity()
                * std::mem::size_of::<NativeCriticalCollarMaterialTarget>()
            + self
                .inner
                .targets
                .iter()
                .map(|target| target.leaf.retained_bytes())
                .sum::<usize>()
            + self.inner.native_ports.as_ref().map_or(0, |ports| {
                2 * std::mem::size_of::<usize>()
                    + std::mem::size_of::<Vec<NativeCollarPort>>()
                    + ports.capacity() * std::mem::size_of::<NativeCollarPort>()
            })
            + self.inner.cells.capacity() * std::mem::size_of::<NativeCriticalCollarCell>()
    }
}

fn failure(code: GeometryErrorCode, message: impl AsRef<str>) -> GeometryError {
    GeometryError::new(code, message)
}
fn unresolved(message: impl AsRef<str>) -> GeometryError {
    failure(GeometryErrorCode::AnalysisIndeterminate, message)
}
fn cell_context(
    error: GeometryError,
    stage: &str,
    face: Option<usize>,
    uv: Box2,
    depth: u8,
) -> GeometryError {
    failure(
        error.code,
        format!(
            "native collar {stage}, face {face:?}, domain {uv:?}, depth {depth}: {}",
            error.message
        ),
    )
}

fn target_height_distance(target: &NativeCriticalCollarMaterialTarget, occupied: [f64; 2]) -> f64 {
    let target_interval = target.leaf.world_height_interval_mm();
    let below = Interval::point(target_interval[0])
        .sub(Interval::point(occupied[0]))
        .hi
        .max(0.0);
    let above = Interval::point(occupied[1])
        .sub(Interval::point(target_interval[1]))
        .hi
        .max(0.0);
    below.max(above)
}

/// Bound the source points represented by a boundary crossing, not just the
/// crossing itself. Join a source point to the actual sample-plane point in
/// its vertical column. Unless that column meets the filled sample, its first
/// exit crosses the original boundary. At an event face whose actual-world
/// outward normal has positive Z, this exit is upward: the source point is
/// below the crossing, hence below the event's conservative upper height.
/// Negative Z gives the dual lower bound. Keep the other slab endpoint:
/// interior material between that endpoint and the crossing still needs cover.
///
/// This partitions crossing shadows, NOT all occupancy at an event's XY.
/// Other components/crossings there are covered by the sample or other faces.
/// Closed face shadows include their original paired ports by closure; no
/// independent smooth-face normal is asserted at a nonsmooth coedge.
/// Only events recertified under the captured world pose may enter here.
fn material_facing_source_column(
    occupied: [f64; 2],
    event: Option<&NativeCollarFaceEvent>,
) -> [f64; 2] {
    match event {
        Some(event) if event.outward_height_sign > 0 => {
            [occupied[0], occupied[1].min(event.height[1])]
        }
        Some(event) if event.outward_height_sign < 0 => {
            [occupied[0].max(event.height[0]), occupied[1]]
        }
        _ => occupied,
    }
}
fn dot(a: [Interval; 3], b: [Interval; 3]) -> Interval {
    (0..3).fold(Interval::point(0.0), |s, i| s.add(a[i].mul(b[i])))
}
fn cross(a: [Interval; 3], b: [Interval; 3]) -> [Interval; 3] {
    [
        a[1].mul(b[2]).sub(a[2].mul(b[1])),
        a[2].mul(b[0]).sub(a[0].mul(b[2])),
        a[0].mul(b[1]).sub(a[1].mul(b[0])),
    ]
}
fn hypot(a: f64, b: f64) -> Result<f64, GeometryError> {
    let squared = Interval::point(a).square().add(Interval::point(b).square());
    Interval {
        lo: squared.lo.max(0.0),
        hi: squared.hi,
    }
    .sqrt()
    .map(|value| value.hi)
}

/// A genuine tangent field with dh(v)=1. The numerator is transformed through
/// the actual stored polynomial; a nonzero height derivative is indispensable.
/// Arithmetic overflow/conditioning leaves this optional witness unavailable.
fn world_xy_tangent_speed(
    tangent: [Interval; 3],
    height_derivative: Interval,
    matrix: Matrix,
) -> Option<f64> {
    if height_derivative.contains_zero()
        || !height_derivative.lo.is_finite()
        || !height_derivative.hi.is_finite()
    {
        return None;
    }
    let denominator = height_derivative.lo.abs().min(height_derivative.hi.abs());
    if denominator <= 0.0 {
        return None;
    }
    let x = dot(matrix[0], tangent).abs_upper();
    let y = dot(matrix[1], tangent).abs_upper();
    let numerator = hypot(x, y).ok()?;
    if numerator == 0.0 {
        return Some(0.0);
    }
    let speed = Interval::point(numerator)
        .div(Interval::point(denominator))
        .ok()?
        .hi;
    speed.is_finite().then_some(speed)
}

fn best_face_world_xy_speed(
    jet: &crate::parametric::SurfaceEnclosure,
    functional: super::HeightFunctional,
    matrix: Matrix,
) -> Option<f64> {
    let u = world_xy_tangent_speed(jet.du, functional.dot(jet.du), matrix);
    let v = world_xy_tangent_speed(jet.dv, functional.dot(jet.dv), matrix);
    match (u, v) {
        (Some(u), Some(v)) => Some(u.min(v)),
        (Some(u), None) => Some(u),
        (None, Some(v)) => Some(v),
        (None, None) => None,
    }
}
fn intervals(bounds: Box2) -> [Interval; 2] {
    bounds.map(|axis| Interval {
        lo: axis[0],
        hi: axis[1],
    })
}
fn expand(bounds: Box2, radius: f64) -> Box2 {
    bounds.map(|axis| {
        let value = Interval {
            lo: axis[0],
            hi: axis[1],
        }
        .add(Interval {
            lo: -radius,
            hi: radius,
        });
        [value.lo, value.hi]
    })
}

struct Capacity {
    limit: usize,
    fixed: usize,
}
impl Capacity {
    fn remaining(
        &self,
        cells: &Vec<NativeCriticalCollarCell>,
        extra: usize,
    ) -> Result<usize, GeometryError> {
        self.limit
            .checked_sub(
                self.fixed
                    .saturating_add(
                        cells
                            .capacity()
                            .saturating_mul(std::mem::size_of::<NativeCriticalCollarCell>()),
                    )
                    .saturating_add(extra),
            )
            .ok_or_else(|| {
                failure(
                    GeometryErrorCode::ResourceLimit,
                    "native collar coexistent capacity exhausted",
                )
            })
    }
    fn push(
        &self,
        cells: &mut Vec<NativeCriticalCollarCell>,
        cell: NativeCriticalCollarCell,
        temporary: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        if cells.len() == cells.capacity() {
            let additional = 64;
            let bytes = additional * std::mem::size_of::<NativeCriticalCollarCell>();
            self.remaining(cells, temporary.saturating_add(bytes))?;
            work.charge(bytes as u64, cancel)?;
            cells.try_reserve_exact(additional).map_err(|_| {
                failure(
                    GeometryErrorCode::ResourceLimit,
                    "native collar cover allocation refused",
                )
            })?;
        }
        cells.push(cell);
        Ok(())
    }
}

fn orientation(a: [f64; 2], b: [f64; 2], p: [Interval; 2]) -> Interval {
    super::planar::orientation(a, b, p)
}

/// Conservative exclusion of every segment from a rectangle. When a segment
/// touches a cell, uncertainty is retained and that cell is refined/covered.
fn boundary_disjoint(
    bounds: Box2,
    loops: &[SectionLoopMm],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    for ring in loops {
        for edge in ring.points_mm.windows(2) {
            work.charge(1, cancel)?;
            let a = [edge[0][0], edge[0][1]];
            let b = [edge[1][0], edge[1][1]];
            if (0..2).any(|axis| {
                a[axis].max(b[axis]) < bounds[axis][0] || a[axis].min(b[axis]) > bounds[axis][1]
            }) {
                continue;
            }
            let side = orientation(a, b, intervals(bounds));
            if side.lo > 0.0 || side.hi < 0.0 {
                continue;
            }
            return Ok(false);
        }
    }
    Ok(true)
}

/// Material membership away from every edge. Even-odd membership respects
/// multiple outers and holes without treating a hole as an enclosing material
/// polygon. An ambiguous ray crossing is never rounded into a membership fact.
fn material_at(
    p: [f64; 2],
    loops: &[SectionLoopMm],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<bool>, GeometryError> {
    super::planar::material_at(p, loops, super::planar::FillRule::EvenOdd, work, cancel)
}

/// Every returned bound applies to the entire rectangle. For a fixed closed
/// segment, distance is convex: its maximum on a rectangle is bounded by the
/// maximum at the four corners. Keep the segment fixed until after that maximum;
/// corner-wise distances to different material edges would fill protected holes.
/// A filled convex subset is a stronger distance witness than any one polygon
/// edge. Complete boundary exclusion and one unambiguous membership establish
/// that the entire inset rectangle lies in the CLOSED material, even for
/// nonconvex outers and protected holes. Coordinate clamping then maps EVERY
/// original rectangle point to that subset with the outward bounded distance;
/// membership at a few corners alone would not establish this implication.
fn material_distance_upper(
    bounds: Box2,
    loops: &[SectionLoopMm],
    maximum_distance_mm: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<f64, GeometryError> {
    work.charge(1, cancel)?;
    let center = bounds.map(|axis| axis[0] * 0.5 + axis[1] * 0.5);
    if boundary_disjoint(bounds, loops, work, cancel)?
        && material_at(center, loops, work, cancel)? == Some(true)
    {
        return Ok(0.0);
    }
    if maximum_distance_mm > 0.0 {
        // This is a witness search distance drawn from the existing policy,
        // not an acceptance tolerance. Its actual clamping distance is proved
        // below and must still satisfy the caller's unchanged numerical limit.
        let margin = maximum_distance_mm * 0.25;
        let inset: Box2 = std::array::from_fn(|axis| {
            [
                (bounds[axis][0] + margin)
                    .min(center[axis])
                    .max(bounds[axis][0]),
                (bounds[axis][1] - margin)
                    .max(center[axis])
                    .min(bounds[axis][1]),
            ]
        });
        if boundary_disjoint(inset, loops, work, cancel)?
            && material_at(center, loops, work, cancel)? == Some(true)
        {
            let displacement: [f64; 2] = std::array::from_fn(|axis| {
                Interval::point(inset[axis][0])
                    .sub(Interval::point(bounds[axis][0]))
                    .hi
                    .max(
                        Interval::point(bounds[axis][1])
                            .sub(Interval::point(inset[axis][1]))
                            .hi,
                    )
                    .max(0.0)
            });
            let distance = hypot(displacement[0], displacement[1])?;
            if distance <= maximum_distance_mm {
                return Ok(distance);
            }
        }
    }
    let mut best = f64::INFINITY;
    for ring in loops {
        for edge in ring.points_mm.windows(2) {
            work.charge(1, cancel)?;
            let a = [edge[0][0], edge[0][1]];
            let b = [edge[1][0], edge[1][1]];
            // Distance to the segment's AABB is a lower bound on distance
            // to that same closed segment. One rectangle corner already
            // suffices to exclude a segment from improving the current
            // whole-rectangle witness; retain interval subtraction rounding.
            if (0..2).any(|axis| {
                Interval::point(a[axis].min(b[axis]))
                    .sub(Interval::point(bounds[axis][0]))
                    .lo
                    .max(
                        Interval::point(bounds[axis][1])
                            .sub(Interval::point(a[axis].max(b[axis])))
                            .lo,
                    )
                    > best
            }) {
                continue;
            }
            let mut maximum = 0.0_f64;
            for x in bounds[0] {
                for y in bounds[1] {
                    work.charge(1, cancel)?;
                    maximum = maximum.max(
                        super::planar::point_segment_distance(
                            [Interval::point(x), Interval::point(y)],
                            a,
                            b,
                        )?
                        .hi,
                    );
                }
            }
            best = best.min(maximum);
            if maximum <= maximum_distance_mm {
                // Any one fixed segment is a complete convex distance
                // witness. A smaller bound is unnecessary for acceptance.
                return Ok(maximum);
            }
        }
    }
    Ok(best)
}

/// Retain the complete necessary CLOSED height preimage, then enclose its
/// continuous source image and one-sided first derivatives. This grants no
/// C1, Hessian or regularity authority to rectangle coverage or subdivision.
fn clip_height_first_box(
    surface: &dyn crate::parametric::SurfaceEvaluator,
    uv: Box2,
    functional: HeightFunctional,
    heights: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<(Box2, SurfaceFirstEnclosure)>, GeometryError> {
    let Some(uv) = clip_height_parameter_box(surface, uv, functional, heights, work, cancel)?
    else {
        return Ok(None);
    };
    let jet = surface_piecewise_first_enclosure(surface, uv, work, cancel)?;
    Ok(Some((uv, jet)))
}

fn split(bounds: Box2, depth: u8) -> Result<[Box2; 2], GeometryError> {
    let axis = usize::from(bounds[1][1] - bounds[1][0] > bounds[0][1] - bounds[0][0]);
    split_axis(bounds, depth, axis)
}
fn split_axis(bounds: Box2, depth: u8, axis: usize) -> Result<[Box2; 2], GeometryError> {
    let mid = bounds[axis][0] * 0.5 + bounds[axis][1] * 0.5;
    if depth >= MAX_DEPTH || mid <= bounds[axis][0] || mid >= bounds[axis][1] {
        return Err(unresolved(
            format!(
                "native collar whole-cell material coverage cannot be resolved at binary64 parameter conditioning: reason={}, depth={depth}/{MAX_DEPTH}, axis={axis}, midpoint={mid:.17e}, axis_bounds={:?}",
                if depth >= MAX_DEPTH { "depth_limit" } else { "selected_axis_collapse" },
                bounds[axis],
            ),
        ));
    }
    let mut children = [bounds; 2];
    children[0][axis][1] = mid;
    children[1][axis][0] = mid;
    Ok(children)
}

fn spatial_split(
    uv: Box2,
    depth: u8,
    jet: &SurfaceFirstEnclosure,
    matrix: Matrix,
) -> Result<[Box2; 2], GeometryError> {
    let motion = [jet.du, jet.dv].map(|tangent| {
        matrix
            .into_iter()
            .take(2)
            .map(|row| dot(row, tangent))
            .map(Interval::abs_upper)
            .fold(0.0_f64, f64::max)
    });
    let scores: [f64; 2] = std::array::from_fn(|axis| {
        Interval::point(uv[axis][1])
            .sub(Interval::point(uv[axis][0]))
            .mul(Interval::point(motion[axis]))
            .hi
    });
    // Only projected XY coverage is unresolved here: the CLOSED height column
    // and its target distance were already checked independently. Splitting a
    // purely vertical chart direction cannot improve that remaining obligation.
    // Parameter-domain scale is not spatial conditioning. Independent NURBS
    // domains receive the same geometric split as their affine reparameterization.
    split_axis(uv, depth, usize::from(scores[1] > scores[0]))
}

/// The actual transformed native sample plane must cut every vertical column
/// in the source's projected bounding rectangle *inside* the current CLOSED
/// slab. This establishes the vertical crossing argument even for accepted
/// nonorthogonal quaternion polynomials. No midpoint plane equality is used.
pub(super) fn sample_plane_height_bounds(
    definition: &Definition,
    analysis: &SlabAnalysis,
    matrix: Matrix,
    pose: RigidPoseMm,
) -> Result<Interval, GeometryError> {
    let plane = analysis.source_sample_plane();
    let columns: [[Interval; 3]; 3] =
        std::array::from_fn(|column| std::array::from_fn(|row| matrix[row][column]));
    let cofactors = [
        cross(columns[1], columns[2]),
        cross(columns[2], columns[0]),
        cross(columns[0], columns[1]),
    ];
    // det(R) R^-T n: the actual accepted polynomial need not be orthogonal.
    // No source request frame axis is substituted for the native query plane.
    let normal: [Interval; 3] = std::array::from_fn(|axis| {
        (0..3).fold(Interval::point(0.0), |sum, column| {
            sum.add(cofactors[column][axis].mul(Interval::point(plane.normal[column])))
        })
    });
    if normal[2].contains_zero() {
        return Err(failure(
            GeometryErrorCode::ConditioningFailure,
            "native collar actual sample plane is not a resolved vertical graph",
        ));
    }
    let origin =
        super::placed::transformed_position(matrix, pose, plane.origin_mm.map(Interval::point))?;
    let bounds = definition.bounds_mm();
    let world = super::placed::transformed_position(
        matrix,
        pose,
        std::array::from_fn(|axis| Interval {
            lo: bounds.min[axis],
            hi: bounds.max[axis],
        }),
    )?;
    let displacement = normal[0]
        .mul(world[0].sub(origin[0]))
        .add(normal[1].mul(world[1].sub(origin[1])))
        .div(normal[2])?;
    Ok(origin[2].sub(displacement))
}

fn validate_material(
    loops: &[SectionLoopMm],
    z: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    if loops.is_empty() {
        return Err(unresolved(
            "native collar requires actual positive adjacent material",
        ));
    }
    for ring in loops {
        work.charge(1, cancel)?;
        if ring.points_mm.len() < 4
            || ring.points_mm.first() != ring.points_mm.last()
            || ring
                .points_mm
                .iter()
                .any(|p| p.iter().any(|v| !v.is_finite()) || p[2] != z)
        {
            return Err(failure(
                GeometryErrorCode::InvalidGeometry,
                "native collar adjacent material must be the actual closed finite XY source-M cycles at its captured sample",
            ));
        }
    }
    Ok(())
}

/// Cover all CLOSED original-source occupancy, rather than only its sampled
/// contour. For each vertical column, material either meets the actual native
/// sample plane, or the segment to that plane crosses an original boundary in
/// the slab. Consequently Cxy is enclosed by the filled native sample cover
/// plus the height-clipped original trimmed face covers below. This construction
/// also covers positive-width irrational event bands and blind-pocket floors.
/// Distances cover the complete source shadows of those crossings. Certified
/// actual-world event orientation bounds only their non-material-facing end;
/// neither the event's boundary height alone nor a source-space event suffices.
/// Native critical/void incidence is independently mandatory.
pub fn certify_native_critical_collar(
    definition: &Definition,
    analysis: &SlabAnalysis,
    inputs: &[NativeCriticalCollarTarget<'_>],
    policy: NativeCriticalCollarPolicy,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<NativeCriticalCollar, GeometryError> {
    let limit = (policy.byte_limit as usize).min(MAX_BYTES);
    let prepared = prepare_solid(definition, limit, work, cancel)?;
    let remaining = limit.checked_sub(prepared.retained_bytes()).ok_or_else(|| {
        failure(GeometryErrorCode::ResourceLimit, "native collar source exceeds coexistent capacity")
    })?;
    let policy = NativeCriticalCollarPolicy { byte_limit: remaining as u32, ..policy };
    certify_native_critical_collar_using(
        prepared.source_view()?,
        analysis,
        inputs,
        policy,
        work,
        cancel,
    )
}

impl PreparedSolid<'_> {
    /// Certify the actual collar while borrowing this source's original trim proofs.
    /// `policy.byte_limit` excludes every retained source and coexisting buffer.
    pub fn certify_native_critical_collar(
        &self,
        analysis: &SlabAnalysis,
        inputs: &[NativeCriticalCollarTarget<'_>],
        policy: NativeCriticalCollarPolicy,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<NativeCriticalCollar, GeometryError> {
        certify_native_critical_collar_using(
            self.source_view()?,
            analysis,
            inputs,
            policy,
            work,
            cancel,
        )
    }
}

fn certify_native_critical_collar_using(
    source: MaterialSourceView<'_, '_>,
    analysis: &SlabAnalysis,
    inputs: &[NativeCriticalCollarTarget<'_>],
    policy: NativeCriticalCollarPolicy,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<NativeCriticalCollar, GeometryError> {
    let definition = source.definition();
    work.charge(1, cancel)?;
    let pose = analysis.placed_pose().ok_or_else(|| {
        failure(
            GeometryErrorCode::InvalidGeometry,
            "native collar requires an immutable placed source capture",
        )
    })?;
    let interval = analysis
        .world_height_interval_mm()
        .ok_or_else(|| unresolved("native collar lacks CLOSED world extent"))?;
    let sample = analysis
        .sample_world_z_mm()
        .ok_or_else(|| unresolved("native collar lacks actual source sample"))?;
    let native_policy = analysis
        .world_shape_error_policy_mm()
        .ok_or_else(|| unresolved("native collar lacks source policy"))?;
    if !analysis.matches_original_material(definition)
        || analysis.section().definition_id() != definition.id()
        || !analysis.section().is_native_intersection()
        || !(interval[0] < sample && sample < interval[1])
        || inputs.is_empty()
        || inputs.len() > 3
        || !policy.maximum_xy_cover_error_mm.is_finite()
        || policy.maximum_xy_cover_error_mm <= 0.0
        || !policy.maximum_distance_mm.is_finite()
        || policy.maximum_distance_mm <= 0.0
    {
        return Err(failure(
            GeometryErrorCode::InvalidGeometry,
            "native collar source/sample/material targets/policy capture mismatch",
        ));
    }
    let native_sample_sha256 = super::joint::native_sample_hash(analysis, work, cancel)?;
    let native_ports = analysis
        .collar_complex
        .as_ref()
        .map(|complex| Arc::clone(&complex.ports));
    let world_regular_boundary_xy_speed = analysis
        .collar_complex
        .as_ref()
        .and_then(NativeCollarComplex::world_regular_boundary_xy_speed_mm_per_mm);
    let complex_sha256 = if let Some(complex) = analysis.collar_complex.as_ref() {
        if complex.definition_id() != definition.id()
            || complex.material_realization_identity() != analysis.material_realization_identity()
        {
            return Err(unresolved(
                "native collar continuum complex has another original source",
            ));
        }
        let capture=complex.world_capture.ok_or_else(||unresolved(
            "native collar requires actual-world recertification of its original event-face complex"))?;
        if capture.pose != pose || capture.interval != interval || capture.sample != sample {
            return Err(failure(
                GeometryErrorCode::InvalidGeometry,
                "native collar actual-world complex capture mismatch",
            ));
        }
        complex.native_complex_sha256()
    } else {
        let complex = analysis.critical.as_ref().ok_or_else(|| unresolved(
            "native collar metric enclosure cannot replace missing original critical/face/void incidence"))?;
        if complex.definition_id() != definition.id()
            || complex.material_realization_identity() != analysis.material_realization_identity()
        {
            return Err(unresolved(
                "native collar critical complex has another original source",
            ));
        }
        let mut hash = Sha256::new();
        hash.update(b"spiling-existing-native-critical-complex-1\0");
        hash.update(definition.id().as_str().as_bytes());
        hash.update(analysis.material_realization_identity().identity_bytes());
        let request = complex.request();
        for value in request
            .height_interval_mm
            .into_iter()
            .chain([request.sample_height_mm, request.native_shape_error_mm])
            .chain(request.frame.origin_mm)
            .chain(request.frame.x_axis)
            .chain(request.frame.y_axis)
            .chain(request.frame.z_axis)
        {
            hash.update(value.to_bits().to_le_bytes());
        }
        let (faces, coedges) = complex.covered_cells();
        hash.update(faces.to_le_bytes());
        hash.update(coedges.to_le_bytes());
        for event in &complex.event_neighborhoods {
            work.charge(1, cancel)?;
            for value in event.height_bracket_mm() {
                hash.update(value.to_bits().to_le_bytes());
            }
            hash.update((event.source_face_ordinal().unwrap_or(usize::MAX) as u64).to_le_bytes());
        }
        hash.finalize().into()
    };
    let stack_bytes = (MAX_DEPTH as usize + 2) * std::mem::size_of::<(Box2, u8)>();
    let target_bytes = inputs.len() * std::mem::size_of::<NativeCriticalCollarMaterialTarget>()
        + inputs
            .iter()
            .map(|input| input.leaf.retained_bytes())
            .sum::<usize>();
    let port_bytes = native_ports.as_ref().map_or(0, |ports| {
        2 * std::mem::size_of::<usize>()
            + std::mem::size_of::<Vec<NativeCollarPort>>()
            + ports.capacity() * std::mem::size_of::<NativeCollarPort>()
    });
    let fixed = std::mem::size_of::<NativeCriticalCollar>()
        + std::mem::size_of::<NativeCriticalCollarCapture>()
        + 2 * std::mem::size_of::<usize>()
        + definition.id().allocated_bytes()
        + target_bytes
        + port_bytes
        + stack_bytes
        + SURFACE_COVER_SCRATCH_BYTES;
    let capacity = Capacity {
        limit: (policy.byte_limit as usize).min(MAX_BYTES),
        fixed,
    };
    let mut cells = Vec::new();
    capacity.remaining(&cells, 0)?;
    work.charge(fixed as u64, cancel)?;
    let mut targets = Vec::new();
    targets.try_reserve_exact(inputs.len()).map_err(|_| {
        failure(
            GeometryErrorCode::ResourceLimit,
            "native collar material-target allocation refused",
        )
    })?;
    for input in inputs {
        work.charge(1, cancel)?;
        let leaf = input.leaf;
        if !leaf.matches_original_material(definition)
            || leaf.material_realization_identity() != analysis.material_realization_identity()
            || leaf.pose() != pose
            || !leaf.is_positive()
            || leaf.occurrence_id() != inputs[0].leaf.occurrence_id()
        {
            return Err(failure(
                GeometryErrorCode::InvalidGeometry,
                "native collar target is not this original occurrence's actual positive leaf",
            ));
        }
        let target_interval = leaf.world_height_interval_mm();
        let side = if target_interval == interval && leaf.sample_world_z_mm() == sample {
            if leaf.native_sample_sha256() != &native_sample_sha256
                || leaf.native_shape_error_policy_mm() != native_policy
            {
                return Err(failure(
                    GeometryErrorCode::InvalidGeometry,
                    "native collar current material target has a different actual native sample",
                ));
            }
            NativeCriticalCollarTargetSide::Current
        } else if target_interval[1] == interval[0] {
            NativeCriticalCollarTargetSide::Lower
        } else if target_interval[0] == interval[1] {
            NativeCriticalCollarTargetSide::Upper
        } else {
            return Err(failure(
                GeometryErrorCode::InvalidGeometry,
                "native collar target is neither actual current nor immediately adjacent M",
            ));
        };
        if targets
            .iter()
            .any(|target: &NativeCriticalCollarMaterialTarget| target.side == side)
        {
            return Err(failure(
                GeometryErrorCode::InvalidGeometry,
                "native collar has duplicate alternative leaves for one actual stream position",
            ));
        }
        validate_material(
            input.actual_material,
            leaf.sample_world_z_mm(),
            work,
            cancel,
        )?;
        super::joint::verify_source_leaf_candidate(leaf, input.actual_material, work, cancel)?;
        targets.push(NativeCriticalCollarMaterialTarget {
            leaf: leaf.clone(),
            side,
            used: false,
        });
    }
    let source_world_z_support = super::placed::placed_bounds_with_scratch_limit(
        source,
        pose,
        [0.0, 0.0, 1.0],
        super::BoundsPolicy {
            maximum_endpoint_uncertainty_mm: policy.maximum_distance_mm.min(native_policy) * 0.25,
        },
        capacity.remaining(&cells, 0)?,
        work,
        cancel,
    )
    .map_err(|error| {
        failure(
            error.code,
            format!(
                "native collar whole-source placed Z support, CLOSED {interval:?}, sample {sample}: {}",
                error.message
            ),
        )
    })?;
    // This is an independent bound on the ENTIRE posed original source, not a
    // boundary-cell height or an event midpoint. It excludes empty portions of
    // a scheduled column while preserving the original CLOSED request capture.
    let occupied_interval = [
        interval[0].max(source_world_z_support.minimum[0]),
        interval[1].min(source_world_z_support.maximum[1]),
    ];
    if occupied_interval[0] > occupied_interval[1] {
        return Err(unresolved(
            "native collar whole-source support excludes the captured critical slab",
        ));
    }
    // Eligibility is per represented shadow below. Requiring one target to
    // cover the entire occupied interval here would reject a valid union of
    // separately owned event shadows, even with an empty filled sample.
    let mut stack = Vec::new();
    stack
        .try_reserve_exact(MAX_DEPTH as usize + 2)
        .map_err(|_| {
            failure(
                GeometryErrorCode::ResourceLimit,
                "native collar subdivision stack allocation refused",
            )
        })?;
    let matrix = super::placed::rotation(pose)?;
    let sample_heights = sample_plane_height_bounds(definition, analysis, matrix, pose)?;
    if sample_heights.lo <= interval[0] || sample_heights.hi >= interval[1] {
        return Err(failure(
            GeometryErrorCode::ConditioningFailure,
            "native collar actual sample-plane graph is not strictly inside the CLOSED slab over the entire source projection",
        ));
    }
    let section = analysis.section();
    let approximation = Interval::point(section.achieved_chord_error_mm())
        .add(Interval::point(section.achieved_plane_error_mm()))
        .add(Interval::point(section.boolean_tolerance_mm()))
        .hi;
    if !approximation.is_finite() || approximation < 0.0 {
        return Err(failure(
            GeometryErrorCode::ConditioningFailure,
            "native collar filled-section approximation is not finite",
        ));
    }
    let sample_to_source = if let Some(target) = targets
        .iter_mut()
        .find(|target| target.side == NativeCriticalCollarTargetSide::Current)
    {
        // This sealed receipt proves the actual polygons verified above, not
        // merely their unquantized native section. It can be genuinely stronger
        // than the generic one-sided native-S construction (e.g. a box prism).
        target.used = true;
        target.leaf.directed_sample_to_closed_source_error_mm()
    } else if section.loops().is_empty() {
        // The forward distance of an empty filled sample is vacuous even when
        // the CLOSED slab contains an event-covered component elsewhere.
        0.0
    } else {
        return Err(failure(
            GeometryErrorCode::InvalidGeometry,
            "native collar positive native sample requires its actual current source-M leaf",
        ));
    };
    let mut sample_bounds = [[f64::INFINITY, f64::NEG_INFINITY]; 2];
    for ring in section.loops() {
        for point in &ring.points_mm {
            work.charge(1, cancel)?;
            for axis in 0..2 {
                sample_bounds[axis][0] = sample_bounds[axis][0].min(point[axis]);
                sample_bounds[axis][1] = sample_bounds[axis][1].max(point[axis]);
            }
        }
    }
    let mut xy_error: f64 = 0.0;
    let mut source_to_material: f64 = 0.0;
    if sample_bounds[0][0].is_finite() {
        // Reuse the current leaf's independently sealed FILLED-region
        // correspondence before searching rectangle witnesses. The vertical
        // crossing argument needs Sxy, not every empty point of bbox(Sxy).
        // Holes and nonconvex components remain excluded by that authority;
        // births/deaths away from S still require every face/port cover below.
        let mut sample_cover = choose_material_target(
            MaterialTargetQuery {
                coverage: MaterialCoverage::FilledNativeSample {
                    approximation_mm: approximation,
                },
                inputs,
                targets: &targets,
                interval: occupied_interval,
                event: None,
                source_sample_band: [sample_heights.lo, sample_heights.hi],
                policy,
                native_policy,
            },
            work,
            cancel,
        )?;
        stack.push((sample_bounds, 0));
        while let Some((bounds, depth)) = stack.pop() {
            work.charge(1, cancel)?;
            let center = bounds.map(|axis| axis[0] * 0.5 + axis[1] * 0.5);
            if sample_cover.is_none()
                && boundary_disjoint(bounds, section.loops(), work, cancel)?
                && material_at(center, section.loops(), work, cancel)? == Some(false)
            {
                continue;
            }
            let outer = expand(bounds, approximation);
            let target = if depth == 0 && sample_cover.is_some() {
                sample_cover.take()
            } else {
                // A narrower collar policy may need a tighter geometric
                // witness than the leaf's conservative correspondence bound.
                choose_material_target(
                    MaterialTargetQuery {
                        coverage: MaterialCoverage::Rectangle(outer),
                        inputs,
                        targets: &targets,
                        interval: occupied_interval,
                        event: None,
                        source_sample_band: [sample_heights.lo, sample_heights.hi],
                        policy,
                        native_policy,
                    },
                    work,
                    cancel,
                )
                .map_err(|error| {
                    cell_context(error, "sample material coverage", None, bounds, depth)
                })?
            };
            if let Some((index, dx, distance)) = target {
                xy_error = xy_error.max(dx);
                source_to_material = source_to_material.max(distance);
                targets[index].used = true;
                capacity.push(
                    &mut cells,
                    NativeCriticalCollarCell {
                        source_face_ordinal: None,
                        source_uv: None,
                        whole_original_face: false,
                        world_xy: outer,
                        world_height: occupied_interval,
                        material_target_index: index,
                        native_port_parameter: None,
                        xy_distance_mm: dx,
                        material_distance_mm: distance,
                    },
                    0,
                    work,
                    cancel,
                )?;
            } else {
                for child in split(bounds, depth)? {
                    stack.push((child, depth + 1));
                }
            }
        }
    }
    // Freeze whole filled face images before using their original opposed ports
    // as metric anchors. Source face order does not establish proof order.
    for (filled_only, face_index, face) in [true, false].into_iter().flat_map(|filled_only| {
        definition.face_ids().iter().copied().enumerate()
            .map(move |(index, face)| (filled_only, index, face))
    }) {
        if !filled_only {
            let mut covered = false;
            for cell in &cells {
                work.charge(1, cancel)?;
                covered |= cell.whole_original_face && cell.source_face_ordinal == Some(face_index);
            }
            if covered {
                continue;
            }
        }
        work.charge(1, cancel)?;
        let event = analysis
            .collar_complex
            .as_ref()
            .and_then(|complex| complex.events.iter().find(|event| event.face == face_index));
        let source_column = material_facing_source_column(occupied_interval, event);
        let mut port_count = 0_usize;
        if !filled_only && let Some(ports) = &native_ports {
            for port in ports.iter() {
                work.charge(1, cancel)?;
                if port.neighbor_face == face_index {
                    port_count += 1;
                }
            }
        }
        // The immutable source owner retains the trim domain; only the
        // coexistent port-image inventory consumes additional query capacity.
        let port_storage = port_image_storage(port_count)?;
        let domain = source
            .face_domain(
                face_index,
                capacity.remaining(&cells, port_storage)?,
                work,
                cancel,
            )
            .map_err(|error| {
                failure(
                    error.code,
                    format!(
                        "native collar original face {face_index} trim preparation: {}",
                        error.message
                    ),
                )
            })?;
        // A whole coefficient image avoids repeated refinement when its
        // conservative rectangle satisfies the complete material-distance bound.
        // This is optional metric authority only; the original event/void
        // complex and material-facing CLOSED shadow remain unchanged.
        let filled = if filled_only {
            try_filled_face_cover(
                FilledFaceCoverQuery {
                    definition,
                    face,
                    face_index,
                    domain: &domain,
                    matrix,
                    pose,
                    inputs,
                    targets: &targets,
                    source_column,
                    event,
                    sample_band: [sample_heights.lo, sample_heights.hi],
                    policy,
                    native_policy,
                },
                work,
                cancel,
            )
        } else {
            try_port_face_cover(
                PortFaceCoverQuery {
                    face,
                    face_index,
                    domain: &domain,
                    matrix,
                    pose,
                    ports: native_ports.as_deref().map(Vec::as_slice),
                    cells: &cells,
                    targets: &targets,
                    source_column,
                    interval,
                    policy,
                    native_policy,
                    count: port_count,
                    byte_limit: capacity.remaining(&cells, 0)?,
                },
                work,
                cancel,
            )
        };
        match filled {
            Ok(Some(cell)) => {
                targets[cell.material_target_index].used = true;
                xy_error = xy_error.max(cell.xy_distance_mm);
                source_to_material = source_to_material.max(cell.material_distance_mm);
                capacity.push(&mut cells, cell, 0, work, cancel)?;
                continue;
            }
            Err(error)
                if matches!(
                    error.code,
                    GeometryErrorCode::ResourceLimit | GeometryErrorCode::Cancelled
                ) =>
            {
                return Err(error);
            }
            // A missing affine projection, uncertain cycle map/topology or a
            // policy miss retains the existing complete original-cell proof.
            _ => {}
        }
        if filled_only {
            continue;
        }
        let covered = (|surface| -> Result<(), GeometryError> {
            let uv = super::slab::trim_parameter_box(&domain)?;
            let mut spans =
                surface_parameter_boxes(surface, uv, work, cancel).map_err(|error| {
                    cell_context(error, "source parameter spans", Some(face_index), uv, 0)
                })?;
            while let Some((uv, depth)) = stack.pop().or_else(|| spans.next().map(|uv| (uv, 0))) {
                work.charge(1, cancel)?;
                let clipped = clip_height_first_box(
                    surface,
                    uv,
                    HeightFunctional {
                        direction: matrix[2],
                        offset: Interval::point(pose.translation_mm[2]),
                    },
                    interval,
                    work,
                    cancel,
                );
                let Some((uv, jet)) = clipped.map_err(|error| {
                    cell_context(error, "CLOSED height clipping", Some(face_index), uv, depth)
                })?
                else {
                    continue;
                };
                let world = super::placed::transformed_position(matrix, pose, jet.position)?;
                if world[2].hi < interval[0] || world[2].lo > interval[1] {
                    continue;
                }
                // A vertical source-to-sample segment not meeting filled S
                // first exits through the original boundary. Strict positive
                // outward Z puts its source point below this crossing; strict
                // negative Z puts it above. Enclose EVERY one-sided first jet
                // and transform the tangents before taking their cross product:
                // an accepted pose need not be exactly orthogonal. This is a
                // whole-cell shadow bound, not an event or regular-speed proof.
                // Uncertain/degenerate signs retain the full existing shadow.
                // All incident original-face shadows remain covered by closure;
                // a coedge is never assigned an independent smooth normal.
                work.charge(64, cancel)?;
                let du = [dot(matrix[0], jet.du), dot(matrix[1], jet.du)];
                let dv = [dot(matrix[0], jet.dv), dot(matrix[1], jet.dv)];
                let normal_z = du[0].mul(dv[1]).sub(du[1].mul(dv[0]));
                let normal_z = if domain.orientation() {
                    normal_z
                } else {
                    normal_z.neg()
                };
                let source_column = if normal_z.lo > 0.0 {
                    [source_column[0], source_column[1].min(world[2].hi)]
                } else if normal_z.hi < 0.0 {
                    [source_column[0].max(world[2].lo), source_column[1]]
                } else {
                    source_column
                };
                if source_column[0] > source_column[1] {
                    continue;
                }
                let bounds = [[world[0].lo, world[0].hi], [world[1].lo, world[1].hi]];
                let target = choose_material_target(
                    MaterialTargetQuery {
                        coverage: MaterialCoverage::Rectangle(bounds),
                        inputs,
                        targets: &targets,
                        interval: source_column,
                        event,
                        source_sample_band: [sample_heights.lo, sample_heights.hi],
                        policy,
                        native_policy,
                    },
                    work,
                    cancel,
                )
                .map_err(|error| {
                    cell_context(error, "face material coverage", Some(face_index), uv, depth)
                });
                let mut target = match target {
                    Err(error)
                        if matches!(
                            error.code,
                            GeometryErrorCode::ResourceLimit | GeometryErrorCode::Cancelled
                        ) =>
                    {
                        return Err(error);
                    }
                    other => other,
                };
                if matches!(&target, Ok(None)) {
                    let affine = try_surface_affine_image(
                        SurfaceAffineImageQuery {
                            surface,
                            uv,
                            matrix,
                            pose,
                            chart_error_mm: domain.source_chart_error_mm(),
                            maximum_error_mm: policy.maximum_xy_cover_error_mm,
                        },
                        work,
                        cancel,
                    );
                    match affine {
                        Ok(Some(image)) => {
                            let affine_target = choose_material_target(
                                MaterialTargetQuery {
                                    coverage: MaterialCoverage::AffineImage(&image),
                                    inputs,
                                    targets: &targets,
                                    interval: source_column,
                                    event,
                                    source_sample_band: [sample_heights.lo, sample_heights.hi],
                                    policy,
                                    native_policy,
                                },
                                work,
                                cancel,
                            );
                            match affine_target {
                                Ok(result) => target = Ok(result),
                                Err(error)
                                    if matches!(
                                        error.code,
                                        GeometryErrorCode::ResourceLimit
                                            | GeometryErrorCode::Cancelled
                                    ) =>
                                {
                                    return Err(error);
                                }
                                Err(_) => {}
                            }
                        }
                        Err(error)
                            if matches!(
                                error.code,
                                GeometryErrorCode::ResourceLimit | GeometryErrorCode::Cancelled
                            ) =>
                        {
                            return Err(error);
                        }
                        // Unqualified C0 spans and unavailable Taylor images
                        // retain the original complete-image/trim refinement.
                        _ => {}
                    }
                }
                match target {
                    Ok(Some((index, dx, distance))) => {
                        xy_error = xy_error.max(dx);
                        source_to_material = source_to_material.max(distance);
                        targets[index].used = true;
                        capacity.push(&mut cells, NativeCriticalCollarCell {
                            source_face_ordinal: Some(face_index),
                            source_uv: Some(uv),
                            whole_original_face: false,
                            world_xy: bounds,
                            world_height: source_column,
                            material_target_index: index,
                            native_port_parameter: None,
                            xy_distance_mm: dx,
                            material_distance_mm: distance,
                        }, 0, work, cancel)?;
                    }
                    other => {
                        // A whole-image material cover already bounds every
                        // actual trimmed source point, including boundary cells.
                        // Only unresolved covers need the original trim census.
                        // Resource/cancellation failures never become exclusions.
                        // The retained owner already admitted the immutable
                        // domain; winding queries do not resize or clone it.
                        let classification = domain.classify(uv, work, cancel).map_err(|error| {
                            cell_context(error, "trim classification", Some(face_index), uv, depth)
                        });
                        if matches!(classification?, TrimClassification::Outside) {
                            continue;
                        }
                        let _ = other?;
                        for child in spatial_split(uv, depth, &jet, matrix).map_err(|error| {
                            failure(
                                error.code,
                                format!(
                                    "native collar original face {face_index}, UV {uv:?}: {}",
                                    error.message
                                ),
                            )
                        })? {
                            stack.push((child, depth + 1));
                        }
                    }
                }
            }
            Ok(())
        })(domain.surface_evaluator());
        covered
        .map_err(|error| {
            failure(
                error.code,
                format!(
                    "native collar original face {face_index}, CLOSED {interval:?}, source-occupied {occupied_interval:?}, sample {sample}, event {:?}, cumulative work {}: {}",
                    event.map(|event| (event.height, event.outward_height_sign)),
                    work.consumed(),
                    error.message
                ),
            )
        })?;
    }
    // Retain actual native event-port coverage. Each entire prepared coedge
    // interval is enclosed and assigned a genuine material-facing used leaf;
    // neither the event SHA nor sampled overlap substitutes for this obligation.
    if let Some(ports) = &native_ports {
        for (port_index, port) in ports.iter().enumerate() {
            work.charge(port.event_face as u64 + 1, cancel)?;
            let (edge, _) = native_coedge(
                definition, port.event_face, port.event_wire, port.event_coedge,
            )?;
            if edge.edge != port.edge_id {
                return Err(unresolved(
                    "native collar port original native edge identity changed",
                ));
            }
            let domain = source.face_domain(port.event_face, capacity.remaining(&cells, 0)?, work, cancel)?;
            let mut bound_use = None;
            for (index, use_) in domain.coedges().iter().enumerate() {
                work.charge(1, cancel)?;
                let identity = use_.identity();
                if identity.wire_index == port.event_wire && identity.coedge_index == port.event_coedge {
                    bound_use = Some(index);
                    if identity.edgeuse != port.event_edgeuse || identity.edge_id != port.edge_id {
                        return Err(unresolved("native collar event port changed its exact original edgeuse"));
                    }
                    break;
                }
            }
            let bound_use = bound_use.ok_or_else(|| unresolved("native collar event port lost its original occurrence"))?;
            let source_range = domain.original_coedge_correspondence(bound_use)?.spatial_range();
            let event = NativeCollarFaceEvent {
                face: port.event_face,
                height: port.height,
                outward_height_sign: port.outward_height_sign,
            };
            let source_column = material_facing_source_column(occupied_interval, Some(&event));
            let mut whole_face = None;
            for cell in &cells {
                work.charge(1, cancel)?;
                if cell.whole_original_face
                    && cell.source_face_ordinal == Some(port.event_face)
                    && cell.world_height[0] <= source_column[0]
                    && source_column[1] <= cell.world_height[1]
                {
                    whole_face = Some((
                        cell.world_xy,
                        cell.material_target_index,
                        cell.xy_distance_mm,
                        cell.material_distance_mm,
                    ));
                    break;
                }
            }
            if let Some((bounds, index, dx, distance)) = whole_face {
                // An original face's CLOSED trim includes every original
                // coedge. Reuse its complete filled-image metric proof, not a
                // rectangle's membership, and retain the exact source range.
                capacity.push(
                    &mut cells,
                    NativeCriticalCollarCell {
                        source_face_ordinal: Some(port.event_face),
                        source_uv: None,
                        whole_original_face: false,
                        world_xy: bounds,
                        world_height: source_column,
                        material_target_index: index,
                        native_port_parameter: Some((port_index, source_range)),
                        xy_distance_mm: dx,
                        material_distance_mm: distance,
                    },
                    0,
                    work,
                    cancel,
                )?;
                continue;
            }
            domain.with_original_spatial_curve(
                bound_use, capacity.remaining(&cells, 0)?, work, cancel, |curve, work, cancel| {
                for span in crate::parametric::curve_parameter_spans(curve, source_range, work, cancel)? {
                    stack.push(([[span[0], span[1]], [0.0, 0.0]], 0));
                    while let Some((cell, depth)) = stack.pop() {
                        work.charge(1, cancel)?;
                        let jet = crate::parametric::curve_enclosure(curve, (cell[0][0], cell[0][1]), work, cancel)
                            .map_err(|error| {
                                cell_context(
                                    error,
                                    "original event-port jet",
                                    Some(port.event_face),
                                    cell,
                                    depth,
                                )
                            })?;
                        let world =
                            super::placed::transformed_position(matrix, pose, jet.position)?;
                        if world[2].hi < interval[0] || world[2].lo > interval[1] {
                            continue;
                        }
                        let bounds = [[world[0].lo, world[0].hi], [world[1].lo, world[1].hi]];
                        if let Some((index, dx, distance)) = choose_material_target(
                            MaterialTargetQuery {
                                coverage: MaterialCoverage::Rectangle(bounds),
                                inputs,
                                targets: &targets,
                                interval: source_column,
                                event: Some(&event),
                                source_sample_band: [sample_heights.lo, sample_heights.hi],
                                policy,
                                native_policy,
                            },
                            work,
                            cancel,
                        )
                        .map_err(|error| {
                            cell_context(
                                error,
                                "original event-port material coverage",
                                Some(port.event_face),
                                cell,
                                depth,
                            )
                        })? {
                            targets[index].used = true;
                            xy_error = xy_error.max(dx);
                            source_to_material = source_to_material.max(distance);
                            capacity.push(
                                &mut cells,
                                NativeCriticalCollarCell {
                                    source_face_ordinal: Some(port.event_face),
                                    source_uv: None,
                                    whole_original_face: false,
                                    world_xy: bounds,
                                    world_height: source_column,
                                    material_target_index: index,
                                    native_port_parameter: Some((port_index, cell[0])),
                                    xy_distance_mm: dx,
                                    material_distance_mm: distance,
                                },
                                0,
                                work,
                                cancel,
                            )?;
                        } else {
                            for child in split_axis(cell, depth, 0)? {
                                stack.push((child, depth + 1));
                            }
                        }
                    }
                }
                Ok(())
            })?;
        }
    }
    if sample_to_source > policy.maximum_distance_mm || sample_to_source > native_policy {
        return Err(failure(
            GeometryErrorCode::ShapeErrorExceeded,
            "native collar one-sided source-sample distance exceeds captured policy",
        ));
    }
    let mut source_occupied_xy = [[f64::INFINITY, f64::NEG_INFINITY]; 2];
    for cell in &cells {
        work.charge(1, cancel)?;
        for (axis, bounds) in source_occupied_xy.iter_mut().enumerate() {
            bounds[0] = bounds[0].min(cell.world_xy[axis][0]);
            bounds[1] = bounds[1].max(cell.world_xy[axis][1]);
        }
    }
    // Raw original-F cells overcover the retained complement. Bound every
    // added actual-G ribbon and CLOSED NativeV cone directly: a source-star
    // error is not a physical correspondence to the removed original F disk.
    let closed_source_complement_xy =
        super::critical::material_link::source_transport_projection_cover(
            source,
            matrix,
            pose,
            interval,
            source_occupied_xy,
            policy.maximum_xy_cover_error_mm,
            capacity.remaining(&cells, 0)?,
            work,
            cancel,
        )?;
    for bounds in closed_source_complement_xy {
        Interval::new(bounds[0], bounds[1])?;
    }
    let mut occupied_xy = source_occupied_xy;
    for (input, target) in inputs.iter().zip(&targets) {
        if !target.used {
            continue;
        }
        for ring in input.actual_material {
            for point in &ring.points_mm {
                work.charge(1, cancel)?;
                for axis in 0..2 {
                    occupied_xy[axis][0] = occupied_xy[axis][0].min(point[axis]);
                    occupied_xy[axis][1] = occupied_xy[axis][1].max(point[axis]);
                }
            }
        }
    }
    Ok(NativeCriticalCollar {
        inner: Arc::new(NativeCriticalCollarCapture {
            definition_id: definition.id().clone(),
            material_realization: analysis.material_realization_identity(),
            pose,
            source_request: analysis.request(),
            interval,
            sample,
            native_policy,
            policy,
            targets,
            native_ports,
            sample_to_source,
            source_sample_plane: analysis.source_sample_plane(),
            source_world_z_support,
            actual_sample_world_height_interval: [sample_heights.lo, sample_heights.hi],
            world_regular_boundary_xy_speed,
            source_to_material,
            xy_error,
            complex_sha256,
            native_sample_sha256,
            source_occupied_xy,
            closed_source_complement_xy,
            occupied_xy,
            cells,
        }),
    })
}

fn port_image_storage(count: usize) -> Result<usize, GeometryError> {
    if count == 0 {
        return Ok(0);
    }
    count
        .checked_mul(std::mem::size_of::<PortImageSpan>())
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Vec<PortImageSpan>>()))
        .ok_or_else(|| {
            failure(
                GeometryErrorCode::ResourceLimit,
                "native collar port-image capacity overflow",
            )
        })
}

struct PortImageSpan {
    anchor: usize,
    endpoints: [Box2; 2],
    uv: Box2,
}
impl PortImageSpan {
    fn transverse_interval(&self, axis: usize) -> Option<[f64; 2]> {
        let lower = self.endpoints[0][axis][1].min(self.endpoints[1][axis][1]);
        let upper = self.endpoints[0][axis][0].max(self.endpoints[1][axis][0]);
        (lower <= upper).then_some([lower, upper])
    }
}

/// Continuous original ports span coordinates between certified endpoint tubes
/// by IVT. Preserve their literal gauges: distinct semicircles can jointly span
/// a periodic face, but neither a seam nor an uncovered interval is welded.
/// Only ports owned by the SAME complete filled-face witness may combine.
fn port_image_distance(
    uv: Box2,
    spans: &[PortImageSpan],
    anchor: usize,
    xy_speeds: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<f64, GeometryError> {
    let mut best = f64::INFINITY;
    for axis in 0..2 {
        let other = 1 - axis;
        let mut first = f64::INFINITY;
        let mut longitudinal = 0.0_f64;
        for span in spans {
            work.charge(1, cancel)?;
            if span.anchor != anchor {
                continue;
            }
            let Some([lower, _]) = span.transverse_interval(axis) else {
                continue;
            };
            first = first.min(lower);
            longitudinal = longitudinal.max(
                Interval::point(uv[other][1])
                    .sub(Interval::point(span.uv[other][0]))
                    .abs_upper()
                    .max(
                        Interval::point(span.uv[other][1])
                            .sub(Interval::point(uv[other][0]))
                            .abs_upper(),
                    ),
            );
        }
        if !first.is_finite() {
            continue;
        }
        let mut transverse = Interval::point(first)
            .sub(Interval::point(uv[axis][0]))
            .hi
            .max(0.0);
        let mut end = first;
        loop {
            let mut reach = end;
            let mut next = f64::INFINITY;
            for span in spans {
                work.charge(1, cancel)?;
                if span.anchor != anchor {
                    continue;
                }
                let Some([lower, upper]) = span.transverse_interval(axis) else {
                    continue;
                };
                if lower <= end {
                    reach = reach.max(upper);
                } else {
                    next = next.min(lower);
                }
            }
            if reach > end {
                end = reach;
                continue;
            }
            if !next.is_finite() {
                transverse = transverse.max(
                    Interval::point(uv[axis][1])
                        .sub(Interval::point(end))
                        .hi
                        .max(0.0),
                );
                break;
            }
            if end <= uv[axis][1] && uv[axis][0] <= next {
                // Every coordinate in a bounded gap is within half its width
                // of one of its TWO actual covered endpoints.
                transverse = transverse.max(
                    Interval::point(next)
                        .sub(Interval::point(end))
                        .mul(Interval::point(0.5))
                        .hi,
                );
            }
            end = next;
        }
        best = best.min(
            Interval::point(transverse)
                .mul(Interval::point(xy_speeds[axis]))
                .add(Interval::point(longitudinal).mul(Interval::point(xy_speeds[other])))
                .hi,
        );
    }
    Ok(best)
}

fn try_port_face_cover(
    query: PortFaceCoverQuery<'_, '_>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<NativeCriticalCollarCell>, GeometryError> {
    let PortFaceCoverQuery {
        face,
        face_index,
        domain,
        matrix,
        pose,
        ports,
        cells,
        targets,
        source_column,
        interval,
        policy,
        native_policy,
        count,
        byte_limit,
    } = query;
    let definition = domain.definition();
    if domain.face() != face {
        return Err(unresolved("native collar image changed its prepared face owner"));
    }
    let Some(ports) = ports else { return Ok(None) };
    if count == 0 {
        return Ok(None);
    }
    let full_uv = super::slab::trim_parameter_box(domain)?;
    let image = (|surface| -> Result<_, GeometryError> {
        let Some(uv) = clip_height_parameter_box(
            surface,
            full_uv,
            HeightFunctional {
                direction: matrix[2],
                offset: Interval::point(pose.translation_mm[2]),
            },
            interval,
            work,
            cancel,
        )?
        else {
            return Ok(None);
        };
        // Continuous piecewise derivatives suffice for this Lipschitz image
        // bound. No Hessian, regularity or normal authority is obtained here.
        // Use the full box so every path to the original port remains enclosed.
        let jet = surface_piecewise_first_enclosure(surface, full_uv, work, cancel)?;
        let world = super::placed::transformed_position(matrix, pose, jet.position)?;
        let [sx, sy] = [jet.du, jet.dv]
            .map(|d| hypot(dot(matrix[0], d).abs_upper(), dot(matrix[1], d).abs_upper()));
        Ok(Some((uv, world, [sx?, sy?])))
    })(domain.surface_evaluator())?;
    let Some((uv, world, speeds)) = image else {
        return Ok(None);
    };
    let mut squared_scale = Interval::point(0.0);
    for row in matrix.iter().take(2) {
        for value in row {
            squared_scale = squared_scale.add(value.square());
        }
    }
    let scale = Interval {
        lo: squared_scale.lo.max(0.0),
        hi: squared_scale.hi,
    }
    .sqrt()?
    .hi;
    let chart_error = Interval::point(domain.source_chart_error_mm())
        .mul(Interval::point(scale))
        .hi;
    let header = std::mem::size_of::<Vec<PortImageSpan>>();
    let bytes = port_image_storage(count)?;
    if bytes > byte_limit {
        return Err(failure(
            GeometryErrorCode::ResourceLimit,
            "native collar port images exceed coexistent capacity",
        ));
    }
    work.charge(bytes as u64, cancel)?;
    let mut spans = Vec::new();
    spans.try_reserve_exact(count).map_err(|_| {
        failure(
            GeometryErrorCode::ResourceLimit,
            "native collar port-image allocation refused",
        )
    })?;
    let actual = header + spans.capacity() * std::mem::size_of::<PortImageSpan>();
    if actual > bytes {
        work.charge((actual - bytes) as u64, cancel)?;
    }
    if actual > bytes {
        return Err(failure(
            GeometryErrorCode::ResourceLimit,
            "native collar port-image allocator exceeded its reserved native-query allowance",
        ));
    }
    for port in ports {
        work.charge(1, cancel)?;
        if port.neighbor_face != face_index {
            continue;
        }
        let mut anchor = None;
        for (index, cell) in cells.iter().enumerate() {
            work.charge(1, cancel)?;
            if cell.whole_original_face && cell.source_face_ordinal == Some(port.event_face) {
                anchor = Some(index);
                break;
            }
        }
        let Some(anchor) = anchor else { continue };
        let (edge, _) = native_coedge(
            definition, face_index, port.neighbor_wire, port.neighbor_coedge,
        )?;
        if edge.edge != port.edge_id {
            return Err(unresolved(
                "native collar image changed its opposed original edge",
            ));
        }
        let mut coedge = port.neighbor_coedge;
        for wire_index in 0..port.neighbor_wire {
            work.charge(1, cancel)?;
            coedge = coedge.checked_add(boundary_loop(definition, face_index, wire_index)?.edges.len()).ok_or_else(|| {
                failure(
                    GeometryErrorCode::ResourceLimit,
                    "native collar coedge ordinal overflow",
                )
            })?;
        }
        let identity = domain.coedges().get(coedge)
            .ok_or_else(|| unresolved("native collar image lost its prepared opposed occurrence"))?.identity();
        if identity.edgeuse != port.neighbor_edgeuse || identity.edge_id != port.edge_id {
            return Err(unresolved("native collar image changed its exact opposed edgeuse"));
        }
        let mut range = None;
        let mut port_uv = [[f64::INFINITY, f64::NEG_INFINITY]; 2];
        for span in domain.boundary_parameter_intervals(coedge)? {
            work.charge(1, cancel)?;
            let bounds = domain.boundary_position_enclosure(coedge, span, work, cancel)?;
            for axis in 0..2 {
                port_uv[axis][0] = port_uv[axis][0].min(bounds[axis][0]);
                port_uv[axis][1] = port_uv[axis][1].max(bounds[axis][1]);
            }
            range = Some(range.map_or(span, |old: [f64; 2]| {
                [old[0].min(span[0]), old[1].max(span[1])]
            }));
        }
        let Some(range) = range else { continue };
        let endpoints = [
            domain.boundary_position_enclosure(coedge, [range[0]; 2], work, cancel)?,
            domain.boundary_position_enclosure(coedge, [range[1]; 2], work, cancel)?,
        ];
        spans.push(PortImageSpan {
            anchor,
            endpoints,
            uv: port_uv,
        });
    }
    for (anchor_index, anchor) in cells.iter().enumerate() {
        work.charge(1, cancel)?;
        if !anchor.whole_original_face {
            continue;
        }
        let drift = port_image_distance(uv, &spans, anchor_index, speeds, work, cancel)?;
        if !drift.is_finite() {
            continue;
        }
        let dx = Interval::point(anchor.xy_distance_mm)
            .add(Interval::point(drift))
            .add(Interval::point(chart_error))
            .hi;
        let index = anchor.material_target_index;
        let dz = target_height_distance(&targets[index], source_column);
        let distance = hypot(dx, dz)?;
        if dx > policy.maximum_xy_cover_error_mm
            || distance > policy.maximum_distance_mm
            || distance > native_policy
        {
            continue;
        }
        return Ok(Some(NativeCriticalCollarCell {
            source_face_ordinal: Some(face_index),
            source_uv: Some(uv),
            whole_original_face: false,
            world_xy: [[world[0].lo, world[0].hi], [world[1].lo, world[1].hi]],
            world_height: source_column,
            material_target_index: index,
            native_port_parameter: None,
            xy_distance_mm: dx,
            material_distance_mm: distance,
        }));
    }
    Ok(None)
}

/// Enclose the entire qualified filled source face by one coefficient image.
/// The rectangle deliberately overcovers holes and nonconvexity: target distance
/// must hold for that whole rectangle, never just its corners or sampled rim.
/// Failure leaves the same complete trim-cell refinement below in force.
fn try_filled_face_cover(
    query: FilledFaceCoverQuery<'_, '_, '_>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<NativeCriticalCollarCell>, GeometryError> {
    let FilledFaceCoverQuery {
        definition, face, face_index, domain, matrix, pose, inputs, targets,
        source_column, event, sample_band, policy, native_policy,
    } = query;
    if domain.face() != face || definition.face_ordinal(face) != Some(face_index)
        || !std::ptr::eq(definition, domain.definition())
    {
        return Err(unresolved("native collar whole-face image changed its qualified source owner"));
    }
    let uv = super::slab::trim_parameter_box(domain)?;
    let enclosure = surface_piecewise_first_enclosure(domain.surface_evaluator(), uv, work, cancel)?;
    let mut world = super::placed::transformed_position(matrix, pose, enclosure.position)?;
    // The source-chart association error covers the original spatial ports as
    // well as the carrier image. Bound the actual pose polynomial row by row;
    // orthogonality is not assumed and no source-unit scaling is reapplied.
    for axis in 0..3 {
        let scale = matrix[axis].into_iter()
            .fold(Interval::point(0.0), |sum, value| sum.add(value.square())).sqrt()?.hi;
        let error = Interval::point(domain.source_chart_error_mm()).mul(Interval::point(scale)).hi;
        world[axis] = world[axis].add(Interval { lo: -error, hi: error });
    }
    if world[2].hi < source_column[0] || world[2].lo > source_column[1] {
        return Ok(None);
    }
    let bounds = [[world[0].lo, world[0].hi], [world[1].lo, world[1].hi]];
    let chosen = choose_material_target(
        MaterialTargetQuery {
            coverage: MaterialCoverage::Rectangle(bounds),
            inputs, targets, interval: source_column, event,
            source_sample_band: sample_band, policy, native_policy,
        },
        work, cancel,
    )?;
    Ok(chosen.map(|(index, dx, distance)| NativeCriticalCollarCell {
        source_face_ordinal: Some(face_index),
        source_uv: Some(uv),
        whole_original_face: true,
        world_xy: bounds,
        world_height: source_column,
        material_target_index: index,
        native_port_parameter: None,
        xy_distance_mm: dx,
        material_distance_mm: distance,
    }))
}

struct SurfaceAffineImage {
    corners: [[Interval; 2]; 4],
    remainder_mm: f64,
}

/// Taylor's affine image preserves the correlation lost by an axis-aligned
/// position box. Midpoint coefficients remain interval enclosed. C1 plus
/// bounded piecewise Hessians is independently qualified over the WHOLE box.
fn try_surface_affine_image(
    query: SurfaceAffineImageQuery<'_>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<SurfaceAffineImage>, GeometryError> {
    let SurfaceAffineImageQuery {
        surface,
        uv,
        matrix,
        pose,
        chart_error_mm,
        maximum_error_mm,
    } = query;
    if !crate::parametric::surface_parameter_span_smooth(surface, uv, work, cancel)? {
        return Ok(None);
    }
    // Only a failed rectangle witness reaches this Hessian consumer. The
    // first-order cover above is never promoted into Taylor/C1 authority.
    let whole = surface_piecewise_enclosure(surface, uv, work, cancel)?;
    work.charge(64, cancel)?;
    let middle = uv.map(|axis| axis[0] * 0.5 + axis[1] * 0.5);
    let radius = std::array::from_fn::<_, 2, _>(|axis| {
        Interval::point(uv[axis][0])
            .sub(Interval::point(middle[axis]))
            .abs_upper()
            .max(
                Interval::point(uv[axis][1])
                    .sub(Interval::point(middle[axis]))
                    .abs_upper(),
            )
    });
    let u = Interval::point(radius[0]);
    let v = Interval::point(radius[1]);
    let remainder = [0, 1].map(|axis| {
        Interval::point(dot(matrix[axis], whole.duu).abs_upper())
            .mul(u.square())
            .add(
                Interval::point(dot(matrix[axis], whole.duv).abs_upper())
                    .mul(u)
                    .mul(v)
                    .mul(Interval::point(2.0)),
            )
            .add(Interval::point(dot(matrix[axis], whole.dvv).abs_upper()).mul(v.square()))
            .mul(Interval::point(0.5))
            .hi
    });
    let mut squared_scale = Interval::point(0.0);
    for row in matrix.iter().take(2) {
        for value in row {
            squared_scale = squared_scale.add(value.square());
        }
    }
    let scale = Interval {
        lo: squared_scale.lo.max(0.0),
        hi: squared_scale.hi,
    }
    .sqrt()?
    .hi;
    let remainder_mm = Interval::point(hypot(remainder[0], remainder[1])?)
        .add(Interval::point(chart_error_mm).mul(Interval::point(scale)))
        .hi;
    if remainder_mm > maximum_error_mm {
        return Ok(None);
    }
    let center =
        surface_piecewise_first_enclosure(surface, middle.map(|value| [value; 2]), work, cancel)?;
    let position = super::placed::transformed_position(matrix, pose, center.position)?;
    let du = [dot(matrix[0], center.du), dot(matrix[1], center.du)];
    let dv = [dot(matrix[0], center.dv), dot(matrix[1], center.dv)];
    let corners = std::array::from_fn(|corner| {
        let delta_u = Interval::point(uv[0][corner / 2]).sub(Interval::point(middle[0]));
        let delta_v = Interval::point(uv[1][corner % 2]).sub(Interval::point(middle[1]));
        std::array::from_fn(|axis| {
            position[axis]
                .add(du[axis].mul(delta_u))
                .add(dv[axis].mul(delta_v))
        })
    });
    Ok(Some(SurfaceAffineImage {
        corners,
        remainder_mm,
    }))
}

/// Distance to ONE fixed actual material segment is convex. Hence all four
/// correlated affine corners control its complete parallelogram, and adding
/// the whole Taylor remainder controls every native patch point. Different
/// edges for different corners would be unsound across holes/nonconvex gaps.
fn affine_material_distance_upper(
    image: &SurfaceAffineImage,
    loops: &[SectionLoopMm],
    maximum_distance_mm: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<f64, GeometryError> {
    if image.remainder_mm > maximum_distance_mm {
        return Ok(f64::INFINITY);
    }
    let mut best = f64::INFINITY;
    for ring in loops {
        for edge in ring.points_mm.windows(2) {
            work.charge(1, cancel)?;
            let a = [edge[0][0], edge[0][1]];
            let b = [edge[1][0], edge[1][1]];
            let mut bound = 0.0_f64;
            let mut complete = true;
            for &corner in &image.corners {
                work.charge(1, cancel)?;
                bound = bound.max(super::planar::point_segment_distance(corner, a, b)?.hi);
                if bound > best || bound > maximum_distance_mm {
                    complete = false;
                    break;
                }
            }
            if complete {
                let complete_bound = Interval::point(bound)
                    .add(Interval::point(image.remainder_mm))
                    .hi;
                best = best.min(complete_bound);
                if complete_bound <= maximum_distance_mm {
                    // This SAME segment encloses all four correlated corners,
                    // and the SAME whole-patch remainder is already included.
                    return Ok(complete_bound);
                }
            }
        }
    }
    Ok(best)
}

#[derive(Clone, Copy)]
enum MaterialCoverage<'a> {
    Rectangle(Box2),
    AffineImage(&'a SurfaceAffineImage),
    /// The exact filled native sample, NOT its enclosing rectangle. Its current
    /// target was bound to this sample digest and its actual polygons above.
    FilledNativeSample {
        approximation_mm: f64,
    },
}

struct MaterialTargetQuery<'a, 'leaf> {
    coverage: MaterialCoverage<'a>,
    inputs: &'a [NativeCriticalCollarTarget<'leaf>],
    targets: &'a [NativeCriticalCollarMaterialTarget],
    interval: [f64; 2],
    event: Option<&'a NativeCollarFaceEvent>,
    source_sample_band: [f64; 2],
    policy: NativeCriticalCollarPolicy,
    native_policy: f64,
}

fn choose_material_target(
    query: MaterialTargetQuery<'_, '_>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<(usize, f64, f64)>, GeometryError> {
    let MaterialTargetQuery {
        coverage,
        inputs,
        targets,
        interval,
        event,
        source_sample_band,
        policy,
        native_policy,
    } = query;
    let mut best: Option<(usize, f64, f64)> = None;
    let mut eligible = false;
    for (index, (input, target)) in inputs.iter().zip(targets).enumerate() {
        work.charge(1, cancel)?;
        if matches!(coverage, MaterialCoverage::FilledNativeSample { .. })
            && target.side != NativeCriticalCollarTargetSide::Current
        {
            continue;
        }
        if let Some(event) = event {
            let owns = match target.side {
                NativeCriticalCollarTargetSide::Lower => event.outward_height_sign > 0,
                NativeCriticalCollarTargetSide::Upper => event.outward_height_sign < 0,
                NativeCriticalCollarTargetSide::Current => {
                    if event.outward_height_sign > 0 {
                        source_sample_band[1] < event.height[0]
                    } else {
                        source_sample_band[0] > event.height[1]
                    }
                }
            };
            if !owns {
                continue;
            }
        }
        // Cover every height in this crossing's source shadow, not just the
        // boundary witness. Only a certified world event can restrict its
        // non-material-facing end; sample/non-event shadows remain whole.
        let dz = target_height_distance(target, interval);
        if dz > policy.maximum_distance_mm || dz > native_policy {
            continue;
        }
        eligible = true;
        let dx = match coverage {
            MaterialCoverage::Rectangle(bounds) => {
                let limit = policy.maximum_distance_mm.min(native_policy);
                let remaining_squared = Interval::point(limit)
                    .square()
                    .sub(Interval::point(dz).square());
                let xy_allowance = if remaining_squared.lo > 0.0 {
                    remaining_squared
                        .sqrt()?
                        .lo
                        .min(policy.maximum_xy_cover_error_mm)
                } else {
                    0.0
                };
                material_distance_upper(bounds, input.actual_material, xy_allowance, work, cancel)?
            }
            MaterialCoverage::AffineImage(image) => {
                let limit = policy.maximum_distance_mm.min(native_policy);
                let squared = Interval::point(limit)
                    .square()
                    .sub(Interval::point(dz).square());
                if squared.lo <= 0.0 {
                    continue;
                }
                let allowance = squared.sqrt()?.lo.min(policy.maximum_xy_cover_error_mm);
                affine_material_distance_upper(
                    image,
                    input.actual_material,
                    allowance,
                    work,
                    cancel,
                )?
            }
            MaterialCoverage::FilledNativeSample { approximation_mm } => {
                if let Some(coordinate) = target.leaf.sample_polygon_coordinate_error_mm() {
                    // The sealed oriented-cycle/feature-clearance proof covers
                    // the complete filled polygons, not just matched vertices.
                    // Compose it with exact native S -> frozen polygon error.
                    Interval::point(approximation_mm)
                        .add(Interval::point(coordinate))
                        .hi
                } else if let Some(closed) = target.leaf.closed_shape_error_mm() {
                    // The original native product lane need not retain polygon
                    // correspondence. S is a subset of C, so its full CLOSED
                    // metric also bounds XY distance to the actual footprint.
                    closed
                } else {
                    continue;
                }
            }
        };
        if dx > policy.maximum_xy_cover_error_mm {
            continue;
        }
        let distance = hypot(dx, dz)?;
        if distance > policy.maximum_distance_mm || distance > native_policy {
            continue;
        }
        if best.is_none_or(|(_, _, previous)| distance < previous) {
            best = Some((index, dx, distance));
        }
    }
    if !eligible {
        let captures: [_; 3] = std::array::from_fn(|index| {
            targets
                .get(index)
                .map(|target| (target.side, target.leaf.world_height_interval_mm()))
        });
        return Err(unresolved(format!(
            "native collar no material-facing height-eligible target for source column {interval:?}, source sample {source_sample_band:?}, event {:?}, targets {captures:?}",
            event.map(|event| (event.face, event.height, event.outward_height_sign)),
        )));
    }
    Ok(best)
}

/// An actual original trimmed event-face complex, with its paired original
/// coedges and material-facing orientation. Source seeds use exact coefficient
/// height identities; posed proofs enclose the WHOLE native face/trim height
/// function and exclude unowned critical strata in its complement. Neither
/// case replaces a true event interval with its midpoint.
#[derive(Debug)]
pub(super) struct NativeCollarComplex {
    definition_id: DefinitionId,
    material_realization: crate::kernel::MaterialRealizationIdentity,
    request: super::SlabRequest,
    fingerprint: [u8; 32],
    world_capture: Option<NativeCollarWorldCapture>,
    events: Vec<NativeCollarFaceEvent>,
    ports: Arc<Vec<NativeCollarPort>>,
    boundary_critical_bands: Vec<NativeCollarBoundaryCriticalBand>,
    world_regular_boundary_xy_speed: Option<f64>,
}
#[derive(Debug, Clone, Copy)]
struct NativeCollarWorldCapture {
    pose: RigidPoseMm,
    interval: [f64; 2],
    sample: f64,
}
#[derive(Debug)]
struct NativeCollarFaceEvent {
    face: usize,
    height: [f64; 2],
    outward_height_sign: i8,
}

/// A distinct native critical stratum in the original coface's closed star.
/// Its height is never substituted for the original event-face/port height.
#[derive(Debug)]
pub(super) struct NativeCollarBoundaryCriticalBand {
    event_face: usize,
    source_face: usize,
    source_coedge: Option<(usize, usize)>,
    source_uv: Option<Box2>,
    native_parameter_interval: Option<[f64; 2]>,
    // Larger qualifying domains include the separately excluded regular
    // complement; public domains above describe only the critical connection.
    qualified_uv: Option<Box2>,
    qualified_parameter_interval: Option<[f64; 2]>,
    height: [f64; 2],
    outward_height_sign: i8,
    owner: NativeCollarCriticalOwner,
}
#[derive(Debug, Clone, Copy)]
enum NativeCollarCriticalOwner {
    Port {
        wire: usize,
        coedge: usize,
    },
    Vertex {
        vertex: VertexId,
    },
}
impl NativeCollarBoundaryCriticalBand {
    pub(super) fn event_face(&self) -> usize {
        self.event_face
    }
    pub(super) fn height_interval_mm(&self) -> [f64; 2] {
        self.height
    }
    pub(super) fn outward_height_sign(&self) -> i8 {
        self.outward_height_sign
    }
}
impl NativeCollarComplex {
    pub(super) fn material_realization_identity(
        &self,
    ) -> crate::kernel::MaterialRealizationIdentity {
        self.material_realization
    }
    pub(super) fn definition_id(&self) -> &DefinitionId {
        &self.definition_id
    }
    pub(super) fn native_complex_sha256(&self) -> [u8; 32] {
        self.fingerprint
    }
    pub(super) fn event_faces(&self) -> impl Iterator<Item = (usize, [f64; 2], i8)> + '_ {
        self.events
            .iter()
            .map(|event| (event.face, event.height, event.outward_height_sign))
    }
    pub(super) fn boundary_critical_bands(&self) -> &[NativeCollarBoundaryCriticalBand] {
        &self.boundary_critical_bands
    }
    pub(super) fn world_regular_boundary_xy_speed_mm_per_mm(&self) -> Option<f64> {
        self.world_regular_boundary_xy_speed
    }
    pub(super) fn height_space(&self) -> NativeCollarHeightSpace {
        if self.world_capture.is_some() {
            NativeCollarHeightSpace::World
        } else {
            NativeCollarHeightSpace::Source
        }
    }
    pub(super) fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.definition_id.allocated_bytes()
            + self.events.capacity() * std::mem::size_of::<NativeCollarFaceEvent>()
            + 2 * std::mem::size_of::<usize>()
            + std::mem::size_of::<Vec<NativeCollarPort>>()
            + self.ports.capacity() * std::mem::size_of::<NativeCollarPort>()
            + self.boundary_critical_bands.capacity()
                * std::mem::size_of::<NativeCollarBoundaryCriticalBand>()
    }
}

fn active(height: Interval, range: [f64; 2]) -> bool {
    height.hi >= range[0] && height.lo <= range[1]
}

enum ConstantCoedgeBand {
    Outside,
    Contained,
    Unresolved,
}

fn constant_coedge_band(
    curve: &dyn crate::parametric::CurveEvaluator,
    functional: HeightFunctional,
    height: Interval,
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<ConstantCoedgeBand, GeometryError> {
    if !active(height, range) {
        return Ok(ConstantCoedgeBand::Outside);
    }
    if height.lo >= range[0] && height.hi <= range[1] {
        return Ok(ConstantCoedgeBand::Contained);
    }
    // Compare the complete constant carrier to each exact endpoint before
    // adding a potentially much larger world translation. Subtracting the
    // endpoint from an already rounded world-height enclosure loses the sign
    // of a literal source offset; neither a midpoint nor an epsilon repairs it.
    let relative = |endpoint, work: &mut GeometryWorkBudget| {
        super::slab::constant_native_curve_height(
            curve,
            HeightFunctional {
                offset: functional.offset.sub(Interval::point(endpoint)),
                ..functional
            },
            work,
            cancel,
        )
    };
    let Some(lower) = relative(range[0], work)? else {
        return Ok(ConstantCoedgeBand::Unresolved);
    };
    if lower.hi < 0.0 {
        return Ok(ConstantCoedgeBand::Outside);
    }
    let Some(upper) = relative(range[1], work)? else {
        return Ok(ConstantCoedgeBand::Unresolved);
    };
    if upper.lo > 0.0 {
        Ok(ConstantCoedgeBand::Outside)
    } else if lower.lo >= 0.0 && upper.hi <= 0.0 {
        Ok(ConstantCoedgeBand::Contained)
    } else {
        Ok(ConstantCoedgeBand::Unresolved)
    }
}
fn hash_interval(hash: &mut Sha256, value: Interval) {
    hash.update(value.lo.to_bits().to_le_bytes());
    hash.update(value.hi.to_bits().to_le_bytes());
}
fn hash_uv(hash: &mut Sha256, uv: Box2) {
    for axis in uv {
        for value in axis {
            hash.update(value.to_bits().to_le_bytes());
        }
    }
}

fn exact_zero(value: Interval) -> bool {
    value.lo == 0.0 && value.hi == 0.0
}

fn contained_box(inner: Box2, outer: Box2) -> bool {
    (0..2).all(|axis| inner[axis][0] >= outer[axis][0] && inner[axis][1] <= outer[axis][1])
}
fn critical_geometry_error(error: GeometryError, face: usize, stage: &str) -> GeometryError {
    failure(
        error.code,
        format!(
            "native collar original face {face} {stage}: {}",
            error.message,
        ),
    )
}

fn critical_star_height(
    root: Interval,
    attachment: Interval,
    event: &NativeCollarFaceEvent,
    request: &super::SlabRequest,
) -> Option<[f64; 2]> {
    let height = [
        root.lo
            .min(attachment.lo)
            .min(event.height[0])
            .max(request.height_interval_mm[0]),
        root.hi
            .max(attachment.hi)
            .max(event.height[1])
            .min(request.height_interval_mm[1]),
    ];
    // Capture exactly the relevant CLOSED source, not a narrower request.
    // The sample is strictly beyond the WHOLE retained critical connection.
    (height[0] <= height[1]
        && ((event.outward_height_sign < 0 && height[1] < request.sample_height_mm)
            || (event.outward_height_sign > 0 && request.sample_height_mm < height[0])))
        .then_some(height)
}

/// Audit the COMPLETE original vertex link as one cycle of source wedges.
/// Equal coordinates, equal heights, and repeated native vertex IDs on a seam
/// never merge distinct wire/coedge endpoint occurrences.
fn critical_vertex_owner(
    definition: &Definition,
    events: &[NativeCollarFaceEvent],
    edges: &SourceEdgeIncidence,
    vertex: VertexId,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<usize>, GeometryError> {
    let mut first = None;
    let mut count = 0usize;
    let mut owner = None;
    for face_index in 0..definition.face_ids().len() {
        let (_, face) = native_face(definition, face_index)?;
        for wire_index in 0..face.loops.len() {
            let wire = &boundary_loop(definition, face_index, wire_index)?.edges;
            for (index, edge) in wire.iter().enumerate() {
                work.charge(1, cancel)?;
                let next = &wire[(index + 1) % wire.len()];
                if oriented_vertices(definition, edge)?[1] != vertex
                    || oriented_vertices(definition, next)?[0] != vertex {
                    continue;
                }
                first.get_or_insert((face_index, wire_index, index, true));
                count += 1;
                if events.iter().any(|event| event.face == face_index) {
                    if owner.is_some_and(|previous| previous != face_index) {
                        // A multi-coface cluster needs its own original-coface
                        // connection receipt; height coincidence is insufficient.
                        return Ok(None);
                    }
                    owner = Some(face_index);
                }
            }
        }
    }
    let (Some(start), Some(owner)) = (first, owner) else {
        return Ok(None);
    };
    let mut state = start;
    for visited in 0..count {
        work.charge(1, cancel)?;
        let wire = &boundary_loop(definition, state.0, state.1)?.edges;
        let index = if state.3 {
            (state.2 + 1) % wire.len()
        } else {
            state.2
        };
        let edge = &wire[index];
        let base_front = if state.3 {
            edge.forward
        } else {
            !edge.forward
        };
        let Some(pair) = edges.get(edge.edge) else {
            return Ok(None);
        };
        let Some(second) = pair.second.as_ref() else {
            return Ok(None);
        };
        let other = if pair.first.face == state.0
            && pair.first.wire == state.1
            && pair.first.index == index
        {
            second
        } else {
            &pair.first
        };
        let other_wire = &boundary_loop(definition, other.face, other.wire)?.edges;
        let Some(other_edge) = other_wire.get(other.index) else {
            return Ok(None);
        };
        let at_front = other_edge.forward == base_front;
        let incoming = if at_front {
            (other.index + other_wire.len() - 1) % other_wire.len()
        } else {
            other.index
        };
        if oriented_vertices(definition, &other_wire[incoming])?[1] != vertex
            || oriented_vertices(definition, &other_wire[(incoming + 1) % other_wire.len()])?[0] != vertex
        {
            return Ok(None);
        }
        state = (other.face, other.wire, incoming, !at_front);
        if state == start {
            return Ok((visited + 1 == count).then_some(owner));
        }
    }
    Ok(None)
}

/// Whole literal branch authority, including independent raw/inverse gauges.
/// A source-chart uncertainty tube is NOT an endpoint-coordinate identity.
fn literal_port_coordinate(
    curve: &dyn crate::parametric::CurveEvaluator,
    restriction: [f64; 2],
    coordinate: usize,
    endpoints: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let range = (restriction[0], restriction[1]);
    if !crate::kernel::certified_curve2d_is_straight_regular(
        curve,
        [range.0, range.1],
        work,
        cancel,
    )
    .map_err(|error| {
        failure(
            error.code,
            format!(
                "literal whole port straight/regular coefficient proof over [{},{}]: {}",
                range.0, range.1, error.message,
            ),
        )
    })? {
        return Ok(false);
    }
    for (parameter, value) in [(range.0, endpoints[0]), (range.1, endpoints[1])] {
        if !crate::parametric::curve_endpoint_coordinate_equal(
            curve, parameter, coordinate, value, work, cancel,
        ).map_err(|error| failure(
            error.code,
            format!(
                "literal port endpoint coordinate axis {coordinate}, parameter {parameter}, value {value}: {}",
                error.message,
            ),
        ))? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Propose coordinates from the original curve, then certify their exact whole
/// literal branch. An outward face box is not an original endpoint identity.
fn literal_port_span(
    curve: &dyn crate::parametric::CurveEvaluator,
    restriction: [f64; 2],
    coordinate: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[f64; 2]>, GeometryError> {
    let range = (restriction[0], restriction[1]);
    let mut endpoints = [0.0; 2];
    for (slot, parameter) in [range.0, range.1].into_iter().enumerate() {
        endpoints[slot] = crate::parametric::curve_enclosure(
            curve, (parameter, parameter), work, cancel,
        )?.position[coordinate].mid();
    }
    if !endpoints.into_iter().all(f64::is_finite) || endpoints[0] == endpoints[1] {
        return Ok(None);
    }
    Ok(
        literal_port_coordinate(curve, restriction, coordinate, endpoints, work, cancel)?.then_some([
            endpoints[0].min(endpoints[1]),
            endpoints[0].max(endpoints[1]),
        ]),
    )
}

fn original_port_uv(
    domain: &crate::kernel::PreparedFaceTrimDomain<'_>,
    coedge: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Box2, GeometryError> {
    let mut bounds = [[f64::INFINITY, f64::NEG_INFINITY]; 2];
    for range in domain.boundary_parameter_intervals(coedge)? {
        let cell = domain
            .boundary_position_enclosure(coedge, range, work, cancel)
            .map_err(|error| {
                failure(
                    error.code,
                    format!(
                        "original UV coedge {coedge} interval {range:?} enclosure: {}",
                        error.message,
                    ),
                )
            })?;
        for axis in 0..2 {
            bounds[axis][0] = bounds[axis][0].min(cell[axis][0]);
            bounds[axis][1] = bounds[axis][1].max(cell[axis][1]);
        }
    }
    Ok(bounds)
}

// The caller's outer None is unresolved; Some(None) proves regularity and
// Some(Some(band)) retains the owned critical incidence without allocation.
type CriticalStratumCover = Option<NativeCollarBoundaryCriticalBand>;

/// A regular-chart definite critical vertex needs both a whole differential
/// proof and an independently qualified original source-face relative cut.
/// EndpointUncertainty alone retains non-Jordan completions and is not a disk.
fn isolated_critical_vertex_star(
    query: &CriticalFaceBoundaryStarQuery<'_, '_>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<NativeCollarBoundaryCriticalBand>, GeometryError> {
    let domain = query.domain;
    let functional = query.functional;
    let jet = query.jet;
    work.charge(32, cancel)?;
    let diagonal = [functional.dot(jet.duu), functional.dot(jet.dvv)];
    let mixed = functional.dot(jet.duv);
    // A whole interval determinant, not diagonal signs or sampled corners,
    // proves definiteness throughout this exact convex source cell.
    if !(diagonal[0].mul(diagonal[1]).sub(mixed.square()).lo > 0.0) {
        return Ok(None);
    }
    let sign = if diagonal.iter().all(|value| value.hi < 0.0) {
        1
    } else if diagonal.iter().all(|value| value.lo > 0.0) {
        -1
    } else {
        return Ok(None);
    };
    let source_face_id = domain.face();
    let mut normal = cross(jet.du, jet.dv);
    if !domain.orientation() {
        normal = normal.map(Interval::neg);
    }
    let direction = functional.dot(normal);
    if !((sign > 0 && direction.lo > 0.0) || (sign < 0 && direction.hi < 0.0)) {
        return Ok(None);
    }
    if !crate::parametric::surface_parameter_span_smooth(query.surface, query.cell, work, cancel)
        .map_err(|error| {
            critical_geometry_error(
                error,
                query.face_index,
                "isolated-vertex whole C1 qualification",
            )
        })?
    {
        return Ok(None);
    }
    let Some(root) = super::critical::necessary_stationary_domain(
        query.surface,
        query.cell,
        jet,
        functional,
        work,
        cancel,
    )
    .map_err(|error| {
        critical_geometry_error(
            error,
            query.face_index,
            "isolated-vertex necessary-root contraction",
        )
    })?
    else {
        // No root is not a quantitative regular-speed receipt. Leave ordinary
        // admission to the existing consumer rather than inventing a shortcut.
        return Ok(None);
    };
    let Some(uncertainty) = query.definition.provenance.uncertainty_mm else {
        return Ok(None);
    };
    if !uncertainty.is_finite() || uncertainty < 0.0 {
        return Ok(None);
    }
    let maximum_deviation = uncertainty.min(query.domain.source_chart_error_mm());
    if !maximum_deviation.is_finite() || maximum_deviation < 0.0 {
        return Ok(None);
    }
    let limit_squared = Interval::point(maximum_deviation).square();
    let physical = functional
        .direction
        .into_iter()
        .fold(Interval::point(0.0), |sum, component| {
            sum.add(component.square())
        })
        .sqrt()?
        .mul(Interval::point(query.domain.source_chart_error_mm()))
        .hi;
    let root_height = functional
        .height(
            surface_enclosure(query.surface, root, work, cancel)
                .map_err(|error| {
                    critical_geometry_error(error, query.face_index, "isolated-root surface jet")
                })?
                .position,
        )
        .add(Interval {
            lo: -physical,
            hi: physical,
        });
    for (joint_index, joint) in query.domain.original_joints()?.iter().enumerate() {
        work.charge(1, cancel)?;
        if !matches!(
            joint.kind,
            TrimJointKind::EndpointUncertainty
        ) {
            continue;
        }
        // The native joint can be a zero-width UV line while a necessary
        // root enclosure straddles it. Do not pad or relabel that joint.
        // Qualify the whole connected hull as a separate original source
        // attachment, under the same physical bound and relative cut.
        let envelope: Box2 = std::array::from_fn(|axis| {
            [
                joint.enclosure[axis].lo.min(root[axis][0]),
                joint.enclosure[axis].hi.max(root[axis][1]),
            ]
        });
        let mut original_uses = true;
        for (slot, use_) in [joint.incoming, joint.outgoing].into_iter().enumerate() {
            work.charge(query.domain.coedges().len() as u64, cancel)?;
            let Some((index, _)) =
                query
                    .domain.coedges()
                    .iter()
                    .enumerate()
                    .find(|(_, coedge)| {
                        coedge.identity().face_id == use_.face_id
                            && coedge.identity().edge_id == use_.edge_id
                            && coedge.identity().wire_index == use_.wire_index
                            && coedge.identity().coedge_index == use_.coedge_index
                            && coedge.identity().orientation == use_.orientation
                    })
            else {
                original_uses = false;
                break;
            };
            let (edge, _) = native_coedge(
                query.definition, query.face_index, use_.wire_index, use_.coedge_index,
            )?;
            let correspondence = query.domain.original_coedge_correspondence(index)?;
            let binding = correspondence.binding()
                .ok_or_else(|| unresolved("isolated critical vertex lost its selected material germ"))?;
            let spatial_high = binding.material_forward == (slot == 0);
            let vertex = binding.endpoints[binding.native_endpoint_index(spatial_high)];
            let regular_query = has_regular_original_boundary_query(query.domain, index)?;
            if use_.face_id != source_face_id
                || edge.edge != use_.edge_id
                || edge.forward != use_.orientation
                || vertex != joint.vertex_id
                || !regular_query
            {
                original_uses = false;
                break;
            }
        }
        if !original_uses {
            continue;
        }
        if !crate::parametric::surface_parameter_span_smooth(query.surface, envelope, work, cancel)
            .map_err(|error| {
                critical_geometry_error(
                    error,
                    query.face_index,
                    "original isolated-joint whole C1 qualification",
                )
            })?
        {
            continue;
        }
        let connection_jet =
            surface_enclosure(query.surface, envelope, work, cancel).map_err(|error| {
                critical_geometry_error(
                    error,
                    query.face_index,
                    "original isolated-joint surface jet",
                )
            })?;
        let residual_squared = connection_jet.position.into_iter().enumerate().fold(
            Interval::point(0.0),
            |sum, (axis, value)| {
                sum.add(
                    value
                        .sub(Interval::point(joint.vertex_point[axis]))
                        .square(),
                )
            },
        );
        if !(residual_squared.hi <= limit_squared.lo) {
            continue;
        }
        let mut connection_normal = cross(connection_jet.du, connection_jet.dv);
        if !domain.orientation() {
            connection_normal = connection_normal.map(Interval::neg);
        }
        let connection_direction = functional.dot(connection_normal);
        if !((sign > 0 && connection_direction.lo > 0.0)
            || (sign < 0 && connection_direction.hi < 0.0))
        {
            continue;
        }
        let mut excluded = true;
        for (index, coedge) in query.domain.coedges().iter().enumerate() {
            work.charge(1, cancel)?;
            let incident = [joint.incoming, joint.outgoing].into_iter().any(|use_| {
                coedge.identity().face_id == use_.face_id
                    && coedge.identity().edge_id == use_.edge_id
                    && coedge.identity().wire_index == use_.wire_index
                    && coedge.identity().coedge_index == use_.coedge_index
                    && coedge.identity().orientation == use_.orientation
            });
            if incident {
                continue;
            }
            let boundary = original_port_uv(query.domain, index, work, cancel)?;
            if (0..2).all(|axis| {
                boundary[axis][0] <= envelope[axis][1] && envelope[axis][0] <= boundary[axis][1]
            }) {
                excluded = false;
                break;
            }
        }
        if !excluded {
            continue;
        }
        for (other_index, other) in query.domain.original_joints()?.iter().enumerate() {
            work.charge(1, cancel)?;
            if other_index != joint_index
                && (0..2).all(|axis| {
                    other.enclosure[axis].lo <= envelope[axis][1]
                        && envelope[axis][0] <= other.enclosure[axis].hi
                })
            {
                excluded = false;
                break;
            }
        }
        if !excluded {
            continue;
        }
        let Some(owner) = critical_vertex_owner(
            query.definition,
            query.events,
            query.edges,
            joint.vertex_id,
            work,
            cancel,
        )?
        else {
            continue;
        };
        let event = query
            .events
            .iter()
            .find(|event| event.face == owner)
            .ok_or_else(|| unresolved("isolated critical vertex lost original event coface"))?;
        if event.outward_height_sign != sign {
            continue;
        }
        let attachment = functional.height(connection_jet.position).add(Interval {
            lo: -physical,
            hi: physical,
        });
        let Some(height) = critical_star_height(root_height, attachment, event, query.request)
        else {
            continue;
        };
        // The prepared joint and source-cut token bind the same literal
        // native vertex point; neither is reconstructed from a nearby sample.
        let attached = if let Some(attachments) = query.attachments {
            attachments.qualifies_original_connection(
                query.domain,
                joint_index,
                joint.vertex_id,
                envelope,
                work,
                cancel,
            )?
        } else {
            false
        };
        if !attached {
            return Err(unresolved(format!(
                "native collar original face {} isolated critical joint {joint_index} lacks independently qualified original source-face relative cut for vertex {:?}",
                query.face_index, joint.vertex_id,
            )));
        }
        return Ok(Some(NativeCollarBoundaryCriticalBand {
            event_face: owner,
            source_face: query.face_index,
            source_coedge: None,
            source_uv: Some(envelope),
            native_parameter_interval: None,
            qualified_uv: Some(query.cell),
            qualified_parameter_interval: None,
            height,
            outward_height_sign: sign,
            owner: NativeCollarCriticalOwner::Vertex {
                vertex: joint.vertex_id,
            },
        }));
    }
    Ok(None)
}

fn critical_face_boundary_star(
    query: CriticalFaceBoundaryStarQuery<'_, '_>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<CriticalStratumCover>, GeometryError> {
    if let Some(band) = isolated_critical_vertex_star(&query, work, cancel)? {
        return Ok(Some(Some(band)));
    }
    let CriticalFaceBoundaryStarQuery {
        definition,
        face_index,
        surface,
        domain,
        cell,
        jet: _,
        attachments: _,
        functional,
        request,
        events,
        edges,
    } = query;
    let full = super::slab::trim_parameter_box(domain)?;
    for transverse in 0..2 {
        let longitudinal = 1 - transverse;
        let mut strip = cell;
        strip[longitudinal] = full[longitudinal];
        if !crate::parametric::surface_parameter_span_smooth(surface, strip, work, cancel).map_err(
            |error| {
                critical_geometry_error(error, face_index, "critical-strip whole C1 qualification")
            },
        )? {
            continue;
        }
        let jet = surface_enclosure(surface, strip, work, cancel).map_err(|error| {
            critical_geometry_error(error, face_index, "critical-strip whole surface jet")
        })?;
        let tangents = [jet.du, jet.dv];
        let hessian = [[jet.duu, jet.duv], [jet.duv, jet.dvv]];
        if !exact_zero(functional.dot(tangents[longitudinal]))
            || !exact_zero(functional.dot(hessian[longitudinal][longitudinal]))
            || !exact_zero(functional.dot(hessian[transverse][longitudinal]))
        {
            continue;
        }
        let curvature = functional.dot(hessian[transverse][transverse]);
        if curvature.contains_zero() {
            continue;
        }
        let root = super::critical::necessary_stationary_domain(
            surface, strip, &jet, functional, work, cancel,
        )
        .map_err(|error| {
            critical_geometry_error(
                error,
                face_index,
                "critical-strip necessary-root contraction",
            )
        })?;
        let mut normal = match crate::parametric::surface_geometric_normal_enclosure(
            surface, strip, work, cancel,
        ) {
            Ok(normal) => normal,
            Err(error)
                if matches!(
                    error.code,
                    GeometryErrorCode::ResourceLimit | GeometryErrorCode::Cancelled
                ) =>
            {
                return Err(error);
            }
            Err(_) => continue,
        };
        let Some(root) = root else {
            return Ok(Some(None));
        };
        if !domain.orientation() {
            normal = normal.map(Interval::neg);
        }
        let direction = functional.dot(normal);
        let sign = if direction.lo > 0.0 {
            1
        } else if direction.hi < 0.0 {
            -1
        } else {
            continue;
        };
        if (sign < 0 && curvature.lo <= 0.0) || (sign > 0 && curvature.hi >= 0.0) {
            continue;
        }
        let physical = functional
            .direction
            .into_iter()
            .fold(Interval::point(0.0), |sum, component| {
                sum.add(component.square())
            })
            .sqrt()?
            .mul(Interval::point(domain.source_chart_error_mm()))
            .hi;
        let root_height = functional
            .height(
                surface_enclosure(surface, root, work, cancel)
                    .map_err(|error| {
                        critical_geometry_error(error, face_index, "critical-root surface jet")
                    })?
                    .position,
            )
            .add(Interval {
                lo: -physical,
                hi: physical,
            });

        // A native collapsed-chart completion is a VERTEX stratum, never an
        // invented event coedge. Preserve its complete original shared link.
        for joint in domain.original_joints()? {
            work.charge(1, cancel)?;
            let Some(owner) =
                critical_vertex_owner(definition, events, edges, joint.vertex_id, work, cancel)?
            else {
                continue;
            };
            let event = events
                .iter()
                .find(|event| event.face == owner)
                .ok_or_else(|| unresolved("critical pole lost original event coface"))?;
            if event.outward_height_sign != sign {
                continue;
            }
            let declared_pole = match &joint.kind {
                TrimJointKind::SphericalPole { polar_axis, .. } if *polar_axis != transverse => continue,
                TrimJointKind::SphericalPole { pole, .. } => Some(*pole),
                TrimJointKind::EndpointUncertainty => None,
            };
            // Build the theorem operands from the SEALED ORIGINAL operators.
            // Native generated endpoint UVs cannot supply a source-radian tube.
            let indices = [
                source_coedge_index(domain, joint.incoming.wire_index, joint.incoming.coedge_index, work, cancel)?,
                source_coedge_index(domain, joint.outgoing.wire_index, joint.outgoing.coedge_index, work, cancel)?,
            ];
            let correspondences = [
                domain.original_coedge_correspondence(indices[0])?,
                domain.original_coedge_correspondence(indices[1])?,
            ];
            let mut endpoint_uv = joint.endpoint_uv;
            let mut uncertainty_uv = [0.0_f64; 2];
            let mut qualified_ends = true;
            for (slot, use_) in [joint.incoming, joint.outgoing].into_iter().enumerate() {
                let index = indices[slot];
                let identity = domain.coedges()[index].identity();
                if identity.edgeuse != use_.edgeuse || identity.edge_id != use_.edge_id
                    || identity.face_id != use_.face_id || identity.orientation != use_.orientation
                {
                    return Err(unresolved("original pole endpoint changed its actual native occurrence"));
                }
                if !has_regular_original_boundary_query(domain, index)? {
                    qualified_ends = false;
                    break;
                }
                let correspondence = correspondences[slot];
                let margin = correspondence.margin_uv();
                for axis in 0..2 {
                    uncertainty_uv[axis] = uncertainty_uv[axis].max(margin[axis]);
                }
                let binding = correspondence.binding()
                    .ok_or_else(|| unresolved("original pole lost its selected material germ binding"))?;
                let spatial_high = binding.material_forward == (slot == 0);
                if binding.endpoints[binding.native_endpoint_index(spatial_high)] != joint.vertex_id {
                    return Err(unresolved("original pole source germ changed its canonical shared vertex"));
                }
                let parameter = correspondence.spatial_range()[usize::from(spatial_high)];
                let fresh = domain
                    .original_coedge_uv_piecewise_jet(index, [parameter; 2], work, cancel)?
                    .position;
                for axis in 0..2 {
                    endpoint_uv[slot][axis][0] = endpoint_uv[slot][axis][0].min(fresh[axis].lo);
                    endpoint_uv[slot][axis][1] = endpoint_uv[slot][axis][1].max(fresh[axis].hi);
                }
            }
            if !qualified_ends { continue; }
            let Some(physical_limit_mm) = definition.provenance().uncertainty_mm else { continue };
            let original_joint = super::critical::SourcePoleJoint {
                incoming: joint.incoming.edgeuse,
                outgoing: joint.outgoing.edgeuse,
                vertex: joint.vertex_id,
                vertex_point: joint.vertex_point,
                enclosure: joint.enclosure.map(|value| [value.lo, value.hi]),
                uncertainty_uv,
                physical_limit_mm,
                declared_pole,
                endpoint_uv: &endpoint_uv,
                endpoint_correspondence: Some(correspondences),
            };
            let Some((envelope, world_enclosure)) =
                super::critical::certify_source_pole_connection(
                    surface, domain, &original_joint, root, transverse, work, cancel,
                )
                    .map_err(|error| {
                        critical_geometry_error(
                            error,
                            face_index,
                            "original-pole connection recertification",
                        )
                    })?
            else {
                continue;
            };
            if !crate::parametric::surface_parameter_span_smooth(surface, envelope, work, cancel)
                .map_err(|error| {
                    critical_geometry_error(
                        error,
                        face_index,
                        "root-to-pole whole C1 qualification",
                    )
                })?
            {
                continue;
            }
            let connection_jet =
                surface_enclosure(surface, envelope, work, cancel).map_err(|error| {
                    critical_geometry_error(error, face_index, "root-to-pole whole surface jet")
                })?;
            let connection_tangents = [connection_jet.du, connection_jet.dv];
            let connection_hessian = [
                [connection_jet.duu, connection_jet.duv],
                [connection_jet.duv, connection_jet.dvv],
            ];
            let connection_curvature = functional.dot(connection_hessian[transverse][transverse]);
            if !exact_zero(functional.dot(connection_tangents[longitudinal]))
                || !exact_zero(functional.dot(connection_hessian[longitudinal][longitudinal]))
                || !exact_zero(functional.dot(connection_hessian[transverse][longitudinal]))
                || (sign < 0 && connection_curvature.lo <= 0.0)
                || (sign > 0 && connection_curvature.hi >= 0.0)
            {
                continue;
            }
            let mut connection_normal = crate::parametric::surface_geometric_normal_enclosure(
                surface, envelope, work, cancel,
            )
            .map_err(|error| {
                critical_geometry_error(error, face_index, "root-to-pole geometric normal")
            })?;
            if !domain.orientation() {
                connection_normal = connection_normal.map(Interval::neg);
            }
            let connection_direction = functional.dot(connection_normal);
            if (sign < 0 && connection_direction.hi >= 0.0)
                || (sign > 0 && connection_direction.lo <= 0.0)
            {
                continue;
            }
            let mut complete = true;
            for (index, coedge) in domain.coedges().iter().enumerate() {
                work.charge(1, cancel)?;
                let incident = [joint.incoming, joint.outgoing].into_iter().any(|use_| {
                    coedge.identity().face_id == use_.face_id
                        && coedge.identity().edge_id == use_.edge_id
                        && coedge.identity().wire_index == use_.wire_index
                        && coedge.identity().coedge_index == use_.coedge_index
                        && coedge.identity().orientation == use_.orientation
                });
                if incident {
                    continue;
                }
                let boundary = original_port_uv(domain, index, work, cancel)?;
                if (0..2).all(|axis| {
                    boundary[axis][0] <= envelope[axis][1] && envelope[axis][0] <= boundary[axis][1]
                }) {
                    complete = false;
                    break;
                }
            }
            if !complete {
                continue;
            }
            let attachment = functional.height(world_enclosure).add(Interval {
                lo: -physical,
                hi: physical,
            });
            let Some(height) = critical_star_height(root_height, attachment, event, request) else {
                continue;
            };
            return Ok(Some(Some(NativeCollarBoundaryCriticalBand {
                event_face: owner,
                source_face: face_index,
                source_coedge: None,
                source_uv: Some(envelope),
                native_parameter_interval: None,
                qualified_uv: Some(strip),
                qualified_parameter_interval: None,
                height,
                outward_height_sign: sign,
                owner: NativeCollarCriticalOwner::Vertex {
                    vertex: joint.vertex_id,
                },
            })));
        }

        for (port_index, port) in domain.coedges().iter().enumerate() {
            work.charge(1, cancel)?;
            let correspondence = domain.original_coedge_correspondence(port_index)?;
            if !has_regular_original_boundary_query(domain, port_index)? {
                continue;
            }
            if correspondence.margin_uv()[longitudinal] != 0.0 {
                continue;
            }
            let Some(pair) = edges.get(port.identity().edge_id) else {
                continue;
            };
            let Some(second) = pair.second.as_ref() else {
                continue;
            };
            let neighbor = if pair.first.face == face_index
                && pair.first.wire == port.identity().wire_index
                && pair.first.index == port.identity().coedge_index
            {
                second
            } else {
                &pair.first
            };
            let Some(event) = events.iter().find(|event| event.face == neighbor.face) else {
                continue;
            };
            if event.outward_height_sign != sign {
                continue;
            }
            // An unrelated original coedge cannot attach this critical star.
            // Establish ownership before asking its whole coefficient proof.
            let uv_range = domain.original_coedge_correspondence(port_index)?.uv_range();
            let operand = domain.coedge_operand(port_index)?;
            let Some(span) = literal_port_span(&operand, uv_range, longitudinal, work, cancel)? else {
                continue;
            };
            let port_uv = original_port_uv(domain, port_index, work, cancel)?;
            // The interval is only a coverage enclosure. Endpoint/side identity
            // below uses the original literal span, never its rounded bounds.
            if port_uv[longitudinal][0] > full[longitudinal][0]
                || port_uv[longitudinal][1] < full[longitudinal][1]
            {
                continue;
            }
            if root[transverse][0] < port_uv[transverse][0]
                || root[transverse][1] > port_uv[transverse][1]
            {
                continue;
            }
            let mut connection = strip;
            connection[transverse] = [
                root[transverse][0].min(port_uv[transverse][0]),
                root[transverse][1].max(port_uv[transverse][1]),
            ];
            if !crate::parametric::surface_parameter_span_smooth(surface, connection, work, cancel)
                .map_err(|error| {
                    critical_geometry_error(
                        error,
                        face_index,
                        "root-to-port whole C1 qualification",
                    )
                })?
            {
                continue;
            }
            let connection_jet =
                surface_enclosure(surface, connection, work, cancel).map_err(|error| {
                    critical_geometry_error(error, face_index, "root-to-port whole surface jet")
                })?;
            let second = if transverse == 0 {
                connection_jet.duu
            } else {
                connection_jet.dvv
            };
            let connection_curvature = functional.dot(second);
            if (sign < 0 && connection_curvature.lo <= 0.0)
                || (sign > 0 && connection_curvature.hi >= 0.0)
            {
                continue;
            }
            let connection_tangents = [connection_jet.du, connection_jet.dv];
            let connection_hessian = [
                [connection_jet.duu, connection_jet.duv],
                [connection_jet.duv, connection_jet.dvv],
            ];
            if !exact_zero(functional.dot(connection_tangents[longitudinal]))
                || !exact_zero(functional.dot(connection_hessian[longitudinal][longitudinal]))
                || !exact_zero(functional.dot(connection_hessian[transverse][longitudinal]))
            {
                continue;
            }
            let mut connection_normal = crate::parametric::surface_geometric_normal_enclosure(
                surface, connection, work, cancel,
            )
            .map_err(|error| {
                critical_geometry_error(error, face_index, "root-to-port geometric normal")
            })?;
            if !domain.orientation() {
                connection_normal = connection_normal.map(Interval::neg);
            }
            let connection_direction = functional.dot(connection_normal);
            if (sign < 0 && connection_direction.hi >= 0.0)
                || (sign > 0 && connection_direction.lo <= 0.0)
            {
                continue;
            }
            let (boundary_edge, _) = native_coedge(
                definition, face_index, port.identity().wire_index, port.identity().coedge_index,
            )?;
            let boundary_vertices = oriented_vertices(definition, &boundary_edge)?;
            let mut complete = true;
            for (index, coedge) in domain.coedges().iter().enumerate() {
                work.charge(1, cancel)?;
                if index == port_index {
                    continue;
                }
                let boundary = original_port_uv(domain, index, work, cancel)?;
                if (0..2).any(|axis| {
                    boundary[axis][1] < connection[axis][0]
                        || connection[axis][1] < boundary[axis][0]
                }) {
                    continue;
                }
                let (other, _) = native_coedge(
                    definition, face_index, coedge.identity().wire_index, coedge.identity().coedge_index,
                )?;
                let other_vertices = oriented_vertices(definition, &other)?;
                let shares_endpoint = boundary_vertices.into_iter()
                    .any(|vertex| other_vertices.contains(&vertex));
                if !shares_endpoint || !has_regular_original_boundary_query(domain, index)? {
                    complete = false;
                    break;
                }
                let uv_range = domain.original_coedge_correspondence(index)?.uv_range();
                let operand = domain.coedge_operand(index)?;
                if !literal_port_coordinate(&operand, uv_range, longitudinal, [span[0]; 2], work, cancel)?
                    && !literal_port_coordinate(
                        &operand,
                        uv_range,
                        longitudinal,
                        [span[1]; 2],
                        work,
                        cancel,
                    )?
                {
                    complete = false;
                    break;
                }
            }
            if !complete {
                continue;
            }
            for vertex in oriented_vertices(definition, &boundary_edge)? {
                if critical_vertex_owner(definition, events, edges, vertex, work, cancel)?
                    != Some(event.face)
                {
                    complete = false;
                    break;
                }
            }
            if !complete {
                continue;
            }
            let attachment = functional.height(connection_jet.position).add(Interval {
                lo: -physical,
                hi: physical,
            });
            let Some(height) = critical_star_height(root_height, attachment, event, request) else {
                continue;
            };
            return Ok(Some(Some(NativeCollarBoundaryCriticalBand {
                event_face: event.face,
                source_face: face_index,
                source_coedge: None,
                source_uv: Some(connection),
                native_parameter_interval: None,
                qualified_uv: Some(strip),
                qualified_parameter_interval: None,
                height,
                outward_height_sign: sign,
                owner: NativeCollarCriticalOwner::Port {
                    wire: port.identity().wire_index,
                    coedge: port.identity().coedge_index,
                },
            })));
        }
    }
    Ok(None)
}

fn critical_coedge_normal_sign(
    query: CriticalCoedgeNormalQuery<'_, '_>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let CriticalCoedgeNormalQuery {
        source,
        face_index,
        wire,
        coedge,
        height,
        functional,
        sign,
        remaining_bytes,
    } = query;
    let domain = source
        .face_domain(face_index, remaining_bytes, work, cancel)
        .map_err(|error| {
            critical_geometry_error(error, face_index, "critical-edge incident trim preparation")
        })?;
    let flat = domain.coedges()
        .iter()
        .position(|entry| {
            entry.identity().wire_index == wire && entry.identity().coedge_index == coedge
        })
        .ok_or_else(|| unresolved("critical edge lost original UV occurrence"))?;
    let physical = functional
        .direction
        .into_iter()
        .fold(Interval::point(0.0), |sum, component| {
            sum.add(component.square())
        })
        .sqrt()?
        .mul(Interval::point(domain.source_chart_error_mm()))
        .hi;
    let required = Interval::new(height[0], height[1])?.add(Interval {
        lo: -physical,
        hi: physical,
    });
    let mut stack = [([0.0; 2], 0u8); MAX_DEPTH as usize + 2];
    let mut relevant = false;
    (|surface| {
        for range in domain.boundary_parameter_intervals(flat)? {
            stack[0] = (range, 0);
            let mut len = 1;
            while len != 0 {
                work.charge(1, cancel)?;
                len -= 1;
                let (range, depth) = stack[len];
                let uv = domain
                    .boundary_position_enclosure(flat, range, work, cancel)
                    .map_err(|error| {
                        critical_geometry_error(
                            error,
                            face_index,
                            "critical-edge incident coedge UV enclosure",
                        )
                    })?;
                let mut accepted = true;
                let mut active_patch = false;
                for patch in
                    surface_parameter_boxes(surface, uv, work, cancel).map_err(|error| {
                        critical_geometry_error(
                            error,
                            face_index,
                            "critical-edge incident surface knot inventory",
                        )
                    })?
                {
                    let jet = surface_enclosure(surface, patch, work, cancel).map_err(|error| {
                        critical_geometry_error(
                            error,
                            face_index,
                            "critical-edge incident whole surface jet",
                        )
                    })?;
                    let actual = functional.height(jet.position);
                    if actual.hi < required.lo || required.hi < actual.lo {
                        continue;
                    }
                    active_patch = true;
                    let normal = match crate::parametric::surface_geometric_normal_enclosure(
                        surface, patch, work, cancel,
                    ) {
                        Ok(normal) => normal,
                        Err(error)
                            if matches!(
                                error.code,
                                GeometryErrorCode::ResourceLimit | GeometryErrorCode::Cancelled
                            ) =>
                        {
                            return Err(error);
                        }
                        Err(_) => {
                            accepted = false;
                            break;
                        }
                    };
                    let mut direction = functional.dot(normal);
                    if !domain.orientation() {
                        direction = direction.neg();
                    }
                    if (sign < 0 && direction.hi >= 0.0) || (sign > 0 && direction.lo <= 0.0) {
                        accepted = false;
                        break;
                    }
                }
                if accepted {
                    relevant |= active_patch;
                    continue;
                }
                let midpoint = range[0] * 0.5 + range[1] * 0.5;
                if depth >= MAX_DEPTH || midpoint <= range[0] || midpoint >= range[1] {
                    return Ok(false);
                }
                stack[len] = ([range[0], midpoint], depth + 1);
                stack[len + 1] = ([midpoint, range[1]], depth + 1);
                len += 2;
            }
        }
        Ok(relevant)
    })(domain.surface_evaluator())
}

fn critical_edge_boundary_star(
    query: CriticalEdgeBoundaryStarQuery<'_, '_>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<CriticalStratumCover>, GeometryError> {
    let CriticalEdgeBoundaryStarQuery {
        source,
        face_index,
        wire_index,
        coedge_index,
        curve,
        full,
        cell,
        functional,
        request,
        events,
        edges,
        remaining_bytes,
    } = query;
    let definition = source.definition();
    let (edge, record) = native_coedge(definition, face_index, wire_index, coedge_index)?;
    let domain = source.face_domain(face_index, remaining_bytes, work, cancel)?;
    let flat = source_coedge_index(domain, wire_index, coedge_index, work, cancel)?;
    let correspondence = domain.original_coedge_correspondence(flat)?;
    let binding = correspondence.binding()
        .ok_or_else(|| unresolved("critical edge lost its selected original parameter binding"))?;
    let low_vertex = usize::from(!binding.spatial_forward);
    let pair = edges.get(edge.edge)
        .ok_or_else(|| unresolved("critical edge lost opposed source uses"))?;
    let Some(second_use) = pair.second.as_ref() else {
        return Ok(None);
    };
    let jet = crate::parametric::curve_enclosure(curve, (cell[0], cell[1]), work, cancel)
        .map_err(|error| critical_geometry_error(error, face_index, "critical-edge cell jet"))?;
    let second = functional.dot(jet.second_derivative);
    if second.contains_zero() {
        return Ok(None);
    }
    let midpoint = cell[0] * 0.5 + cell[1] * 0.5;
    let derivative = functional.dot(
        crate::parametric::curve_enclosure(curve, (midpoint, midpoint), work, cancel)
            .map_err(|error| {
                critical_geometry_error(error, face_index, "critical-edge Newton midpoint jet")
            })?
            .derivative,
    );
    let newton = Interval::point(midpoint).sub(derivative.div(second)?);
    let root = [cell[0].max(newton.lo), cell[1].min(newton.hi)];
    if root[0] > root[1] {
        return Ok(Some(None));
    }
    for (parameter, vertex) in [
        (full[0], record.vertices[low_vertex]),
        (full[1], record.vertices[1 - low_vertex]),
    ] {
        let Some(owner) =
            critical_vertex_owner(definition, events, edges, vertex, work, cancel)?
        else {
            continue;
        };
        let event = events
            .iter()
            .find(|event| event.face == owner)
            .ok_or_else(|| unresolved("critical edge lost vertex event owner"))?;
        let sign = event.outward_height_sign;
        if (sign < 0 && second.lo <= 0.0) || (sign > 0 && second.hi >= 0.0) {
            continue;
        }
        let connection = [parameter.min(root[0]), parameter.max(root[1])];
        if !crate::parametric::curve_parameter_span_smooth(curve, connection, work, cancel)
            .map_err(|error| {
                critical_geometry_error(
                    error,
                    face_index,
                    "critical-edge connection C1 qualification",
                )
            })?
        {
            continue;
        }
        let connection_jet = crate::parametric::curve_enclosure(curve, (connection[0], connection[1]), work, cancel)
            .map_err(|error| {
                critical_geometry_error(error, face_index, "critical-edge root-to-vertex jet")
            })?;
        let curvature = functional.dot(connection_jet.second_derivative);
        if (sign < 0 && curvature.lo <= 0.0) || (sign > 0 && curvature.hi >= 0.0) {
            continue;
        }
        let norm_squared = functional
            .direction
            .into_iter()
            .fold(Interval::point(0.0), |sum, value| sum.add(value.square()));
        let slope = functional
            .dot(connection_jet.derivative)
            .div(norm_squared)?;
        let projected: [Interval; 3] = std::array::from_fn(|axis| {
            connection_jet.derivative[axis].sub(functional.direction[axis].mul(slope))
        });
        let projected_length = projected
            .into_iter()
            .fold(Interval::point(0.0), |sum, value| sum.add(value.square()))
            .sqrt()?;
        let speed = connection_jet
            .derivative
            .into_iter()
            .fold(Interval::point(0.0), |sum, value| sum.add(value.square()))
            .sqrt()?;
        // Import's whole-source relative tangent receipt is < speed/4.
        // Require its projection to retain that strict oriented regularity.
        if projected_length.lo <= Interval::point(speed.hi).mul(Interval::point(0.25)).hi {
            continue;
        }
        let mut chart_error = 0.0_f64;
        for use_ in [&pair.first, second_use] {
            let incident = source.face_domain(use_.face, remaining_bytes, work, cancel)?;
            chart_error = chart_error.max(incident.source_chart_error_mm());
        }
        let physical = norm_squared.sqrt()?.mul(Interval::point(chart_error)).hi;
        let attachment = functional.height(connection_jet.position).add(Interval {
            lo: -physical,
            hi: physical,
        });
        let root_height = functional
            .height(
                crate::parametric::curve_enclosure(curve, (root[0], root[1]), work, cancel)
                    .map_err(|error| {
                        critical_geometry_error(
                            error,
                            face_index,
                            "critical-edge contracted-root jet",
                        )
                    })?
                    .position,
            )
            .add(Interval {
                lo: -physical,
                hi: physical,
            });
        let Some(height) = critical_star_height(root_height, attachment, event, request) else {
            continue;
        };
        let mut complete = true;
        for use_ in [&pair.first, second_use] {
            if !critical_coedge_normal_sign(
                CriticalCoedgeNormalQuery {
                    source,
                    face_index: use_.face,
                    wire: use_.wire,
                    coedge: use_.index,
                    height,
                    functional,
                    sign,
                    remaining_bytes,
                },
                work,
                cancel,
            )? {
                complete = false;
                break;
            }
        }
        if !complete {
            continue;
        }
        return Ok(Some(Some(NativeCollarBoundaryCriticalBand {
            event_face: owner,
            source_face: face_index,
            source_coedge: Some((wire_index, coedge_index)),
            source_uv: None,
            native_parameter_interval: Some(connection),
            height,
            outward_height_sign: sign,
            qualified_uv: None,
            qualified_parameter_interval: Some(cell),
            owner: NativeCollarCriticalOwner::Vertex {
                vertex,
            },
        })));
    }
    Ok(None)
}

/// Degree of the actual material-oriented projected link. Four fixed generic
/// rays avoid coordinate-axis ambiguity without angle evaluation or allocation.
struct ProjectedCriticalLink {
    sign: i8,
    valid: [bool; 4],
    crossings: [usize; 4],
    wedges: usize,
}
impl ProjectedCriticalLink {
    fn new(sign: i8) -> Self {
        Self {
            sign,
            valid: [true; 4],
            crossings: [0; 4],
            wedges: 0,
        }
    }
    fn include(&mut self, pair: [[Interval; 2]; 2]) -> bool {
        let turn = pair[0][0].mul(pair[1][1]).sub(pair[0][1].mul(pair[1][0]));
        // Oriented original uses choose the actual wedge. Never silently
        // replace a reflex material wedge by its shorter complementary angle.
        if (self.sign < 0 && turn.hi >= 0.0) || (self.sign > 0 && turn.lo <= 0.0) || self.sign == 0
        {
            return false;
        }
        for (index, ray) in [[1.0, 0.5], [1.0, 0.25], [1.0, 0.75], [1.0, -0.5]]
            .into_iter()
            .enumerate()
        {
            let side = pair.map(|point| {
                Interval::point(ray[0])
                    .mul(point[1])
                    .sub(Interval::point(ray[1]).mul(point[0]))
            });
            if side.iter().any(|value| value.contains_zero()) {
                self.valid[index] = false;
                continue;
            }
            self.crossings[index] += usize::from(if self.sign > 0 {
                side[0].hi < 0.0 && side[1].lo > 0.0
            } else {
                side[0].lo > 0.0 && side[1].hi < 0.0
            });
        }
        self.wedges += 1;
        true
    }
    fn is_degree_one(&self, expected_wedges: usize) -> bool {
        self.wedges != 0
            && self.wedges == expected_wedges
            && self
                .valid
                .iter()
                .zip(self.crossings)
                .any(|(valid, crossings)| *valid && crossings == 1)
    }
}

fn critical_vertex_projection_degree_one(
    source: MaterialSourceView<'_, '_>,
    vertex: VertexId,
    functional: super::HeightFunctional,
    sign: i8,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let definition = source.definition();
    let axis = (0..3)
        .max_by(|a, b| {
            functional.direction[*a]
                .abs_upper()
                .total_cmp(&functional.direction[*b].abs_upper())
        })
        .ok_or_else(|| unresolved("critical vertex has no height direction"))?;
    if functional.direction[axis].contains_zero() {
        return Ok(false);
    }
    let mut projection = [[Interval::point(0.0); 3]; 2];
    for (row, coefficients) in projection.iter_mut().enumerate() {
        let coordinate = (axis + 1 + row) % 3;
        coefficients[coordinate] = Interval::point(1.0);
        coefficients[axis] = functional.direction[coordinate]
            .div(functional.direction[axis])?
            .neg();
    }
    if functional.direction[axis].hi < 0.0 {
        projection[1] = projection[1].map(Interval::neg);
    }
    let row_norms = [
        projection[0]
            .into_iter()
            .fold(Interval::point(0.0), |sum, value| sum.add(value.square()))
            .sqrt()?,
        projection[1]
            .into_iter()
            .fold(Interval::point(0.0), |sum, value| sum.add(value.square()))
            .sqrt()?,
    ];
    struct Ray {
        edge: EdgeId,
        absolute_front: bool,
        direction: [Interval; 2],
    }
    let mut occurrences = 0usize;
    for face_index in 0..definition.face_ids().len() {
        let (_, face) = native_face(definition, face_index)?;
        for wire_index in 0..face.loops.len() {
            for edge in &boundary_loop(definition, face_index, wire_index)?.edges {
                work.charge(1, cancel)?;
                let endpoints = oriented_vertices(definition, edge)?;
                occurrences += usize::from(endpoints[0] == vertex)
                    + usize::from(endpoints[1] == vertex);
            }
        }
    }
    let reserved = occurrences
        .checked_mul(std::mem::size_of::<Ray>())
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Vec<Ray>>()))
        .ok_or_else(|| {
            failure(
                GeometryErrorCode::ResourceLimit,
                "critical vertex ray capacity overflow",
            )
        })?;
    let available = remaining_bytes.checked_sub(reserved).ok_or_else(|| {
        failure(
            GeometryErrorCode::ResourceLimit,
            "critical vertex ray capacity exhausted",
        )
    })?;
    work.charge(reserved as u64, cancel)?;
    let mut rays: Vec<Ray> = Vec::new();
    rays.try_reserve_exact(occurrences).map_err(|_| {
        failure(
            GeometryErrorCode::ResourceLimit,
            "critical vertex ray allocation refused",
        )
    })?;
    if rays.capacity() > occurrences {
        return Err(failure(
            GeometryErrorCode::ResourceLimit,
            "critical vertex ray allocator exceeded reserved capacity",
        ));
    }
    let mut link = ProjectedCriticalLink::new(sign);
    for face_index in 0..definition.face_ids().len() {
        let (_, source_face) = native_face(definition, face_index)?;
        let mut incident = false;
        for wire_index in 0..source_face.loops.len() {
            for edge in &boundary_loop(definition, face_index, wire_index)?.edges {
                work.charge(1, cancel)?;
                incident |= oriented_vertices(definition, edge)?.contains(&vertex);
            }
        }
        if !incident {
            continue;
        }
        let domain = source
            .face_domain(face_index, available, work, cancel)
            .map_err(|error| {
                critical_geometry_error(
                    error,
                    face_index,
                    "critical-vertex incident trim preparation",
                )
            })?;
        for wire_index in 0..source_face.loops.len() {
            let wire = &boundary_loop(definition, face_index, wire_index)?.edges;
            for (incoming_index, incoming_edge) in wire.iter().enumerate() {
                let outgoing_index = (incoming_index + 1) % wire.len();
                let outgoing_edge = &wire[outgoing_index];
                if oriented_vertices(definition, incoming_edge)?[1] != vertex
                    || oriented_vertices(definition, outgoing_edge)?[0] != vertex {
                    continue;
                }
                work.charge(1, cancel)?;
                let incoming = domain.coedges()
                    .iter()
                    .enumerate()
                    .find(|(_, entry)| {
                        entry.identity().wire_index == wire_index
                            && entry.identity().coedge_index == incoming_index
                    })
                    .ok_or_else(|| {
                        unresolved("critical vertex lost incoming original occurrence")
                    })?;
                let outgoing = domain.coedges()
                    .iter()
                    .enumerate()
                    .find(|(_, entry)| {
                        entry.identity().wire_index == wire_index
                            && entry.identity().coedge_index == outgoing_index
                    })
                    .ok_or_else(|| {
                        unresolved("critical vertex lost outgoing original occurrence")
                    })?;
                let incoming_receipt = domain.original_coedge_correspondence(incoming.0)?;
                let outgoing_receipt = domain.original_coedge_correspondence(outgoing.0)?;
                let incoming_binding = incoming_receipt.binding()
                    .ok_or_else(|| unresolved("critical vertex lost its incoming original material binding"))?;
                let outgoing_binding = outgoing_receipt.binding()
                    .ok_or_else(|| unresolved("critical vertex lost its outgoing original material binding"))?;
                let material_loop_forward = (incoming_edge.forward == incoming_binding.spatial_forward)
                    == incoming_binding.material_forward;
                if material_loop_forward != ((outgoing_edge.forward == outgoing_binding.spatial_forward)
                    == outgoing_binding.material_forward)
                {
                    return Err(unresolved("critical vertex original germs disagree on material boundary traversal"));
                }
                let incoming_id = incoming.1.identity().edgeuse;
                let outgoing_id = outgoing.1.identity().edgeuse;
                let joint = domain.original_joints()?.iter().find(|joint| {
                    joint.vertex_id == vertex
                        && ((joint.incoming.edgeuse == incoming_id && joint.outgoing.edgeuse == outgoing_id)
                            || (joint.incoming.edgeuse == outgoing_id && joint.outgoing.edgeuse == incoming_id))
                });
                let mut uv = if let Some(joint) = joint {
                    joint.enclosure.map(|value| [value.lo, value.hi])
                } else {
                    // The original inventory omits only coefficient-proved
                    // zero completions; these remain real wedges in the full link.
                    let a = domain.original_coedge_correspondence(incoming.0)?
                        .spatial_endpoint(incoming_edge.forward);
                    let b = domain.original_coedge_correspondence(outgoing.0)?
                        .spatial_endpoint(!outgoing_edge.forward);
                    let a = domain
                        .original_coedge_uv_piecewise_jet(incoming.0, [a; 2], work, cancel)
                        .map(|jet| jet.position)
                        .map_err(|error| {
                            critical_geometry_error(
                                error,
                                face_index,
                                "critical-vertex incoming endpoint UV enclosure",
                            )
                        })?;
                    let b = domain
                        .original_coedge_uv_piecewise_jet(outgoing.0, [b; 2], work, cancel)
                        .map(|jet| jet.position)
                        .map_err(|error| {
                            critical_geometry_error(
                                error,
                                face_index,
                                "critical-vertex outgoing endpoint UV enclosure",
                            )
                        })?;
                    std::array::from_fn(|axis| {
                        [a[axis].lo.min(b[axis].lo), a[axis].hi.max(b[axis].hi)]
                    })
                };
                // Exact ordinary source endpoints belong to the selected
                // provider's authorised chart. A retained uncertainty connector
                // keeps its WHOLE original enclosure; never clip that evidence.
                if joint.is_none() {
                    let support = domain.surface_evaluator().active_domain();
                    for axis in 0..2 {
                        uv[axis][0] = uv[axis][0].max(support[axis][0]);
                        uv[axis][1] = uv[axis][1].min(support[axis][1]);
                        if uv[axis][0] > uv[axis][1] {
                            return Ok(false);
                        }
                    }
                }
                let normal = (|surface| {
                    crate::parametric::surface_geometric_normal_enclosure(
                        surface, uv, work, cancel,
                    )
                })(domain.surface_evaluator())
                    .map_err(|error| {
                        critical_geometry_error(
                            error,
                            face_index,
                            "critical-vertex joint geometric normal",
                        )
                    })?;
                let mut direction = functional.dot(normal);
                if !domain.orientation() {
                    direction = direction.neg();
                }
                if (sign < 0 && direction.hi >= 0.0) || (sign > 0 && direction.lo <= 0.0) {
                    return Ok(false);
                }
                let mut pair = [[Interval::point(0.0); 2]; 2];
                let mut uses = [
                    (outgoing.0, outgoing_edge.forward),
                    (incoming.0, !incoming_edge.forward),
                ];
                if !material_loop_forward {
                    uses.swap(0, 1);
                }
                for (slot, (index, absolute_front)) in uses.into_iter().enumerate() {
                    let use_ = domain.coedges()[index].identity();
                    if !has_regular_original_boundary_query(domain, index)? {
                        return Ok(false);
                    }
                    let (edge, _) = native_coedge(
                        definition, face_index, use_.wire_index, use_.coedge_index,
                    )?;
                    if let Some(ray) = rays
                        .iter()
                        .find(|ray| ray.edge == edge.edge && ray.absolute_front == absolute_front)
                    {
                        pair[slot] = ray.direction;
                        continue;
                    }
                    let correspondence = domain.original_coedge_correspondence(index)?;
                    let binding = correspondence.binding()
                        .ok_or_else(|| unresolved("critical vertex ray lost its selected source binding"))?;
                    let source_range = correspondence.spatial_range();
                    let parameter = correspondence.spatial_endpoint(!absolute_front);
                    let spatial_high = (!absolute_front) == binding.spatial_forward;
                    let ray = domain.with_original_spatial_curve(index, available, work, cancel, |curve, work, cancel| {
                        if !crate::parametric::curve_parameter_span_smooth(curve, source_range, work, cancel)? {
                            return Err(unresolved(
                                "critical vertex projected link lacks whole-source C1 edges",
                            ));
                        }
                        let mut tangent = crate::parametric::curve_enclosure(curve, (parameter, parameter), work, cancel)
                            .map_err(|error| {
                                critical_geometry_error(
                                    error,
                                    face_index,
                                    "critical-vertex source endpoint tangent jet",
                                )
                            })?
                            .derivative;
                        if spatial_high {
                            tangent = tangent.map(Interval::neg);
                        }
                        let speed = tangent
                            .into_iter()
                            .fold(Interval::point(0.0), |sum, value| sum.add(value.square()))
                            .sqrt()?;
                        Ok(std::array::from_fn(|row| {
                            let error = Interval::point(speed.hi)
                                .mul(Interval::point(0.25))
                                .mul(row_norms[row])
                                .hi;
                            dot(projection[row], tangent).add(Interval {
                                lo: -error,
                                hi: error,
                            })
                        }))
                    })?;
                    rays.push(Ray {
                        edge: edge.edge,
                        absolute_front,
                        direction: ray,
                    });
                    pair[slot] = ray;
                }
                work.charge(64, cancel)?;
                if !link.include(pair) {
                    return Ok(false);
                }
            }
        }
    }
    Ok(occurrences.is_multiple_of(2) && link.is_degree_one(occurrences / 2))
}

/// Prove the ordinary horizontal-coface lane and complete adjacent native
/// critical stars. Ordinary complementary strata remain genuine submersions.
/// Additional roots are retained distinctly: exact longitudinal source-port
/// coverage or a declared collapsed vertex fan, whole signed curvature and
/// normal bounds, opposed original edges and degree-one projected vertex links
/// establish their coface connection. Height proximity never supplies ownership.
pub(super) fn certify_native_collar_complex(
    source: MaterialSourceView<'_, '_>,
    request: &super::SlabRequest,
    functional: super::HeightFunctional,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<NativeCollarComplex>, GeometryError> {
    certify_collar_complex(
        source,
        CollarComplexQuery {
            request,
            functional,
            seed: None,
            world_capture: None,
        },
        remaining_bytes,
        work,
        cancel,
    )
}

pub(super) struct PlacedCollarComplexQuery<'a> {
    pub(super) seed: &'a NativeCollarComplex,
    pub(super) pose: RigidPoseMm,
    pub(super) interval: [f64; 2],
    pub(super) sample: f64,
}

/// Re-prove a captured original event-face complex under the actual accepted
/// world-height polynomial. A formerly horizontal face may become nonconstant:
/// retain its WHOLE native face/coedge complex, prove it lies inside the world
/// event band, and exclude every unowned critical stratum in the complement.
/// Partial event-face clipping without complete level-set incidence refuses.
pub(super) fn place_native_collar_complex(
    source: MaterialSourceView<'_, '_>,
    query: PlacedCollarComplexQuery<'_>,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<NativeCollarComplex, GeometryError> {
    let PlacedCollarComplexQuery {
        seed,
        pose,
        interval,
        sample,
    } = query;
    let definition = source.definition();
    work.charge(1, cancel)?;
    if seed.definition_id() != definition.id()
        || seed.material_realization_identity()
            != crate::kernel::MaterialRealizationIdentity::new(definition)
    {
        return Err(failure(
            GeometryErrorCode::InvalidGeometry,
            "placed collar event complex has another immutable original source",
        ));
    }
    let matrix = super::placed::rotation(pose)?;
    let functional = super::HeightFunctional {
        direction: matrix[2],
        offset: Interval::point(pose.translation_mm[2]),
    };
    let request = super::SlabRequest {
        frame: crate::PlaneFrameMm {
            origin_mm: [0.0; 3],
            x_axis: [1.0, 0.0, 0.0],
            y_axis: [0.0, 1.0, 0.0],
            z_axis: [0.0, 0.0, 1.0],
        },
        height_interval_mm: interval,
        sample_height_mm: sample,
        native_shape_error_mm: seed.request.native_shape_error_mm,
    };
    certify_collar_complex(
        source,
        CollarComplexQuery {
            request: &request,
            functional,
            seed: Some(seed),
            world_capture: Some(NativeCollarWorldCapture {
                pose,
                interval,
                sample,
            }),
        },
        remaining_bytes,
        work,
        cancel,
    )?
    .ok_or_else(|| {
        unresolved("placed original collar event complex has no owned source event faces")
    })
}

struct CollarComplexQuery<'a> {
    request: &'a super::SlabRequest,
    functional: super::HeightFunctional,
    seed: Option<&'a NativeCollarComplex>,
    world_capture: Option<NativeCollarWorldCapture>,
}

fn certify_collar_complex(
    source: MaterialSourceView<'_, '_>,
    query: CollarComplexQuery<'_>,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<NativeCollarComplex>, GeometryError> {
    let CollarComplexQuery {
        request,
        functional,
        seed,
        world_capture,
    } = query;
    let definition = source.definition();
    work.charge(1, cancel)?;
    if !request
        .height_interval_mm
        .iter()
        .all(|value| value.is_finite())
        || !(request.height_interval_mm[0] < request.sample_height_mm
            && request.sample_height_mm < request.height_interval_mm[1])
        || !request.native_shape_error_mm.is_finite()
        || request.native_shape_error_mm <= 0.0
    {
        return Err(failure(
            GeometryErrorCode::InvalidGeometry,
            "native collar complex requires the original finite CLOSED slab, strict sample and positive policy",
        ));
    }
    for value in functional.direction.into_iter().chain([functional.offset]) {
        Interval::new(value.lo, value.hi)?;
    }
    let limit = remaining_bytes.min(MAX_BYTES);
    let stack_bytes = (MAX_DEPTH as usize + 2) * std::mem::size_of::<(Box2, u8)>();
    let mut face_count = 0usize;
    let mut edge_count = 0usize;
    for face_index in 0..definition.face_ids().len() {
        let (_, face) = native_face(definition, face_index)?;
        work.charge(1, cancel)?;
        face_count = face_count.checked_add(1).ok_or_else(|| {
            failure(
                GeometryErrorCode::ResourceLimit,
                "native collar source face count overflow",
            )
        })?;
        for wire_index in 0..face.loops.len() {
            let wire = &boundary_loop(definition, face_index, wire_index)?.edges;
            work.charge(wire.len() as u64, cancel)?;
            edge_count = edge_count.checked_add(wire.len()).ok_or_else(|| {
                failure(
                    GeometryErrorCode::ResourceLimit,
                    "native collar source edge count overflow",
                )
            })?;
        }
    }
    let event_bytes = face_count
        .checked_mul(std::mem::size_of::<NativeCollarFaceEvent>())
        .ok_or_else(|| {
            failure(
                GeometryErrorCode::ResourceLimit,
                "native collar face capacity overflow",
            )
        })?;
    let port_bytes = edge_count
        .checked_mul(std::mem::size_of::<NativeCollarPort>())
        .ok_or_else(|| {
            failure(
                GeometryErrorCode::ResourceLimit,
                "native collar original port capacity overflow",
            )
        })?;
    let band_capacity = edge_count
        .checked_mul(4)
        .and_then(|count| count.checked_add(face_count.checked_mul(2)?))
        .ok_or_else(|| {
            failure(
                GeometryErrorCode::ResourceLimit,
                "critical star capacity overflow",
            )
        })?;
    let band_bytes = band_capacity
        .checked_mul(std::mem::size_of::<NativeCollarBoundaryCriticalBand>())
        .ok_or_else(|| {
            failure(
                GeometryErrorCode::ResourceLimit,
                "critical star byte capacity overflow",
            )
        })?;
    let vertex_capacity = band_capacity.checked_mul(2).ok_or_else(|| {
        failure(
            GeometryErrorCode::ResourceLimit,
            "critical star vertex capacity overflow",
        )
    })?;
    let vertex_bytes = vertex_capacity
        .checked_mul(std::mem::size_of::<(
            VertexId,
            usize,
        )>())
        .ok_or_else(|| {
            failure(
                GeometryErrorCode::ResourceLimit,
                "critical star vertex byte capacity overflow",
            )
        })?;
    let fixed = event_bytes
        .saturating_add(stack_bytes)
        .saturating_add(8 * 1024) // Coexistent bounded critical-star stack/jet scratch.
        .saturating_add(ISOLATED_VERTEX_SCRATCH_BYTES)
        .saturating_add(definition.id().allocated_bytes())
        .saturating_add(std::mem::size_of::<NativeCollarComplex>())
        .saturating_add(port_bytes)
        .saturating_add(band_bytes)
        .saturating_add(vertex_bytes)
        .saturating_add(std::mem::size_of::<
            Vec<(
                VertexId,
                usize,
            )>,
        >())
        .saturating_add(2 * std::mem::size_of::<usize>())
        .saturating_add(std::mem::size_of::<Vec<NativeCollarPort>>());
    if fixed > limit {
        return Err(failure(
            GeometryErrorCode::ResourceLimit,
            "native collar complex capacity exhausted",
        ));
    }
    work.charge(fixed as u64, cancel)?;
    let mut events = Vec::new();
    events.try_reserve_exact(face_count).map_err(|_| {
        failure(
            GeometryErrorCode::ResourceLimit,
            "native collar event-face allocation refused",
        )
    })?;
    for index in 0..definition.face_ids().len() {
        if let Some(seed) = seed {
            work.charge(seed.events.len() as u64, cancel)?;
            if seed.events.iter().any(|event| event.face == index) {
                events.push(NativeCollarFaceEvent {
                    face: index,
                    height: [f64::INFINITY, f64::NEG_INFINITY],
                    outward_height_sign: 0,
                });
            }
        } else if let Some(height) =
            super::slab::constant_source_face_height(definition, index, functional, work, cancel)
                .map_err(|error| {
                    critical_geometry_error(error, index, "constant event-height inventory")
                })?
            && active(height, request.height_interval_mm)
        {
            if height.lo <= request.sample_height_mm && request.sample_height_mm <= height.hi {
                return Err(unresolved(format!(
                    "native collar source sample meets constant face {index} interval [{},{}]",
                    height.lo, height.hi
                )));
            }
            events.push(NativeCollarFaceEvent {
                face: index,
                height: [height.lo, height.hi],
                outward_height_sign: 0,
            });
        }
    }
    if events.is_empty() {
        return Ok(None);
    }
    let mut ports = Vec::new();
    ports.try_reserve_exact(edge_count).map_err(|_| {
        failure(
            GeometryErrorCode::ResourceLimit,
            "native collar original event-port allocation refused",
        )
    })?;
    let mut boundary_critical_bands = Vec::new();
    boundary_critical_bands
        .try_reserve_exact(band_capacity)
        .map_err(|_| {
            failure(
                GeometryErrorCode::ResourceLimit,
                "critical star band allocation refused",
            )
        })?;
    let mut proved_vertices = Vec::new();
    proved_vertices
        .try_reserve_exact(vertex_capacity)
        .map_err(|_| {
            failure(
                GeometryErrorCode::ResourceLimit,
                "critical star vertex allocation refused",
            )
        })?;
    if boundary_critical_bands.capacity() > band_capacity
        || proved_vertices.capacity() > vertex_capacity
    {
        return Err(failure(
            GeometryErrorCode::ResourceLimit,
            "critical star allocator exceeded reserved capacity",
        ));
    }
    let mut stack = Vec::new();
    stack
        .try_reserve_exact(MAX_DEPTH as usize + 2)
        .map_err(|_| {
            failure(
                GeometryErrorCode::ResourceLimit,
                "native collar complex stack allocation refused",
            )
        })?;
    // Preflight the existing original graph's conservative growth policy before
    // its first allocation. Native edge identities, not positional welding,
    // establish the two opposed source uses.
    let graph_bytes = 8 * 1024 * 1024;
    let graph_capacity_bytes = edge_count
        .saturating_add(16)
        .saturating_mul(4)
        .saturating_mul(std::mem::size_of::<(
            EdgeId,
            super::source_incidence::EdgeUses,
        )>());
    if graph_capacity_bytes > graph_bytes {
        return Err(failure(
            GeometryErrorCode::ResourceLimit,
            "native collar original graph frozen capacity exhausted",
        ));
    }
    if fixed.saturating_add(graph_bytes) > limit {
        return Err(failure(
            GeometryErrorCode::ResourceLimit,
            "native collar original incidence graph exceeds coexistent byte limit",
        ));
    }
    // Charge the source-sized conservative allocation bound, not the frozen
    // scratch ceiling. The constructor separately charges every original use.
    work.charge(graph_capacity_bytes as u64, cancel)?;
    let edges = SourceEdgeIncidence::new(definition, graph_bytes, work, cancel)?;
    let occupied = fixed.saturating_add(graph_bytes);
    let mut hash = Sha256::new();
    hash.update(b"spiling-original-native-collar-complex-2\\0");
    hash.update(definition.id().as_str().as_bytes());
    hash.update(crate::kernel::MaterialRealizationIdentity::new(definition).identity_bytes());
    for value in functional.direction.into_iter().chain([functional.offset]) {
        hash_interval(&mut hash, value);
    }
    for value in request
        .height_interval_mm
        .into_iter()
        .chain([request.sample_height_mm, request.native_shape_error_mm])
    {
        hash.update(value.to_bits().to_le_bytes());
    }
    hash.update([u8::from(world_capture.is_some())]);
    if let Some(capture) = world_capture {
        for value in capture
            .pose
            .translation_mm
            .into_iter()
            .chain(capture.pose.rotation_xyzw)
        {
            hash.update(value.to_bits().to_le_bytes());
        }
    }
    if let Some(seed) = seed {
        hash.update(seed.fingerprint);
    }
    let world_matrix = world_capture
        .map(|capture| super::placed::rotation(capture.pose))
        .transpose()?;
    let mut world_regular_boundary_xy_speed = world_matrix.map(|_| 0.0_f64);
    // Establish original event cofaces before adjacent native stars consume
    // their orientation; source order does not establish proof order.
    for event_pass in [true, false] {
        for (index, &face) in definition.face_ids().iter().enumerate() {
            work.charge(1, cancel)?;
            let event_index = events.iter().position(|event| event.face == index);
            if event_index.is_some() != event_pass {
                continue;
            }
            hash.update((index as u64).to_le_bytes());
            let info = definition
                .faces
                .get(index)
                .ok_or_else(|| unresolved("native collar face identity mapping incomplete"))?;
            hash.update(info.face_id.as_str().as_bytes());
            hash.update([u8::from(face_orientation(definition, index, work, cancel)?)]);
            let domain = source
                .face_domain(index, limit.saturating_sub(occupied), work, cancel)
                .map_err(|error| {
                    critical_geometry_error(error, index, "original trim preparation")
                })?;
            // Source-cut qualification may visit other immutable carriers;
            // establish that geometry-only authority before face admission.
            let attachments = if event_index.is_none() {
                super::critical::OriginalFaceCornerAttachments::prepare(
                    source,
                    &domain,
                    limit
                        .saturating_sub(occupied),
                    work,
                    cancel,
                )
                .map_err(|error| {
                    critical_geometry_error(
                        error,
                        index,
                        "original corner relative-cut qualification",
                    )
                })?
            } else {
                None
            };
            (|surface| -> Result<(), GeometryError> {
            let uv = super::slab::trim_parameter_box(&domain)?;
            // Admission precedes every knot/adaptive partition: splitting a
            // nested C0 ridge into locally smooth cells cannot erase its
            // independent height-critical incidence obligation.
            if !super::critical::critical_face_gluing(surface,uv,work,cancel)
                .map_err(|error| critical_geometry_error(error, index, "whole source surface gluing"))? {
                return Err(unresolved(format!("native collar original face {index} lacks whole-source C1/exact G1 gluing or complete internal crease incidence")));
            }
            let mut spans = surface_parameter_boxes(surface, uv, work, cancel)
                .map_err(|error| critical_geometry_error(error, index, "whole source surface knot inventory"))?;
            while let Some((uv, depth)) = stack.pop().or_else(|| spans.next().map(|uv| (uv, 0))) {
                work.charge(1, cancel)?;
                if matches!(domain.classify(uv, work, cancel)
                    .map_err(|error| critical_geometry_error(error, index, "source UV cell trim classification"))?,
                    TrimClassification::Outside) { continue; }
                let jet = surface_enclosure(surface, uv, work, cancel)
                    .map_err(|error| critical_geometry_error(error, index, "source UV cell surface jet"))?;
                let height=functional.height(jet.position);
                if !(seed.is_some() && event_index.is_some()) && !active(height,request.height_interval_mm) { continue; }
                let mut normal = cross(jet.du, jet.dv);
                if !domain.orientation() { normal = normal.map(Interval::neg); }
                let regular = normal.iter().any(|coordinate| !coordinate.contains_zero());
                let mut accepted = if let Some(event_index) = event_index {
                    let direction=if let Some(matrix)=world_matrix {
                        let du=matrix.map(|row|dot(row,jet.du)); let dv=matrix.map(|row|dot(row,jet.dv));
                        let n=cross(du,dv)[2]; if domain.orientation() { n } else { n.neg() }
                    } else { functional.dot(normal) };
                    let sign=if direction.lo>0.0 { 1 } else if direction.hi<0.0 { -1 } else { 0 };
                    let whole_event=seed.is_none() || (height.lo>=request.height_interval_mm[0] && height.hi<=request.height_interval_mm[1]);
                    if regular && sign!=0 && whole_event {
                        let previous = events[event_index].outward_height_sign;
                        if previous != 0 && previous != sign { return Err(unresolved(
                            format!("native collar event face {index} has inconsistent original material-facing orientation"))); }
                        events[event_index].outward_height_sign = sign;
                        if seed.is_some() {
                            events[event_index].height[0]=events[event_index].height[0].min(height.lo);
                            events[event_index].height[1]=events[event_index].height[1].max(height.hi);
                        }
                        true
                    } else { false }
                } else { regular && (!functional.dot(jet.du).contains_zero()
                    || !functional.dot(jet.dv).contains_zero()) };
                let mut critical_owned = false;
                if !accepted && event_index.is_none() {
                    let covered = boundary_critical_bands.iter().any(|band: &NativeCollarBoundaryCriticalBand|
                        band.source_face == index && band.source_coedge.is_none()
                            && band.qualified_uv.is_some_and(|covered| contained_box(uv, covered)));
                    if covered {
                        accepted = true;
                        critical_owned = true;
                    } else if let Some(cover) = critical_face_boundary_star(CriticalFaceBoundaryStarQuery { definition, face_index: index, surface, domain: &domain, cell: uv, jet: &jet, attachments: attachments.as_ref(), functional, request, events: &events, edges: &edges }, work, cancel)? {
                        if let Some(band) = cover {
                            if boundary_critical_bands.len() == band_capacity {
                                return Err(failure(GeometryErrorCode::ResourceLimit, "critical star band capacity exhausted"));
                            }
                            boundary_critical_bands.push(band);
                        }
                        accepted = true;
                        critical_owned = true;
                    }
                    if critical_owned { world_regular_boundary_xy_speed = None; }
                }
                if accepted {
                    if event_index.is_none() && !critical_owned
                        && let (Some(matrix),Some(previous))=(world_matrix,world_regular_boundary_xy_speed)
                    {
                        work.charge(32,cancel)?;
                        world_regular_boundary_xy_speed=best_face_world_xy_speed(&jet,functional,matrix)
                            .map(|speed|previous.max(speed));
                    }
                    hash_uv(&mut hash, uv);
                    for point in jet.position { hash_interval(&mut hash, point); }
                } else {
                    let motion = [jet.du, jet.dv].map(|tangent| tangent.into_iter()
                        .map(Interval::abs_upper).fold(0.0_f64, f64::max));
                    let scores: [f64; 2] = std::array::from_fn(|axis|
                        motion[axis] * (uv[axis][1] - uv[axis][0]));
                    for child in split_axis(uv, depth, usize::from(scores[1] > scores[0]))
                        .map_err(|error| failure(error.code, format!("native collar face {index} has unresolved native critical/root incidence at {uv:?}: {}", error.message)))? {
                        stack.push((child, depth + 1));
                    }
                }
            }
            Ok(())
                    })(domain.surface_evaluator()).map_err(|error| critical_geometry_error(error, index, "whole original face qualification"))?;
            let (_, native_face_record) = native_face(definition, index)?;
            for wire_index in 0..native_face_record.loops.len() {
                let wire = &boundary_loop(definition, index, wire_index)?.edges;
                for (coedge_index, edge) in wire.iter().enumerate() {
                    work.charge(1, cancel)?;
                    let paired = edges.get(edge.edge).ok_or_else(|| {
                        unresolved("native collar lost original native edge incidence")
                    })?;
                    let second = paired.second.as_ref().ok_or_else(|| {
                        unresolved("native collar has an unpaired original coedge")
                    })?;
                    for use_ in [&paired.first, second] {
                        hash.update((use_.face as u64).to_le_bytes());
                        hash.update((use_.wire as u64).to_le_bytes());
                        hash.update((use_.index as u64).to_le_bytes());
                        hash.update([u8::from(use_.orientation)]);
                    }
                    if let Some(event_index) = event_index {
                        let event = &events[event_index];
                        let own = if paired.first.face == index
                            && paired.first.wire == wire_index
                            && paired.first.index == coedge_index
                        {
                            &paired.first
                        } else {
                            second
                        };
                        let neighbor = if std::ptr::eq(own, &paired.first) {
                            second
                        } else {
                            &paired.first
                        };
                        ports.push(NativeCollarPort {
                            edge_id: edge.edge,
                            event_edgeuse: own.edgeuse,
                            neighbor_edgeuse: neighbor.edgeuse,
                            event_face: index,
                            event_wire: wire_index,
                            event_coedge: coedge_index,
                            neighbor_face: neighbor.face,
                            neighbor_wire: neighbor.wire,
                            neighbor_coedge: neighbor.index,
                            orientation: own.orientation,
                            height: event.height,
                            outward_height_sign: event.outward_height_sign,
                            height_space: if world_capture.is_some() {
                                NativeCollarHeightSpace::World
                            } else {
                                NativeCollarHeightSpace::Source
                            },
                        });
                    }
                    // The graph is visited in original face/wire/coedge order;
                    // hash-map iteration never determines source identity.
                    hash.update((wire_index as u64).to_le_bytes());
                    hash.update((coedge_index as u64).to_le_bytes());
                    let attached_event = events
                        .iter()
                        .any(|event| event.face == paired.first.face || event.face == second.face);
                    let flat = source_coedge_index(domain, wire_index, coedge_index, work, cancel)?;
                    let correspondence = domain.original_coedge_correspondence(flat)?;
                    let binding = correspondence.binding()
                        .ok_or_else(|| unresolved("native collar lost its selected original vertex binding"))?;
                    let source_range = correspondence.spatial_range();
                    for (endpoint, vertex) in oriented_vertices(definition, edge)?.into_iter().enumerate() {
                        let point = definition.brep().vertex(vertex)
                            .map_err(|e| unresolved(format!("native collar source vertex: {e}")))?.point;
                        let native_back = endpoint == usize::from(edge.forward);
                        let parameter = correspondence.spatial_endpoint(native_back);
                        let declared = binding.source_vertex_points_mm[usize::from(native_back)];
                        let source_height = domain.with_original_spatial_curve(
                            flat, limit.saturating_sub(occupied), work, cancel,
                            |curve, work, cancel| super::slab::original_endpoint_height(
                                curve, parameter, declared, functional, work, cancel,
                            ),
                        )?;
                        let height = functional.vertex([point.x, point.y, point.z]).hull(source_height);
                        hash_interval(&mut hash, height);
                        let mut event_incident = attached_event;
                        if active(height, request.height_interval_mm) && !event_incident {
                            // A seam/vertical coedge's endpoint can belong to
                            // the event disk through a *different* incident
                            // coedge. Preserve actual native vertex identity,
                            // not only the two faces paired across this edge.
                            for candidate_index in 0..definition.face_ids().len() {
                                let (_, candidate) = native_face(definition, candidate_index)?;
                                work.charge(1, cancel)?;
                                if !events.iter().any(|event| event.face == candidate_index) {
                                    continue;
                                }
                                for boundary_index in 0..candidate.loops.len() {
                                    for candidate_edge in &boundary_loop(definition, candidate_index, boundary_index)?.edges {
                                        work.charge(1, cancel)?;
                                        event_incident |= oriented_vertices(definition, candidate_edge)?.contains(&vertex);
                                    }
                                }
                            }
                            if !event_incident {
                                return Err(unresolved(format!(
                                    "native collar original face {index} coedge {coedge_index} vertex needs isolated native event incidence"
                                )));
                            }
                        }
                    }
                    domain.with_original_spatial_curve(
                        flat, limit.saturating_sub(occupied), work, cancel, |curve, work, cancel| {
                        if let Some(height) = super::slab::constant_native_curve_height(curve, functional, work, cancel)? {
                            hash_interval(&mut hash, height);
                            let band = constant_coedge_band(curve, functional, height,
                                request.height_interval_mm, work, cancel)?;
                            // This excludes only a WHOLE carrier strictly outside
                            // the CLOSED request, not an overlapping event port.
                            // Keep the paired original port and NativeV census
                            // above: actual-G ribbons and closed vertex cones
                            // still have their independent complete cover.
                            if matches!(band, ConstantCoedgeBand::Outside) {
                                return Ok(());
                            }
                            if !attached_event {
                                return Err(unresolved(format!("native collar original face {index} coedge {coedge_index} has a constant event not incident to a certified source face")));
                            }
                            if seed.is_some() && !matches!(band, ConstantCoedgeBand::Contained) {
                                return Err(unresolved(format!("native collar original event coedge {index}/{coedge_index} is not wholly inside the actual-world event band")));
                            }
                            return Ok(());
                        }
                        let mut whole_coedge_gluing_checked=false;
                        // Reuse the bounded UV stack for curve intervals; its
                        // second axis is inert and creates no additional buffer.
                        for span in crate::parametric::curve_parameter_spans(curve, source_range, work, cancel)? {
                            stack.push(([[span[0], span[1]], [0.0, 0.0]], 0));
                            while let Some((cell, depth)) = stack.pop() {
                                work.charge(1, cancel)?;
                                let jet = crate::parametric::curve_enclosure(curve, (cell[0][0], cell[0][1]), work, cancel)?;
                                let height=functional.height(jet.position);
                                let event_edge=seed.is_some() && attached_event;
                                if !event_edge && !active(height,request.height_interval_mm) { continue; }
                                // The first potentially relevant cell triggers
                                // qualification of the ENTIRE retained native
                                // coedge, never only this knot/adaptive piece.
                                // This also guards nonconstant event ports:
                                // an internal C0 knot is not an original BREP
                                // vertex with already-owned event incidence.
                                if !whole_coedge_gluing_checked {
                                    if !crate::parametric::curve_parameter_span_smooth(curve,source_range,work,cancel)? {
                                        return Err(unresolved(format!("native collar original face {index} coedge {coedge_index} lacks whole-source C1 or complete internal knot-event incidence")));
                                    }
                                    whole_coedge_gluing_checked=true;
                                }
                                let mut accepted=if event_edge { height.lo>=request.height_interval_mm[0] && height.hi<=request.height_interval_mm[1] }
                                    else { !functional.dot(jet.derivative).contains_zero() };
                                let mut critical_owned = false;
                                if !accepted {
                                    let covered = boundary_critical_bands.iter().any(|band|
                                        band.source_face == index && band.source_coedge == Some((wire_index, coedge_index))
                                            && band.qualified_parameter_interval.is_some_and(|range|
                                                cell[0][0] >= range[0] && cell[0][1] <= range[1]));
                                    if covered {
                                        accepted = true;
                                        critical_owned = true;
                                    } else if let Some(cover) = critical_edge_boundary_star(CriticalEdgeBoundaryStarQuery { source, face_index: index, wire_index, coedge_index, curve, full: source_range, cell: cell[0], functional, request, events: &events, edges: &edges, remaining_bytes: limit.saturating_sub(occupied) }, work, cancel)? {
                                        if let Some(band) = cover {
                                            if boundary_critical_bands.len() == band_capacity {
                                                return Err(failure(GeometryErrorCode::ResourceLimit, "critical star edge capacity exhausted"));
                                            }
                                            boundary_critical_bands.push(band);
                                        }
                                        accepted = true;
                                        critical_owned = true;
                                    }
                                    if critical_owned { world_regular_boundary_xy_speed = None; }
                                }
                                if accepted {
                                    if !attached_event && !critical_owned
                                        && let (Some(matrix),Some(previous))=(world_matrix,world_regular_boundary_xy_speed)
                                    {
                                        work.charge(16,cancel)?;
                                        world_regular_boundary_xy_speed=world_xy_tangent_speed(jet.derivative,
                                            functional.dot(jet.derivative),matrix).map(|speed|previous.max(speed));
                                    }
                                    hash_uv(&mut hash, cell);
                                } else {
                                    for child in split_axis(cell, depth, 0).map_err(|error| failure(error.code,
                                        format!("native collar original face {index} coedge {coedge_index} needs an isolated true-root incidence: {}", error.message)))? {
                                        stack.push((child, depth + 1));
                                    }
                                }
                            }
                        }
                        Ok(())
                    }).map_err(|error| critical_geometry_error(error, index,
                        &format!("original coedge {wire_index}/{coedge_index} qualification")))?;
                }
            }
        }
    }
    if events.iter().any(|event| event.outward_height_sign == 0) {
        return Err(unresolved(
            "native collar constant event face has no complete regular material-facing native cover",
        ));
    }
    // A closed abstract link can still wrap twice after projection. Audit each
    // original vertex ONCE, including the material-oriented collapsed pole fan.
    for band in &boundary_critical_bands {
        let vertices = match band.owner {
            NativeCollarCriticalOwner::Vertex { vertex } => [Some(vertex), None],
            NativeCollarCriticalOwner::Port { wire, coedge } => {
                let (edge, _) = native_coedge(definition, band.source_face, wire, coedge)?;
                oriented_vertices(definition, &edge)?.map(Some)
            }
        };
        for vertex in vertices.into_iter().flatten() {
            work.charge(1, cancel)?;
            if let Some((_, owner)) = proved_vertices.iter().find(|(proved, _)| *proved == vertex) {
                if *owner != band.event_face {
                    return Err(unresolved(
                        "critical star vertex mixes unrelated original coface owners",
                    ));
                }
                continue;
            }
            if !critical_vertex_projection_degree_one(
                source,
                vertex,
                functional,
                band.outward_height_sign,
                limit.saturating_sub(occupied),
                work,
                cancel,
            )? {
                return Err(unresolved(
                    "original critical vertex has no degree-one, nonoverlapping projected source link",
                ));
            }
            if proved_vertices.len() == vertex_capacity {
                return Err(failure(
                    GeometryErrorCode::ResourceLimit,
                    "critical projected vertex capacity exhausted",
                ));
            }
            proved_vertices.push((vertex, band.event_face));
        }
    }
    for band in &boundary_critical_bands {
        hash.update(b"owned-boundary-critical-stratum\\0");
        hash.update((band.event_face as u64).to_le_bytes());
        hash.update((band.source_face as u64).to_le_bytes());
        hash.update(band.outward_height_sign.to_le_bytes());
        for height in band.height {
            hash.update(height.to_bits().to_le_bytes());
        }
        if let Some(uv) = band.source_uv {
            hash.update([1]);
            hash_uv(&mut hash, uv);
        } else {
            hash.update([0]);
        }
        if let Some(uv) = band.qualified_uv {
            hash.update([1]);
            hash_uv(&mut hash, uv);
        } else {
            hash.update([0]);
        }
        if let Some((wire, coedge)) = band.source_coedge {
            hash.update([1]);
            hash.update((wire as u64).to_le_bytes());
            hash.update((coedge as u64).to_le_bytes());
        } else {
            hash.update([0]);
        }
        if let Some(range) = band.native_parameter_interval {
            for value in range {
                hash.update(value.to_bits().to_le_bytes());
            }
        }
        if let Some(range) = band.qualified_parameter_interval {
            for value in range {
                hash.update(value.to_bits().to_le_bytes());
            }
        }
        match band.owner {
            NativeCollarCriticalOwner::Port { wire, coedge } => {
                hash.update([0]);
                hash.update((wire as u64).to_le_bytes());
                hash.update((coedge as u64).to_le_bytes());
            }
            NativeCollarCriticalOwner::Vertex { vertex } => {
                hash.update([1]);
                let (_, face) = native_face(definition, band.event_face)?;
                for wire in 0..face.loops.len() {
                    let boundary = &boundary_loop(definition, band.event_face, wire)?.edges;
                    for (coedge, edge) in boundary.iter().enumerate() {
                        for (endpoint, candidate) in
                            oriented_vertices(definition, edge)?.into_iter().enumerate()
                        {
                            work.charge(1, cancel)?;
                            if candidate == vertex {
                                hash.update((wire as u64).to_le_bytes());
                                hash.update((coedge as u64).to_le_bytes());
                                hash.update([endpoint as u8]);
                            }
                        }
                    }
                }
            }
        }
    }
    for event in &events {
        hash.update((event.face as u64).to_le_bytes());
        for height in event.height {
            hash.update(height.to_bits().to_le_bytes());
        }
        hash.update(event.outward_height_sign.to_le_bytes());
    }
    for port in &mut ports {
        let event = events
            .iter()
            .find(|event| event.face == port.event_face)
            .ok_or_else(|| unresolved("native collar event port lost its original event face"))?;
        port.height = event.height;
        port.outward_height_sign = event.outward_height_sign;
    }
    hash.update([u8::from(world_regular_boundary_xy_speed.is_some())]);
    if let Some(speed) = world_regular_boundary_xy_speed {
        hash.update(speed.to_bits().to_le_bytes());
    }
    Ok(Some(NativeCollarComplex {
        definition_id: definition.id().clone(),
        material_realization: crate::kernel::MaterialRealizationIdentity::new(definition),
        request: seed.map_or(*request, |seed| seed.request),
        world_capture,
        fingerprint: hash.finalize().into(),
        events,
        ports: Arc::new(ports),
        boundary_critical_bands,
        world_regular_boundary_xy_speed,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PlaneFrameMm;
    use spiling_contracts::geometry::OccurrenceId;
    use solidcore::surface::surface::p2::SurfaceKind;
    use solidcore::surface::surface::Surface;
    use solidcore::curve::curve::Curve;
    use solidcore::nurbs::{b_spline_curve::BSplineCurve, b_spline_surface::BSplineSurface,
        crv_basic, srf_basic, math::{CPoint, NOW}};

    fn linear_spline_surface(
        points: Vec<Vec<CPoint>>, u_knots: Vec<f64>, v_knots: Vec<f64>,
    ) -> Surface {
        let raw = srf_basic::Surface::new(points, 1, 1, u_knots, v_knots).unwrap();
        Surface::new(SurfaceKind::BSpline(BSplineSurface::from_nurb(raw, true)))
    }

    #[test]
    fn native_literal_port_retains_exact_restricted_endpoint_coordinates() {
        let no_z = solidcore::nurbs::math::NOZ;
        let raw = crv_basic::Curve::new(
            vec![CPoint::new(0.0, 2.0, no_z, 2.0), CPoint::new(4.0, 2.0, no_z, 2.0)],
            1, vec![0.0, 0.0, 1.0, 1.0],
        ).unwrap();
        let curve: Curve = BSplineCurve::from_nurb(raw, 2, true).into();
        let mut work = GeometryWorkBudget::new(10_000_000);
        let cancel = AtomicBool::new(false);
        assert_eq!(
            literal_port_span(&curve, [0.0, 1.0], 0, &mut work, &cancel).unwrap(),
            Some([0.0, 2.0]),
        );
        assert!(
            literal_port_coordinate(&curve, [0.25, 0.75], 1, [1.0; 2], &mut work, &cancel).unwrap(),
        );
        assert!(
            !literal_port_coordinate(
                &curve, [0.25, 0.75], 1, [1.0, 1.0_f64.next_up()], &mut work, &cancel,
            ).unwrap(),
            "overlapping endpoint enclosures cannot replace exact native coefficient identities",
        );
    }

    fn frame() -> PlaneFrameMm {
        PlaneFrameMm {
            origin_mm: [0.0; 3],
            x_axis: [1.0, 0.0, 0.0],
            y_axis: [0.0, 1.0, 0.0],
            z_axis: [0.0, 0.0, 1.0],
        }
    }
    fn rectangle(x0: f64, y0: f64, x1: f64, y1: f64, hole: bool) -> SectionLoopMm {
        SectionLoopMm {
            is_hole: hole,
            points_mm: vec![
                [x0, y0, 0.0],
                [x1, y0, 0.0],
                [x1, y1, 0.0],
                [x0, y1, 0.0],
                [x0, y0, 0.0],
            ],
        }
    }
    fn source(name: &str) -> Definition {
        super::super::test_source::recipe(name)
    }
    fn candidate(analysis: &SlabAnalysis) -> Vec<SectionLoopMm> {
        let z = analysis.sample_world_z_mm().unwrap();
        analysis
            .section()
            .loops()
            .iter()
            .map(|ring| SectionLoopMm {
                is_hole: ring.is_hole,
                points_mm: ring
                    .points_mm
                    .iter()
                    .map(|point| [point[0], point[1], z])
                    .collect(),
            })
            .collect()
    }

    #[test]
    fn constant_coedge_closed_endpoint_ownership_survives_large_translation() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(10_000);
        let delta = 2.0_f64.powi(-52);
        for translation in [0.0, 16.0, -16.0] {
            let functional = HeightFunctional {
                direction: [0.0, 0.0, 1.0].map(Interval::point),
                offset: Interval::point(translation),
            };
            let range = [translation, translation + 0.5];
            for (z, outside) in [
                (-delta, true),
                (0.0, false),
                (delta, false),
                (0.5, false),
                (0.5 + delta, true),
            ] {
                let curve: Curve = solidcore::curve::line::Line::from_points(
                    solidcore::math::Point3::new(0.0, 0.0, z),
                    solidcore::math::Point3::new(1.0, 0.0, z), 3,
                ).into();
                let height = super::super::slab::constant_native_curve_height(
                    &curve, functional, &mut work, &cancel,
                )
                .unwrap()
                .unwrap();
                let band =
                    constant_coedge_band(&curve, functional, height, range, &mut work, &cancel)
                        .unwrap();
                if outside {
                    assert!(matches!(band, ConstantCoedgeBand::Outside));
                } else {
                    assert!(matches!(band, ConstantCoedgeBand::Contained));
                }
            }
        }
    }

    #[test]
    fn projected_original_vertex_link_requires_one_complete_material_winding() {
        let rays = [[1.0, 0.0], [0.0, 1.0], [-1.0, 0.0], [0.0, -1.0]];
        for sign in [-1, 1] {
            let mut once = ProjectedCriticalLink::new(sign);
            for index in 0..4 {
                let mut pair = [rays[index], rays[(index + 1) % 4]];
                if sign < 0 {
                    pair.reverse();
                }
                assert!(once.include(pair.map(|point| point.map(Interval::point))));
            }
            assert!(once.is_degree_one(4));
            assert!(!once.is_degree_one(5));
            for index in 0..4 {
                let mut pair = [rays[index], rays[(index + 1) % 4]];
                if sign < 0 {
                    pair.reverse();
                }
                assert!(once.include(pair.map(|point| point.map(Interval::point))));
            }
            assert!(
                !once.is_degree_one(8),
                "a twice-covered projected star is not a local graph"
            );
        }
        let mut missing = ProjectedCriticalLink::new(1);
        for index in 0..3 {
            assert!(
                missing.include(
                    [rays[index], rays[index + 1]].map(|point| point.map(Interval::point))
                )
            );
        }
        assert!(
            !missing.is_degree_one(4),
            "all original source wedges are necessary"
        );
        let mut reflex = ProjectedCriticalLink::new(1);
        assert!(
            !reflex.include([rays[0], rays[3]].map(|point| point.map(Interval::point))),
            "a reflex material wedge must not be replaced by its minor complement"
        );
        assert!(
            !reflex.include([rays[0], rays[2]].map(|point| point.map(Interval::point))),
            "an unresolved half-plane turn gives no projected-degree certificate"
        );
    }

    #[test]
    fn affine_fixed_segment_cover_does_not_bridge_actual_holes_or_nonconvex_gaps() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let holes = vec![
            rectangle(-3.0, -3.0, 3.0, 3.0, false),
            rectangle(-1.0, -1.0, 1.0, 1.0, true),
        ];
        let nonconvex = vec![SectionLoopMm {
            is_hole: false,
            points_mm: [
                [0.0, 0.0],
                [4.0, 0.0],
                [4.0, 4.0],
                [3.0, 4.0],
                [3.0, 1.0],
                [1.0, 1.0],
                [1.0, 4.0],
                [0.0, 4.0],
                [0.0, 0.0],
            ]
            .map(|p| [p[0], p[1], 0.0])
            .to_vec(),
        }];
        for (material, corners) in [
            (holes, [[-2.0, -2.0], [2.0, -2.0], [-2.0, 2.0], [2.0, 2.0]]),
            (nonconvex, [[0.5, 0.5], [3.5, 0.5], [0.5, 3.5], [3.5, 3.5]]),
        ] {
            for corner in corners {
                assert_eq!(
                    material_at(corner, &material, &mut work, &cancel).unwrap(),
                    Some(true)
                );
            }
            let image = SurfaceAffineImage {
                corners: corners.map(|corner| corner.map(Interval::point)),
                remainder_mm: 0.0,
            };
            assert!(
                affine_material_distance_upper(&image, &material, 0.1, &mut work, &cancel,)
                    .unwrap()
                    > 0.1,
                "different material edges cannot witness different affine corners"
            );
        }
    }

    #[test]
    fn affine_fixed_segment_cover_includes_the_whole_patch_remainder() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let material = [rectangle(-2.0, 0.0, 2.0, 2.0, false)];
        let image = SurfaceAffineImage {
            corners: [[-1.0, 0.02], [-1.0, 0.03], [1.0, 0.02], [1.0, 0.03]]
                .map(|corner| corner.map(Interval::point)),
            remainder_mm: 0.04,
        };
        let bound =
            affine_material_distance_upper(&image, &material, 0.08, &mut work, &cancel).unwrap();
        assert!(bound <= 0.08);
        assert!(
            affine_material_distance_upper(&image, &material, 0.06, &mut work, &cancel).unwrap()
                > 0.06,
            "a fixed-segment corner witness without the whole remainder cannot authorize coverage"
        );
    }

    #[test]
    fn affine_image_accepts_independent_uv_scales_but_retains_posed_chart_error() {
        let surface = linear_spline_surface(
            vec![
                vec![
                    CPoint::new(-2.0, -2.0, 0.0, NOW),
                    CPoint::new(-1.999, -2.001, 0.0, NOW),
                ],
                vec![CPoint::new(2.0, 2.0, 0.0, NOW), CPoint::new(2.001, 1.999, 0.0, NOW)],
            ],
            vec![1e-3, 1e-3, 2e-3, 2e-3],
            vec![1e6, 1e6, 2e6, 2e6],
        );
        let pose = RigidPoseMm {
            translation_mm: [17.0, -23.0, 2.0],
            rotation_xyzw: [0.0, 0.0, 0.185_f64.sin(), 0.185_f64.cos()],
        };
        let matrix = super::super::placed::rotation(pose).unwrap();
        let uv = [[1e-3, 2e-3], [1e6, 2e6]];
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let image = try_surface_affine_image(
            SurfaceAffineImageQuery {
                surface: &surface,
                uv,
                matrix,
                pose,
                chart_error_mm: 0.0,
                maximum_error_mm: 0.01,
            },
            &mut work,
            &cancel,
        )
        .unwrap()
        .unwrap();
        let qz = pose.rotation_xyzw[2];
        let qw = pose.rotation_xyzw[3];
        let cosine = 1.0 - 2.0 * qz * qz;
        let sine = 2.0 * qz * qw;
        for [x, y] in [[-2.0, -2.0], [-1.999, -2.001], [2.0, 2.0], [2.001, 1.999]] {
            let actual = [
                pose.translation_mm[0] + cosine * x - sine * y,
                pose.translation_mm[1] + sine * x + cosine * y,
            ];
            assert!(
                image.corners.iter().any(|corner| (0..2).all(|axis| {
                    corner[axis].lo - image.remainder_mm <= actual[axis]
                        && actual[axis] <= corner[axis].hi + image.remainder_mm
                })),
                "actual posed native corner escaped its complete affine image"
            );
        }
        assert!(
            try_surface_affine_image(
                SurfaceAffineImageQuery {
                    surface: &surface,
                    uv,
                    matrix,
                    pose,
                    chart_error_mm: 0.1,
                    maximum_error_mm: 0.01
                },
                &mut work,
                &cancel,
            )
            .unwrap()
            .is_none(),
            "physical source-chart error must not disappear under pose"
        );
    }

    #[test]
    fn affine_image_never_uses_piecewise_hessians_across_an_unqualified_c0_kink() {
        let surface = linear_spline_surface(
            [(0.0, 0.0), (0.5, 0.5), (1.0, 1.5)]
                .map(|(x, z)| vec![CPoint::new(x, 0.0, z, NOW), CPoint::new(x, 1.0, z, NOW)])
                .to_vec(),
            vec![0.0, 0.0, 0.5, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
        );
        let uv = [[0.0, 1.0]; 2];
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let pose = RigidPoseMm::IDENTITY;
        let matrix = super::super::placed::rotation(pose).unwrap();
        assert!(
            try_surface_affine_image(
                SurfaceAffineImageQuery {
                    surface: &surface,
                    uv,
                    matrix,
                    pose,
                    chart_error_mm: 0.0,
                    maximum_error_mm: 10.0
                },
                &mut work,
                &cancel,
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn original_port_image_requires_the_whole_parameter_span_and_bounds_longitudinal_drift() {
        let uv = [[0.0, 4.0], [0.0, 10.0]];
        let distance = |endpoints, port, speeds| {
            port_image_distance(
                uv,
                &[PortImageSpan {
                    anchor: 0,
                    endpoints,
                    uv: port,
                }],
                0,
                speeds,
                &mut GeometryWorkBudget::new(1000),
                &AtomicBool::new(false),
            )
            .unwrap()
        };
        let endpoints = [[[0.1, 0.2], [0.0, 0.0]], [[3.8, 3.9], [0.0, 0.0]]];
        let port = [[0.1, 3.9], [0.0, 0.0]];
        let vertical = distance(endpoints, port, [2.0, 0.0]);
        assert!((0.4..0.400000000001).contains(&vertical));
        assert!(distance(endpoints, port, [2.0, 1.0]) >= 10.4);
        let short = [[[1.0, 1.0], [0.0, 0.0]], [[3.0, 3.0], [0.0, 0.0]]];
        assert!(distance(short, port, [2.0, 0.0]) >= 2.0);
        let ambiguous = [[[1.0, 2.0], [0.0, 0.0]]; 2];
        assert!(distance(ambiguous, port, [2.0, 0.0]) >= 7.8);
    }

    #[test]
    fn original_port_union_preserves_periodic_gauges_gaps_and_anchor_identity() {
        let uv = [[0.0, 4.0], [0.0, 10.0]];
        let span = |a, b, anchor| PortImageSpan {
            anchor,
            endpoints: [[[a, a], [0.0, 0.0]], [[b, b], [0.0, 0.0]]],
            uv: [[a, b], [0.0, 0.0]],
        };
        let distance = |spans: &[PortImageSpan]| {
            port_image_distance(
                uv,
                spans,
                0,
                [2.0, 0.0],
                &mut GeometryWorkBudget::new(1000),
                &AtomicBool::new(false),
            )
            .unwrap()
        };
        assert!(distance(&[span(0.0, 2.0, 0), span(2.0, 4.0, 0)]) < 1e-10);
        assert!(distance(&[span(0.0, 2.0, 0)]) >= 4.0);
        assert!(distance(&[span(0.0, 2.0, 0), span(2.0, 4.0, 1)]) >= 4.0);
        assert!(distance(&[span(0.0, 1.5, 0), span(2.5, 4.0, 0)]) >= 1.0);
        // Literal gauges are never wrapped to make distant endpoint witnesses
        // look like a complete periodic span.
        assert!(distance(&[span(4.0, 6.0, 0), span(6.0, 8.0, 0)]) >= 8.0);
        let mut limited = GeometryWorkBudget::new(0);
        assert_eq!(
            port_image_distance(
                uv,
                &[span(0.0, 4.0, 0)],
                0,
                [2.0, 0.0],
                &mut limited,
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::ResourceLimit
        );
        let mut cancelled = GeometryWorkBudget::new(1000);
        assert_eq!(
            port_image_distance(
                uv,
                &[span(0.0, 4.0, 0)],
                0,
                [2.0, 0.0],
                &mut cancelled,
                &AtomicBool::new(true)
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::Cancelled
        );
        assert!(cancelled.consumed() > 0);
    }

    #[test]
    fn real_original_round_cap_and_lateral_image_reuse_complete_filled_material() {
        certify_real_cap_image(include_bytes!(
            "../../../../fixtures/geometry/cylinder.step"
        ));
    }

    #[test]
    fn actual_tapered_fitting_cap_uses_bounded_correlated_image_coverage() {
        certify_real_cap_image(include_bytes!(
            "../../../../fixtures/geometry/independent-exporters/occt-tapered-fitting-mm.step"
        ));
    }

    #[test]
    fn actual_rounded_housing_incoming_closed_leaf_retains_tangent_boundary_incidence() {
        let cancel = AtomicBool::new(false);
        let definition = crate::from_step_bytes_with_cancel(include_bytes!(
            "../../../../fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step"
        ),
        &cancel,)
        .unwrap();
        let pose = RigidPoseMm {
            translation_mm: [0.0, 0.0, -definition.bounds_mm().min[2]],
            ..RigidPoseMm::IDENTITY
        };
        let mut work = GeometryWorkBudget::new(100_000_000);
        let prepared = prepare_solid(&definition, crate::MAX_PREPARED_SECTION_SOURCE_BYTES, &mut work, &cancel)
        .unwrap();
        let analysis = prepared
            .analyze_placed_slab(
                &super::super::PlacedSlabRequest {
                    pose,
                    world_height_interval_mm: [0.0, 0.1],
                    sample_world_z_mm: 0.05,
                    native_shape_error_mm: 0.1,
                },
                crate::MAX_PREPARED_SECTION_BYTES.saturating_sub(prepared.retained_bytes()),
                &mut work,
                &cancel,
            )
            .expect(
                "a real fillet tangent to its incoming original cap needs closed native incidence",
            );
        assert!(!analysis.section().loops().is_empty());
        let actual = candidate(&analysis);
        let leaf = super::super::joint::certify_native_source_leaf(
            &super::super::joint::SourceLeafRequest {
                source: &prepared,
                occurrence_id: &OccurrenceId::new(76).unwrap(),
                pose,
                analysis: &analysis,
                actual_used_source_loops: &actual,
                policy: super::super::joint::NativeUnionPolicy {
                    candidate_coordinate_error_mm: 0.0,
                },
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(leaf.is_positive());
        assert_eq!(leaf.world_height_interval_mm(), [0.0, 0.1]);
        assert!(leaf.directed_sample_to_closed_source_error_mm() <= 0.1);
        let mut false_hole = actual;
        let mut hole = rectangle(11.0, 8.0, 13.0, 10.0, true);
        for point in &mut hole.points_mm {
            point[2] = 0.05;
        }
        false_hole.push(hole);
        let error = super::super::joint::certify_native_source_leaf(
            &super::super::joint::SourceLeafRequest {
                source: &prepared,
                occurrence_id: &OccurrenceId::new(77).unwrap(),
                pose,
                analysis: &analysis,
                actual_used_source_loops: &false_hole,
                policy: super::super::joint::NativeUnionPolicy {
                    candidate_coordinate_error_mm: 0.0,
                },
            },
            &mut work,
            &cancel,
        )
        .expect_err("a source critical star must not authorize an invented material hole");
        assert!(!matches!(
            error.code,
            GeometryErrorCode::ResourceLimit | GeometryErrorCode::Cancelled
        ));
    }

    #[test]
    fn actual_rounded_housing_outgoing_critical_vertices_bind_closed_source_leaf() {
        let cancel = AtomicBool::new(false);
        let definition = crate::from_step_bytes_with_cancel(include_bytes!(
            "../../../../fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step"
        ),
        &cancel,)
        .unwrap();
        let bounds = definition.bounds_mm();
        let pose = RigidPoseMm {
            translation_mm: [0.0, 0.0, -bounds.min[2]],
            ..RigidPoseMm::IDENTITY
        };
        let top = bounds.max[2] - bounds.min[2];
        let interval = [top - 0.1, top];
        let mut work = GeometryWorkBudget::new(100_000_000);
        let prepared = prepare_solid(&definition, crate::MAX_PREPARED_SECTION_SOURCE_BYTES, &mut work, &cancel)
        .unwrap();
        let analysis = prepared
            .analyze_placed_slab(
                &super::super::PlacedSlabRequest {
                    pose,
                    world_height_interval_mm: interval,
                    sample_world_z_mm: interval[0] + (interval[1] - interval[0]) * 0.5,
                    native_shape_error_mm: 0.1,
                },
                crate::MAX_PREPARED_SECTION_BYTES.saturating_sub(prepared.retained_bytes()),
                &mut work,
                &cancel,
            )
            .expect("the complete outgoing original critical-vertex star must remain owned");
        assert!(!analysis.section().loops().is_empty());
        let actual = candidate(&analysis);
        let occurrence_id = OccurrenceId::new(78).unwrap();
        let leaf = super::super::joint::certify_native_source_leaf(
            &super::super::joint::SourceLeafRequest {
                source: &prepared,
                occurrence_id: &occurrence_id,
                pose,
                analysis: &analysis,
                actual_used_source_loops: &actual,
                policy: super::super::joint::NativeUnionPolicy {
                    candidate_coordinate_error_mm: 0.0,
                },
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(leaf.is_positive());
        assert_eq!(leaf.world_height_interval_mm(), interval);
        assert!(leaf.directed_sample_to_closed_source_error_mm() <= 0.1);
    }

    fn certify_real_cap_image(source: &[u8]) {
        let cancel = AtomicBool::new(false);
        let definition = crate::from_step_bytes_with_cancel(source, &cancel).unwrap();
        let mut work = GeometryWorkBudget::new(100_000_000);
        let prepared = prepare_solid(&definition, crate::MAX_PREPARED_SECTION_SOURCE_BYTES, &mut work, &cancel)
        .unwrap();
        let base = definition.bounds_mm().min[2];
        let pose = RigidPoseMm::IDENTITY;
        let current = prepared
            .analyze_placed_slab(
                &super::super::PlacedSlabRequest {
                    pose,
                    world_height_interval_mm: [base, base + 0.2],
                    sample_world_z_mm: base + 0.1,
                    native_shape_error_mm: 0.3,
                },
                crate::MAX_PREPARED_SECTION_BYTES.saturating_sub(prepared.retained_bytes()),
                &mut work,
                &cancel,
            )
            .unwrap();
        let actual = candidate(&current);
        let leaf = super::super::joint::certify_native_source_leaf(
            &super::super::joint::SourceLeafRequest {
                source: &prepared,
                occurrence_id: &OccurrenceId::new(75).unwrap(),
                pose,
                analysis: &current,
                actual_used_source_loops: &actual,
                policy: super::super::joint::NativeUnionPolicy {
                    candidate_coordinate_error_mm: 0.0,
                },
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        let inputs = [NativeCriticalCollarTarget {
            leaf: &leaf,
            actual_material: &actual,
        }];
        let policy = NativeCriticalCollarPolicy {
            maximum_xy_cover_error_mm: 0.05,
            maximum_distance_mm: 0.3,
            byte_limit: MAX_BYTES as u32,
        };
        // Isolate the collar's work selector; this is not a whole Compile budget.
        let mut metric_work = GeometryWorkBudget::new(2_000_000);
        let collar = prepared.certify_native_critical_collar(&current,
        &inputs,
        policy,
        &mut metric_work,
        &cancel,)
        .unwrap();
        assert!(collar.achieved_xy_cover_error_mm() <= policy.maximum_xy_cover_error_mm);
        assert!(collar.closed_source_to_material_error_mm() <= policy.maximum_distance_mm);
        assert!(
            collar
                .native_event_ports()
                .iter()
                .enumerate()
                .all(|(index, _)| collar.projected_cover().iter().any(|cell| cell
                    .native_port_parameter()
                    .is_some_and(|(port, _)| port == index)))
        );
        let mut limited = GeometryWorkBudget::new(100_000_000);
        assert_eq!(
            prepared.certify_native_critical_collar(
                &current,
                &inputs,
                NativeCriticalCollarPolicy {
                    byte_limit: 1,
                    ..policy
                },
                &mut limited,
                &cancel,
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::ResourceLimit
        );
        let mut cancelled = GeometryWorkBudget::new(100_000_000);
        assert_eq!(
            prepared.certify_native_critical_collar(
                &current,
                &inputs,
                policy,
                &mut cancelled,
                &AtomicBool::new(true),
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::Cancelled
        );
        assert!(cancelled.consumed() > 0);
    }

    #[test]
    fn all_cell_material_cover_does_not_fill_a_protected_hole() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let material = [
            rectangle(-4.0, -4.0, 4.0, 4.0, false),
            rectangle(-1.0, -1.0, 1.0, 1.0, true),
        ];
        assert_eq!(
            material_distance_upper([[2.0, 2.5], [2.0, 2.5]], &material, 0.0, &mut work, &cancel)
                .unwrap(),
            0.0
        );
        let gap = material_distance_upper(
            [[-0.1, 0.1], [-0.1, 0.1]],
            &material,
            0.0,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(
            gap >= 1.0,
            "an actual material hole is not filled by its outer bounding rectangle"
        );
        let wide = material_distance_upper(
            [[-3.0, 3.0], [-3.0, 3.0]],
            &material,
            0.0,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(
            wide > 1.0,
            "a covered center never establishes whole-cell coverage"
        );
    }

    #[test]
    fn filled_original_projection_accepts_unequal_partitions_but_not_a_bridged_concavity() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        // Rotate an L-shaped material domain so exact orthogonal-vertex
        // reduction cannot erase the deliberately different partitions.
        let xy = [
            [0.0, 0.0],
            [4.0, 0.0],
            [4.0, 1.0],
            [1.0, 1.0],
            [1.0, 4.0],
            [0.0, 4.0],
            [0.0, 0.0],
        ];
        let point = |p: [f64; 2]| [p[0] - p[1], p[0] + p[1], 0.0];
        let original = [SectionLoopMm {
            is_hole: false,
            points_mm: xy.into_iter().map(point).collect(),
        }];
        let mut partitioned = Vec::new();
        for (index, edge) in xy.windows(2).enumerate() {
            partitioned.push(point(edge[0]));
            let t = if index % 2 == 0 { 0.25 } else { 0.5 };
            partitioned.push(point(std::array::from_fn(|axis| {
                edge[0][axis] + t * (edge[1][axis] - edge[0][axis])
            })));
        }
        partitioned.push(point(xy[0]));
        let candidate = [SectionLoopMm {
            is_hole: false,
            points_mm: partitioned,
        }];
        assert!(
            super::super::joint::filled_projection_distance(
                &original,
                &candidate,
                0.0,
                1e-10,
                1 << 20,
                &mut work,
                &cancel,
            )
            .unwrap()
                <= 1e-10
        );
        // Same outer inventory and orientation, but the proposed material
        // bridges a real nonconvex gap. Boundary/corner membership is not a
        // filled coverage proof.
        let bridged = [SectionLoopMm {
            is_hole: false,
            points_mm: [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0], [0.0, 0.0]]
                .into_iter()
                .map(point)
                .collect(),
        }];
        assert_eq!(
            super::super::joint::filled_projection_distance(
                &original,
                &bridged,
                0.0,
                0.01,
                1 << 20,
                &mut work,
                &cancel,
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::AnalysisIndeterminate
        );
    }

    #[test]
    fn filled_convex_witness_uses_whole_material_not_outer_corners() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let nonconvex = [SectionLoopMm {
            is_hole: false,
            points_mm: vec![
                [0.0, 0.0, 0.0],
                [4.0, 0.0, 0.0],
                [4.0, 1.0, 0.0],
                [1.0, 1.0, 0.0],
                [1.0, 4.0, 0.0],
                [0.0, 4.0, 0.0],
                [0.0, 0.0, 0.0],
            ],
        }];
        let distance = material_distance_upper(
            [[-1e-9, 4.0 + 1e-9], [-1e-9, 1.0 + 1e-9]],
            &nonconvex,
            0.025,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!((1e-9..=0.025).contains(&distance));
        let holed = [
            rectangle(-4.0, -4.0, 4.0, 4.0, false),
            rectangle(-1.0, -1.0, 1.0, 1.0, true),
        ];
        assert!(
            material_distance_upper(
                [[-3.0, 3.0], [-3.0, 3.0]],
                &holed,
                0.025,
                &mut work,
                &cancel,
            )
            .unwrap()
                > 1.0,
            "material corners cannot authorize a convex witness across a protected hole"
        );
    }

    #[test]
    fn rational_constant_height_and_translation_remain_intervals() {
        let surface = linear_spline_surface(
            vec![
                vec![
                    CPoint::new(0.0, 0.0, 1.0, 3.0),
                    CPoint::new(0.0, 3.0, 1.0, 3.0),
                ],
                vec![
                    CPoint::new(3.0, 0.0, 1.0, 3.0),
                    CPoint::new(3.0, 3.0, 1.0, 3.0),
                ],
            ],
            vec![1e-3, 1e-3, 2e-3, 2e-3],
            vec![1e6, 1e6, 2e6, 2e6],
        );
        let pose = RigidPoseMm {
            translation_mm: [0.0, 0.0, 0.1],
            ..RigidPoseMm::IDENTITY
        };
        let matrix = super::super::placed::rotation(pose).unwrap();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let uv = [[1e-3, 2e-3], [1e6, 2e6]];
        let jet = surface_enclosure(&surface, uv, &mut work, &cancel).unwrap();
        let source_h = jet.position[2];
        assert!(source_h.lo < source_h.hi);
        assert!(source_h.lo <= 1.0 / 3.0 && source_h.hi >= 1.0 / 3.0);
        let world_h =
            super::super::placed::transformed_position(matrix, pose, jet.position).unwrap()[2];
        assert!(
            world_h.lo < world_h.hi,
            "nonrepresentable rational plus translation is never midpoint equality"
        );
        let functional = HeightFunctional {
            direction: matrix[2],
            offset: Interval::point(pose.translation_mm[2]),
        };
        let (clipped, _) = clip_height_first_box(
            &surface,
            uv,
            functional,
            [world_h.lo, world_h.hi],
            &mut work,
            &cancel,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            clipped, uv,
            "the entire actual constant-height face survives its true interval event band"
        );
        assert!(
            clip_height_first_box(&surface, uv, functional, [0.6, 0.7], &mut work, &cancel)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn height_clipping_retains_continuous_kink_roots_but_refuses_position_jumps() {
        let surface = |jump: bool| {
            let (knots, profile): (&[f64], &[(f64, f64)]) = if jump {
                (
                    &[0.0, 0.0, 0.5, 0.5, 1.0, 1.0],
                    &[(0.0, 0.0), (0.5, 0.5), (0.5, 0.75), (1.0, 1.5)],
                )
            } else {
                // A degree-one simple interior knot is genuinely C0:
                // positions agree, but its two height derivatives differ.
                (
                    &[0.0, 0.0, 0.5, 1.0, 1.0],
                    &[(0.0, 0.0), (0.5, 0.5), (1.0, 1.5)],
                )
            };
            linear_spline_surface(
                profile
                    .iter()
                    .map(|&(x, z)| vec![CPoint::new(x, 0.0, z, NOW), CPoint::new(x, 1.0, z, NOW)])
                    .collect(),
                knots.to_vec(),
                vec![0.0, 0.0, 1.0, 1.0],
            )
        };
        let cancel = AtomicBool::new(false);
        let pose = RigidPoseMm::IDENTITY;
        let matrix = super::super::placed::rotation(pose).unwrap();
        let functional = HeightFunctional {
            direction: matrix[2],
            offset: Interval::point(pose.translation_mm[2]),
        };
        let mut work = GeometryWorkBudget::new(100_000);
        let continuous = surface(false);
        let uv = [[0.0, 1.0]; 2];
        let (retained, first) = clip_height_first_box(
            &continuous,
            uv,
            functional,
            [0.25, 0.75],
            &mut work,
            &cancel,
        )
        .unwrap()
        .expect("the continuous kink has actual height roots on both sides");
        // z=u on the first piece and z=2u-0.5 on the second. The
        // necessary cover must retain every root, not only the knot itself.
        assert!(retained[0][0] <= 0.25 && retained[0][1] >= 0.625);
        assert_eq!(retained[1], uv[1]);
        assert!(first.position[2].lo <= 0.25 && first.position[2].hi >= 0.75);
        assert!(first.du[2].lo <= 1.0 && first.du[2].hi >= 2.0);
        assert!(
            clip_height_first_box(
                &continuous,
                uv,
                functional,
                [10.0, 11.0],
                &mut work,
                &cancel,
            )
            .unwrap()
            .is_none()
        );
        assert_eq!(
            clip_height_first_box(
                &surface(true),
                uv,
                functional,
                [0.25, 0.75],
                &mut work,
                &cancel,
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::SingularGeometry,
            "a positional jump cannot supply the continuous-source mean-value theorem"
        );
    }

    #[test]
    fn genuine_closed_product_forward_and_empty_forward_are_not_half_slab_height() {
        let definition = source("native-parity-bspline-box.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(500_000_000);
        let prepared = prepare_solid(&definition, crate::MAX_PREPARED_SECTION_SOURCE_BYTES, &mut work, &cancel)
        .unwrap();
        let pose = RigidPoseMm::IDENTITY;
        let occurrence = OccurrenceId::new(74).unwrap();
        let current = prepared
            .analyze_placed_slab(
                &super::super::PlacedSlabRequest {
                    pose,
                    world_height_interval_mm: [0.0, 0.2],
                    sample_world_z_mm: 0.1,
                    native_shape_error_mm: 0.3,
                },
                crate::MAX_PREPARED_SECTION_BYTES.saturating_sub(prepared.retained_bytes()),
                &mut work,
                &cancel,
            )
            .unwrap();
        let actual = candidate(&current);
        let leaf = super::super::joint::certify_native_source_leaf(
            &super::super::joint::SourceLeafRequest {
                source: &prepared,
                occurrence_id: &occurrence,
                pose,
                analysis: &current,
                actual_used_source_loops: &actual,
                policy: super::super::joint::NativeUnionPolicy {
                    candidate_coordinate_error_mm: 0.0,
                },
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        let policy = NativeCriticalCollarPolicy {
            maximum_xy_cover_error_mm: 0.05,
            maximum_distance_mm: 0.3,
            byte_limit: MAX_BYTES as u32,
        };
        let collar = prepared.certify_native_critical_collar(&current,
        &[NativeCriticalCollarTarget {
            leaf: &leaf,
            actual_material: &actual,
        }],
        policy,
        &mut work,
        &cancel,)
        .unwrap();
        assert_eq!(
            collar.sample_to_closed_source_error_mm(),
            leaf.directed_sample_to_closed_source_error_mm()
        );
        assert!(
            collar.sample_to_closed_source_error_mm() < 0.1,
            "the genuine native closed product is stronger than the generic half-slab-height bound"
        );
        assert!(
            collar
                .used_material_targets()
                .any(|target| target.side() == NativeCriticalCollarTargetSide::Current)
        );
        drop((collar, leaf, actual, current));

        let empty_current = prepared
            .analyze_placed_slab(
                &super::super::PlacedSlabRequest {
                    pose,
                    world_height_interval_mm: [-0.2, 0.01],
                    sample_world_z_mm: -0.1,
                    native_shape_error_mm: 0.3,
                },
                crate::MAX_PREPARED_SECTION_BYTES.saturating_sub(prepared.retained_bytes()),
                &mut work,
                &cancel,
            )
            .unwrap();
        assert!(empty_current.section().loops().is_empty());
        let adjacent = prepared
            .analyze_placed_slab(
                &super::super::PlacedSlabRequest {
                    pose,
                    world_height_interval_mm: [0.01, 0.21],
                    sample_world_z_mm: 0.1,
                    native_shape_error_mm: 0.3,
                },
                crate::MAX_PREPARED_SECTION_BYTES
                    .saturating_sub(prepared.retained_bytes())
                    .saturating_sub(empty_current.retained_bytes()),
                &mut work,
                &cancel,
            )
            .unwrap();
        let actual = candidate(&adjacent);
        let leaf = super::super::joint::certify_native_source_leaf(
            &super::super::joint::SourceLeafRequest {
                source: &prepared,
                occurrence_id: &occurrence,
                pose,
                analysis: &adjacent,
                actual_used_source_loops: &actual,
                policy: super::super::joint::NativeUnionPolicy {
                    candidate_coordinate_error_mm: 0.0,
                },
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        let collar = prepared.certify_native_critical_collar(&empty_current,
        &[NativeCriticalCollarTarget {
            leaf: &leaf,
            actual_material: &actual,
        }],
        policy,
        &mut work,
        &cancel,)
        .unwrap();
        assert_eq!(collar.sample_to_closed_source_error_mm(), 0.0);
        assert!(
            !collar.projected_cover().is_empty(),
            "empty actual sample does not erase CLOSED native occupancy"
        );
        assert!(
            collar
                .used_material_targets()
                .any(|target| target.side() == NativeCriticalCollarTargetSide::Upper)
        );
    }

    #[test]
    fn real_blind_pocket_and_native_nurbs_floor_own_full_nonzero_collar() {
        for name in [
            "native-parity-blind-pocket.step",
            "native-parity-blind-pocket-nurbs-floor.step",
        ] {
            let definition = source(name);
            let cancel = AtomicBool::new(false);
            let mut work = GeometryWorkBudget::new(300_000_000);
            let request = super::super::SlabRequest {
                frame: frame(),
                height_interval_mm: [3.99, 4.01],
                sample_height_mm: 4.005,
                native_shape_error_mm: 0.1,
            };
            let prepared = prepare_solid(&definition, crate::MAX_PREPARED_SECTION_SOURCE_BYTES, &mut work, &cancel).unwrap();
            let complex = certify_native_collar_complex(
                prepared.source_view().unwrap(),
                &request,
                super::super::HeightFunctional::from_frame(frame()),
                MAX_BYTES,
                &mut work,
                &cancel,
            )
            .unwrap()
            .expect("actual horizontal floor complex");
            assert_eq!(complex.event_faces().count(), 1);
            assert!(complex.event_faces().all(|(_, _, sign)| sign > 0));
            assert!(!complex.ports.is_empty());
            drop(complex);
            // 4 + binary64(0.1) is not representable. The true placed event
            // stays inside a nonzero interval; no endpoint equality path applies.
            let pose = RigidPoseMm {
                translation_mm: [0.0, 0.0, 0.1],
                ..RigidPoseMm::IDENTITY
            };
            let adjacent = prepared
                .analyze_placed_slab(
                    &super::super::PlacedSlabRequest {
                        pose,
                        world_height_interval_mm: [4.07, 4.09],
                        sample_world_z_mm: 4.08,
                        native_shape_error_mm: 0.1,
                    },
                    crate::MAX_PREPARED_SECTION_BYTES.saturating_sub(prepared.retained_bytes()),
                    &mut work,
                    &cancel,
                )
                .unwrap();
            let current = prepared
                .analyze_placed_slab(
                    &super::super::PlacedSlabRequest {
                        pose,
                        world_height_interval_mm: [4.09, 4.11],
                        sample_world_z_mm: 4.105,
                        native_shape_error_mm: 0.1,
                    },
                    crate::MAX_PREPARED_SECTION_BYTES
                        .saturating_sub(prepared.retained_bytes())
                        .saturating_sub(adjacent.retained_bytes()),
                    &mut work,
                    &cancel,
                )
                .unwrap();
            assert_eq!(
                current
                    .section()
                    .loops()
                    .iter()
                    .filter(|ring| ring.is_hole)
                    .count(),
                1
            );
            assert_eq!(
                adjacent
                    .section()
                    .loops()
                    .iter()
                    .filter(|ring| ring.is_hole)
                    .count(),
                0
            );
            let actual = candidate(&adjacent);
            let leaf = super::super::joint::certify_native_source_leaf(
                &super::super::joint::SourceLeafRequest {
                    source: &prepared,
                    occurrence_id: &OccurrenceId::new(73).unwrap(),
                    pose,
                    analysis: &adjacent,
                    actual_used_source_loops: &actual,
                    policy: super::super::joint::NativeUnionPolicy {
                        candidate_coordinate_error_mm: 0.0,
                    },
                },
                &mut work,
                &cancel,
            )
            .unwrap();
            let current_actual = candidate(&current);
            let current_leaf = super::super::joint::certify_native_one_sided_source_leaf(
                &super::super::joint::SourceLeafRequest {
                    source: &prepared,
                    occurrence_id: &OccurrenceId::new(73).unwrap(),
                    pose,
                    analysis: &current,
                    actual_used_source_loops: &current_actual,
                    policy: super::super::joint::NativeUnionPolicy {
                        candidate_coordinate_error_mm: 0.0,
                    },
                },
                &mut work,
                &cancel,
            )
            .unwrap();
            // A shared filled-region receipt is reusable only for its exact
            // captured actual M. Matching outer envelopes cannot authorize a
            // candidate that silently fills the current protected hole.
            let without_hole: Vec<_> = candidate(&current)
                .into_iter()
                .filter(|ring| !ring.is_hole)
                .collect();
            assert_eq!(
                prepared.certify_native_critical_collar(
                    &current,
                    &[
                        NativeCriticalCollarTarget {
                            leaf: &leaf,
                            actual_material: &actual,
                        },
                        NativeCriticalCollarTarget {
                            leaf: &current_leaf,
                            actual_material: &without_hole,
                        },
                    ],
                    NativeCriticalCollarPolicy {
                        maximum_xy_cover_error_mm: 0.04,
                        maximum_distance_mm: 0.1,
                        byte_limit: MAX_BYTES as u32,
                    },
                    &mut work,
                    &cancel,
                )
                .unwrap_err()
                .code,
                GeometryErrorCode::InvalidGeometry,
                "native filled-sample reuse cannot lose an actual protected hole"
            );
            assert_eq!(
                prepared.certify_native_critical_collar(
                    &current,
                    &[NativeCriticalCollarTarget {
                        leaf: &current_leaf,
                        actual_material: &current_actual,
                    }],
                    NativeCriticalCollarPolicy {
                        maximum_xy_cover_error_mm: 0.04,
                        maximum_distance_mm: 0.1,
                        byte_limit: MAX_BYTES as u32,
                    },
                    &mut work,
                    &cancel,
                )
                .unwrap_err()
                .code,
                GeometryErrorCode::AnalysisIndeterminate,
                "the current ring cannot replace the missing material-facing floor owner"
            );
            let proof = prepared.certify_native_critical_collar(&current,
            &[
                NativeCriticalCollarTarget {
                    leaf: &leaf,
                    actual_material: &actual,
                },
                NativeCriticalCollarTarget {
                    leaf: &current_leaf,
                    actual_material: &current_actual,
                },
            ],
            NativeCriticalCollarPolicy {
                maximum_xy_cover_error_mm: 0.04,
                maximum_distance_mm: 0.1,
                byte_limit: MAX_BYTES as u32,
            },
            &mut work,
            &cancel,)
            .unwrap();
            assert_eq!(
                proof.material_targets()[0].side(),
                NativeCriticalCollarTargetSide::Lower
            );
            assert!(
                proof
                    .projected_cover()
                    .iter()
                    .any(|cell| cell.source_face_ordinal().is_some())
            );
            assert!(proof.closed_source_to_material_error_mm() <= 0.1);
            assert_eq!(
                proof.material_targets()[0]
                    .leaf()
                    .candidate_geometry_sha256(),
                leaf.candidate_geometry_sha256()
            );
            assert!(!proof.native_event_ports().is_empty());
            assert!(
                proof
                    .projected_cover()
                    .iter()
                    .any(|cell| cell.native_port_parameter().is_some())
            );
            assert!(
                proof
                    .used_material_targets()
                    .any(|target| target.side() == NativeCriticalCollarTargetSide::Current)
            );
            assert!(
                proof
                    .used_material_targets()
                    .any(|target| target.side() == NativeCriticalCollarTargetSide::Lower)
            );
            assert!(
                proof
                    .projected_cover()
                    .iter()
                    .filter(|cell| cell.native_port_parameter().is_some())
                    .all(|cell| cell.material_target_index() == 0),
                "every genuine floor port faces the actual lower owner, not the current ring"
            );
            for cell in proof.projected_cover() {
                let occupied = [
                    current.world_height_interval_mm().unwrap()[0]
                        .max(proof.source_world_z_support().minimum[0]),
                    current.world_height_interval_mm().unwrap()[1]
                        .min(proof.source_world_z_support().maximum[1]),
                ];
                let event_port = proof
                    .native_event_ports()
                    .iter()
                    .find(|port| Some(port.event_face_ordinal()) == cell.source_face_ordinal());
                let expected = if let Some(port) = event_port {
                    assert_eq!(port.height_space(), NativeCollarHeightSpace::World);
                    assert!(port.height_interval_mm()[0] > 4.0);
                    [occupied[0], occupied[1].min(port.height_interval_mm()[1])]
                } else {
                    occupied
                };
                assert_eq!(
                    cell.world_height_interval_mm(),
                    expected,
                    "retained cells cover full material-facing source shadows, not only boundary heights"
                );
                let target = &proof.material_targets()[cell.material_target_index()];
                assert!(
                    cell.material_distance_mm()
                        >= target_height_distance(target, cell.world_height_interval_mm()),
                    "the published metric covers every retained source-column height"
                );
            }
        }
    }

    #[test]
    fn disjoint_opposing_void_events_keep_the_common_sample_and_opposite_owners() {
        let definition = source("native-parity-blind-pocket-nurbs-floor.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(500_000_000);
        let prepared = prepare_solid(&definition, crate::MAX_PREPARED_SECTION_SOURCE_BYTES, &mut work, &cancel)
        .unwrap();
        let poses = [
            RigidPoseMm {
                translation_mm: [0.0, 0.0, 0.1],
                ..RigidPoseMm::IDENTITY
            },
            RigidPoseMm {
                translation_mm: [40.0, 20.0, 8.107],
                rotation_xyzw: [1.0, 0.0, 0.0, 0.0],
            },
        ];
        let mut proofs: Vec<NativeCriticalCollar> = Vec::new();
        for (index, pose) in poses.into_iter().enumerate() {
            let remaining = crate::MAX_PREPARED_SECTION_BYTES
                .saturating_sub(prepared.retained_bytes())
                .saturating_sub(
                    (proofs.capacity() - proofs.len())
                        * std::mem::size_of::<NativeCriticalCollar>(),
                )
                .saturating_sub(
                    proofs
                        .iter()
                        .map(NativeCriticalCollar::retained_bytes)
                        .sum::<usize>(),
                );
            let current = prepared
                .analyze_placed_slab(
                    &super::super::PlacedSlabRequest {
                        pose,
                        world_height_interval_mm: [4.09, 4.11],
                        sample_world_z_mm: 4.105,
                        native_shape_error_mm: 0.1,
                    },
                    remaining,
                    &mut work,
                    &cancel,
                )
                .unwrap();
            assert_eq!(
                current
                    .section()
                    .loops()
                    .iter()
                    .filter(|ring| ring.is_hole)
                    .count(),
                1,
                "both true void phases retain their actual common ring sample"
            );
            let (range, sample) = if index == 0 {
                ([4.07, 4.09], 4.08)
            } else {
                ([4.11, 4.13], 4.12)
            };
            let adjacent = prepared
                .analyze_placed_slab(
                    &super::super::PlacedSlabRequest {
                        pose,
                        world_height_interval_mm: range,
                        sample_world_z_mm: sample,
                        native_shape_error_mm: 0.1,
                    },
                    remaining.saturating_sub(current.retained_bytes()),
                    &mut work,
                    &cancel,
                )
                .unwrap();
            let actual = candidate(&adjacent);
            let leaf = super::super::joint::certify_native_source_leaf(
                &super::super::joint::SourceLeafRequest {
                    source: &prepared,
                    occurrence_id: &OccurrenceId::new(index as u32 + 91).unwrap(),
                    pose,
                    analysis: &adjacent,
                    actual_used_source_loops: &actual,
                    policy: super::super::joint::NativeUnionPolicy {
                        candidate_coordinate_error_mm: 0.0,
                    },
                },
                &mut work,
                &cancel,
            )
            .unwrap();
            let current_actual = candidate(&current);
            let current_leaf = super::super::joint::certify_native_source_leaf(
                &super::super::joint::SourceLeafRequest {
                    source: &prepared,
                    occurrence_id: &OccurrenceId::new(index as u32 + 91).unwrap(),
                    pose,
                    analysis: &current,
                    actual_used_source_loops: &current_actual,
                    policy: super::super::joint::NativeUnionPolicy {
                        candidate_coordinate_error_mm: 0.0,
                    },
                },
                &mut work,
                &cancel,
            )
            .unwrap();
            let proof = prepared.certify_native_critical_collar(&current,
            &[
                NativeCriticalCollarTarget {
                    leaf: &leaf,
                    actual_material: &actual,
                },
                NativeCriticalCollarTarget {
                    leaf: &current_leaf,
                    actual_material: &current_actual,
                },
            ],
            NativeCriticalCollarPolicy {
                maximum_xy_cover_error_mm: 0.04,
                maximum_distance_mm: 0.1,
                byte_limit: MAX_BYTES as u32,
            },
            &mut work,
            &cancel,)
            .unwrap();
            assert_eq!(proof.sample_world_z_mm(), 4.105);
            assert_eq!(
                proof.material_targets()[0]
                    .leaf()
                    .candidate_geometry_sha256(),
                leaf.candidate_geometry_sha256()
            );
            proofs.push(proof);
        }
        assert_eq!(
            proofs[0].material_targets()[0].side(),
            NativeCriticalCollarTargetSide::Lower
        );
        assert_eq!(
            proofs[1].material_targets()[0].side(),
            NativeCriticalCollarTargetSide::Upper
        );
        assert_ne!(proofs[0].complex_sha256(), proofs[1].complex_sha256());
        assert!(proofs[0].occupied_xy_bounds_mm()[0][1] < proofs[1].occupied_xy_bounds_mm()[0][0]);
    }

    #[test]
    fn actual_nonorthogonal_pose_keeps_world_event_intervals_and_original_ports() {
        let definition = source("native-parity-blind-pocket-nurbs-floor.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(300_000_000);
        let request = super::super::SlabRequest {
            frame: frame(),
            height_interval_mm: [3.99, 4.01],
            sample_height_mm: 4.005,
            native_shape_error_mm: 0.1,
        };
        let source_owner = prepare_solid(&definition, MAX_BYTES, &mut work, &cancel).unwrap();
        let seed = certify_native_collar_complex(
            source_owner.source_view().unwrap(),
            &request,
            super::super::HeightFunctional::from_frame(frame()),
            MAX_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap()
        .unwrap();
        let scale = 1.0 + 2e-11;
        let pose = RigidPoseMm {
            translation_mm: [7.0, -3.0, 0.1],
            rotation_xyzw: [0.0, 0.0, 0.2_f64.sin() * scale, 0.2_f64.cos() * scale],
        };
        let matrix = super::super::placed::rotation(pose).unwrap();
        let length = dot(matrix[0], matrix[0]);
        assert!(
            length.hi > 1.0,
            "the accepted actual polynomial is not silently orthogonalized"
        );
        let world = place_native_collar_complex(
            source_owner.source_view().unwrap(),
            PlacedCollarComplexQuery {
                seed: &seed,
                pose,
                interval: [4.09, 4.11],
                sample: 4.105,
            },
            MAX_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert_eq!(world.height_space(), NativeCollarHeightSpace::World);
        assert_eq!(world.request.height_interval_mm, request.height_interval_mm);
        assert_eq!(world.world_capture.unwrap().pose, pose);
        assert_eq!(world.ports.len(), seed.ports.len());
        assert!(seed.world_regular_boundary_xy_speed_mm_per_mm().is_none());
        assert!(
            world
                .world_regular_boundary_xy_speed_mm_per_mm()
                .is_some_and(|speed| speed.is_finite() && speed >= 0.0)
        );
        for (face, height, sign) in world.event_faces() {
            assert_eq!(sign, 1);
            assert!(height[0] >= 4.09 && height[1] <= 4.11);
            assert!(
                seed.event_faces()
                    .any(|(source_face, _, _)| source_face == face)
            );
        }
        assert!(
            world
                .ports
                .iter()
                .all(|port| port.height_space() == NativeCollarHeightSpace::World)
        );
        assert_ne!(world.native_complex_sha256(), seed.native_complex_sha256());
    }

    #[test]
    fn regular_world_xy_flow_uses_real_height_derivatives_not_material_distance() {
        let matrix = super::super::placed::rotation(RigidPoseMm::IDENTITY).unwrap();
        let z = Interval::point(0.0);
        assert_eq!(
            world_xy_tangent_speed(
                [z, z, Interval::point(7.0)],
                Interval { lo: 6.9, hi: 7.1 },
                matrix
            ),
            Some(0.0)
        );
        assert!(
            world_xy_tangent_speed(
                [
                    Interval::point(3.0),
                    Interval::point(4.0),
                    Interval::point(2.0)
                ],
                Interval::point(2.0),
                matrix
            )
            .is_some_and(|speed| speed >= 2.5)
        );
        assert!(
            world_xy_tangent_speed([z, z, z], Interval { lo: -0.1, hi: 0.1 }, matrix).is_none(),
            "a vanishing/uncertain height derivative is never normalized into a flow witness"
        );
        let jet = crate::parametric::SurfaceEnclosure {
            position: [z; 3],
            du: [Interval::point(2.0), z, Interval::point(1.0)],
            dv: [z, z, Interval::point(3.0)],
            duu: [z; 3],
            duv: [z; 3],
            dvv: [z; 3],
        };
        assert_eq!(
            best_face_world_xy_speed(
                &jet,
                super::super::HeightFunctional::from_frame(frame()),
                matrix
            ),
            Some(0.0),
            "the genuine vertical chart direction beats a moving alternative without erasing either derivative obligation"
        );
    }

    #[test]
    fn nonzero_piece_gradients_do_not_erase_a_true_internal_knot_height_event() {
        let raw = crv_basic::Curve::new(
            vec![
                CPoint::new(0.0, 0.0, 0.0, NOW),
                CPoint::new(1.0, 0.0, 1.0, NOW),
                CPoint::new(2.0, 0.0, 0.0, NOW),
            ], 1, vec![0.0, 0.0, 0.5, 1.0, 1.0],
        ).unwrap();
        let curve: Curve = BSplineCurve::from_nurb(raw, 3, true).into();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let prepared = crate::parametric::prepare_curve_enclosure(
            &curve,
            (0.0, 1.0),
            MAX_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let mut ascending = false;
        let mut descending = false;
        for range in prepared.parameter_intervals() {
            let jet = prepared.jet(range, &mut work, &cancel).unwrap();
            assert!(!jet.derivative[2].contains_zero());
            ascending |= jet.derivative[2].lo > 0.0;
            descending |= jet.derivative[2].hi < 0.0;
        }
        assert!(ascending && descending);
        assert!(
            !prepared
                .is_smooth_on((0.0, 1.0), &mut work, &cancel)
                .unwrap(),
            "whole original coedge gluing must reject the height maximum hidden exactly on the prepared-piece boundary"
        );
    }

    #[test]
    fn collar_complex_debits_cancellation_and_preflights_capacity() {
        let definition = source("native-parity-blind-pocket.step");
        let request = super::super::SlabRequest {
            frame: frame(),
            height_interval_mm: [3.99, 4.01],
            sample_height_mm: 4.005,
            native_shape_error_mm: 0.1,
        };
        let mut preparation_work = GeometryWorkBudget::new(300_000_000);
        let source_owner = prepare_solid(
            &definition, MAX_BYTES, &mut preparation_work, &AtomicBool::new(false),
        ).unwrap();
        let functional = super::super::HeightFunctional::from_frame(frame());
        let mut work = GeometryWorkBudget::new(100_000);
        assert_eq!(
            certify_native_collar_complex(
                source_owner.source_view().unwrap(),
                &request,
                functional,
                1,
                &mut work,
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::ResourceLimit
        );
        assert!(work.consumed() > 0);
        let mut work = GeometryWorkBudget::new(100_000);
        assert_eq!(
            certify_native_collar_complex(
                source_owner.source_view().unwrap(),
                &request,
                functional,
                MAX_BYTES,
                &mut work,
                &AtomicBool::new(true)
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::Cancelled
        );
        assert!(work.consumed() > 0);
    }
}
