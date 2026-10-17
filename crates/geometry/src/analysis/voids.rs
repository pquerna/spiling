// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Lazy, source-bound outer projections of original native void branches.
//!
//! An outer mask is deliberately not a material section, a collision witness,
//! or a claim that its convexified interior is empty. It protects a corridor:
//! failure to route around it must remain an unresolved support corridor.
//!
//! Proper native regular flow preserves the original Hole roles. Its source
//! height velocity is projected through the actual stored pose polynomial; a
//! validated outward Minkowski hull is not replaced by a universal bore AABB.
//! Exact original Boolean/root face owners and paired coedges distinguish an
//! open lower rim from a solid pocket floor. Active material correction requires
//! a complete isolated wall degree proof and its actual floor closure; missing
//! branch incidence remains typed unresolved, never an extension to global -inf.

use super::{GeometryWorkBudget, SlabAnalysis, SlabRequest};
use crate::parametric::{Interval, up};
use crate::{Definition, GeometryError, SectionLoopMm};
use spiling_contracts::geometry::{DefinitionId, GeometryErrorCode, RigidPoseMm};
use std::sync::atomic::AtomicBool;

const MAX_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct ProtectedVoidPolicy {
    /// Native polygon, plane and Boolean approximation allowance. Actual branch
    /// motion and deliberate convexification are geometric enclosure, not an
    /// invented small Hausdorff error assigned to the outer mask.
    pub maximum_xy_error_mm: f64,
    pub byte_limit: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ProtectedVoidHeightScope {
    Bounded { lower_mm: f64, upper_mm: f64 },
    OpenBelow { upper_mm: f64 },
}

#[derive(Debug)]
pub struct NativeProtectedVoidEnvelope {
    source_loop_index: usize,
    world_xy_outer: SectionLoopMm,
    height_scope: ProtectedVoidHeightScope,
}
impl NativeProtectedVoidEnvelope {
    pub fn source_loop_index(&self) -> usize {
        self.source_loop_index
    }
    pub fn world_xy_outer(&self) -> &SectionLoopMm {
        &self.world_xy_outer
    }
    pub fn height_scope(&self) -> ProtectedVoidHeightScope {
        self.height_scope
    }
    pub fn retained_bytes(&self) -> usize {
        self.world_xy_outer
            .points_mm
            .capacity()
            .saturating_mul(std::mem::size_of::<[f64; 3]>())
    }
}

#[derive(Debug)]
struct NativeCapturedHole {
    source_loop_index: usize,
    boundary: SectionLoopMm,
    source_faces: Vec<usize>,
}

/// The producer's original complete authority moves with the immutable slab
/// capture. A collar is not a regular flow or a zero-void count.
#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "Retain owned native proof inline; boxing adds per-query allocation and splits resource accounting"
)]
enum NativeVoidContinuation {
    Regular {
        source_speed: f64,
        source_realization_error_mm: f64,
    },
    Critical {
        counts: [usize; 2],
        events: Vec<(Option<usize>, [f64; 2], bool)>,
        source_realization_error_mm: f64,
    },
    OriginalCollar(super::collar::NativeCollarComplex),
    ClosedRadial(super::critical::ClosedRadialMaterialProduct),
    Unresolved,
}
impl NativeVoidContinuation {
    fn regular_speed(&self) -> Option<f64> {
        match self {
            Self::Regular { source_speed, .. } => Some(*source_speed),
            _ => None,
        }
    }
    /// Physical error proved over this capture's entire closed slab. This is
    /// not authority for an extended branch or its distant floor.
    fn source_realization_error_mm(&self) -> Option<f64> {
        match self {
            Self::Regular {
                source_realization_error_mm,
                ..
            }
            | Self::Critical {
                source_realization_error_mm,
                ..
            } => Some(*source_realization_error_mm),
            _ => None,
        }
    }
    fn critical_counts(&self) -> Option<[usize; 2]> {
        match self {
            Self::Critical { counts, .. } => Some(*counts),
            _ => None,
        }
    }
    #[cfg(test)]
    fn critical_events(&self) -> &[(Option<usize>, [f64; 2], bool)] {
        match self {
            Self::Critical { events, .. } => events,
            _ => &[],
        }
    }
    fn retained_bytes(&self) -> usize {
        match self {
            Self::Critical { events, .. } => events
                .capacity()
                .saturating_mul(std::mem::size_of::<(Option<usize>, [f64; 2], bool)>()),
            Self::OriginalCollar(complex) => complex.retained_bytes(),
            Self::ClosedRadial(product) => product.retained_bytes(),
            _ => 0,
        }
    }
}

/// Compact continuation receipt retained after joint acceptance. Original Hole
/// buffers and any complete original collar authority move here; material and
/// occupied contours die with the consumed slab. Projection stays lazy.
#[derive(Debug)]
pub struct NativeVoidProjectionCapture {
    definition_id: DefinitionId,
    material_realization: crate::kernel::MaterialRealizationIdentity,
    request: SlabRequest,
    _source_sample_plane: crate::PlaneMm,
    source_sample_height_band_mm: [f64; 2],
    pose: Option<RigidPoseMm>,
    world_interval: [f64; 2],
    world_sample: f64,
    world_policy: Option<f64>,
    native_errors: [f64; 3],
    continuation: NativeVoidContinuation,
    holes: Vec<NativeCapturedHole>,
}
impl NativeVoidProjectionCapture {
    pub fn definition_id(&self) -> &DefinitionId {
        &self.definition_id
    }
    pub fn placed_pose(&self) -> Option<RigidPoseMm> {
        self.pose
    }
    pub fn world_height_interval_mm(&self) -> [f64; 2] {
        self.world_interval
    }
    pub fn sample_world_z_mm(&self) -> f64 {
        self.world_sample
    }
    pub fn retained_bytes(&self) -> usize {
        let bytes = self
            .definition_id
            .allocated_bytes()
            .saturating_add(
                self.holes
                    .capacity()
                    .saturating_mul(std::mem::size_of::<NativeCapturedHole>()),
            )
            .saturating_add(self.continuation.retained_bytes());
        self.holes.iter().fold(bytes, |bytes, hole| {
            bytes
                .saturating_add(hole.boundary.points_mm.capacity().saturating_mul(24))
                .saturating_add(
                    hole.source_faces
                        .capacity()
                        .saturating_mul(std::mem::size_of::<usize>()),
                )
        })
    }
}

/// Consume a completed, joint-accepted slab without projecting void polygons.
/// Only a whole-interval proof of zero native voids permits `None`. Unsupported
/// future projection/scope obligations remain deferred until actual demand.
pub fn capture_projected_voids(
    mut analysis: SlabAnalysis,
    byte_limit: u32,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<NativeVoidProjectionCapture>, GeometryError> {
    work.charge(1, cancel)?;
    if !analysis.section().is_native_intersection()
        || analysis.definition_id() != analysis.section().definition_id()
    {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "protected void capture requires original native slab sample",
        ));
    }
    let material_realization = analysis.material_realization_identity();
    if analysis.critical.as_ref().is_some_and(|proof| {
        proof.definition_id() != analysis.definition_id()
            || proof.material_realization_identity() != material_realization
    }) || analysis.invariance.as_ref().is_some_and(|proof| {
        proof.definition_id() != analysis.definition_id()
            || proof.material_realization_identity() != material_realization
    }) || analysis.collar_complex.as_ref().is_some_and(|proof| {
        proof.definition_id() != analysis.definition_id()
            || proof.material_realization_identity() != material_realization
    }) {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "protected void continuation changed its original material realization",
        ));
    }
    if analysis.section().loop_source_faces.len() != analysis.section().loops().len() {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "native Hole owner receipt cardinality disagrees with the immutable section",
        ));
    }
    let request = analysis.request();
    let pose = analysis.placed_pose();
    let (world_interval, world_sample) = if pose.is_some() {
        (
            analysis.world_height_interval_mm().ok_or_else(|| {
                error(
                    GeometryErrorCode::InvalidGeometry,
                    "protected void capture lacks placed interval",
                )
            })?,
            analysis.sample_world_z_mm().ok_or_else(|| {
                error(
                    GeometryErrorCode::InvalidGeometry,
                    "protected void capture lacks placed sample",
                )
            })?,
        )
    } else {
        if request.frame.origin_mm != [0.0; 3] || request.frame.z_axis != [0.0, 0.0, 1.0] {
            return Err(error(
                GeometryErrorCode::InvalidGeometry,
                "unplaced protected void capture requires actual WORLD Z",
            ));
        }
        (request.height_interval_mm, request.sample_height_mm)
    };
    let regular_speed = analysis
        .critical
        .as_ref()
        .and_then(|proof| proof.regular_flow_speed_mm_per_height())
        .or_else(|| analysis.invariance().map(|_| 0.0));
    let source_sample_error = analysis
        .critical
        .as_ref()
        .map(|proof| proof.source_realization_error_mm())
        // This invariance is issued only for unchanged material geometry.
        .or_else(|| analysis.invariance().map(|_| 0.0));
    let critical_counts = analysis
        .critical
        .as_ref()
        .map(|proof| [proof.bounded_void_count(), proof.tunnel_cycle_count()]);
    let count = analysis
        .section()
        .loops()
        .iter()
        .filter(|boundary| boundary.is_hole)
        .count();
    work.charge(analysis.section().loops().len() as u64, cancel)?;
    if count == 0 && (regular_speed.is_some() || critical_counts == Some([0, 0])) {
        return Ok(None);
    }
    let mut storage = Storage {
        used: 0,
        limit: (byte_limit as usize).min(MAX_BYTES),
    };
    storage.debit(analysis.definition_id().allocated_bytes(), work, cancel)?;
    let mut holes = Vec::new();
    storage.reserve(&mut holes, count, work, cancel)?;
    for (index, boundary) in analysis
        .section()
        .loops()
        .iter()
        .enumerate()
        .filter(|(_, boundary)| boundary.is_hole)
    {
        storage.debit(
            boundary.points_mm.capacity().saturating_mul(24),
            work,
            cancel,
        )?;
        storage.debit(
            analysis.section().loop_source_faces[index]
                .capacity()
                .saturating_mul(std::mem::size_of::<usize>()),
            work,
            cancel,
        )?;
    }
    let mut critical_events = Vec::new();
    let critical_covered_cells = analysis
        .critical
        .as_ref()
        .map(|proof| proof.covered_cells());
    if regular_speed.is_none() {
        storage.reserve(
            &mut critical_events,
            analysis.event_neighborhoods().len(),
            work,
            cancel,
        )?;
        for event in analysis.event_neighborhoods() {
            work.charge(1, cancel)?;
            critical_events.push((
                event.source_face_ordinal(),
                event.height_bracket_mm(),
                event.stationary_root_proved(),
            ));
        }
    }
    let continuation = if let Some(source_speed) = regular_speed {
        NativeVoidContinuation::Regular {
            source_speed,
            source_realization_error_mm: source_sample_error.ok_or_else(|| {
                error(
                    GeometryErrorCode::InvalidGeometry,
                    "regular material continuation lacks its source error proof",
                )
            })?,
        }
    } else if let (Some(counts), Some(_)) = (critical_counts, critical_covered_cells) {
        NativeVoidContinuation::Critical {
            counts,
            events: critical_events,
            source_realization_error_mm: source_sample_error.ok_or_else(|| {
                error(
                    GeometryErrorCode::InvalidGeometry,
                    "critical material continuation lacks its source error proof",
                )
            })?,
        }
    } else if let Some(complex) = analysis.collar_complex.take() {
        storage.debit(complex.retained_bytes(), work, cancel)?;
        NativeVoidContinuation::OriginalCollar(complex)
    } else if let Some(radial) = analysis.radial_slab.take() {
        let product = radial.into_product();
        storage.debit(product.retained_bytes(), work, cancel)?;
        NativeVoidContinuation::ClosedRadial(product)
    } else {
        NativeVoidContinuation::Unresolved
    };
    let definition_id = analysis.definition_id().clone();
    let world_policy = analysis.world_shape_error_policy_mm();
    let source_sample_plane = analysis.capture.source_sample_plane;
    let source_sample_height_band_mm = analysis.capture.source_sample_height_band_mm;
    let native_errors = [
        analysis.section().achieved_chord_error_mm(),
        analysis.section().achieved_plane_error_mm(),
        analysis.section().boolean_tolerance_mm(),
    ];
    let (mut section, _occupied) = analysis.into_parts();
    let owners = std::mem::take(&mut section.loop_source_faces);
    for (index, (boundary, source_faces)) in
        section.into_loops().into_iter().zip(owners).enumerate()
    {
        work.charge(1, cancel)?;
        if boundary.is_hole {
            holes.push(NativeCapturedHole {
                source_loop_index: index,
                boundary,
                source_faces,
            });
        }
    }
    Ok(Some(NativeVoidProjectionCapture {
        definition_id,
        material_realization,
        request,
        pose,
        world_interval,
        world_sample,
        world_policy,
        _source_sample_plane: source_sample_plane,
        source_sample_height_band_mm,
        native_errors,
        continuation,
        holes,
    }))
}

#[derive(Debug)]
pub struct NativeProjectedVoids {
    definition_id: DefinitionId,
    material_realization: crate::kernel::MaterialRealizationIdentity,
    placed_pose: Option<RigidPoseMm>,
    world_height_interval_mm: [f64; 2],
    sample_world_z_mm: f64,
    // Bind both policies, not just the shape of a coincident output polygon.
    _source_request: SlabRequest,
    _world_shape_error_policy_mm: Option<f64>,
    _policy: ProtectedVoidPolicy,
    envelopes: Vec<NativeProtectedVoidEnvelope>,
}
impl NativeProjectedVoids {
    /// Match the exact immutable source and material realization. Placement,
    /// captured interval and projection policies are independent query fields.
    pub fn is_compatible_with_definition(&self, definition: &Definition) -> bool {
        self.definition_id == *definition.id()
            && self.material_realization
                == crate::kernel::MaterialRealizationIdentity::new(definition)
    }
    pub fn definition_id(&self) -> &DefinitionId {
        &self.definition_id
    }
    pub fn placed_pose(&self) -> Option<RigidPoseMm> {
        self.placed_pose
    }
    pub fn world_height_interval_mm(&self) -> [f64; 2] {
        self.world_height_interval_mm
    }
    pub fn sample_world_z_mm(&self) -> f64 {
        self.sample_world_z_mm
    }
    pub fn envelopes(&self) -> &[NativeProtectedVoidEnvelope] {
        &self.envelopes
    }
    pub fn retained_bytes(&self) -> usize {
        self.envelopes.iter().fold(
            self.definition_id.allocated_bytes().saturating_add(
                self.envelopes
                    .capacity()
                    .saturating_mul(std::mem::size_of::<NativeProtectedVoidEnvelope>()),
            ),
            |bytes, envelope| bytes.saturating_add(envelope.retained_bytes()),
        )
    }
}

fn error(code: GeometryErrorCode, message: &'static str) -> GeometryError {
    GeometryError::new(code, message)
}
fn unresolved(message: &'static str) -> GeometryError {
    error(GeometryErrorCode::AnalysisIndeterminate, message)
}
fn finite(value: Interval) -> Result<Interval, GeometryError> {
    Interval::new(value.lo, value.hi)
}
fn dot(a: [Interval; 3], b: [Interval; 3]) -> Interval {
    (0..3).fold(Interval::point(0.0), |sum, axis| {
        sum.add(a[axis].mul(b[axis]))
    })
}
fn norm(a: &[Interval]) -> Result<Interval, GeometryError> {
    let squared = a.iter().enumerate().try_fold(
        Interval::point(0.0),
        |sum, (index, value)| -> Result<Interval, GeometryError> {
            let value = Interval::new(value.lo, value.hi).map_err(|cause| {
                GeometryError::new(
                    cause.code,
                    format!("protected void norm operand {index} is invalid: {value:?}; {cause}"),
                )
            })?;
            Ok(sum.add(value.square()))
        },
    )?;
    // Each summand is a square of a validated real interval. Their exact sum
    // is nonnegative independently of the outward addition enclosure.
    let image = Interval::new(squared.lo.max(0.0), squared.hi).map_err(|cause| {
        GeometryError::new(cause.code, format!(
            "protected void squared norm has no finite nonnegative image: sum={squared:?}, operands={:?}, count={}; {cause}",
            &a[..a.len().min(6)], a.len()
        ))
    })?;
    image.sqrt().map_err(|cause| {
        GeometryError::new(cause.code, format!(
            "protected void norm square root failed: sum={squared:?}, image={image:?}, operands={:?}, count={}; {cause}",
            &a[..a.len().min(6)], a.len()
        ))
    })
}

/// Cumulative owned capacities, including temporary hull buffers. Reserving a
/// later hole never resets the debit made for an earlier one.
struct Storage {
    used: usize,
    limit: usize,
}
impl Storage {
    fn debit(
        &mut self,
        bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        work.charge(u64::try_from(bytes).unwrap_or(u64::MAX), cancel)?;
        self.used = self.used.checked_add(bytes).ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "protected void capacity arithmetic overflow",
            )
        })?;
        if self.used > self.limit {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                "protected void cumulative owned capacity exhausted",
            ));
        }
        Ok(())
    }
    fn reserve<T>(
        &mut self,
        values: &mut Vec<T>,
        count: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        if values.capacity() != 0 || !values.is_empty() {
            return Err(error(
                GeometryErrorCode::InvalidGeometry,
                "protected void buffer reserved more than once",
            ));
        }
        let bytes = count.checked_mul(std::mem::size_of::<T>()).ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "protected void buffer size overflow",
            )
        })?;
        self.debit(bytes, work, cancel)?;
        values.try_reserve_exact(count).map_err(|_| {
            error(
                GeometryErrorCode::ResourceLimit,
                "protected void buffer allocation refused",
            )
        })?;
        if values.capacity() > count {
            self.debit(
                (values.capacity() - count).saturating_mul(std::mem::size_of::<T>()),
                work,
                cancel,
            )?;
        }
        Ok(())
    }
}

fn orientation(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> Result<i8, GeometryError> {
    if a == b
        || b == c
        || a == c
        || (a[0] == b[0] && b[0] == c[0])
        || (a[1] == b[1] && b[1] == c[1])
    {
        return Ok(0);
    }
    let u: [Interval; 2] =
        std::array::from_fn(|i| Interval::point(b[i]).sub(Interval::point(a[i])));
    let v: [Interval; 2] =
        std::array::from_fn(|i| Interval::point(c[i]).sub(Interval::point(a[i])));
    let cross = finite(u[0].mul(v[1]).sub(u[1].mul(v[0])))?;
    if cross.lo > 0.0 {
        Ok(1)
    } else if cross.hi < 0.0 {
        Ok(-1)
    } else if cross.lo == 0.0 && cross.hi == 0.0 {
        Ok(0)
    } else {
        Err(error(
            GeometryErrorCode::ConditioningFailure,
            "protected void hull orientation is unresolved in outward arithmetic",
        ))
    }
}

/// The four corners at every stored vertex enclose the Minkowski sum of the
/// entire polygonal boundary with a radius-r disk. Taking their convex hull
/// encloses the true hole interior as well. This preserves circular bore lanes;
/// it is not a universal source bounding rectangle.
fn outward_hull(
    boundary: &SectionLoopMm,
    radius: f64,
    storage: &mut Storage,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SectionLoopMm, GeometryError> {
    if boundary.points_mm.len() < 4
        || boundary.points_mm.first() != boundary.points_mm.last()
        || !radius.is_finite()
        || radius < 0.0
    {
        return Err(unresolved(
            "protected native void has no finite closed sample cycle",
        ));
    }
    let count = (boundary.points_mm.len() - 1)
        .checked_mul(4)
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "protected void Minkowski corner count overflow",
            )
        })?;
    let mut corners = Vec::new();
    storage.reserve(&mut corners, count, work, cancel)?;
    for point in &boundary.points_mm[..boundary.points_mm.len() - 1] {
        work.charge(4, cancel)?;
        if point.iter().any(|value| !value.is_finite()) {
            return Err(error(
                GeometryErrorCode::ConditioningFailure,
                "protected void sample has nonfinite coordinates",
            ));
        }
        let x = finite(Interval::point(point[0]).add(Interval {
            lo: -radius,
            hi: radius,
        }))?;
        let y = finite(Interval::point(point[1]).add(Interval {
            lo: -radius,
            hi: radius,
        }))?;
        for p in [[x.lo, y.lo], [x.lo, y.hi], [x.hi, y.lo], [x.hi, y.hi]] {
            corners.push(p);
        }
    }
    // sort_unstable does not allocate. Comparisons are finite total-order binary64.
    let comparisons = count.checked_mul(usize::BITS as usize).ok_or_else(|| {
        error(
            GeometryErrorCode::ResourceLimit,
            "protected void sorting work overflow",
        )
    })?;
    work.charge(comparisons as u64, cancel)?;
    corners.sort_unstable_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    corners.dedup();
    let mut hull = Vec::new();
    storage.reserve(
        &mut hull,
        count.checked_mul(2).ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "protected void hull capacity overflow",
            )
        })?,
        work,
        cancel,
    )?;
    for &point in &corners {
        while hull.len() >= 2 {
            work.charge(1, cancel)?;
            if orientation(hull[hull.len() - 2], hull[hull.len() - 1], point)? > 0 {
                break;
            }
            hull.pop();
        }
        hull.push(point);
    }
    let lower_len = hull.len();
    for &point in corners.iter().rev().skip(1) {
        while hull.len() > lower_len {
            work.charge(1, cancel)?;
            if orientation(hull[hull.len() - 2], hull[hull.len() - 1], point)? > 0 {
                break;
            }
            hull.pop();
        }
        hull.push(point);
    }
    if hull.len() < 4 || hull.first() != hull.last() {
        return Err(unresolved(
            "protected void outward hull is degenerate or open",
        ));
    }
    // Validate every supporting edge against every generator, independently of
    // the monotone-chain construction. Positive turns plus these separations
    // prove closed CCW convexity and containment, not just a sampled area sign.
    for i in 0..hull.len() - 1 {
        work.charge(1, cancel)?;
        let a = hull[i];
        let b = hull[i + 1];
        let c = hull[(i + 2) % (hull.len() - 1)];
        if orientation(a, b, c)? <= 0 {
            return Err(unresolved("protected void hull is not strictly convex"));
        }
        for &point in &corners {
            work.charge(1, cancel)?;
            if orientation(a, b, point)? < 0 {
                return Err(unresolved(
                    "protected void hull fails outward generator separation",
                ));
            }
        }
    }
    let mut points_mm = Vec::new();
    storage.reserve(&mut points_mm, hull.len(), work, cancel)?;
    for point in hull {
        work.charge(1, cancel)?;
        points_mm.push([point[0], point[1], 0.0]);
    }
    Ok(SectionLoopMm {
        is_hole: false,
        points_mm,
    })
}

/// Project the actual source-height flow. The transverse speed is Euclidean in
/// source space; the axial flow is n/(n·n), NOT n or a presumed orthogonal Rn.
fn projected_flow_speed(
    request: SlabRequest,
    matrix: [[Interval; 3]; 3],
    transverse: f64,
) -> Result<f64, GeometryError> {
    let n = request.frame.z_axis.map(Interval::point);
    let nn = dot(n, n);
    if nn.lo <= 0.0 || !transverse.is_finite() || transverse < 0.0 {
        return Err(error(
            GeometryErrorCode::ConditioningFailure,
            "protected void source height flow is ill-conditioned",
        ));
    }
    let axial = [dot(matrix[0], n).div(nn)?, dot(matrix[1], n).div(nn)?];
    // Frobenius bounds the actual 2x3 projection's operator norm without an
    // orthogonality assumption on accepted near-unit stored quaternions.
    let projection = norm(&[
        matrix[0][0],
        matrix[0][1],
        matrix[0][2],
        matrix[1][0],
        matrix[1][1],
        matrix[1][2],
    ])?;
    Ok(finite(norm(&axial)?.add(projection.mul(Interval::point(transverse))))?.hi)
}

#[derive(Clone, Copy)]
enum ColumnFace {
    Other,
    Wall,
    Cap { height: Interval, sign: i8 },
}

fn source_z() -> super::slab::HeightFunctional {
    super::slab::HeightFunctional {
        direction: [
            Interval::point(0.0),
            Interval::point(0.0),
            Interval::point(1.0),
        ],
        offset: Interval::point(0.0),
    }
}
fn normal(jet: &crate::parametric::SurfaceEnclosure) -> [Interval; 3] {
    let a = jet.du;
    let b = jet.dv;
    [
        a[1].mul(b[2]).sub(a[2].mul(b[1])),
        a[2].mul(b[0]).sub(a[0].mul(b[2])),
        a[0].mul(b[1]).sub(a[1].mul(b[0])),
    ]
}
fn classify_column_face(
    definition: &Definition,
    index: usize,
    remaining: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<ColumnFace, GeometryError> {
    use crate::{
        kernel::{TrimClassification, prepare_face_trim_domain_with_byte_limit},
        parametric::{surface_enclosure, surface_parameter_boxes},
    };
    let face = definition
        .solid
        .face_iter()
        .nth(index)
        .ok_or_else(|| unresolved("native void face index is absent"))?;
    let cap =
        super::slab::constant_source_face_height(definition, index, source_z(), work, cancel)?;
    let domain =
        prepare_face_trim_domain_with_byte_limit(definition, index, remaining, work, cancel)?;
    let uv = domain
        .certified_uv_domain()
        .unwrap_or_else(|| domain.uv_bounds());
    face.with_surface(|surface| {
        let mut cap_sign = None;
        let mut seen = false;
        let mut pending = [([[0.0; 2]; 2], 0u8); 66];
        let mut len = 0usize;
        let mut spans = surface_parameter_boxes(surface, uv, work, cancel)?;
        while let Some((uv, depth)) = if len > 0 {
            len -= 1;
            Some(pending[len])
        } else {
            spans.next().map(|uv| (uv, 0))
        } {
            work.charge(1, cancel)?;
            if matches!(
                domain.classify(uv, work, cancel)?,
                TrimClassification::Outside
            ) {
                continue;
            }
            let jet = surface_enclosure(surface, uv, work, cancel)?;
            let n = normal(&jet);
            if cap.is_none() && (n[2].lo != 0.0 || n[2].hi != 0.0) {
                return Ok(ColumnFace::Other);
            }
            let signed = if face.orientation() { n[2] } else { n[2].neg() };
            let sign = if signed.lo > 0.0 {
                Some(1)
            } else if signed.hi < 0.0 {
                Some(-1)
            } else {
                None
            };
            let regular = norm(&n)?.lo > 0.0 && (cap.is_none() || sign.is_some());
            if regular {
                seen = true;
                if cap.is_some() {
                    if cap_sign.is_some() && cap_sign != sign {
                        return Ok(ColumnFace::Other);
                    }
                    cap_sign = sign;
                }
                continue;
            }
            if depth >= 60 {
                return Ok(ColumnFace::Other);
            }
            let axis = usize::from(uv[1][1] - uv[1][0] > uv[0][1] - uv[0][0]);
            let mid = uv[axis][0] * 0.5 + uv[axis][1] * 0.5;
            if mid <= uv[axis][0] || mid >= uv[axis][1] {
                return Ok(ColumnFace::Other);
            }
            let mut a = uv;
            let mut b = uv;
            a[axis][1] = mid;
            b[axis][0] = mid;
            pending[len] = (a, depth + 1);
            pending[len + 1] = (b, depth + 1);
            len += 2;
        }
        if cap.is_none() {
            for crease in crate::parametric::surface_crease_interfaces(surface, uv, work, cancel)? {
                if super::critical::interface_regular_join(surface, crease, work, cancel)? {
                    continue;
                }
                if crease.iter().all(|span| span[0] != span[1]) {
                    return Ok(ColumnFace::Other);
                }
                if matches!(
                    domain.classify(crease, work, cancel)?,
                    TrimClassification::Outside
                ) {
                    continue;
                }
                let moving = usize::from(crease[0][0] == crease[0][1]);
                let jet = surface_enclosure(surface, crease, work, cancel)?;
                let tangent = if moving == 0 { jet.du } else { jet.dv };
                if tangent[..2].iter().any(|d| d.lo != 0.0 || d.hi != 0.0)
                    || tangent[2].contains_zero()
                {
                    return Ok(ColumnFace::Other);
                }
            }
        }
        Ok(if !seen {
            ColumnFace::Other
        } else if let (Some(height), Some(sign)) = (cap, cap_sign) {
            ColumnFace::Cap { height, sign }
        } else {
            ColumnFace::Wall
        })
    })
}

/// Pair by original native shared edge ID and oriented use, never positional
/// welding. The admitted source lineage must retain both actual occurrences.
fn neighbor_use(
    definition: &Definition,
    own: (usize, usize, usize),
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(usize, usize, usize), GeometryError> {
    let face = definition
        .solid
        .face_iter()
        .nth(own.0)
        .ok_or_else(|| unresolved("void coedge face is absent"))?;
    let edge = &face.absolute_boundaries()[own.1][own.2];
    let orientation = edge.orientation() == face.orientation();
    let mut neighbor = None;
    for (fi, other) in definition.solid.face_iter().enumerate() {
        for (wi, wire) in other.absolute_boundaries().iter().enumerate() {
            for (ei, use_) in wire.iter().enumerate() {
                work.charge(1, cancel)?;
                if use_.id() != edge.id() || (fi, wi, ei) == own {
                    continue;
                }
                if neighbor.is_some() || (use_.orientation() == other.orientation()) == orientation
                {
                    return Err(error(
                        GeometryErrorCode::SingularGeometry,
                        "protected void native coedge is not paired with opposite material orientation",
                    ));
                }
                if definition
                    .source_topology
                    .faces
                    .get(fi)
                    .and_then(|face| face.wires.get(wi))
                    .and_then(|wire| wire.coedges.get(ei))
                    .is_none()
                {
                    return Err(unresolved(
                        "protected void original coedge lineage is absent",
                    ));
                }
                neighbor = Some((fi, wi, ei));
            }
        }
    }
    neighbor.ok_or_else(|| unresolved("protected void native coedge has no original neighbor"))
}

fn wire_area(
    definition: &Definition,
    face_index: usize,
    wire_index: usize,
    remaining: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Interval, GeometryError> {
    use monstertruck_modeling::BoundedCurve;
    let face = definition
        .solid
        .face_iter()
        .nth(face_index)
        .ok_or_else(|| unresolved("void rim face is absent"))?;
    let mut area = Interval::point(0.0);
    for edge in &face.absolute_boundaries()[wire_index] {
        let contribution = edge.with_curve(|curve| {
            let prepared = definition.prepare_boundary_curve(
                edge.id(),
                curve,
                curve.range_tuple(),
                remaining,
                work,
                cancel,
            )?;
            let mut sum = Interval::point(0.0);
            for (lo, hi) in prepared.parameter_intervals() {
                // A quadrature ENCLOSURE: each integrand interval covers its
                // whole parameter cell. No point evaluations certify an arc.
                for i in 0..128 {
                    let a = if i == 0 {
                        lo
                    } else {
                        lo + (hi - lo) * (i as f64 / 128.0)
                    };
                    let b = if i == 127 {
                        hi
                    } else {
                        lo + (hi - lo) * ((i + 1) as f64 / 128.0)
                    };
                    if a >= b {
                        return Err(error(
                            GeometryErrorCode::ConditioningFailure,
                            "native void rim integral cannot subdivide",
                        ));
                    }
                    let jet = prepared.jet((a, b), work, cancel)?;
                    let integrand = jet.position[0]
                        .mul(jet.derivative[1])
                        .sub(jet.position[1].mul(jet.derivative[0]));
                    sum = sum.add(integrand.mul(Interval::point(b).sub(Interval::point(a))));
                }
            }
            Ok::<_, GeometryError>(sum)
        })?;
        area = area.add(if edge.orientation() == face.orientation() {
            contribution
        } else {
            contribution.neg()
        });
    }
    finite(area)
}

fn edge_vertical(
    definition: &Definition,
    a: (usize, usize, usize),
    remaining: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    use monstertruck_modeling::BoundedCurve;
    let first = definition
        .solid
        .face_iter()
        .nth(a.0)
        .ok_or_else(|| unresolved("void wall face is absent"))?;
    let edge = &first.absolute_boundaries()[a.1][a.2];
    edge.with_curve(|curve| {
        let prepared = definition.prepare_boundary_curve(
            edge.id(),
            curve,
            curve.range_tuple(),
            remaining,
            work,
            cancel,
        )?;
        for span in prepared.parameter_intervals() {
            let jet = prepared.jet(span, work, cancel)?;
            if jet.derivative[..2]
                .iter()
                .any(|d| d.lo != 0.0 || d.hi != 0.0)
                || jet.derivative[2].contains_zero()
            {
                return Ok(false);
            }
        }
        Ok(true)
    })
}

/// The raw whole-column incidence theorem below is reusable only where the
/// complete physical correction support is absent. This is a fresh closed-scope
/// obligation, independent of the smaller slab used to capture the Hole.
fn revalidate_raw_scope(
    scope: ProtectedVoidHeightScope,
    supports: &[super::critical::material_link::SourceTransportSupport],
    translation_z: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<(usize, [f64; 2])>, GeometryError> {
    let (lower, upper) = match scope {
        ProtectedVoidHeightScope::Bounded { lower_mm, upper_mm } => (lower_mm, upper_mm),
        ProtectedVoidHeightScope::OpenBelow { upper_mm } => (f64::NEG_INFINITY, upper_mm),
    };
    for (index, support) in supports.iter().enumerate() {
        work.charge(1, cancel)?;
        // A zero scalar displacement does NOT prove that the physical branch
        // is unchanged: horizontal correction may change its material topology.
        let base = Interval::new(support.base_interval[0], support.base_interval[1])?;
        let corrected =
            Interval::new(support.corrected_interval[0], support.corrected_interval[1])?;
        let image = finite(base.hull(corrected).add(Interval::point(translation_z)))?;
        if image.lo <= upper && lower <= image.hi {
            return Ok(Some((index, [image.lo, image.hi])));
        }
    }
    Ok(None)
}

/// Necessary original-source precondition only. This does not issue actual-M
/// branch authority: complete incidence, corrections and full-scope geometry
/// are still qualified by `native_height_scopes` below.
fn original_column_faces(
    definition: &Definition,
    holes: &[NativeCapturedHole],
    storage: &mut Storage,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Vec<ColumnFace>, GeometryError> {
    if holes.is_empty() {
        return Ok(Vec::new());
    }
    let mut faces = Vec::new();
    storage.reserve(&mut faces, definition.faces.len(), work, cancel)?;
    for index in 0..definition.faces.len() {
        faces.push(classify_column_face(
            definition,
            index,
            storage.limit.saturating_sub(storage.used),
            work,
            cancel,
        )?);
    }
    for hole in holes {
        work.charge(hole.source_faces.len() as u64 + 1, cancel)?;
        if hole.source_faces.is_empty()
            || hole
                .source_faces
                .iter()
                .any(|&face| !matches!(faces.get(face), Some(ColumnFace::Wall)))
        {
            return Err(unresolved(
                "protected native Hole has no original material-facing column wall ownership",
            ));
        }
    }
    Ok(faces)
}

fn native_height_scopes(
    definition: &Definition,
    capture: &NativeVoidProjectionCapture,
    faces: &[ColumnFace],
    transport_supports: &[super::critical::material_link::SourceTransportSupport],
    storage: &mut Storage,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Vec<Option<ProtectedVoidHeightScope>>, GeometryError> {
    // Original column incidence supplies a representation-independent raw lane.
    // Wherever G is active, the complete actual wall/floor/clearance/degree
    // theorem below is required before this full source scope is reusable.
    let face_count = faces.len();
    let mut visited = Vec::new();
    storage.reserve(&mut visited, face_count, work, cancel)?;
    visited.resize(face_count, false);
    let mut component = Vec::new();
    storage.reserve(&mut component, face_count, work, cancel)?;
    let matrix = super::placed::rotation(capture.pose.unwrap_or(RigidPoseMm::IDENTITY))?;
    let translation = capture.pose.map_or([0.0; 3], |pose| pose.translation_mm);
    work.charge(transport_supports.len() as u64, cancel)?;
    let floor_transport_error = norm(&matrix[2])?.mul(Interval::point(
        transport_supports
            .iter()
            .fold(0.0_f64, |e, s| e.max(s.source_error_mm)),
    ));
    let down_is_source_lower = if matrix[2][2].lo > 0.0 {
        true
    } else if matrix[2][2].hi < 0.0 {
        false
    } else {
        return Err(unresolved(
            "native void column has unresolved world material-facing height direction",
        ));
    };
    let mut result = Vec::new();
    storage.reserve(&mut result, capture.holes.len(), work, cancel)?;
    result.resize(capture.holes.len(), None);
    for seed in 0..face_count {
        if visited[seed] || !matches!(faces[seed], ColumnFace::Wall) {
            continue;
        }
        component.clear();
        component.push(seed);
        visited[seed] = true;
        let mut cursor = 0;
        let mut valid = true;
        let mut lower = f64::INFINITY;
        let mut upper = f64::NEG_INFINITY;
        while cursor < component.len() {
            let fi = component[cursor];
            cursor += 1;
            let face = definition
                .solid
                .face_iter()
                .nth(fi)
                .ok_or_else(|| unresolved("void wall component is absent"))?;
            for (wi, wire) in face.absolute_boundaries().iter().enumerate() {
                for (ei, edge) in wire.iter().enumerate() {
                    let neighbor = neighbor_use(definition, (fi, wi, ei), work, cancel)?;
                    match faces[neighbor.0] {
                        ColumnFace::Cap { height, .. } => {
                            let h = edge.with_curve(|curve| {
                                super::slab::constant_native_curve_height(
                                    curve,
                                    source_z(),
                                    work,
                                    cancel,
                                )
                            })?;
                            if h.is_none_or(|h| h.lo > height.hi || h.hi < height.lo) {
                                valid = false;
                            }
                            lower = lower.min(height.lo);
                            upper = upper.max(height.hi);
                        }
                        ColumnFace::Wall => {
                            if !edge_vertical(
                                definition,
                                (fi, wi, ei),
                                storage.limit.saturating_sub(storage.used),
                                work,
                                cancel,
                            )? {
                                valid = false;
                            }
                            if !visited[neighbor.0] {
                                visited[neighbor.0] = true;
                                component.push(neighbor.0);
                            }
                        }
                        ColumnFace::Other => valid = false,
                    }
                }
            }
        }
        if !valid || !lower.is_finite() || !upper.is_finite() || lower >= upper {
            continue;
        }
        // Nonrepresentable constant faces retain their actual height brackets.
        // The regular source column lies between the complete lower/upper cap
        // bands; a source vertex in that open interval would invalidate flow.
        let midpoint = lower * 0.5 + upper * 0.5;
        let mut lower_safe = f64::NEG_INFINITY;
        let mut upper_safe = f64::INFINITY;
        for &fi in &component {
            let face = definition
                .solid
                .face_iter()
                .nth(fi)
                .ok_or_else(|| unresolved("void cap-band component is absent"))?;
            for (wi, wire) in face.absolute_boundaries().iter().enumerate() {
                for (ei, _) in wire.iter().enumerate() {
                    let neighbor = neighbor_use(definition, (fi, wi, ei), work, cancel)?;
                    if let ColumnFace::Cap { height, .. } = faces[neighbor.0] {
                        if height.hi < midpoint {
                            lower_safe = lower_safe.max(height.hi);
                        } else if height.lo > midpoint {
                            upper_safe = upper_safe.min(height.lo);
                        } else {
                            valid = false;
                        }
                    }
                }
            }
        }
        if !valid || !lower_safe.is_finite() || !upper_safe.is_finite() || lower_safe >= upper_safe
        {
            continue;
        }
        for &fi in &component {
            let face = definition
                .solid
                .face_iter()
                .nth(fi)
                .ok_or_else(|| unresolved("void source vertex component is absent"))?;
            for vertex in face.vertex_iter() {
                work.charge(1, cancel)?;
                let z = vertex.point().z;
                if z > lower_safe && z < upper_safe {
                    valid = false;
                }
            }
        }
        if !valid {
            continue;
        }
        let mut matched = None;
        let mut opening = None;
        let mut lower_world = f64::INFINITY;
        // Actual native Boolean/source-root lineage, NOT a sampled nearest
        // point, binds this immutable Hole cycle to its original wall complex.
        for (index, hole) in capture.holes.iter().enumerate() {
            work.charge(
                hole.source_faces.len().saturating_mul(component.len()) as u64,
                cancel,
            )?;
            if !hole.source_faces.is_empty()
                && hole
                    .source_faces
                    .iter()
                    .all(|face| component.contains(face))
            {
                if matched.is_some() {
                    return Err(unresolved(
                        "one original native wall component owns multiple unresolved Hole branches",
                    ));
                }
                matched = Some(index);
            }
        }
        if matched.is_none() {
            continue;
        }
        for &fi in &component {
            let face = definition
                .solid
                .face_iter()
                .nth(fi)
                .ok_or_else(|| unresolved("void wall component face is absent"))?;
            for (wi, wire) in face.absolute_boundaries().iter().enumerate() {
                for (ei, _) in wire.iter().enumerate() {
                    let neighbor = neighbor_use(definition, (fi, wi, ei), work, cancel)?;
                    let ColumnFace::Cap { height, sign } = faces[neighbor.0] else {
                        continue;
                    };
                    let source_lower = height.hi < midpoint;
                    if source_lower != down_is_source_lower {
                        continue;
                    }
                    // Process each original cap wire once. Every one of its
                    // paired wall uses must belong to this SAME native column.
                    if neighbor.2 != 0 {
                        continue;
                    }
                    let cap_face = definition
                        .solid
                        .face_iter()
                        .nth(neighbor.0)
                        .ok_or_else(|| unresolved("void material-facing cap face is absent"))?;
                    for (ci, _) in cap_face.absolute_boundaries()[neighbor.1]
                        .iter()
                        .enumerate()
                    {
                        let wall =
                            neighbor_use(definition, (neighbor.0, neighbor.1, ci), work, cancel)?;
                        work.charge(component.len() as u64, cancel)?;
                        if !component.contains(&wall.0) {
                            valid = false;
                        }
                    }
                    if !valid {
                        continue;
                    }
                    let area = wire_area(
                        definition,
                        neighbor.0,
                        neighbor.1,
                        storage.limit.saturating_sub(storage.used),
                        work,
                        cancel,
                    )?;
                    // Original inward rim degree is positive at the source
                    // lower end and negative at the upper end. An exterior
                    // column cannot impersonate an original native Hole.
                    if (down_is_source_lower && area.lo <= 0.0)
                        || (!down_is_source_lower && area.hi >= 0.0)
                    {
                        valid = false;
                        continue;
                    }
                    let is_open = if down_is_source_lower {
                        sign < 0
                    } else {
                        sign > 0
                    };
                    if opening.is_some_and(|previous| previous != is_open) {
                        valid = false;
                    }
                    opening = Some(is_open);
                    if let Some(height) =
                        super::critical::material_link::constant_material_face_height(
                            definition,
                            neighbor.0,
                            super::slab::HeightFunctional {
                                direction: matrix[2],
                                offset: Interval::point(translation[2]),
                            },
                            work,
                            cancel,
                        )?
                    {
                        lower_world = lower_world.min(height);
                        continue;
                    }
                    use monstertruck_modeling::BoundedCurve;
                    for rim in &cap_face.absolute_boundaries()[neighbor.1] {
                        rim.with_curve(|curve| {
                            let prepared = definition.prepare_boundary_curve(
                                rim.id(),
                                curve,
                                curve.range_tuple(),
                                storage.limit.saturating_sub(storage.used),
                                work,
                                cancel,
                            )?;
                            for span in prepared.parameter_intervals() {
                                let position = prepared.jet(span, work, cancel)?.position;
                                lower_world = lower_world.min(
                                    finite(
                                        dot(matrix[2], position)
                                            .add(Interval::point(translation[2]))
                                            .sub(floor_transport_error),
                                    )?
                                    .lo,
                                );
                            }
                            Ok::<_, GeometryError>(())
                        })?;
                    }
                }
            }
        }
        if valid && let Some(index) = matched {
            let candidate = if opening == Some(true) {
                ProtectedVoidHeightScope::OpenBelow {
                    upper_mm: capture.world_interval[1],
                }
            } else if opening == Some(false) && lower_world.is_finite() {
                ProtectedVoidHeightScope::Bounded {
                    lower_mm: lower_world,
                    upper_mm: capture.world_interval[1],
                }
            } else {
                continue;
            };
            if revalidate_raw_scope(candidate, transport_supports, translation[2], work, cancel)?
                .is_some()
            {
                // The raw column theorem is not authority where G is active.
                // Revalidate the COMPLETE original component, its actual
                // collars/cones, all exits, floor sheets and nonincident source
                // clearance before using ANY scalar or physical error bound.
                super::critical::material_link::prove_source_wall_component(
                    crate::section::OriginalTrimSource::standalone(definition),
                    &component,
                    [lower_safe, upper_safe],
                    storage.limit.saturating_sub(storage.used),
                    work,
                    cancel,
                )?;
            }
            if result[index].is_some() {
                return Err(unresolved(
                    "protected native Hole has ambiguous original wall components",
                ));
            }
            result[index] = Some(candidate);
        }
    }
    if result.iter().any(Option::is_none) {
        return Err(unresolved(
            "protected native Hole has no completed original material-facing column/floor incidence",
        ));
    }
    Ok(result)
}

/// Project lazily after actual support demand, under the caller's cumulative
/// work/cancellation and remaining owned-capacity allowance. All returned loops
/// are positive WORLD-XY outer masks at z=0, never deposited/source material.
pub fn projected_protected_void_envelopes(
    definition: &Definition,
    capture: &NativeVoidProjectionCapture,
    policy: ProtectedVoidPolicy,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<NativeProjectedVoids, GeometryError> {
    work.charge(1, cancel)?;
    if !policy.maximum_xy_error_mm.is_finite() || policy.maximum_xy_error_mm <= 0.0 {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "protected void policy requires a positive finite XY error",
        ));
    }
    if definition.id() != capture.definition_id()
        || capture.material_realization
            != crate::kernel::MaterialRealizationIdentity::new(definition)
    {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "protected void source/slab/native sample capture mismatch",
        ));
    }
    let request = capture.request;
    let pose = capture.pose;
    let matrix = super::placed::rotation(pose.unwrap_or(RigidPoseMm::IDENTITY))?;
    let mut storage = Storage {
        used: 0,
        limit: (policy.byte_limit as usize).min(MAX_BYTES),
    };
    storage.debit(definition.id().allocated_bytes(), work, cancel)?;
    storage.debit(
        66 * std::mem::size_of::<([[f64; 2]; 2], u8)>(),
        work,
        cancel,
    )?;
    storage.debit(definition.retained_query_proof_bytes(), work, cancel)?;
    storage.debit(capture.retained_bytes(), work, cancel)?;
    // Reject a structurally noncolumn branch before evaluating the entire
    // source realization. Reuse this SAME classification in the full proof;
    // passing this necessary precondition never grants material authority.
    let faces = original_column_faces(definition, &capture.holes, &mut storage, work, cancel)?;
    // This provider visits EVERY original central collar and corner plan, not
    // merely the strata active in the captured slab. Its error is physical 3D;
    // querying world Z does not turn it into a scalar-only displacement bound.
    let transport_supports = if capture.holes.is_empty() {
        Vec::new()
    } else {
        super::critical::material_link::source_transport_supports(
            crate::section::OriginalTrimSource::standalone(definition),
            matrix[2],
            None,
            storage.limit.saturating_sub(storage.used),
            work,
            cancel,
        )?
    };
    storage.debit(
        transport_supports
            .capacity()
            .checked_mul(std::mem::size_of::<
                super::critical::material_link::SourceTransportSupport,
            >())
            .ok_or_else(|| {
                error(
                    GeometryErrorCode::ResourceLimit,
                    "protected void support capacity overflow",
                )
            })?,
        work,
        cancel,
    )?;
    let mut source_realization_error_mm = 0.0_f64;
    for support in &transport_supports {
        work.charge(1, cancel)?;
        if !support.source_error_mm.is_finite() || support.source_error_mm < 0.0 {
            return Err(error(
                GeometryErrorCode::ConditioningFailure,
                "invalid protected void source realization bound",
            ));
        }
        source_realization_error_mm = source_realization_error_mm.max(support.source_error_mm);
    }
    let mut envelopes = Vec::new();
    storage.reserve(&mut envelopes, capture.holes.len(), work, cancel)?;
    if capture.holes.is_empty() && capture.continuation.critical_counts() != Some([0, 0]) {
        let no_void = match &capture.continuation {
            NativeVoidContinuation::ClosedRadial(product) => {
                product.kind() == super::critical::RadialRegionKind::Disk
                    && capture.pose.is_some_and(|pose| product.matches_world(
                        definition, pose, capture.world_interval, capture.world_sample,
                    ))
            }
            NativeVoidContinuation::OriginalCollar(complex)
                if complex.definition_id() == definition.id()
                    && complex.material_realization_identity() == capture.material_realization =>
            {
                // A complete original disk product has no holes at ANY height,
                // including a closed birth/death outside the captured sample.
                // Under the actual invertible affine pose, a plane section is
                // the original disk intersected with a strip (or parallel
                // fibers), so it cannot acquire a bounded complementary hole.
                super::slab::native_closed_disk_product(
                    definition,
                    storage.limit.saturating_sub(storage.used),
                    work,
                    cancel,
                )?
                .is_some()
            }
            _ => false,
        };
        if !no_void {
            return Err(unresolved(
                "critical native void has no complete original disk-product or sample Hole branch ownership",
            ));
        }
    }
    let speed = if let Some(transverse) = capture.continuation.regular_speed() {
        projected_flow_speed(request, matrix, transverse)?
    } else {
        0.0
    };
    // Keep native WORLD polygon, plane and Boolean errors independent of flow.
    let sample_band = Interval::new(
        capture.source_sample_height_band_mm[0],
        capture.source_sample_height_band_mm[1],
    )?;
    // The complete provider's global error remains necessary for branch/floor
    // obligations. Use the tighter actually captured error only for this seed
    // and its already proved closed-slab height flow.
    let sample_realization_error = capture
        .continuation
        .source_realization_error_mm()
        .unwrap_or(source_realization_error_mm);
    let source_height_error = norm(&request.frame.z_axis.map(Interval::point))?
        .mul(Interval::point(sample_realization_error));
    if capture.continuation.regular_speed().is_some()
        && (sample_band.lo - source_height_error.hi < request.height_interval_mm[0]
            || sample_band.hi + source_height_error.hi > request.height_interval_mm[1])
    {
        return Err(unresolved(
            "protected void realized sample-height shift leaves its certified regular-flow slab",
        ));
    }
    let projection = norm(&[
        matrix[0][0],
        matrix[0][1],
        matrix[0][2],
        matrix[1][0],
        matrix[1][1],
        matrix[1][2],
    ])?;
    let realization_seed_error = Interval::point(sample_realization_error)
        .mul(projection)
        .add(Interval::point(speed).mul(source_height_error));
    if capture
        .native_errors
        .iter()
        .any(|value| !value.is_finite() || *value < 0.0)
    {
        return Err(error(
            GeometryErrorCode::ConditioningFailure,
            "protected void native error capture is invalid",
        ));
    }
    let native_error = finite(
        capture
            .native_errors
            .into_iter()
            .fold(Interval::point(0.0), |sum, value| {
                sum.add(Interval::point(value))
            }),
    )?;
    let sample_error = finite(native_error.add(realization_seed_error))?;
    if sample_error.hi > policy.maximum_xy_error_mm {
        return Err(error(
            GeometryErrorCode::ShapeErrorExceeded,
            "protected void native and realized sample error exceeds XY policy",
        ));
    }
    let scopes = if capture.holes.is_empty() {
        Vec::new()
    } else {
        native_height_scopes(
            definition,
            capture,
            &faces,
            &transport_supports,
            &mut storage,
            work,
            cancel,
        )?
    };
    // Full-column branch/floor authority precedes all global metric use.
    // Raw column flow is source-Z, evaluated through the actual affine pose,
    // not a captured local height-flow estimate extrapolated to the floor.
    let radius = if capture.holes.is_empty() {
        0.0
    } else {
        let vertical = if matrix[2][2].lo > 0.0 {
            matrix[2][2]
        } else {
            matrix[2][2].neg()
        };
        let column_speed = norm(&[matrix[0][2], matrix[1][2]])?.div(vertical)?;
        let bounds = definition.bounds_mm();
        let source_box = std::array::from_fn(|i| Interval {
            lo: bounds.min[i],
            hi: bounds.max[i],
        });
        let source_world_height = dot(matrix[2], source_box)
            .add(Interval::point(pose.map_or(0.0, |p| p.translation_mm[2])));
        let full_distance = source_world_height
            .sub(Interval::point(capture.world_sample))
            .abs_upper();
        let full_realization_error = Interval::point(source_realization_error_mm)
            .mul(projection.add(column_speed.mul(norm(&matrix[2])?)));
        let full_metric_error = finite(native_error.add(full_realization_error))?;
        if full_metric_error.hi > policy.maximum_xy_error_mm {
            return Err(error(
                GeometryErrorCode::ShapeErrorExceeded,
                "protected void full-scope realized source error exceeds XY policy",
            ));
        }
        finite(full_metric_error.add(column_speed.mul(Interval::point(up(full_distance)))))?.hi
    };
    for (index, hole) in capture.holes.iter().enumerate() {
        work.charge(1, cancel)?;
        let height_scope = scopes[index]
            .ok_or_else(|| unresolved("native protected void scope receipt is missing"))?;
        // Completed source-column authority proves the original raw XY branch
        // invariant throughout its full column. Its immutable Hole lineage,
        // global G metric allowance and actual-pose axial drift therefore
        // authorize this PER-HOLE boundary even for a critical slab capture.
        let world_xy_outer = outward_hull(&hole.boundary, radius, &mut storage, work, cancel)?;
        envelopes.push(NativeProtectedVoidEnvelope {
            source_loop_index: hole.source_loop_index,
            world_xy_outer,
            height_scope,
        });
    }
    Ok(NativeProjectedVoids {
        definition_id: definition.id().clone(),
        material_realization: capture.material_realization,
        placed_pose: pose,
        world_height_interval_mm: capture.world_interval,
        sample_world_z_mm: capture.world_sample,
        _source_request: request,
        _world_shape_error_policy_mm: capture.world_policy,
        _policy: policy,
        envelopes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PlaneFrameMm;

    #[test]
    fn zero_containing_norm_uses_only_the_proved_sum_of_squares_image() {
        let values = [Interval {
            lo: -1.0e-12,
            hi: 1.0e-12,
        }; 2];
        let raw = values
            .iter()
            .fold(Interval::point(0.0), |s, v| s.add(v.square()));
        assert!(
            raw.lo < 0.0,
            "directed zero additions may have a negative enclosure endpoint"
        );
        let result = norm(&values).unwrap();
        assert_eq!(result.lo, 0.0);
        assert!(result.hi >= 1.414e-12);
        let exact_zero = norm(&[Interval::point(0.0); 2]).unwrap();
        assert_eq!([exact_zero.lo, exact_zero.hi], [0.0, 0.0]);
        assert!(
            norm(&[Interval {
                lo: f64::NAN,
                hi: 1.0
            }])
            .is_err()
        );
        assert!(norm(&[Interval { lo: 1.0, hi: 0.0 }]).is_err());
        assert!(norm(&[Interval::point(f64::MAX)]).is_err());
        assert!(
            Interval {
                lo: -f64::MIN_POSITIVE,
                hi: 1.0
            }
            .sqrt()
            .is_err(),
            "generic square root still rejects unproved negative operands"
        );
    }

    fn world_frame() -> PlaneFrameMm {
        PlaneFrameMm {
            origin_mm: [0.0; 3],
            x_axis: [1.0, 0.0, 0.0],
            y_axis: [0.0, 1.0, 0.0],
            z_axis: [0.0, 0.0, 1.0],
        }
    }
    #[test]
    fn raw_void_scope_revalidation_uses_full_closed_physical_support() {
        let supports = [
            super::super::critical::material_link::SourceTransportSupport {
                owner:
                    super::super::critical::material_link::SourceTransportOwner::OriginalCoedge {
                        face: 0,
                        coedge: 0,
                    },
                base_interval: [4.0, 4.0],
                corrected_interval: [4.0, 4.0],
                // Horizontal-only correction is still an active material obligation.
                displacement_interval: [0.0, 0.0],
                source_error_mm: 1.0e-8,
            },
        ];
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100);
        let slab_only = ProtectedVoidHeightScope::Bounded {
            lower_mm: 5.9,
            upper_mm: 6.1,
        };
        assert!(
            revalidate_raw_scope(slab_only, &supports, 0.0, &mut work, &cancel)
                .unwrap()
                .is_none()
        );
        let whole_branch = ProtectedVoidHeightScope::Bounded {
            lower_mm: 4.0,
            upper_mm: 6.1,
        };
        assert!(
            revalidate_raw_scope(whole_branch, &supports, 0.0, &mut work, &cancel)
                .unwrap()
                .is_some()
        );
        let posed = ProtectedVoidHeightScope::Bounded {
            lower_mm: 16.0,
            upper_mm: 18.1,
        };
        assert!(
            revalidate_raw_scope(posed, &supports, 12.0, &mut work, &cancel)
                .unwrap()
                .is_some()
        );
        assert!(
            revalidate_raw_scope(whole_branch, &[], 0.0, &mut work, &cancel)
                .unwrap()
                .is_none()
        );
        let mut exhausted = GeometryWorkBudget::new(0);
        assert_eq!(
            revalidate_raw_scope(whole_branch, &supports, 0.0, &mut exhausted, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::ResourceLimit,
        );
        assert_eq!(
            revalidate_raw_scope(
                whole_branch,
                &supports,
                0.0,
                &mut work,
                &AtomicBool::new(true)
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::Cancelled,
        );
    }

    fn original_definition(name: &str, solid: crate::parity_fixtures::StepSolid) -> Definition {
        let source = crate::parity_fixtures::original(name, solid);
        crate::import_step(source.as_bytes(), &AtomicBool::new(false))
            .expect("original immutable native STEP")
    }
    fn recipe(name: &str) -> Definition {
        let (_, solid, _) = crate::parity_fixtures::recipes()
            .into_iter()
            .find(|(candidate, _, _)| *candidate == name)
            .expect("actual native recipe");
        original_definition(name, solid)
    }
    #[test]
    fn noncolumn_original_faces_refuse_before_transport_preparation() {
        let definition = recipe("native-parity-ring-torus.step");
        let holes = [NativeCapturedHole {
            source_loop_index: 0,
            // The necessary source-face check neither reads this proposal nor
            // issues authority for it. Only the actual coefficient owners below
            // determine whether the current column lane is even applicable.
            boundary: square(),
            source_faces: (0..definition.faces.len()).collect(),
        }];
        let mut storage = Storage {
            used: 0,
            limit: MAX_BYTES,
        };
        let mut work = GeometryWorkBudget::new(20_000_000);
        let error = original_column_faces(
            &definition,
            &holes,
            &mut storage,
            &mut work,
            &AtomicBool::new(false),
        )
        .err()
        .expect("noncolumn original owners cannot enter the column lane");
        assert_eq!(error.code, GeometryErrorCode::AnalysisIndeterminate);
        assert!(error.message.contains("material-facing"));
    }

    fn analytic_bore() -> Definition {
        let mut hole = crate::parity_fixtures::circle(10.0, 10.0, 3.0);
        hole.invert();
        original_definition(
            "original-protected-through-bore.step",
            crate::parity_fixtures::extruded(
                vec![crate::parity_fixtures::rectangle(20.0, 20.0), hole],
                8.0,
            ),
        )
    }
    /// Sew the actual rational-cylinder source wall, with its original rational
    /// rims/seam/pcurves, into independent rectangular outer cap cycles. This is
    /// not an analytic family rename or a trusted sampled circular certificate.
    fn rational_bore() -> Definition {
        use monstertruck_io::step::load::step_geometry::re_exports::*;
        use monstertruck_topology::{Face, Solid};
        let (_, plug, _) = crate::parity_fixtures::recipes()
            .into_iter()
            .find(|(name, _, _)| *name == "native-parity-nurbs-cylinder.step")
            .unwrap();
        let translation = Matrix4::from_translation(Vector3::new(-10.0, -10.0, 0.0));
        let outer = crate::parity_fixtures::extruded(
            vec![crate::parity_fixtures::rectangle(20.0, 20.0)],
            8.0,
        )
        .try_mapped(
            |point| Some(translation.transform_point(*point)),
            |curve| Some(curve.transformed(translation)),
            |surface| Some(surface.transformed(translation)),
        )
        .unwrap();
        let cap_height = |face: &monstertruck_topology::Face<
            Point3,
            monstertruck_io::step::load::step_geometry::Curve3D,
            monstertruck_io::step::load::step_geometry::Surface,
        >| {
            let mut vertices = face.vertex_iter();
            let z = vertices.next().unwrap().point().z;
            vertices.all(|vertex| vertex.point().z == z).then_some(z)
        };
        let mut faces = Vec::new();
        for face in outer.face_iter() {
            if let Some(z) = cap_height(face) {
                let cap = plug
                    .face_iter()
                    .find(|candidate| cap_height(candidate) == Some(z))
                    .unwrap();
                let mut boundaries = face.boundaries();
                boundaries.extend(cap.boundaries().into_iter().map(|wire| wire.inverse()));
                if !cap.orientation() {
                    boundaries = boundaries.into_iter().map(|wire| wire.inverse()).collect();
                }
                let ring = Face::new(boundaries, cap.surface());
                faces.push(if cap.orientation() {
                    ring
                } else {
                    ring.inverse()
                });
            } else {
                faces.push(face.clone());
            }
        }
        faces.extend(
            plug.face_iter()
                .filter(|face| cap_height(face).is_none())
                .map(|face| face.inverse()),
        );
        let shell = faces
            .into_iter()
            .collect::<monstertruck_topology::Shell<_, _, _>>();
        shell
            .check_solid_boundary()
            .expect("original rational bore shares exact rim edge identities");
        original_definition(
            "original-rational-protected-bore.step",
            Solid::new(vec![shell]),
        )
    }
    fn captured(
        definition: &Definition,
        pose: RigidPoseMm,
        interval: [f64; 2],
        sample: f64,
        work: &mut GeometryWorkBudget,
    ) -> NativeVoidProjectionCapture {
        let cancel = AtomicBool::new(false);
        let source = crate::prepare_material_source(
            definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &cancel,
            work,
        )
        .unwrap();
        let analysis = source
            .analyze_placed_slab(
                &super::super::PlacedSlabRequest {
                    pose,
                    world_height_interval_mm: interval,
                    sample_world_z_mm: sample,
                    native_shape_error_mm: 0.05,
                },
                crate::MAX_PREPARED_SECTION_BYTES.saturating_sub(source.retained_bytes()),
                work,
                &cancel,
            )
            .expect("completed actual posed native slab");
        let native_indices = analysis
            .section()
            .loops()
            .iter()
            .enumerate()
            .filter_map(|(i, boundary)| boundary.is_hole.then_some(i))
            .collect::<Vec<_>>();
        let buffers = native_indices
            .iter()
            .map(|&index| {
                (
                    analysis.section().loops()[index].points_mm.as_ptr(),
                    analysis.section().loop_source_faces[index].as_ptr(),
                )
            })
            .collect::<Vec<_>>();
        let capture = capture_projected_voids(analysis, MAX_BYTES as u32, work, &cancel)
            .unwrap()
            .unwrap();
        assert_eq!(
            capture
                .holes
                .iter()
                .map(|hole| hole.source_loop_index)
                .collect::<Vec<_>>(),
            native_indices
        );
        assert!(
            capture
                .holes
                .iter()
                .all(|hole| !hole.source_faces.is_empty())
        );
        for (hole, (points, owners)) in capture.holes.iter().zip(buffers) {
            assert_eq!(
                hole.boundary.points_mm.as_ptr(),
                points,
                "Hole points must move, not clone"
            );
            assert_eq!(
                hole.source_faces.as_ptr(),
                owners,
                "original source owner buffers must move, not clone"
            );
        }
        capture
    }
    fn query(
        definition: &Definition,
        capture: &NativeVoidProjectionCapture,
        work: &mut GeometryWorkBudget,
    ) -> NativeProjectedVoids {
        projected_protected_void_envelopes(
            definition,
            capture,
            ProtectedVoidPolicy {
                maximum_xy_error_mm: spiling_contracts::geometry::SECTION_SAMPLING_TOLERANCE_MM,
                byte_limit: MAX_BYTES as u32,
            },
            work,
            &AtomicBool::new(false),
        )
        .expect("actual native outer protected void")
    }

    #[test]
    fn protected_void_projection_retains_and_rejects_changed_material_recipe() {
        let mut definition = recipe("native-parity-blind-pocket.step");
        let mut work = GeometryWorkBudget::new(200_000_000);
        let mut capture = captured(
            &definition,
            RigidPoseMm::IDENTITY,
            [5.9, 6.1],
            6.0,
            &mut work,
        );
        let result = query(&definition, &capture, &mut work);
        assert!(result.is_compatible_with_definition(&definition));
        assert_eq!(
            result.envelopes()[0].height_scope(),
            ProtectedVoidHeightScope::Bounded {
                lower_mm: 4.0,
                upper_mm: 6.1
            },
            "the actual material branch proof must retain the FULL floor-to-demand scope",
        );
        // A captured regular slab is NOT the endpoint inventory of the whole
        // original wall: its upper edge cuts live material, not a source cap.
        let error = super::super::critical::material_link::prove_source_wall_component(
            crate::section::OriginalTrimSource::standalone(&definition),
            &capture.holes[0].source_faces,
            [4.0, 6.1],
            MAX_BYTES,
            &mut work,
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert_eq!(error.code, GeometryErrorCode::AnalysisIndeterminate);
        assert!(error.message.contains("boundary exit"));
        let allowance = if definition.provenance().uncertainty_mm.is_some() {
            None
        } else {
            Some(0.0)
        };
        capture.material_realization =
            crate::kernel::MaterialRealizationIdentity::with_allowance(allowance);
        assert_eq!(
            projected_protected_void_envelopes(
                &definition,
                &capture,
                ProtectedVoidPolicy {
                    maximum_xy_error_mm: spiling_contracts::geometry::SECTION_SAMPLING_TOLERANCE_MM,
                    byte_limit: MAX_BYTES as u32,
                },
                &mut work,
                &AtomicBool::new(false),
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::InvalidGeometry,
        );
        definition.provenance.uncertainty_mm = allowance;
        assert!(!result.is_compatible_with_definition(&definition));
    }
    fn outside_ccw(cycle: &SectionLoopMm, point: [f64; 2]) -> bool {
        cycle.points_mm.windows(2).any(|pair| {
            orientation([pair[0][0], pair[0][1]], [pair[1][0], pair[1][1]], point).unwrap() < 0
        })
    }

    #[test]
    fn actual_analytic_rational_and_independent_through_bores_preserve_diagonal_corridors() {
        let independent = crate::import_step(
            include_bytes!(
                "../../../../fixtures/geometry/independent-exporters/occt-through-hole-mm.step"
            ),
            &AtomicBool::new(false),
        )
        .unwrap();
        for (definition, center, radius) in [
            (analytic_bore(), [10.0, 10.0], 3.0),
            (rational_bore(), [0.0, 0.0], 5.0),
            (independent, [10.0, 10.0], 3.0),
        ] {
            let mut work = GeometryWorkBudget::new(200_000_000);
            let pose = RigidPoseMm {
                translation_mm: [7.0, -3.0, 12.0],
                ..RigidPoseMm::IDENTITY
            };
            let capture = captured(&definition, pose, [17.9, 18.1], 18.0, &mut work);
            let before = work.consumed();
            let result = query(&definition, &capture, &mut work);
            assert!(work.consumed() > before);
            assert_eq!(result.definition_id(), definition.id());
            assert_eq!(result.placed_pose(), Some(pose));
            assert_eq!(result.world_height_interval_mm(), [17.9, 18.1]);
            assert_eq!(result.sample_world_z_mm(), 18.0);
            assert_eq!(result.envelopes().len(), 1);
            let envelope = &result.envelopes()[0];
            assert!(matches!(
                envelope.height_scope(),
                ProtectedVoidHeightScope::OpenBelow { upper_mm: 18.1 }
            ));
            let outer = envelope.world_xy_outer();
            assert!(!outer.is_hole && outer.points_mm.len() > 8);
            assert_eq!(outer.points_mm.first(), outer.points_mm.last());
            assert!(
                outer
                    .points_mm
                    .iter()
                    .all(|point| point[2] == 0.0 && point.iter().all(|x| x.is_finite()))
            );
            let center = [center[0] + 7.0, center[1] - 3.0];
            // A square enclosing r would cover this diagonal point; the true
            // circular-ish Minkowski hull must leave the breakaway lane open.
            let diagonal = (radius + 0.4) / 2.0_f64.sqrt();
            assert!(outside_ccw(
                outer,
                [center[0] + diagonal, center[1] + diagonal]
            ));
            for step in 0..256 {
                let angle = step as f64 * std::f64::consts::TAU / 256.0;
                assert!(!outside_ccw(
                    outer,
                    [
                        center[0] + radius * angle.cos(),
                        center[1] + radius * angle.sin()
                    ]
                ));
            }
        }
    }

    #[test]
    fn floating_blind_floor_and_exact_rational_floor_terminate_at_original_face() {
        for name in [
            "native-parity-blind-pocket.step",
            "native-parity-blind-pocket-nurbs-floor.step",
        ] {
            let definition = recipe(name);
            let pose = RigidPoseMm {
                translation_mm: [0.0, 0.0, 12.0],
                ..RigidPoseMm::IDENTITY
            };
            let mut work = GeometryWorkBudget::new(200_000_000);
            let capture = captured(&definition, pose, [17.9, 18.1], 18.0, &mut work);
            let result = query(&definition, &capture, &mut work);
            assert_eq!(result.envelopes().len(), 1);
            let ProtectedVoidHeightScope::Bounded { lower_mm, upper_mm } =
                result.envelopes()[0].height_scope()
            else {
                panic!("a real solid pocket floor must never become bottom-open");
            };
            assert!(lower_mm <= 16.0 && lower_mm > 15.999999);
            assert_eq!(upper_mm, 18.1);
            assert!(lower_mm > definition.bounds_mm().min[2] + pose.translation_mm[2] + 3.99);
        }
    }

    #[test]
    fn independent_rounded_floor_is_not_the_global_source_minimum() {
        let definition = crate::import_step(
            include_bytes!(
                "../../../../fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step"
            ),
            &AtomicBool::new(false),
        )
        .unwrap();
        let mut work = GeometryWorkBudget::new(300_000_000);
        let capture = captured(
            &definition,
            RigidPoseMm::IDENTITY,
            [6.0, 6.1],
            6.05,
            &mut work,
        );
        let result = query(&definition, &capture, &mut work);
        let ProtectedVoidHeightScope::Bounded { lower_mm, .. } =
            result.envelopes()[0].height_scope()
        else {
            panic!("independent original pocket has a floor");
        };
        assert!(lower_mm <= 4.13 && lower_mm > 4.129999);
    }

    #[test]
    fn actual_near_unit_pose_projects_axial_source_height_instead_of_assuming_rigid_orthogonality()
    {
        let pose = RigidPoseMm {
            translation_mm: [11.0, -7.0, 12.0],
            rotation_xyzw: [
                0.01,
                -0.02,
                0.03,
                (1.0_f64 - 0.0014).sqrt() * (1.0 + 1.0e-13),
            ],
        };
        let definition = analytic_bore();
        let mut work = GeometryWorkBudget::new(200_000_000);
        let capture = captured(&definition, pose, [17.9, 18.1], 18.0, &mut work);
        let matrix = super::super::placed::rotation(pose).unwrap();
        let speed = projected_flow_speed(
            capture.request,
            matrix,
            capture.continuation.regular_speed().unwrap(),
        )
        .unwrap();
        let n = capture.request.frame.z_axis.map(Interval::point);
        let axial = [
            dot(matrix[0], n).div(dot(n, n)).unwrap(),
            dot(matrix[1], n).div(dot(n, n)).unwrap(),
        ];
        assert!(speed >= norm(&axial).unwrap().hi);
        let result = query(&definition, &capture, &mut work);
        assert_eq!(result.placed_pose(), Some(pose));
        assert!(matches!(
            result.envelopes()[0].height_scope(),
            ProtectedVoidHeightScope::OpenBelow { .. }
        ));
    }

    fn square() -> SectionLoopMm {
        SectionLoopMm {
            is_hole: true,
            points_mm: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0],
            ],
        }
    }
    #[test]
    fn outward_hull_arithmetic_capacity_and_cancel_are_real_obligations() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let mut storage = Storage {
            used: 0,
            limit: MAX_BYTES,
        };
        let outer = outward_hull(&square(), 0.125, &mut storage, &mut work, &cancel).unwrap();
        assert!(!outside_ccw(&outer, [-0.125, -0.125]));
        let first = storage.used;
        storage.limit = first + 1;
        let failure = outward_hull(&square(), 0.125, &mut storage, &mut work, &cancel).unwrap_err();
        assert_eq!(failure.code, GeometryErrorCode::ResourceLimit);
        let mut storage = Storage {
            used: 0,
            limit: MAX_BYTES,
        };
        let failure = outward_hull(
            &square(),
            0.125,
            &mut storage,
            &mut work,
            &AtomicBool::new(true),
        )
        .unwrap_err();
        assert_eq!(failure.code, GeometryErrorCode::Cancelled);
        assert_eq!(storage.used, 0);
        let mut huge = square();
        huge.points_mm[0][0] = f64::MAX;
        huge.points_mm[4] = huge.points_mm[0];
        assert_eq!(
            outward_hull(&huge, f64::MAX, &mut storage, &mut work, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::ConditioningFailure
        );
        let ill = orientation([0.0, 0.0], [1.1, 1.1], [2.2, up(2.2)]);
        assert!(
            ill.is_err(),
            "unresolved last-bit orientation cannot manufacture an outer certificate"
        );
    }

    #[test]
    fn all_z_zero_void_is_proved_and_source_mismatch_does_not_reuse_a_capture() {
        let definition = recipe("native-parity-nurbs-cylinder.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000_000);
        let source = crate::prepare_material_source(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &cancel,
            &mut work,
        )
        .unwrap();
        let analysis = source
            .analyze_slab(
                SlabRequest {
                    frame: world_frame(),
                    height_interval_mm: [3.9, 4.1],
                    sample_height_mm: 4.0,
                    native_shape_error_mm: 0.05,
                },
                crate::MAX_PREPARED_SECTION_BYTES.saturating_sub(source.retained_bytes()),
                &mut work,
                &cancel,
            )
            .unwrap();
        assert!(
            capture_projected_voids(analysis, MAX_BYTES as u32, &mut work, &cancel)
                .unwrap()
                .is_none()
        );
        drop(source);
        let bore = analytic_bore();
        let capture = captured(&bore, RigidPoseMm::IDENTITY, [5.9, 6.1], 6.0, &mut work);
        let error = projected_protected_void_envelopes(
            &definition,
            &capture,
            ProtectedVoidPolicy {
                maximum_xy_error_mm: 0.025,
                byte_limit: MAX_BYTES as u32,
            },
            &mut work,
            &cancel,
        )
        .unwrap_err();
        assert_eq!(error.code, GeometryErrorCode::InvalidGeometry);
    }

    #[test]
    fn closed_birth_without_sample_holes_requires_complete_original_disk_product() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(20_000_000);
        let definition = crate::import_step(
            include_bytes!("../../../../fixtures/geometry/box-mm.step"),
            &cancel,
        )
        .unwrap();
        let capture = captured(
            &definition,
            RigidPoseMm {
                translation_mm: [0.0, 0.0, 2.0000004],
                ..RigidPoseMm::IDENTITY
            },
            [1.99, 2.02],
            1.995,
            &mut work,
        );
        assert!(capture.holes.is_empty());
        assert!(
            query(&definition, &capture, &mut work)
                .envelopes()
                .is_empty()
        );

        // The native floor is above this sample but inside its CLOSED slab.
        // A filled sample cannot erase the off-grid original pocket birth.
        let pocket = recipe("native-parity-blind-pocket.step");
        let mut work = GeometryWorkBudget::new(20_000_000);
        let capture = captured(
            &pocket,
            RigidPoseMm::IDENTITY,
            [3.99, 4.02],
            3.995,
            &mut work,
        );
        assert!(capture.holes.is_empty());
        let error = projected_protected_void_envelopes(
            &pocket,
            &capture,
            ProtectedVoidPolicy {
                maximum_xy_error_mm: 0.025,
                byte_limit: MAX_BYTES as u32,
            },
            &mut work,
            &cancel,
        )
        .unwrap_err();
        assert_eq!(error.code, GeometryErrorCode::AnalysisIndeterminate);
    }

    #[test]
    fn critical_noncolumn_branch_defers_unproved_material_facing_scope() {
        let definition = recipe("native-parity-ring-torus.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(200_000_000);
        let source = crate::prepare_material_source(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &cancel,
            &mut work,
        )
        .unwrap();
        let analysis = source
            .analyze_slab(
                SlabRequest {
                    frame: world_frame(),
                    height_interval_mm: [1.9, 2.1],
                    sample_height_mm: 1.99,
                    native_shape_error_mm: 0.5,
                },
                crate::MAX_PREPARED_SECTION_BYTES.saturating_sub(source.retained_bytes()),
                &mut work,
                &cancel,
            )
            .unwrap();
        assert!(
            analysis
                .critical
                .as_ref()
                .unwrap()
                .regular_flow_speed_mm_per_height()
                .is_none()
        );
        // Extraction succeeds with a compact deferred native obligation; a
        // no-support job does not fail merely because a future scope is hard.
        let capture = capture_projected_voids(analysis, MAX_BYTES as u32, &mut work, &cancel)
            .unwrap()
            .unwrap();
        assert!(!capture.continuation.critical_events().is_empty());
        let error = projected_protected_void_envelopes(
            &definition,
            &capture,
            ProtectedVoidPolicy {
                maximum_xy_error_mm: 0.025,
                byte_limit: MAX_BYTES as u32,
            },
            &mut work,
            &cancel,
        )
        .unwrap_err();
        assert_eq!(error.code, GeometryErrorCode::AnalysisIndeterminate);
        assert!(error.message.contains("material-facing"));
    }

    #[test]
    fn independent_submillimeter_pocket_does_not_alias_away_its_native_floor() {
        let definition = crate::import_step(
            include_bytes!(
                "../../../../fixtures/geometry/independent-exporters/occt-narrow-pocket-mm.step"
            ),
            &AtomicBool::new(false),
        )
        .unwrap();
        let mut work = GeometryWorkBudget::new(200_000_000);
        let capture = captured(
            &definition,
            RigidPoseMm::IDENTITY,
            [2.9, 3.1],
            3.0,
            &mut work,
        );
        let result = query(&definition, &capture, &mut work);
        assert_eq!(result.envelopes().len(), 1);
        let ProtectedVoidHeightScope::Bounded { lower_mm, .. } =
            result.envelopes()[0].height_scope()
        else {
            panic!("the narrow original pocket has a material floor");
        };
        assert!(lower_mm <= 1.07 && lower_mm > 1.069999);
    }

    #[test]
    fn inverted_blind_pocket_uses_actual_world_facing_opening_not_source_bottom() {
        let definition = recipe("native-parity-blind-pocket.step");
        let pose = RigidPoseMm {
            translation_mm: [0.0, 0.0, 12.0],
            rotation_xyzw: [1.0, 0.0, 0.0, 0.0],
        };
        let mut work = GeometryWorkBudget::new(200_000_000);
        let capture = captured(&definition, pose, [5.9, 6.1], 6.0, &mut work);
        let result = query(&definition, &capture, &mut work);
        assert!(matches!(
            result.envelopes()[0].height_scope(),
            ProtectedVoidHeightScope::OpenBelow { upper_mm: 6.1 }
        ));
    }
}
