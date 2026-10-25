// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! A sufficient, representation-independent critical-neighborhood proof. Selected
//! source charts and native-bound oriented uses form the complex; boxes only cover it.
//! The closed capture is bounded by original material and actual halfspace cuts.
//! Convexity proves a disk complex, or convex meridian fibers with positive
//! radial degree about a separated represented core prove an annular complex.
//! Whole-source convexity is separate, strictly stronger evidence.

pub(crate) mod material_link;
pub(super) use material_link::OriginalFaceCornerAttachments;
pub(crate) use material_link::PreparedSourceVertexPlans;
pub(super) use material_link::{ClosedRadialMaterialProduct, RadialRegionKind};
pub(super) mod ray_index;
mod source_chart;
pub(in crate::analysis) use source_chart::{SourcePoleJoint, certify_source_pole_connection};
mod tensor_g1;
pub(super) use tensor_g1::interface_regular_join;
#[cfg(test)]
use tensor_g1::coefficient_tangent_join;
use tensor_g1::periodic_regular_join;

use super::slab::HeightFunctional;
use super::source_incidence::SourceEdgeIncidence;
use super::{GeometryWorkBudget, SlabRequest};
use crate::kernel::{PreparedFaceTrimDomain, TrimClassification};
use crate::parametric::{
    Interval, SurfaceEnclosure, down, surface_crease_interfaces, surface_enclosure,
    surface_parameter_boxes, surface_parameter_span_smooth, up, SurfaceEvaluator, CurveEvaluator,
};
use crate::analysis::material_source::MaterialSourceView;
use crate::kernel::trim::PreparedTrimSurface;
use crate::{Definition, GeometryError, NativeSection, PlaneFrameMm, SectionLoopMm};
use solidcore::curve::curve::{Curve, CurveKind};
use solidcore::surface::surface::SurfaceKind;
use solidcraft_step::geom::{Curve as SourceCurve, Surface as SourceSurface};
use spiling_contracts::geometry::{DefinitionId, GeometryErrorCode};
use std::sync::{Arc, atomic::AtomicBool};

const MAX_DEPTH: u8 = 52;
const MAX_NEIGHBORHOODS: usize = 65_536;
const MAX_SCRATCH_BYTES: usize = 32 * 1024 * 1024;
const MAX_EVENT_BYTES: usize = MAX_SCRATCH_BYTES / 4;
const MAX_EDGE_BYTES: usize = MAX_SCRATCH_BYTES / 8;
const MAX_SAMPLE_BYTES: usize = MAX_SCRATCH_BYTES / 8;
const RESERVED_STACK_BYTES: usize = 128 * 1024;

#[derive(Debug)]
struct CriticalCapture {
    definition_id: DefinitionId,
    request: SlabRequest,
    direction: [Interval; 3],
    offset: Interval,
    material_realization: crate::kernel::MaterialRealizationIdentity,
}
impl CriticalCapture {
    fn new(
        definition: &Definition,
        request: &SlabRequest,
        functional: HeightFunctional,
    ) -> Arc<Self> {
        Arc::new(Self {
            definition_id: definition.id().clone(),
            request: *request,
            direction: functional.direction,
            offset: functional.offset,
            material_realization: crate::kernel::MaterialRealizationIdentity::new(definition),
        })
    }
}

fn critical_storage_bytes(definition_id: &DefinitionId, event_capacity: usize) -> Option<usize> {
    event_capacity
        .checked_mul(std::mem::size_of::<CriticalEventNeighborhood>())?
        .checked_add(2 * std::mem::size_of::<usize>())?
        .checked_add(std::mem::size_of::<CriticalCapture>())?
        .checked_add(definition_id.allocated_bytes())
}
fn check_critical_storage(
    definition_id: &DefinitionId,
    event_capacity: usize,
    remaining_bytes: usize,
) -> Result<(), GeometryError> {
    if critical_storage_bytes(definition_id, event_capacity)
        .is_none_or(|bytes| bytes > remaining_bytes.min(MAX_SCRATCH_BYTES))
    {
        return Err(failure(
            GeometryErrorCode::ResourceLimit,
            "critical capture/event capacity exceeds remaining slab allowance",
            0,
            event_capacity,
        ));
    }
    Ok(())
}

/// Opaque incidence evidence for a completely covered critical neighborhood.
/// A bracket is a covering, not an assertion that its midpoint is an event.
#[derive(Debug, Clone)]
pub struct CriticalEventNeighborhood {
    capture: Arc<CriticalCapture>,
    height_bracket_mm: [f64; 2],
    source_face: Option<usize>,
    stationary_root_proved: bool,
    incoming_regular_endpoint: bool,
    pairing: ComponentVoidPairing,
}
impl CriticalEventNeighborhood {
    pub fn definition_id(&self) -> &DefinitionId {
        &self.capture.definition_id
    }
    pub fn request(&self) -> SlabRequest {
        self.capture.request
    }
    pub fn height_direction_enclosure(&self) -> [[f64; 2]; 3] {
        self.capture.direction.map(|value| [value.lo, value.hi])
    }
    pub fn height_offset_enclosure(&self) -> [f64; 2] {
        [self.capture.offset.lo, self.capture.offset.hi]
    }
    pub fn height_bracket_mm(&self) -> [f64; 2] {
        self.height_bracket_mm
    }
    pub fn material_component_count(&self) -> usize {
        self.pairing.material_components
    }
    pub fn bounded_void_count(&self) -> usize {
        self.pairing.bounded_voids
    }
    pub fn tunnel_cycle_count(&self) -> usize {
        self.pairing.tunnel_cycles
    }
    pub fn stationary_root_proved(&self) -> bool {
        self.stationary_root_proved
    }
    pub fn source_face_ordinal(&self) -> Option<usize> {
        self.source_face
    }
    /// Original oriented cap/trim incidence plus complete regular continuation
    /// identifies an incoming face with the core closure, not extra strata.
    /// This is source-space authority only: the consumer must independently
    /// prove exact equality with the actual posed CLOSED request endpoint.
    pub(super) fn proved_incoming_endpoint_face(
        &self,
        definition_id: &DefinitionId,
        request: &SlabRequest,
    ) -> Option<(usize, usize)> {
        let captured = self.capture.request;
        if !self.incoming_regular_endpoint
            || self.definition_id() != definition_id
            || captured.frame != request.frame
            || captured.height_interval_mm != request.height_interval_mm
            || captured.sample_height_mm.to_bits() != request.sample_height_mm.to_bits()
            || captured.native_shape_error_mm.to_bits() != request.native_shape_error_mm.to_bits()
        {
            return None;
        }
        let endpoint = (0..2).find(|&endpoint| {
            self.height_bracket_mm == [request.height_interval_mm[endpoint]; 2]
        })?;
        Some((self.source_face?, endpoint))
    }
}

#[derive(Debug, Clone, Copy)]
struct ComponentVoidPairing {
    material_components: usize,
    bounded_voids: usize,
    tunnel_cycles: usize,
}

/// These degrees are for ORIGINAL material intersected with the two actual
/// height halfspaces, not for the polygon candidate's invented cap material.
#[derive(Debug)]
struct ClosedCaptureBoundary {
    excluded_face_cells: u64,
    excluded_coedge_cells: u64,
}

#[derive(Debug)]
enum SourceBoundaryScope {
    WholeSource,
    ClosedCapture(ClosedCaptureBoundary),
}

/// The native sample and policy against which an existing slab proof is bound.
pub(super) struct ProvedSlabSample<'a> {
    pub(super) definition: &'a Definition,
    pub(super) request: &'a SlabRequest,
    pub(super) section: &'a NativeSection,
    pub(super) boundary_motion_bound_mm: f64,
}

/// Original trimmed geometry evaluated at the captured height functional.
pub(super) struct CriticalSlabQuery<'a, 'source, 'domains> {
    pub(super) source: MaterialSourceView<'source, 'domains>,
    pub(super) request: &'a SlabRequest,
    pub(super) section: &'a NativeSection,
    pub(super) functional: HeightFunctional,
    pub(super) actual_sample_height: Interval,
}

#[derive(Debug)]
pub(super) struct CriticalSlabCertificate {
    pub(super) boundary_motion_bound_mm: f64,
    pub(super) occupied: Option<NativeSection>,
    pub(super) event_neighborhoods: Vec<CriticalEventNeighborhood>,
    capture: Arc<CriticalCapture>,
    pairing: ComponentVoidPairing,
    covered_face_cells: u64,
    covered_coedge_cells: u64,
    regular_flow_speed: Option<f64>,
    source_realization_error_mm: f64,
    boundary_scope: Option<SourceBoundaryScope>,
}
impl CriticalSlabCertificate {
    pub(super) fn definition_id(&self) -> &DefinitionId {
        &self.capture.definition_id
    }
    pub(super) fn material_realization_identity(
        &self,
    ) -> crate::kernel::MaterialRealizationIdentity {
        self.capture.material_realization
    }
    pub(super) fn request(&self) -> SlabRequest {
        self.capture.request
    }
    pub(super) fn material_component_count(&self) -> usize {
        self.pairing.material_components
    }
    pub(super) fn bounded_void_count(&self) -> usize {
        self.pairing.bounded_voids
    }
    pub(super) fn tunnel_cycle_count(&self) -> usize {
        self.pairing.tunnel_cycles
    }
    pub(super) fn covered_cells(&self) -> (u64, u64) {
        match &self.boundary_scope {
            Some(SourceBoundaryScope::ClosedCapture(proof)) => (
                self.covered_face_cells + proof.excluded_face_cells,
                self.covered_coedge_cells + proof.excluded_coedge_cells,
            ),
            _ => (self.covered_face_cells, self.covered_coedge_cells),
        }
    }
    /// A whole-original-source property, never inferred from a local band,
    /// sample loop count, annular fiber proof, or regular continuation alone.
    /// Convexity is preserved by the accepted actual affine pose and by every
    /// linear height section; the enclosing SlabCapture binds those requests.
    pub(super) fn has_native_convex_section_topology_proof(&self) -> bool {
        self.pairing.tunnel_cycles == 0
            && matches!(
                self.boundary_scope.as_ref(),
                Some(SourceBoundaryScope::WholeSource)
            )
    }
    /// Euclidean transverse source-plane speed per unit captured height.
    /// World XY enclosures must additionally project the actual height flow
    /// and native polygon/plane errors; critical 3D prism error is not XY motion.
    pub(super) fn regular_flow_speed_mm_per_height(&self) -> Option<f64> {
        self.regular_flow_speed
    }
    pub(super) fn source_realization_error_mm(&self) -> f64 {
        self.source_realization_error_mm
    }
    pub(super) fn retained_bytes(&self) -> usize {
        // Every neighborhood is constructed with this certificate's capture;
        // cloning an Arc adds no second capture allocation.
        self.event_neighborhoods
            .capacity()
            .saturating_mul(std::mem::size_of::<CriticalEventNeighborhood>())
            .saturating_add(
                self.occupied
                    .as_ref()
                    .map_or(0, NativeSection::retained_bytes),
            )
            .saturating_add(2 * std::mem::size_of::<usize>())
            .saturating_add(std::mem::size_of::<CriticalCapture>())
            .saturating_add(self.capture.definition_id.allocated_bytes())
    }
    /// The original trimmed faces, physical edges and endpoint incidences have
    /// a proved open-slab submersion. Its closure carries the complete native
    /// sample's component/void cycles; outgoing original faces remain separate.
    pub(super) fn from_proved_regular_core(
        sample: ProvedSlabSample<'_>,
        proof: &super::slab::RegularContinuation<'_>,
        strata: &[super::NativeEndpointStratum],
        endpoints: &[super::slab::EndpointFaceProof],
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        let ProvedSlabSample {
            definition,
            request,
            section,
            boundary_motion_bound_mm,
        } = sample;
        work.charge(1, cancel)?;
        if proof.definition_id() != definition.id()
            || proof.material_realization_identity()
                != crate::kernel::MaterialRealizationIdentity::new(definition)
            || section.definition_id() != definition.id()
            || !section.is_native_intersection()
            || proof.frame() != request.frame
            || proof.range() != request.height_interval_mm
            || !boundary_motion_bound_mm.is_finite()
            || boundary_motion_bound_mm < 0.0
            || boundary_motion_bound_mm > request.native_shape_error_mm
        {
            return Err(failure(
                GeometryErrorCode::InvalidGeometry,
                "regular core proof capture disagrees with source/sample/policy",
                0,
                request.height_interval_mm,
            ));
        }
        if endpoints.len() > MAX_NEIGHBORHOODS {
            return Err(failure(
                GeometryErrorCode::ResourceLimit,
                "core endpoint event capacity exhausted",
                0,
                endpoints.len(),
            ));
        }
        work.charge(2 * section.loops.len() as u64, cancel)?;
        let pairing = ComponentVoidPairing {
            material_components: section
                .loops
                .iter()
                .filter(|boundary| !boundary.is_hole)
                .count(),
            bounded_voids: 0,
            tunnel_cycles: section
                .loops
                .iter()
                .filter(|boundary| boundary.is_hole)
                .count(),
        };
        check_critical_storage(definition.id(), endpoints.len(), remaining_bytes)?;
        let mut events = Vec::new();
        events.try_reserve_exact(endpoints.len()).map_err(|_| {
            failure(
                GeometryErrorCode::ResourceLimit,
                "core endpoint event allocation refused",
                0,
                endpoints.len(),
            )
        })?;
        check_critical_storage(definition.id(), events.capacity(), remaining_bytes)?;
        let capture = CriticalCapture::new(
            definition,
            request,
            HeightFunctional::from_frame(request.frame),
        );
        for stratum in strata {
            work.charge(1, cancel)?;
            if stratum.definition_id != *definition.id()
                || stratum.request.frame != request.frame
                || stratum.request.height_interval_mm != request.height_interval_mm
            {
                return Err(failure(
                    GeometryErrorCode::InvalidGeometry,
                    "core endpoint stratum capture mismatch",
                    stratum.face_index,
                    stratum.height_mm,
                ));
            }
        }
        for endpoint in endpoints {
            work.charge(1, cancel)?;
            events.push(CriticalEventNeighborhood {
                capture: Arc::clone(&capture),
                height_bracket_mm: [request.height_interval_mm[endpoint.endpoint()]; 2],
                source_face: Some(endpoint.source_face()),
                stationary_root_proved: false,
                incoming_regular_endpoint: !endpoint.is_outgoing(),
                pairing,
            });
        }
        let (covered_face_cells, covered_coedge_cells) = proof.covered_cells();
        Ok(Self {
            boundary_motion_bound_mm,
            occupied: None,
            event_neighborhoods: events,
            capture,
            pairing,
            covered_face_cells,
            covered_coedge_cells,
            regular_flow_speed: Some(proof.speed()),
            source_realization_error_mm: 0.0,
            boundary_scope: None,
        })
    }
    /// The caller supplies the opaque proof constructed by the native closed
    /// round-carrier routine, not a family flag or an observed loop count.
    pub(super) fn from_proved_round(
        sample: ProvedSlabSample<'_>,
        proof: &super::slab::RoundIncidenceProof<'_>,
        event_brackets: &[[f64; 2]],
        occupied: Option<NativeSection>,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        let ProvedSlabSample {
            definition,
            request,
            section,
            boundary_motion_bound_mm,
        } = sample;
        work.charge(1, cancel)?;
        if section.definition_id() != definition.id()
            || proof.definition_id() != definition.id()
            || proof.material_realization_identity()
                != crate::kernel::MaterialRealizationIdentity::new(definition)
            || !section.is_native_intersection()
            || !boundary_motion_bound_mm.is_finite()
            || boundary_motion_bound_mm < 0.0
            || boundary_motion_bound_mm > request.native_shape_error_mm
        {
            return Err(failure(
                GeometryErrorCode::InvalidGeometry,
                "proved round incidence disagrees with its captured native sample/policy",
                0,
                request.height_interval_mm,
            ));
        }
        if event_brackets.len() > MAX_NEIGHBORHOODS {
            return Err(failure(
                GeometryErrorCode::ResourceLimit,
                "round critical neighborhood capacity exhausted",
                0,
                event_brackets.len(),
            ));
        }
        let pairing = ComponentVoidPairing {
            material_components: proof.material_component_count(),
            bounded_voids: proof.bounded_void_count(),
            tunnel_cycles: proof.tunnel_cycle_count(),
        };
        check_critical_storage(definition.id(), event_brackets.len(), remaining_bytes)?;
        let mut events = Vec::new();
        events
            .try_reserve_exact(event_brackets.len())
            .map_err(|_| {
                failure(
                    GeometryErrorCode::ResourceLimit,
                    "round critical event allocation refused",
                    0,
                    event_brackets.len(),
                )
            })?;
        check_critical_storage(definition.id(), events.capacity(), remaining_bytes)?;
        let capture = CriticalCapture::new(
            definition,
            request,
            HeightFunctional::from_frame(request.frame),
        );
        for bracket in event_brackets {
            work.charge(1, cancel)?;
            if !bracket.iter().all(|v| v.is_finite()) || bracket[0] > bracket[1] {
                return Err(failure(
                    GeometryErrorCode::ConditioningFailure,
                    "proved round event bracket is invalid",
                    0,
                    bracket,
                ));
            }
            events.push(CriticalEventNeighborhood {
                capture: Arc::clone(&capture),
                height_bracket_mm: *bracket,
                source_face: None,
                stationary_root_proved: true,
                incoming_regular_endpoint: false,
                pairing,
            });
        }
        Ok(Self {
            boundary_motion_bound_mm,
            occupied,
            event_neighborhoods: events,
            capture,
            pairing,
            covered_face_cells: proof.source_face_count() as u64,
            covered_coedge_cells: 0,
            regular_flow_speed: None,
            source_realization_error_mm: 0.0,
            boundary_scope: proof
                .has_native_convex_source_proof()
                .then_some(SourceBoundaryScope::WholeSource),
        })
    }
}

fn failure(
    code: GeometryErrorCode,
    obligation: &str,
    face: usize,
    cell: impl std::fmt::Debug,
) -> GeometryError {
    GeometryError::new(
        code,
        format!("critical slab {obligation}; source face {face}, cell {cell:?}"),
    )
}
fn add(a: Interval, b: Interval) -> Interval {
    if a.lo == 0.0 && a.hi == 0.0 {
        b
    } else if b.lo == 0.0 && b.hi == 0.0 {
        a
    } else {
        a.add(b)
    }
}
fn mul(a: Interval, b: Interval) -> Interval {
    if (a.lo == 0.0 && a.hi == 0.0) || (b.lo == 0.0 && b.hi == 0.0) {
        Interval::point(0.0)
    } else {
        a.mul(b)
    }
}
fn square(value: Interval) -> Interval {
    if value.lo == 0.0 && value.hi == 0.0 {
        Interval::point(0.0)
    } else {
        value.square()
    }
}
fn dot(a: [Interval; 3], b: [Interval; 3]) -> Interval {
    (0..3).fold(Interval::point(0.0), |sum, axis| {
        add(sum, mul(a[axis], b[axis]))
    })
}
fn cross(a: [Interval; 3], b: [Interval; 3]) -> [Interval; 3] {
    [
        add(mul(a[1], b[2]), mul(a[2], b[1]).neg()),
        add(mul(a[2], b[0]), mul(a[0], b[2]).neg()),
        add(mul(a[0], b[1]), mul(a[1], b[0]).neg()),
    ]
}
fn length(a: [Interval; 3]) -> Result<Interval, GeometryError> {
    let squared = a
        .into_iter()
        .fold(Interval::point(0.0), |sum, x| add(sum, square(x)));
    Interval {
        lo: squared.lo.max(0.0),
        hi: squared.hi.max(0.0),
    }
    .sqrt()
}
fn normal(jet: &SurfaceEnclosure, orientation: bool) -> [Interval; 3] {
    let n = cross(jet.du, jet.dv);
    if orientation { n } else { n.map(Interval::neg) }
}
fn overlaps(height: Interval, range: [f64; 2]) -> bool {
    height.lo <= range[1] && height.hi >= range[0]
}
fn intersection(a: Interval, b: Interval) -> Option<Interval> {
    let lo = a.lo.max(b.lo);
    let hi = a.hi.min(b.hi);
    (lo <= hi).then_some(Interval { lo, hi })
}
fn strictly_contains(a: Interval, b: Interval) -> bool {
    a.lo < b.lo && b.hi < a.hi
}
fn split(uv: [[f64; 2]; 2], depth: u8, face: usize) -> Result<[[[f64; 2]; 2]; 2], GeometryError> {
    split_axis(uv, depth, face, usize::from(depth & 1 != 0))
}

fn split_metric(
    uv: [[f64; 2]; 2],
    depth: u8,
    face: usize,
    jet: &SurfaceEnclosure,
) -> Result<[[[f64; 2]; 2]; 2], GeometryError> {
    split_metric_derivatives(uv, depth, face, &jet.du, &jet.dv)
}

fn split_metric_derivatives(
    uv: [[f64; 2]; 2],
    depth: u8,
    face: usize,
    du: &[Interval],
    dv: &[Interval],
) -> Result<[[[f64; 2]; 2]; 2], GeometryError> {
    // Parameter widths have unrelated units. Refine the larger physical
    // variation, not a contracted straight direction merely to alternate axes.
    // This changes scheduling only; every child retains the full proof tests.
    let variation = |derivative: &[Interval], range: [f64; 2]| {
        derivative
            .iter()
            .map(|value| value.abs_upper())
            .fold(0.0_f64, f64::max)
            * (range[1] - range[0])
    };
    let u = variation(du, uv[0]);
    let v = variation(dv, uv[1]);
    let axis = if u.is_finite() && v.is_finite() && u != v {
        usize::from(v > u)
    } else {
        usize::from(depth & 1 != 0)
    };
    split_axis(uv, depth, face, axis)
}

fn split_axis(
    uv: [[f64; 2]; 2],
    depth: u8,
    face: usize,
    mut axis: usize,
) -> Result<[[[f64; 2]; 2]; 2], GeometryError> {
    let mut mid = uv[axis][0] * 0.5 + uv[axis][1] * 0.5;
    if mid <= uv[axis][0] || mid >= uv[axis][1] {
        axis ^= 1;
        mid = uv[axis][0] * 0.5 + uv[axis][1] * 0.5;
    }
    if depth >= MAX_DEPTH || mid <= uv[axis][0] || mid >= uv[axis][1] {
        return Err(failure(
            GeometryErrorCode::AnalysisIndeterminate,
            "native patch subdivision cannot resolve its incidence/metric obligation",
            face,
            uv,
        ));
    }
    let mut children = [uv; 2];
    children[0][axis][1] = mid;
    children[1][axis][0] = mid;
    Ok(children)
}

/// The signed second fundamental form is negative semidefinite for an outward
/// convex boundary. Structural zero derivatives remain exact zero: flat faces
/// and rank-one extrusion/revolution patches need no invented curvature epsilon.
fn locally_convex(jet: &SurfaceEnclosure, n: [Interval; 3]) -> bool {
    let a = dot(n, jet.duu).neg();
    let b = dot(n, jet.duv).neg();
    let c = dot(n, jet.dvv).neg();
    let determinant = add(mul(a, c), square(b).neg());
    a.lo >= 0.0 && c.lo >= 0.0 && determinant.lo >= 0.0
}

use crate::exact_coefficient::ExactCoefficient;
/// Exact equality of a linear functional on two homogeneous coefficients.
/// The common value need not be representable in binary64.
pub(super) fn exact_homogeneous_height_identity(
    first: [f64; 4],
    other: [f64; 4],
    direction: [f64; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(1, cancel)?;
    if first
        .into_iter()
        .chain(other)
        .chain(direction)
        .any(|value| !value.is_finite())
    {
        return Ok(false);
    }
    let mut sum = ExactCoefficient::point(0.0);
    for axis in 0..3 {
        if direction[axis] == 0.0 {
            continue;
        }
        let Some(mut difference) = ExactCoefficient::point(other[axis]).product(
            &ExactCoefficient::point(first[3]),
            work,
            cancel,
        )?
        else {
            return Ok(false);
        };
        let Some(reference) = ExactCoefficient::point(first[axis]).product(
            &ExactCoefficient::point(other[3]),
            work,
            cancel,
        )?
        else {
            return Ok(false);
        };
        if difference.subtract(&reference).is_none() {
            return Ok(false);
        }
        let Some(term) =
            difference.product(&ExactCoefficient::point(direction[axis]), work, cancel)?
        else {
            return Ok(false);
        };
        // Negating the accumulated identity does not change zero.
        if sum.subtract(&term).is_none() {
            return Ok(false);
        }
    }
    Ok(sum.sign() == 0)
}
pub(super) fn exact_unit_vector_identity(
    vector: [f64; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let mut residual = ExactCoefficient::point(1.0);
    for coordinate in vector {
        work.charge(1, cancel)?;
        if !coordinate.is_finite() {
            return Ok(false);
        }
        let Some(square) = ExactCoefficient::point(coordinate).product(
            &ExactCoefficient::point(coordinate),
            work,
            cancel,
        )?
        else {
            return Ok(false);
        };
        if residual.subtract(&square).is_none() {
            return Ok(false);
        }
    }
    Ok(residual.sign() == 0)
}


/// A coefficient identity on the selected immutable UV operand. Restrictions
/// and reversals preserve its image; no fitted endpoint line or rounded
/// homogenization/knot insertion is used as identity evidence.
fn chart_linear_identity(
    curve: &(impl CurveEvaluator + ?Sized),
    coefficients: &[ExactCoefficient; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(1, cancel)?;
    let point_identity = |point: [f64; 3],
                          work: &mut GeometryWorkBudget|
     -> Result<bool, GeometryError> {
        if point.into_iter().any(|value| !value.is_finite()) {
            return Ok(false);
        }
        let mut residual = ExactCoefficient::point(0.0);
        for axis in 0..3 {
            let Some(product) =
                coefficients[axis].product(&ExactCoefficient::point(point[axis]), work, cancel)?
            else {
                return Ok(false);
            };
            for &term in &product.terms[..product.len] {
                if residual.push(term).is_none() {
                    return Ok(false);
                }
            }
        }
        Ok(residual.sign() == 0)
    };
    if let Some(mut original) = curve.original_curve() {
        for _ in 0..32 {
            work.charge(1, cancel)?;
            match original {
                SourceCurve::Trimmed { basis, .. } => original = basis,
                // This provider evaluates the literal spatial operand;
                // preferred-master equivalence is separate source admission.
                SourceCurve::SurfaceCurve { curve_3d, .. } => original = curve_3d,
                SourceCurve::Line { origin, direction, magnitude } => {
                    return Ok(magnitude.is_finite()
                        && point_identity([origin[0], origin[1], 1.0], work)?
                        && (*magnitude == 0.0
                            || point_identity([direction[0], direction[1], 0.0], work)?));
                }
                SourceCurve::BSpline(spline) => {
                    for (index, point) in spline.points.iter().enumerate() {
                        work.charge(1, cancel)?;
                        let weight = spline.weights.as_ref().map_or(1.0, |weights| weights[index]);
                        if !weight.is_finite() { return Ok(false); }
                        // Original controls are Euclidean. A nonzero weight
                        // multiplies this exact entire affine identity, so no
                        // rounded xyz*weight need be constructed.
                        if weight != 0.0 && !point_identity([point[0], point[1], 1.0], work)? {
                            return Ok(false);
                        }
                    }
                    return Ok(!spline.points.is_empty());
                }
                SourceCurve::Polyline(points) => {
                    for point in points {
                        work.charge(1, cancel)?;
                        if !point_identity([point[0], point[1], 1.0], work)? {
                            return Ok(false);
                        }
                    }
                    return Ok(points.len() >= 2);
                }
                _ => return Ok(false),
            }
        }
        return Ok(false);
    }
    let Some(native) = curve.native_curve() else { return Ok(false); };
    if let CurveKind::Line(line) = native.kind() {
        let canonical = line.canonical_unit();
        // The affine identity holds throughout the line iff it annihilates
        // both the stored origin and stored direction. Scale, domain mapping
        // and reversal do not change this exact image identity.
        return Ok(point_identity([canonical.point.x, canonical.point.y, 1.0], work)?
            && point_identity([canonical.vector.x, canonical.vector.y, 0.0], work)?);
    }
    let Some(nurbs) = crate::parametric::native_curve_nurbs(curve)? else {
        return Ok(false);
    };
    for point in nurbs.cpts() {
        work.charge(1, cancel)?;
        if !point_identity(
            [point.x, point.y, if nurbs.is_rational() { point.w } else { 1.0 }],
            work,
        )? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn spline_seam_join(
    surface: &(impl SurfaceEvaluator + ?Sized),
    domain: &PreparedFaceTrimDomain<'_>,
    first: usize,
    second: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    // A nominal constant coordinate is source authority only without a source
    // chart uncertainty tube. Otherwise the full normal-union proof is required.
    if domain.source_chart_error_mm() != 0.0 {
        return Ok(false);
    }
    let constant = |coedge: usize,
                    axis: usize,
                    value: f64,
                    work: &mut GeometryWorkBudget|
     -> Result<bool, GeometryError> {
        let coefficients = std::array::from_fn(|coordinate| {
            ExactCoefficient::point(if coordinate == 2 {
                -value
            } else if coordinate == axis {
                1.0
            } else {
                0.0
            })
        });
        chart_linear_identity(&domain.coedge_operand(coedge)?, &coefficients, work, cancel)
    };
    let Some(axes) = tensor_g1::direct_tensor_axes(surface, domain.uv_domain(), work, cancel)? else {
        return Ok(false);
    };
    for (axis, (knots, degree, count)) in axes.into_iter().enumerate() {
        let opposite = (constant(first, axis, knots[degree], work)?
            && constant(second, axis, knots[count], work)?)
            || (constant(second, axis, knots[degree], work)?
                && constant(first, axis, knots[count], work)?);
        if opposite && periodic_regular_join(surface, axis, domain.uv_domain(), work, cancel)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// A coefficient-structural smoothness fact, not an operation permission.
/// Arbitrary spline/sweep profiles are intentionally absent: smooth patch
/// interiors do not establish the link of an identified endpoint.
fn intrinsically_smooth_profile(curve: &Curve) -> bool {
    match curve.kind() {
        CurveKind::Line(_) | CurveKind::Ellipse(_)
        | CurveKind::Hyperbola(_) | CurveKind::Parabola(_) => true,
        CurveKind::CrvOnSurf(curve) =>
            matches!(curve.uv_curve().kind(), CurveKind::Ellipse(_)
                | CurveKind::Hyperbola(_) | CurveKind::Parabola(_))
                && matches!(curve.surface().kind(), SurfaceKind::Plane(_)),
        _ => false,
    }
}

fn intrinsically_smooth_carrier(surface: &(impl SurfaceEvaluator + ?Sized)) -> bool {
    if let Some(mut original) = surface.original_surface() {
        for _ in 0..32 {
            match original {
                SourceSurface::RectangularTrimmed { basis, .. } => original = basis,
                SourceSurface::Plane { .. } | SourceSurface::Cylinder { .. }
                | SourceSurface::Cone { .. } | SourceSurface::Sphere { .. }
                | SourceSurface::Torus { .. } | SourceSurface::DegenerateTorus { .. } => return true,
                SourceSurface::Extrusion { profile, .. }
                | SourceSurface::Revolution { profile, .. } => {
                    let mut profile = profile.as_ref();
                    for _ in 0..32 {
                        match profile {
                            SourceCurve::Trimmed { basis, .. } => profile = basis,
                            SourceCurve::SurfaceCurve { curve_3d, .. } => profile = curve_3d,
                            SourceCurve::Line { .. } | SourceCurve::Circle { .. }
                            | SourceCurve::Ellipse { .. } | SourceCurve::Parabola { .. } => return true,
                            _ => return false,
                        }
                    }
                    return false;
                }
                _ => return false,
            }
        }
        return false;
    }
    let Some(mut surface) = surface.native_surface() else { return false; };
    for _ in 0..32 {
        match surface.kind() {
            SurfaceKind::Plane(_) | SurfaceKind::Sphere(_) | SurfaceKind::Torus(_)
            | SurfaceKind::Cone(_) => return true,
            SurfaceKind::SurfOfRevolution(surface) =>
                return surface.gen_curve().is_some_and(intrinsically_smooth_profile),
            SurfaceKind::SurfOfExtrusion(surface) =>
                return surface.gen_curve().is_some_and(intrinsically_smooth_profile),
            SurfaceKind::Step(wrapper) => {
                let Some(basis) = wrapper.surface() else { return false; };
                surface = basis;
            }
            SurfaceKind::CurveBounded(wrapper) => surface = wrapper.basis(),
            _ => return false,
        }
    }
    false
}

/// One original paired native occurrence link, shared by critical topology and
/// height regularity. This is geometric G1 continuation, not height transversality.
pub(super) fn same_face_source_seam_join(
    surface: &(impl SurfaceEvaluator + ?Sized),
    domain: &PreparedFaceTrimDomain<'_>,
    first: usize,
    second: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(1, cancel)?;
    let coedges = domain.coedges();
    let (Some(a), Some(b)) = (coedges.get(first), coedges.get(second)) else {
        return Ok(false);
    };
    let selected = domain.surface_operand();
    let same_carrier = match (surface.native_surface(), selected.native_surface()) {
        (Some(first), Some(second)) => std::ptr::eq(first, second),
        (None, None) => match (surface.original_surface(), selected.original_surface()) {
            (Some(first), Some(second)) => std::ptr::eq(first, second),
            _ => false,
        },
        _ => false,
    };
    if !same_carrier || first == second || a.edge != b.edge || a.forward == b.forward
    {
        return Ok(false);
    }
    if tensor_g1::direct_tensor_axes(surface, domain.uv_domain(), work, cancel)?.is_some() {
        spline_seam_join(surface, domain, first, second, work, cancel)
    } else {
        Ok(intrinsically_smooth_carrier(surface))
    }
}

/// The selected carrier is borrowed from the same immutable prepared source as
/// the retained domain. A copied or differently gauged surface cannot attach here.
pub(super) struct OriginalSourceFace<'a, 'd> {
    surface: PreparedTrimSurface<'a, 'd>,
    domain: &'a PreparedFaceTrimDomain<'d>,
    bounds: [[f64; 2]; 2],
}
pub(super) fn with_original_source_face<R>(
    domain: &PreparedFaceTrimDomain<'_>,
    bounds: [[f64; 2]; 2],
    visit: impl FnOnce(OriginalSourceFace<'_, '_>) -> Result<R, GeometryError>,
) -> Result<R, GeometryError> {
    visit(OriginalSourceFace {
        surface: domain.surface_operand(),
        domain,
        bounds,
    })
}
impl<'a, 'd> OriginalSourceFace<'a, 'd> {
    pub fn surface(&self) -> &PreparedTrimSurface<'a, 'd> {
        &self.surface
    }
    pub fn prove_gluing(
        &self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<GluedSourceFace<'a, 'd>>, GeometryError> {
        GluedSourceFace::prove(self.surface, self.domain, self.bounds, work, cancel)
    }
}

/// A private whole-original-face G1 receipt bound to the exact borrowed carrier,
/// retained native face/domain and admitted finite UV box. It never asserts C1
/// parameter jets at a C0 knot or transfers to a different surface.
pub(super) struct GluedSourceFace<'a, 'd> {
    surface: PreparedTrimSurface<'a, 'd>,
    domain: &'a PreparedFaceTrimDomain<'d>,
    bounds: [[f64; 2]; 2],
}
impl<'a, 'd> GluedSourceFace<'a, 'd> {
    fn prove(
        surface: PreparedTrimSurface<'a, 'd>,
        domain: &'a PreparedFaceTrimDomain<'d>,
        bounds: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        if !critical_face_gluing(&surface, bounds, work, cancel)? {
            return Ok(None);
        }
        Ok(Some(Self {
            surface,
            domain,
            bounds,
        }))
    }
    pub fn surface(&self) -> &PreparedTrimSurface<'a, 'd> {
        &self.surface
    }
    pub fn domain(&self) -> &PreparedFaceTrimDomain<'d> {
        self.domain
    }
    pub fn normal(
        &self,
        uv: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[Interval; 3], GeometryError> {
        for (axis, range) in uv.iter().enumerate() {
            Interval::new(range[0], range[1])?;
            if range[0] < self.bounds[axis][0] || range[1] > self.bounds[axis][1] {
                return Err(GeometryError::new(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "source normal query leaves its whole-face G1 receipt",
                ));
            }
        }
        match crate::parametric::surface_geometric_normal_enclosure(
            &self.surface, uv, work, cancel,
        ) {
            Ok(normal) => Ok(normal),
            Err(error)
                if error.code == GeometryErrorCode::AnalysisIndeterminate
                    && one_sided_tensor_joint(&self.surface, uv, work, cancel)? =>
            {
                // The positive original coefficient G1 theorem identifies all
                // incident tangent planes. One-sided nonzero native jets supply
                // their common NORMAL DIRECTION, never a fictitious C1 jet.
                Ok(normal(
                    &crate::parametric::surface_piecewise_enclosure(&self.surface, uv, work, cancel)?,
                    true,
                ))
            }
            Err(error) => Err(error),
        }
    }
}

fn one_sided_tensor_joint(
    surface: &(impl SurfaceEvaluator + ?Sized),
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let Some(axes) = tensor_g1::direct_tensor_axes(surface, uv, work, cancel)? else {
        return Ok(false);
    };
    let mut joint = false;
    for (axis, (knots, degree, count)) in axes.into_iter().enumerate() {
        let levels = usize::BITS - knots.len().leading_zeros();
        work.charge(6 * u64::from(levels) + 1, cancel)?;
        let (lo, hi) = (uv[axis][0], uv[axis][1]);
        let after = knots.partition_point(|knot| *knot <= lo);
        if knots.get(after).is_some_and(|knot| *knot < hi) {
            return Ok(false); // Never choose one side of a box crossing interiors.
        }
        for endpoint in [lo, hi] {
            if knots[degree] < endpoint && endpoint < knots[count] {
                let first = knots.partition_point(|knot| *knot < endpoint);
                let last = knots.partition_point(|knot| *knot <= endpoint);
                joint |= last.saturating_sub(first) >= degree;
            }
        }
    }
    Ok(joint)
}

/// Global local-convexity needs geometric C1 gluing on the WHOLE source face,
/// not just on the eventual adaptive cells. A direct tensor chart additionally
/// admits exact homogeneous projective G1 identities at ALL internal knots.
/// That coefficient proof does not cover an arbitrary nested profile carrier:
/// the recursive whole-domain qualifier must establish its premise instead.
pub(super) fn critical_face_gluing(
    surface: &(impl SurfaceEvaluator + ?Sized),
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    if surface_parameter_span_smooth(surface, uv, work, cancel)? {
        return Ok(true);
    }
    if tensor_g1::direct_tensor_axes(surface, uv, work, cancel)?.is_none() {
        return Ok(false);
    }
    let mut joined = false;
    for interface in surface_crease_interfaces(surface, uv, work, cancel)? {
        if !interface_regular_join(surface, interface, work, cancel)? {
            return Ok(false);
        }
        joined = true;
    }
    Ok(joined)
}

/// A root tensor interface is needed only where its original image can meet
/// the CLOSED capture. Every discarded interface has a whole-image height
/// exclusion; unresolved/nested interfaces on active original spans still
/// require the recursive span qualifier below, before adaptive subdivision.
fn capture_face_gluing(
    surface: &(impl SurfaceEvaluator + ?Sized),
    uv: [[f64; 2]; 2],
    functional: HeightFunctional,
    request: &SlabRequest,
    face: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    if critical_face_gluing(surface, uv, work, cancel)? {
        return Ok(true);
    }
    for interface in surface_crease_interfaces(surface, uv, work, cancel)? {
        let image =
            crate::parametric::surface_piecewise_enclosure(surface, interface, work, cancel)?;
        if overlaps(
            functional.height(image.position),
            request.height_interval_mm,
        ) && !interface_regular_join(surface, interface, work, cancel)?
        {
            return Err(failure(
                GeometryErrorCode::AnalysisIndeterminate,
                "closed capture has an unproved original C1/projective-G1 source interface",
                face,
                interface,
            ));
        }
    }
    Ok(false)
}

#[derive(Clone, Copy, Debug)]
enum RootIsolation {
    Excluded,
    Unique([[f64; 2]; 2]),
    ConvexCriticalCover,
}

/// Finite Krawczyk inclusion for (d.Su,d.Sv). The midpoint inverse is merely
/// a preconditioner; every residual and Jacobian product remains an interval,
/// including an enclosed placed direction. No approximate root grants existence.
fn stationary_root(
    surface: &(impl SurfaceEvaluator + ?Sized),
    uv: [[f64; 2]; 2],
    jet: &SurfaceEnclosure,
    height: HeightFunctional,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<RootIsolation, GeometryError> {
    let gradient = [height.dot(jet.du), height.dot(jet.dv)];
    if gradient.iter().any(|g| !g.contains_zero()) {
        return Ok(RootIsolation::Excluded);
    }
    // A union of piecewise Hessians is not the Jacobian of a discontinuous
    // gradient. Preserve the complete range as an unproved critical cover;
    // only gradient exclusion above is valid without a C1 source premise.
    if !surface_parameter_span_smooth(surface, uv, work, cancel)? {
        return Ok(RootIsolation::ConvexCriticalCover);
    }
    let hessian = [
        [height.dot(jet.duu), height.dot(jet.duv)],
        [height.dot(jet.duv), height.dot(jet.dvv)],
    ];
    let determinant =
        hessian[0][0].mid() * hessian[1][1].mid() - hessian[0][1].mid() * hessian[1][0].mid();
    if determinant == 0.0 || !determinant.is_finite() {
        return Ok(RootIsolation::ConvexCriticalCover);
    }
    let inverse = [
        [
            hessian[1][1].mid() / determinant,
            -hessian[0][1].mid() / determinant,
        ],
        [
            -hessian[1][0].mid() / determinant,
            hessian[0][0].mid() / determinant,
        ],
    ];
    if !inverse.iter().flatten().all(|v| v.is_finite()) {
        return Ok(RootIsolation::ConvexCriticalCover);
    }
    let inverse_determinant = add(
        mul(
            Interval::point(inverse[0][0]),
            Interval::point(inverse[1][1]),
        ),
        mul(
            Interval::point(inverse[0][1]),
            Interval::point(inverse[1][0]),
        )
        .neg(),
    );
    if inverse_determinant.contains_zero() {
        return Ok(RootIsolation::ConvexCriticalCover);
    }
    let center = uv.map(|v| v[0] * 0.5 + v[1] * 0.5);
    let at_center = surface_enclosure(surface, center.map(|v| [v, v]), work, cancel)?;
    let value = [height.dot(at_center.du), height.dot(at_center.dv)];
    work.charge(1, cancel)?;
    let krawczyk: [Interval; 2] = std::array::from_fn(|row| {
        let residual = (0..2).fold(Interval::point(center[row]), |s, col| {
            add(s, mul(Interval::point(-inverse[row][col]), value[col]))
        });
        (0..2).fold(residual, |s, col| {
            let coefficient = (0..2).fold(
                Interval::point(if row == col { 1.0 } else { 0.0 }),
                |v, k| add(v, mul(Interval::point(-inverse[row][k]), hessian[k][col])),
            );
            add(
                s,
                mul(
                    coefficient,
                    Interval {
                        lo: uv[col][0],
                        hi: uv[col][1],
                    }
                    .sub(Interval::point(center[col])),
                ),
            )
        })
    });
    let boxes = uv.map(|v| Interval { lo: v[0], hi: v[1] });
    if (0..2).any(|axis| intersection(boxes[axis], krawczyk[axis]).is_none()) {
        return Ok(RootIsolation::Excluded);
    }
    if (0..2).all(|axis| strictly_contains(boxes[axis], krawczyk[axis])) {
        Ok(RootIsolation::Unique(krawczyk.map(|v| [v.lo, v.hi])))
    } else {
        Ok(RootIsolation::ConvexCriticalCover)
    }
}

/// Necessary stationary-root cover, never an existence or trim certificate.
/// Scalar interval Newton may use either gradient equation to contract a
/// coordinate. Mixed Hessians retain saddle/pole roots when both diagonals
/// contain zero; singular full Hessians still retain whole critical lines.
/// Every midpoint strip stays inside the qualified original source cell.
pub(super) fn necessary_stationary_domain(
    surface: &(impl SurfaceEvaluator + ?Sized),
    mut uv: [[f64; 2]; 2],
    jet: &SurfaceEnclosure,
    height: HeightFunctional,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[[f64; 2]; 2]>, GeometryError> {
    work.charge(1, cancel)?;
    if !surface_parameter_span_smooth(surface, uv, work, cancel)? {
        return Ok(Some(uv));
    }
    // These bounds cover the ORIGINAL cell throughout every contraction.
    // A continuous source gradient, not a union of unrelated C0 Hessians,
    // supplies the one-coordinate mean-value theorem for either equation.
    let mixed = height.dot(jet.duv);
    let hessian = [[height.dot(jet.duu), mixed], [mixed, height.dot(jet.dvv)]];
    for _ in 0..MAX_DEPTH {
        let previous = uv;
        for axis in 0..2 {
            if uv[axis][0] == uv[axis][1] {
                continue;
            }
            let equation = if !hessian[axis][axis].contains_zero() {
                axis
            } else if !hessian[1 - axis][axis].contains_zero() {
                1 - axis
            } else {
                continue;
            };
            work.charge(1, cancel)?;
            let midpoint = uv[axis][0] * 0.5 + uv[axis][1] * 0.5;
            let mut strip = uv;
            strip[axis] = [midpoint, midpoint];
            let at_midpoint = surface_enclosure(surface, strip, work, cancel)?;
            let gradient = height.dot(if equation == 0 {
                at_midpoint.du
            } else {
                at_midpoint.dv
            });
            let newton = Interval::point(midpoint).sub(gradient.div(hessian[equation][axis])?);
            let Some(contracted) = intersection(
                Interval {
                    lo: uv[axis][0],
                    hi: uv[axis][1],
                },
                newton,
            ) else {
                return Ok(None);
            };
            uv[axis] = [contracted.lo, contracted.hi];
        }
        if uv == previous {
            break;
        }
    }
    Ok(Some(uv))
}

/// Existence-only interface for height discovery. The caller must independently
/// certify that the complete input box is inside the true trimmed source face.
pub(super) fn proved_stationary_height(
    surface: &(impl SurfaceEvaluator + ?Sized),
    uv: [[f64; 2]; 2],
    jet: &SurfaceEnclosure,
    functional: HeightFunctional,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Interval>, GeometryError> {
    match stationary_root(surface, uv, jet, functional, work, cancel)? {
        RootIsolation::Unique(root) => Ok(Some(
            functional.height(surface_enclosure(surface, root, work, cancel)?.position),
        )),
        RootIsolation::Excluded | RootIsolation::ConvexCriticalCover => Ok(None),
    }
}

fn project(position: [Interval; 3], frame: PlaneFrameMm) -> [Interval; 2] {
    let relative =
        std::array::from_fn(|axis| position[axis].sub(Interval::point(frame.origin_mm[axis])));
    [
        dot(relative, frame.x_axis.map(Interval::point)),
        dot(relative, frame.y_axis.map(Interval::point)),
    ]
}

fn projected_displacement(a: [f64; 3], b: [f64; 3], frame: PlaneFrameMm) -> [Interval; 2] {
    // Cancel the shared origin before projection. Subtracting separately
    // projected boxes loses the sign of valid one-ulp boundary segments.
    let delta = std::array::from_fn(|axis| Interval::point(b[axis]).sub(Interval::point(a[axis])));
    [
        dot(delta, frame.x_axis.map(Interval::point)),
        dot(delta, frame.y_axis.map(Interval::point)),
    ]
}

fn reference_centroid(
    points: &[[f64; 3]],
    frame: PlaneFrameMm,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[f64; 3], GeometryError> {
    // A coordinate proposal, not material authority. Area weighting avoids
    // moving the radial origin when one native arc is sampled more densely.
    let origin = points[0];
    let mut previous = [0.0; 3];
    let mut previous_uv = [0.0; 2];
    let mut area = 0.0;
    let mut moment = [0.0; 3];
    for point in &points[1..] {
        work.charge(1, cancel)?;
        let current: [f64; 3] = std::array::from_fn(|axis| point[axis] - origin[axis]);
        let uv = [frame.x_axis, frame.y_axis]
            .map(|row| (0..3).map(|axis| current[axis] * row[axis]).sum::<f64>());
        let weight = previous_uv[0] * uv[1] - previous_uv[1] * uv[0];
        area += weight;
        for axis in 0..3 {
            moment[axis] += (previous[axis] + current[axis]) * weight;
        }
        previous = current;
        previous_uv = uv;
    }
    let denominator = 3.0 * area;
    let center = std::array::from_fn(|axis| origin[axis] + moment[axis] / denominator);
    if denominator == 0.0
        || !denominator.is_finite()
        || center.iter().any(|value| !value.is_finite())
    {
        return Err(failure(
            GeometryErrorCode::ConditioningFailure,
            "sampled reference centroid has no finite nonzero area",
            0,
            area,
        ));
    }
    Ok(center)
}
fn orient(a: [Interval; 2], b: [Interval; 2], p: [Interval; 2]) -> Interval {
    add(
        mul(b[0].sub(a[0]), p[1].sub(a[1])),
        mul(b[1].sub(a[1]), p[0].sub(a[0])).neg(),
    )
}

struct SamplePrism {
    vertices: Vec<[Interval; 2]>,
    ray_index: Option<ray_index::RayEdgeIndex>,
    outer_count: usize,
    center: [f64; 3],
    frame: PlaneFrameMm,
    dual: [[Interval; 3]; 3],
    height_dual: [[Interval; 3]; 3],
    height_vertical_norm: f64,
    core_radius: Option<f64>,
    convex_disk: bool,
    inner_radius_upper: f64,
    range: [f64; 2],
    source_height: HeightFunctional,
    approximation: f64,
    vertical_return: f64,
}

fn next_sample_vertex(edge: usize, outer_count: usize, count: usize) -> usize {
    if edge + 1 == outer_count {
        0
    } else if edge + 1 == count {
        outer_count
    } else {
        edge + 1
    }
}
impl SamplePrism {
    fn required_heap_bytes(count: usize, annular: bool) -> Option<usize> {
        let vertices = count.checked_mul(std::mem::size_of::<[Interval; 2]>())?;
        vertices.checked_add(if annular {
            ray_index::RayEdgeIndex::required_heap_bytes(count)?
        } else {
            0
        })
    }

    fn heap_bytes(&self) -> usize {
        self.vertices
            .capacity()
            .saturating_mul(std::mem::size_of::<[Interval; 2]>())
            .saturating_add(
                self.ray_index
                    .as_ref()
                    .map_or(0, ray_index::RayEdgeIndex::heap_bytes),
            )
    }

    fn ray_index(
        vertices: &[[Interval; 2]],
        outer_count: usize,
        heap_limit: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<ray_index::RayEdgeIndex, GeometryError> {
        ray_index::RayEdgeIndex::new(
            vertices.len(),
            heap_limit,
            |edge| {
                let a = vertices[edge];
                let b = vertices[next_sample_vertex(edge, outer_count, vertices.len())];
                std::array::from_fn(|axis| a[axis].hull(b[axis]))
            },
            work,
            cancel,
        )
    }

    /// Strict center-to-edge determinants are already proved by `new`.
    /// A single positive-ray crossing gives angular degree one and simplicity;
    /// nonnegative turns then prove convexity. Neither premise alone suffices.
    fn convex_radial_polygon(
        vertices: &[[Interval; 2]],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let count = vertices.len();
        if count < 3 {
            return Ok(false);
        }
        // The sum lies inside the first edge's angular sector. Unlike a fixed
        // axis, it does not deliberately run through an ordinary polygon vertex.
        let ray = std::array::from_fn(|axis| add(vertices[0][axis], vertices[1][axis]));
        let side = |p: [Interval; 2]| {
            let value = orient([Interval::point(0.0); 2], ray, p);
            if value.lo > 0.0 {
                Some(true)
            } else if value.hi < 0.0 {
                Some(false)
            } else {
                None
            }
        };
        let Some(mut previous_side) = side(vertices[count - 1]) else {
            return Ok(false);
        };
        let mut crossings = 0;
        for index in 0..count {
            work.charge(1, cancel)?;
            let previous = if index == 0 { count - 1 } else { index - 1 };
            let next = if index + 1 == count { 0 } else { index + 1 };
            if orient(vertices[previous], vertices[index], vertices[next]).lo < 0.0 {
                return Ok(false);
            }
            let Some(current_side) = side(vertices[index]) else {
                return Ok(false);
            };
            if !previous_side && current_side {
                crossings += 1;
                if crossings > 1 {
                    return Ok(false);
                }
            }
            previous_side = current_side;
        }
        Ok(crossings == 1)
    }

    fn new(
        section: &NativeSection,
        request: &SlabRequest,
        source_height: HeightFunctional,
        actual_height: Interval,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        let outer = section
            .loops
            .iter()
            .position(|boundary| !boundary.is_hole)
            .ok_or_else(|| {
                failure(
                    GeometryErrorCode::AnalysisIndeterminate,
                    "sample has no material component",
                    0,
                    section.loops.len(),
                )
            })?;
        let hole = section.loops.iter().position(|boundary| boundary.is_hole);
        for boundary in &section.loops {
            if boundary.points_mm.len() < 4
                || boundary.points_mm.first() != boundary.points_mm.last()
            {
                return Err(failure(
                    GeometryErrorCode::NativeIntersectionFailure,
                    "sample is not an exactly closed native polygon",
                    0,
                    boundary.points_mm.len(),
                ));
            }
        }
        let outer_count = section.loops[outer].points_mm.len() - 1;
        let count = outer_count + hole.map_or(0, |index| section.loops[index].points_mm.len() - 1);
        if Self::required_heap_bytes(count, hole.is_some())
            .is_none_or(|bytes| bytes > MAX_SAMPLE_BYTES)
        {
            return Err(failure(
                GeometryErrorCode::ResourceLimit,
                "sample projection capacity exhausted",
                0,
                count,
            ));
        }
        work.charge(count as u64, cancel)?;
        // An annular chart needs its origin inside the hole, not merely inside
        // the outer polygon. Every acceptance predicate still checks this proposal.
        let center = reference_centroid(
            &section.loops[hole.unwrap_or(outer)].points_mm,
            section.frame,
            work,
            cancel,
        )?;
        let frame = PlaneFrameMm {
            origin_mm: center,
            ..section.frame
        };
        // Inverse columns of the EXACT stored projection rows: an approximately
        // orthonormal frame must not be silently treated as an exact isometry.
        let x = frame.x_axis.map(Interval::point);
        let y = frame.y_axis.map(Interval::point);
        let z = frame.z_axis.map(Interval::point);
        let determinant = dot(x, cross(y, z));
        if determinant.lo <= 0.0 {
            return Err(failure(
                GeometryErrorCode::ConditioningFailure,
                "native radial projection frame has no positive orientation",
                0,
                determinant,
            ));
        }
        let dual = [cross(y, z), cross(z, x), cross(x, y)]
            .map(|column| column.map(|value| value.div(determinant)))
            .map(|column| -> Result<[Interval; 3], GeometryError> {
                let [a, b, c] = column;
                Ok([a?, b?, c?])
            });
        let [a, b, c] = dual;
        let dual = [a?, b?, c?];
        let h = source_height.direction;
        let height_dual = if h.iter().zip(z).all(|(a, b)| a.lo == b.lo && a.hi == b.hi) {
            dual
        } else {
            let determinant = dot(x, cross(y, h));
            if determinant.lo <= 0.0 {
                return Err(failure(
                    GeometryErrorCode::ConditioningFailure,
                    "actual slab height is not transverse to the native projection frame",
                    0,
                    determinant,
                ));
            }
            let columns = [cross(y, h), cross(h, x), cross(x, y)]
                .map(|column| column.map(|value| value.div(determinant)))
                .map(|column| -> Result<[Interval; 3], GeometryError> {
                    let [a, b, c] = column;
                    Ok([a?, b?, c?])
                });
            let [a, b, c] = columns;
            [a?, b?, c?]
        };
        let height_vertical_norm = length(height_dual[2])?.hi;
        let mut vertices = Vec::with_capacity(count);
        for index in [Some(outer), hole].into_iter().flatten() {
            let boundary = &section.loops[index];
            vertices.extend(
                boundary.points_mm[..boundary.points_mm.len() - 1]
                    .iter()
                    .map(|p| project(p.map(Interval::point), frame)),
            );
        }
        let actual = Interval::new(actual_height.lo, actual_height.hi)?;
        if actual.lo < request.height_interval_mm[0] || actual.hi > request.height_interval_mm[1] {
            return Err(failure(
                GeometryErrorCode::AnalysisIndeterminate,
                "actual native source sample plane is not enclosed by the candidate slab",
                0,
                [actual.lo, actual.hi],
            ));
        }
        let direction_length = length(source_height.direction)?;
        if direction_length.lo <= 0.0 {
            return Err(failure(
                GeometryErrorCode::ConditioningFailure,
                "height direction has zero norm",
                0,
                request.frame.z_axis,
            ));
        }
        let plane_drift = actual.sub(Interval::point(actual.mid())).abs_upper();
        let plane_return = mul(
            Interval::point(plane_drift),
            Interval::point(height_vertical_norm),
        )
        .hi;
        let approximation = up(section.achieved_chord_error_mm()
            + section.achieved_plane_error_mm()
            + section.boolean_tolerance_mm
            + plane_return);
        let mut outer_radius_lower = f64::INFINITY;
        let mut inner_radius_upper = 0.0_f64;
        for edge in 0..count {
            work.charge(1, cancel)?;
            let inner = edge >= outer_count;
            let end = if inner { count } else { outer_count };
            let begin = if inner { outer_count } else { 0 };
            let next = if edge + 1 == end { begin } else { edge + 1 };
            let a = vertices[edge];
            let boundary = &section.loops[if inner {
                hole.expect("admitted inner loop")
            } else {
                outer
            }];
            let raw_a = boundary.points_mm[edge - begin];
            let raw_b = boundary.points_mm[next - begin];
            let delta = projected_displacement(raw_a, raw_b, frame);
            let side = a[0].mul(delta[1]).sub(a[1].mul(delta[0]));
            let signed = if inner { side.neg() } else { side };
            let edge_length = length([delta[0], delta[1], Interval::point(0.0)])?;
            if signed.lo <= 0.0 || edge_length.hi <= 0.0 {
                return Err(failure(
                    GeometryErrorCode::AnalysisIndeterminate,
                    "sampled native reference loop has no separated radial degree",
                    0,
                    edge,
                ));
            }
            let separation = Interval::point(signed.lo)
                .div(Interval::point(edge_length.hi))?
                .lo;
            if separation <= 2.0 * approximation {
                return Err(failure(
                    GeometryErrorCode::AnalysisIndeterminate,
                    "native radial degree witness meets a source approximation tube",
                    0,
                    edge,
                ));
            }
            if inner {
                inner_radius_upper =
                    inner_radius_upper.max(length([a[0], a[1], Interval::point(0.0)])?.hi);
            } else {
                outer_radius_lower = outer_radius_lower.min(separation);
            }
        }
        let core_radius = if hole.is_some() {
            let low = up(inner_radius_upper + 3.0 * approximation);
            let high = down(outer_radius_lower - 3.0 * approximation);
            if low >= high {
                return Err(failure(
                    GeometryErrorCode::AnalysisIndeterminate,
                    "native annulus has no full separated represented core cycle",
                    0,
                    [low, high],
                ));
            }
            Some(low * 0.5 + high * 0.5)
        } else {
            None
        };
        let vertical = actual
            .sub(Interval::point(request.height_interval_mm[0]))
            .hull(Interval::point(request.height_interval_mm[1]).sub(actual))
            .abs_upper();
        let vertical_return = up(mul(
            Interval::point(vertical),
            Interval::point(height_vertical_norm),
        )
        .hi + approximation);
        let index = if core_radius.is_some() {
            let vertex_bytes = vertices
                .capacity()
                .checked_mul(std::mem::size_of::<[Interval; 2]>())
                .and_then(|bytes| MAX_SAMPLE_BYTES.checked_sub(bytes))
                .ok_or_else(|| {
                    failure(
                        GeometryErrorCode::ResourceLimit,
                        "sample vertices exceed ray-index admission",
                        0,
                        count,
                    )
                })?;
            Some(Self::ray_index(
                &vertices,
                outer_count,
                vertex_bytes,
                work,
                cancel,
            )?)
        } else {
            None
        };
        let convex_disk =
            core_radius.is_none() && Self::convex_radial_polygon(&vertices, work, cancel)?;
        Ok(Self {
            vertices,
            ray_index: index,
            outer_count,
            center,
            frame,
            dual,
            height_dual,
            height_vertical_norm,
            core_radius,
            convex_disk,
            inner_radius_upper,
            range: request.height_interval_mm,
            source_height,
            approximation,
            vertical_return,
        })
    }

    /// For the lower/upper actual halfspace cap the outward covector is
    /// -dh/+dh, and its radial degree is h(witness)-lower / upper-h(witness).
    /// Enclose the ENTIRE represented core circle when present: the native
    /// sample frame and source height frame need not be exactly parallel.
    fn closed_capture_boundary(
        &self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<ClosedCaptureBoundary, GeometryError> {
        work.charge(1, cancel)?;
        let mut height = self.source_height.height(self.center.map(Interval::point));
        if let Some(radius) = self.core_radius {
            let angular = Interval { lo: -1.0, hi: 1.0 };
            let transverse = add(
                mul(self.source_height.dot(self.dual[0]), angular),
                mul(self.source_height.dot(self.dual[1]), angular),
            );
            height = add(height, mul(Interval::point(radius), transverse));
        }
        let cap_radial_degree = [
            height.sub(Interval::point(self.range[0])),
            Interval::point(self.range[1]).sub(height),
        ];
        if cap_radial_degree.iter().any(|degree| degree.lo <= 0.0) {
            return Err(failure(
                GeometryErrorCode::AnalysisIndeterminate,
                "closed source halfspace caps have no strictly interior radial witness",
                0,
                (self.range, [height.lo, height.hi]),
            ));
        }
        Ok(ClosedCaptureBoundary {
            excluded_face_cells: 0,
            excluded_coedge_cells: 0,
        })
    }

    fn next_vertex(&self, edge: usize) -> usize {
        next_sample_vertex(edge, self.outer_count, self.vertices.len())
    }

    /// Locate one angular sector of the proved convex, degree-one disk.
    /// Every comparison must hold for the entire position box. Ambiguity
    /// retains the complete support scan; no nearest-edge inference is used.
    fn convex_ray_sector(
        &self,
        p: [Interval; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<usize>, GeometryError> {
        if !self.convex_disk {
            return Ok(None);
        }
        let count = self.outer_count;
        // This ray is strictly inside the first edge's angular sector.
        // Ordering v1,...,v(n-1),v0 starts immediately after that ray.
        let ray = std::array::from_fn(|axis| add(self.vertices[0][axis], self.vertices[1][axis]));
        let half = |value| {
            let side = orient([Interval::point(0.0); 2], ray, value);
            if side.lo > 0.0 {
                Some(false)
            } else if side.hi < 0.0 {
                Some(true)
            } else {
                None
            }
        };
        work.charge(1, cancel)?;
        let Some(p_half) = half(p) else {
            return Ok(None);
        };
        let vertex_at = |ordered| if ordered + 1 == count { 0 } else { ordered + 1 };
        let mut lower = 0;
        let mut upper = count;
        while lower < upper {
            let middle = lower + (upper - lower) / 2;
            let vertex = self.vertices[vertex_at(middle)];
            work.charge(1, cancel)?;
            let Some(vertex_half) = half(vertex) else {
                return Ok(None);
            };
            let before = if vertex_half != p_half {
                !vertex_half
            } else {
                work.charge(1, cancel)?;
                let side = orient([Interval::point(0.0); 2], vertex, p);
                if side.lo > 0.0 {
                    true
                } else if side.hi < 0.0 {
                    false
                } else {
                    return Ok(None);
                }
            };
            if before {
                lower = middle + 1;
            } else {
                upper = middle;
            }
        }
        let previous = if lower == 0 { count - 1 } else { lower - 1 };
        Ok(Some(vertex_at(previous)))
    }

    /// Exact stored frame rows and their interval-enclosed dual columns define
    /// the radial map. The toroidal fiber velocity is q in those coordinates,
    /// not a world-space orthonormal-frame approximation.
    fn radial_projection_frame(
        &self,
        position: [Interval; 3],
    ) -> Result<Option<([Interval; 3], Option<[Interval; 3]>)>, GeometryError> {
        let relative =
            std::array::from_fn(|axis| position[axis].sub(Interval::point(self.center[axis])));
        let Some(core_radius) = self.core_radius else {
            return Ok(Some((relative, None)));
        };
        let p = project(position, self.frame);
        let rho = length([p[0], p[1], Interval::point(0.0)])?;
        if rho.lo <= 0.0 {
            return Ok(None);
        }
        let cosine = p[0].div(rho)?;
        let sine = p[1].div(rho)?;
        let radial = std::array::from_fn(|axis| {
            add(
                mul(self.dual[0][axis], cosine),
                mul(self.dual[1][axis], sine),
            )
        });
        let vertical = dot(relative, self.frame.z_axis.map(Interval::point));
        let q = std::array::from_fn(|axis| {
            add(
                mul(radial[axis], rho.sub(Interval::point(core_radius))),
                mul(self.dual[2][axis], vertical),
            )
        });
        Ok(Some((q, Some(radial))))
    }

    /// A toroidal radial degree is not a fitted torus. Its base is the actual
    /// represented core cycle; every source meridian fiber must independently
    /// prove star degree one and convexity. This permits nonconstant radii and
    /// arbitrary rational/spline/sweep encodings of the same native complex.
    fn patch_incidence(
        &self,
        jet: &SurfaceEnclosure,
        n: [Interval; 3],
        source_convex: Option<bool>,
    ) -> Result<(Interval, bool), GeometryError> {
        let Some((q, radial)) = self.radial_projection_frame(jet.position)? else {
            return Ok((
                Interval {
                    lo: f64::NEG_INFINITY,
                    hi: f64::INFINITY,
                },
                false,
            ));
        };
        let radial_degree = dot(n, q);
        let Some(radial) = radial else {
            return Ok((
                radial_degree,
                source_convex.unwrap_or_else(|| locally_convex(jet, n)),
            ));
        };
        if let Some(convex) = source_convex {
            // A coefficient-proved convex source carrier is convex in every
            // tangent direction. No inverse of collapsed covering coordinates
            // is needed to restrict its form to this meridian plane.
            return Ok((radial_degree, convex));
        }
        let nr = dot(n, radial);
        let nz = dot(n, self.dual[2]);
        let tangent = std::array::from_fn(|axis| {
            add(mul(radial[axis], nz), mul(self.dual[2][axis], nr).neg())
        });
        let a = dot(jet.du, jet.du);
        let b = dot(jet.du, jet.dv);
        let c = dot(jet.dv, jet.dv);
        let determinant = add(mul(a, c), square(b).neg());
        if determinant.lo <= 0.0 {
            return Ok((
                Interval {
                    lo: f64::NEG_INFINITY,
                    hi: f64::INFINITY,
                },
                false,
            ));
        }
        let u = dot(jet.du, tangent);
        let v = dot(jet.dv, tangent);
        let alpha = add(mul(c, u), mul(b, v).neg()).div(determinant)?;
        let beta = add(mul(a, v), mul(b, u).neg()).div(determinant)?;
        let acceleration = std::array::from_fn(|axis| {
            add(
                add(
                    mul(jet.duu[axis], square(alpha)),
                    mul(jet.duv[axis], mul(Interval::point(2.0), mul(alpha, beta))),
                ),
                mul(jet.dvv[axis], square(beta)),
            )
        });
        Ok((radial_degree, dot(n, acceleration).hi <= 0.0))
    }

    /// A convex polygon support halfspace also contains its entire filled
    /// prism. Return a proved Euclidean separation of EVERY point in this box,
    /// not a failed upper-distance estimate or an unqualified point sample.
    fn exterior_separation(
        &self,
        position: [Interval; 3],
        minimum_mm: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<f64>, GeometryError> {
        if !self.convex_disk {
            return Ok(None);
        }
        let p = project(position, self.frame);
        for edge in 0..self.outer_count {
            work.charge(1, cancel)?;
            let a = self.vertices[edge];
            let b = self.vertices[self.next_vertex(edge)];
            let signed = orient(a, b, p);
            if signed.hi >= 0.0 {
                continue;
            }
            let delta = [b[0].sub(a[0]), b[1].sub(a[1])];
            let gradient = std::array::from_fn(|axis| {
                mul(delta[0], Interval::point(self.frame.y_axis[axis]))
                    .sub(mul(delta[1], Interval::point(self.frame.x_axis[axis])))
            });
            let scale = length(gradient)?.hi;
            if scale > 0.0 && scale.is_finite() {
                let separation = Interval::point(-signed.hi).div(Interval::point(scale))?.lo;
                if separation > minimum_mm {
                    return Ok(Some(separation));
                }
            }
        }
        Ok(None)
    }

    /// Every witness is on the actual polygon prism. In annular mode side
    /// witnesses use the SAME azimuth as the source point, as required by the
    /// convex-meridian artificial-cap covering proof.
    fn boundary_distance(
        &self,
        position: [Interval; 3],
        sufficient_error_mm: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<f64, GeometryError> {
        let scale = position
            .into_iter()
            .map(Interval::abs_upper)
            .fold(1.0_f64, f64::max);
        let reserve = add(
            mul(Interval::point(3.0), Interval::point(self.approximation)),
            mul(
                Interval::point(scale),
                Interval::point(f64::EPSILON * 128.0),
            ),
        )
        .hi;
        let original =
            self.boundary_witness_distance(position, sufficient_error_mm, reserve, work, cancel)?;
        if original <= sufficient_error_mm || sufficient_error_mm <= 0.0 || !self.convex_disk {
            return Ok(original);
        }
        let half_height = mul(
            mul(
                Interval::point(self.range[1]).sub(Interval::point(self.range[0])),
                Interval::point(0.5),
            ),
            Interval::point(self.height_vertical_norm),
        )
        .hi;
        let mut bound = up(half_height + reserve);
        if bound > sufficient_error_mm {
            return Ok(original);
        }
        // Distance to the FILLED convex prism is convex, so its maximum over
        // this Cartesian source box is bounded by its eight extreme points.
        // Outside, a closest filled-prism point lies on the boundary. Inside,
        // the nearer height cap gives the independent half-height bound above.
        // This is a complete box bound, not sampled source-surface authority.
        for mask in 0..8 {
            work.charge(1, cancel)?;
            let point = std::array::from_fn(|axis| {
                Interval::point(if mask & (1 << axis) == 0 {
                    position[axis].lo
                } else {
                    position[axis].hi
                })
            });
            bound = bound.max(self.boundary_witness_distance(
                point,
                sufficient_error_mm,
                reserve,
                work,
                cancel,
            )?);
            if bound > sufficient_error_mm {
                return Ok(original);
            }
        }
        Ok(original.min(bound))
    }

    fn boundary_witness_distance(
        &self,
        position: [Interval; 3],
        sufficient_error_mm: f64,
        reserve: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<f64, GeometryError> {
        let with_reserve = |distance| up(distance + reserve);
        let p = project(position, self.frame);
        let rho = length([p[0], p[1], Interval::point(0.0)])?;
        let mut inside = true;
        let mut first_side = 0;
        if let Some(edge) = self.convex_ray_sector(p, work, cancel)? {
            work.charge(1, cancel)?;
            first_side = edge;
            inside = orient(
                self.vertices[edge],
                self.vertices[self.next_vertex(edge)],
                p,
            )
            .lo >= 0.0;
        } else {
            for edge in 0..self.outer_count {
                work.charge(1, cancel)?;
                if orient(
                    self.vertices[edge],
                    self.vertices[self.next_vertex(edge)],
                    p,
                )
                .lo < 0.0
                {
                    inside = false;
                    first_side = edge;
                    break;
                }
            }
        }
        let inside_outer = inside;
        if self.core_radius.is_some() && rho.lo <= self.inner_radius_upper {
            inside = false;
        }
        let height = self.source_height.height(position);
        // Side witnesses use the actual closed-cut height, keeping native XY
        // coordinates fixed. Merely testing an allowance-expanded cell as
        // active does not put every raw point between the two cuts.
        let height_miss = Interval::point(self.range[0])
            .sub(height)
            .hi
            .max(0.0)
            .max(height.sub(Interval::point(self.range[1])).hi.max(0.0));
        let side_height_error = mul(
            Interval::point(height_miss),
            Interval::point(self.height_vertical_norm),
        )
        .hi;
        let cap_distance = || {
            let vertical = height
                .sub(Interval::point(self.range[0]))
                .abs_upper()
                .min(height.sub(Interval::point(self.range[1])).abs_upper());
            mul(
                Interval::point(vertical),
                Interval::point(self.height_vertical_norm),
            )
            .hi
        };
        let mut distance = if inside {
            cap_distance()
        } else {
            f64::INFINITY
        };
        // A feasible cap witness already within the requested policy needs no
        // additional side witness. Zero requests the full minimum of bounds.
        let cap_bound = with_reserve(distance);
        if cap_bound <= sufficient_error_mm {
            return Ok(cap_bound);
        }
        if self.core_radius.is_some() && rho.lo > 0.0 {
            let cosine = p[0].div(rho)?;
            let sine = p[1].div(rho)?;
            let radial: [Interval; 3] = std::array::from_fn(|axis| {
                add(
                    mul(self.height_dual[0][axis], cosine),
                    mul(self.height_dual[1][axis], sine),
                )
            });
            let scale = length(radial)?.hi;
            let mut loop_bounds = [0.0_f64; 2];
            let mut candidates = [0_usize; 2];
            let mut all_inner_intersections_before = true;
            let index = self.ray_index.as_ref().ok_or_else(|| {
                failure(
                    GeometryErrorCode::InvalidGeometry,
                    "annular sample is missing its admitted ray index",
                    0,
                    self.vertices.len(),
                )
            })?;
            index.visit(p, work, cancel, |edge, work| {
                work.charge(1, cancel)?;
                let inner = usize::from(edge >= self.outer_count);
                let a = self.vertices[edge];
                let b = self.vertices[self.next_vertex(edge)];
                let sign = if inner == 0 {
                    Interval::point(1.0)
                } else {
                    Interval::point(-1.0)
                };
                let first = mul(sign, orient([Interval::point(0.0); 2], a, p));
                let second = mul(sign, orient([Interval::point(0.0); 2], p, b));
                if first.hi < 0.0 || second.hi < 0.0 {
                    return Ok(());
                }
                let support = mul(sign, orient(a, b, [Interval::point(0.0); 2]));
                let edge_length = length([b[0].sub(a[0]), b[1].sub(a[1]), Interval::point(0.0)])?;
                let low = Interval::point(support.lo.max(0.0))
                    .div(Interval::point(edge_length.hi))?
                    .lo;
                let high = length([a[0], a[1], Interval::point(0.0)])?
                    .hi
                    .max(length([b[0], b[1], Interval::point(0.0)])?.hi);
                let fallback = Interval::point(rho.sub(Interval { lo: low, hi: high }).abs_upper())
                    .mul(Interval::point(scale))
                    .hi;
                // The actual same-azimuth ray/edge intersection contracts at
                // sampled corners. A global edge-radius range does not.
                let edge_vector = [b[0].sub(a[0]), b[1].sub(a[1])];
                let denominator = mul(
                    sign,
                    add(mul(p[0], edge_vector[1]), mul(p[1], edge_vector[0]).neg()),
                );
                let reserve = if denominator.lo > 0.0 {
                    let parameter = support.div(denominator)?;
                    if inner == 1 {
                        all_inner_intersections_before &= parameter.hi <= 1.0;
                    }
                    let radial_error = mul(rho, Interval::point(1.0).sub(parameter)).abs_upper();
                    mul(Interval::point(radial_error), Interval::point(scale)).hi
                } else {
                    if inner == 1 {
                        all_inner_intersections_before = false;
                    }
                    fallback
                };
                loop_bounds[inner] = loop_bounds[inner].max(reserve);
                candidates[inner] += 1;
                Ok(())
            })?;
            // The enclosing disk is only a sufficient outside-hole test.
            // Every compatible actual ray intersection may prove the same
            // cap material even when that disk test is inconclusive.
            if !inside && inside_outer && candidates[1] > 0 && all_inner_intersections_before {
                distance = distance.min(cap_distance());
            }
            // Each actual star loop has a ray intersection. Enumerate every
            // interval-compatible edge: MAX, not min, bounds the unknown feasible
            // intersection even when a source cell straddles a sampled vertex.
            for index in 0..2 {
                if candidates[index] > 0 {
                    distance = distance.min(up(loop_bounds[index] + side_height_error));
                }
            }
        } else if self.core_radius.is_none() {
            // A violated support proposes a nearby side; it grants no metric
            // authority. Every witness below is independently bounded.
            for offset in 0..self.vertices.len() {
                let candidate = first_side + offset;
                let edge = if candidate >= self.vertices.len() {
                    candidate - self.vertices.len()
                } else {
                    candidate
                };
                work.charge(1, cancel)?;
                let a = self.vertices[edge];
                let b = self.vertices[self.next_vertex(edge)];
                let side: [Interval; 3] = std::array::from_fn(|axis| {
                    add(
                        mul(b[0].sub(a[0]), self.height_dual[0][axis]),
                        mul(b[1].sub(a[1]), self.height_dual[1][axis]),
                    )
                });
                let relative: [Interval; 3] = std::array::from_fn(|axis| {
                    add(
                        mul(p[0].sub(a[0]), self.height_dual[0][axis]),
                        mul(p[1].sub(a[1]), self.height_dual[1][axis]),
                    )
                });
                let squared_length = side
                    .into_iter()
                    .map(Interval::square)
                    .fold(Interval::point(0.0), add);
                let transverse = if squared_length.lo > 0.0 && squared_length.hi.is_finite() {
                    let side_length = squared_length.sqrt()?;
                    let parameter = dot(relative, side).div(squared_length)?;
                    let overshoot = (-parameter.lo)
                        .max(parameter.sub(Interval::point(1.0)).hi)
                        .max(0.0);
                    // One feasible orthogonal projection for EACH actual point,
                    // clamped to the actual finite segment. The cross product
                    // preserves the along-side cancellation of a long cell.
                    let perpendicular = Interval::point(length(cross(relative, side))?.hi)
                        .div(Interval::point(side_length.lo))?;
                    add(perpendicular, mul(Interval::point(overshoot), side_length)).hi
                } else {
                    // An unresolved interval denominator still has an actual
                    // finite-segment witness at this fixed proposed parameter.
                    let d = [b[0].mid() - a[0].mid(), b[1].mid() - a[1].mid()];
                    let denominator = d[0] * d[0] + d[1] * d[1];
                    if denominator <= 0.0 || !denominator.is_finite() {
                        continue;
                    }
                    let parameter = (((p[0].mid() - a[0].mid()) * d[0]
                        + (p[1].mid() - a[1].mid()) * d[1])
                        / denominator)
                        .clamp(0.0, 1.0);
                    let q: [Interval; 2] = std::array::from_fn(|axis| {
                        add(
                            mul(a[axis], Interval::point(1.0 - parameter)),
                            mul(b[axis], Interval::point(parameter)),
                        )
                    });
                    length(std::array::from_fn(|axis| {
                        add(
                            mul(p[0].sub(q[0]), self.height_dual[0][axis]),
                            mul(p[1].sub(q[1]), self.height_dual[1][axis]),
                        )
                    }))?
                    .hi
                };
                distance = distance.min(up(transverse + side_height_error));
                let side_bound = with_reserve(distance);
                if sufficient_error_mm > 0.0 && side_bound <= sufficient_error_mm {
                    return Ok(side_bound);
                }
            }
        }
        // The cap material is convex globally (disk complex) or in each proven
        // meridian fiber (cycle complex). Native sampling transfers that exact
        // source region to its actual polygon in both directions.
        Ok(with_reserve(distance))
    }
}

/// Numeric proposal only. Original F membership is independently checked against
/// the same borrowed prepared source before the proposal becomes a counterexample.
struct SourceFNegativeProposal {
    cell_uv: [[f64; 2]; 2],
    depth: u8,
    height_preconditioned: bool,
    point_uv: [[f64; 2]; 2],
    position: [Interval; 3],
    separation: f64,
}

type PositionNormalEnclosure = ([Interval; 3], [Interval; 3]);

/// UV and 3D edge parameters are independent. Cover the complete retained UV
/// coedge in its OWN parameter, and exclude a UV subarc only by disjoint native
/// POSITION enclosures. Union every compatible source normal: this is complete
/// incidence coverage, never a sampled inverse/nearest parameter assignment.
fn coedge_normal_enclosure(
    domain: &PreparedFaceTrimDomain<'_>,
    coedge_index: usize,
    position: [Interval; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[Interval; 3], GeometryError> {
    let surface = &domain.surface_operand();
    let source_chart = source_chart::SphereChart::new(surface, domain, work, cancel)?;
    let target_diameter = length(position.map(|value| Interval::point(value.width())))?.hi;
    let scale = position
        .into_iter()
        .map(Interval::abs_upper)
        .fold(1.0_f64, f64::max);
    let target_width = up(2.0 * target_diameter
        + 8.0 * domain.source_chart_error_mm()
        + scale * f64::EPSILON * 128.0);
    let mut enclosure: Option<[Interval; 3]> = None;
    let mut stack = Vec::with_capacity(usize::from(MAX_DEPTH) + 1);
    for span in domain.boundary_parameter_intervals(coedge_index)? {
        work.charge(1, cancel)?;
        stack.push((span, 0_u8));
        while let Some((span, depth)) = stack.pop() {
            work.charge(1, cancel)?;
            let uv = domain.boundary_position_enclosure(coedge_index, span, work, cancel)?;
            // The pcurve's own knot spans need not be surface knot spans.
            // Partition its ENTIRE position tube at every original surface knot
            // before requesting one-sided jets. A projective G1 join can have
            // a genuine parameter derivative jump, so evaluating a single jet
            // across it would refuse an otherwise exact regular source seam.
            let compatible = (|| -> Result<Option<PositionNormalEnclosure>, GeometryError> {
                    let mut result: Option<PositionNormalEnclosure> = None;
                    for patch in surface_parameter_boxes(surface, uv, work, cancel)? {
                        let jet = crate::parametric::surface_piecewise_enclosure(surface, patch, work, cancel)?;
                        if (0..3)
                            .any(|axis| intersection(position[axis], jet.position[axis]).is_none())
                        {
                            continue;
                        }
                        let n = match &source_chart {
                            Some(chart) => chart.normal(jet.position)?,
                            None => normal(&jet, domain.orientation()),
                        };
                        result = Some(match result {
                            Some((p, previous)) => (
                                std::array::from_fn(|axis| p[axis].hull(jet.position[axis])),
                                std::array::from_fn(|axis| previous[axis].hull(n[axis])),
                            ),
                            None => (jet.position, n),
                        });
                    }
                    Ok(result)
                })()?;
            let Some((image, n)) = compatible else {
                continue;
            };
            let image_width = length(image.map(|value| Interval::point(value.width())))?.hi;
            let normal_width = length(n.map(|value| Interval::point(value.width())))?.hi;
            // These are refinement heuristics only. Every accepted normal is
            // still an outward whole-subarc enclosure; the actual link-sign
            // proof below must succeed using its complete union.
            let mid = span[0] * 0.5 + span[1] * 0.5;
            if depth < MAX_DEPTH
                && mid > span[0]
                && mid < span[1]
                && image_width > target_width
                && normal_width > length(n)?.hi * 1e-8
            {
                work.charge(2, cancel)?;
                stack.push(([span[0], mid], depth + 1));
                stack.push(([mid, span[1]], depth + 1));
            } else {
                enclosure = Some(match enclosure {
                    Some(previous) => std::array::from_fn(|axis| previous[axis].hull(n[axis])),
                    None => n,
                });
            }
        }
    }
    enclosure.ok_or_else(|| GeometryError::new(GeometryErrorCode::ParameterCorrespondenceUnavailable,
        format!("critical source coedge {coedge_index} has no UV-position incidence covering native interval {position:?}")))
}

/// Partition geometry evaluation at BOTH original coefficient inventories,
/// retaining both sides of an outward mapped knot band. These are numerical
/// coverage cells, never isolated material-height events.
fn original_coedge_parameter_cuts(
    domains: [(&PreparedFaceTrimDomain<'_>, usize); 2],
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Vec<f64>, GeometryError> {
    work.charge(1, cancel)?;
    let mut maximum = 0_usize;
    for (domain, coedge) in domains {
        let inventory = domain
            .original_coedge_correspondence(coedge)?
            .span_inventory();
        maximum = maximum
            .checked_add(
                inventory
                    .source_knot_boundaries
                    .checked_add(inventory.uv_knot_boundaries)
                    .and_then(|n| n.checked_add(inventory.source_knot_bands))
                    .and_then(|n| n.checked_add(inventory.uv_knot_bands))
                    .and_then(|n| n.checked_add(2))
                    .and_then(|n| n.checked_mul(2))
                    .ok_or_else(|| {
                        failure(
                            GeometryErrorCode::ResourceLimit,
                            "original coedge partition count overflow",
                            0,
                            coedge,
                        )
                    })?,
            )
            .ok_or_else(|| {
                failure(
                    GeometryErrorCode::ResourceLimit,
                    "paired original coedge partition count overflow",
                    0,
                    coedge,
                )
            })?;
    }
    if maximum
        .checked_mul(std::mem::size_of::<f64>())
        .is_none_or(|n| n > remaining_bytes)
    {
        return Err(failure(
            GeometryErrorCode::ResourceLimit,
            "original coedge partitions exhaust query scratch",
            0,
            maximum,
        ));
    }
    let mut cuts = Vec::new();
    cuts.try_reserve_exact(maximum).map_err(|_| {
        failure(
            GeometryErrorCode::ResourceLimit,
            "original coedge partition allocation refused",
            0,
            maximum,
        )
    })?;
    if cuts.capacity().saturating_mul(std::mem::size_of::<f64>()) > remaining_bytes {
        return Err(failure(
            GeometryErrorCode::ResourceLimit,
            "original coedge partition allocator exceeds query scratch",
            0,
            cuts.capacity(),
        ));
    }
    for (domain, coedge) in domains {
        domain.visit_original_coedge_span_boundaries(coedge, work, cancel, &mut |boundary| {
            if cuts.len().saturating_add(2) > maximum {
                return Err(failure(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "original coefficient span inventory changed after capture",
                    0,
                    coedge,
                ));
            }
            cuts.push(boundary.lo);
            cuts.push(boundary.hi);
            Ok(())
        })?;
    }
    let comparisons = (usize::BITS - cuts.len().leading_zeros()) as u64;
    work.charge((cuts.len() as u64).saturating_mul(comparisons), cancel)?;
    cuts.sort_unstable_by(f64::total_cmp);
    cuts.dedup_by(|a, b| *a == *b);
    Ok(cuts)
}

struct NeighborhoodInput {
    face: Option<usize>,
    height: Interval,
    root_proved: bool,
    pairing: ComponentVoidPairing,
}

fn add_neighborhood(
    events: &mut Vec<CriticalEventNeighborhood>,
    capture: &Arc<CriticalCapture>,
    input: NeighborhoodInput,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    let NeighborhoodInput {
        face,
        height,
        root_proved,
        pairing,
    } = input;
    if !height.lo.is_finite() || !height.hi.is_finite() {
        return Err(failure(
            GeometryErrorCode::ConditioningFailure,
            "critical height enclosure is nonfinite",
            face.unwrap_or(0),
            [height.lo, height.hi],
        ));
    }
    if !overlaps(height, capture.request.height_interval_mm) {
        work.charge(1, cancel)?;
        return Ok(());
    }
    let next_capacity = if events.len() == events.capacity() {
        events.capacity().saturating_mul(2).max(4)
    } else {
        events.capacity()
    };
    if events.len() >= MAX_NEIGHBORHOODS
        || next_capacity * std::mem::size_of::<CriticalEventNeighborhood>() > MAX_EVENT_BYTES
    {
        return Err(failure(
            GeometryErrorCode::ResourceLimit,
            "critical neighborhood capacity exhausted",
            face.unwrap_or(0),
            [height.lo, height.hi],
        ));
    }
    work.charge(1, cancel)?;
    events.try_reserve(1).map_err(|_| {
        failure(
            GeometryErrorCode::ResourceLimit,
            "critical neighborhood allocation refused",
            face.unwrap_or(0),
            events.len(),
        )
    })?;
    events.push(CriticalEventNeighborhood {
        capture: Arc::clone(capture),
        height_bracket_mm: [height.lo, height.hi],
        source_face: face,
        stationary_root_proved: root_proved,
        incoming_regular_endpoint: false,
        pairing,
    });
    Ok(())
}

/// Minimal transverse height flow relative to the actual, possibly oblique,
/// prism's vertical dual column. Strict |n x h| positivity proves submersion.
fn material_face_flow_rate(
    normal: [Interval; 3],
    height: [Interval; 3],
    vertical: [Interval; 3],
) -> Result<Option<f64>, GeometryError> {
    let denominator = length(cross(normal, height))?;
    if denominator.lo <= 0.0 {
        return Ok(None);
    }
    let numerator = mul(
        Interval::point(dot(normal, vertical).abs_upper()),
        length(height)?,
    );
    let rate = numerator.div(denominator)?.hi;
    Ok(rate.is_finite().then_some(rate))
}

fn material_edge_flow_rate(
    tangent: [Interval; 3],
    height: [Interval; 3],
    vertical: [Interval; 3],
) -> Result<Option<f64>, GeometryError> {
    let derivative = dot(height, tangent);
    if derivative.contains_zero() {
        return Ok(None);
    }
    let mut transverse = [Interval::point(0.0); 3];
    for axis in 0..3 {
        transverse[axis] = tangent[axis].div(derivative)?.sub(vertical[axis]);
    }
    let rate = length(transverse)?.hi;
    Ok(rate.is_finite().then_some(rate))
}

fn include_flow_rate(complete: &mut Option<f64>, next: Option<f64>) {
    *complete = match (*complete, next) {
        (Some(current), Some(next)) => Some(current.max(next)),
        _ => None,
    };
}

/// Returns None only when the actual native sample is outside this sufficient
/// one-component/no-void construction. No family label decides applicability.
pub(super) fn certify_critical_slab(
    query: CriticalSlabQuery<'_, '_, '_>,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<CriticalSlabCertificate>, GeometryError> {
    let CriticalSlabQuery {
        source,
        request,
        section,
        functional,
        actual_sample_height,
    } = query;
    let definition = source.definition();
    work.charge(1, cancel)?;
    if section.definition_id() != definition.id() || !section.is_native_intersection() {
        return Err(failure(
            GeometryErrorCode::InvalidGeometry,
            "sample does not belong to the captured native definition",
            0,
            request.height_interval_mm,
        ));
    }
    if !(section.loops.len() == 1 && !section.loops[0].is_hole)
        && !(section.loops.len() == 2
            && section
                .loops
                .iter()
                .filter(|boundary| boundary.is_hole)
                .count()
                == 1)
    {
        return Ok(None);
    }
    let (_, first_face) = super::source_incidence::native_face(definition, 0)?;
    for ordinal in 1..definition.face_ids().len() {
        work.charge(1, cancel)?;
        let (_, face) = super::source_incidence::native_face(definition, ordinal)?;
        if face.shell != first_face.shell {
            return Ok(None);
        }
    }
    let scratch_limit = remaining_bytes.min(MAX_SCRATCH_BYTES);
    let capture_bytes = 2 * std::mem::size_of::<usize>()
        + std::mem::size_of::<CriticalCapture>()
        + definition.id().allocated_bytes();
    let fixed_scratch = MAX_EVENT_BYTES
        + MAX_EDGE_BYTES
        + material_link::MAX_CORRECTION_BYTES
        + material_link::UV_EMBEDDING_STACK_BYTES
        + std::mem::size_of::<material_link::PairedMaterialLink<'_, '_>>()
        + RESERVED_STACK_BYTES
        + super::slab::HEIGHT_CONTRACTION_SCRATCH_BYTES
        + capture_bytes
        + std::mem::size_of::<SourceFNegativeProposal>()
        + 4 * std::mem::size_of::<[[f64; 2]; 2]>()
        + 4096;
    let sample_count = section
        .loops
        .iter()
        .map(|boundary| boundary.points_mm.len().saturating_sub(1))
        .sum::<usize>();
    let sample_bytes = SamplePrism::required_heap_bytes(
        sample_count,
        section.loops.iter().any(|boundary| boundary.is_hole),
    )
    .ok_or_else(|| {
        failure(
            GeometryErrorCode::ResourceLimit,
            "critical sample index capacity overflow",
            0,
            sample_count,
        )
    })?;
    scratch_limit
        .checked_sub(fixed_scratch.saturating_add(sample_bytes))
        .ok_or_else(|| {
            failure(
                GeometryErrorCode::ResourceLimit,
                "critical sample/source capture capacity exhausted",
                0,
                sample_bytes,
            )
        })?;
    let prism = SamplePrism::new(
        section,
        request,
        functional,
        actual_sample_height,
        work,
        cancel,
    )?;
    let source_allowance = definition
        .provenance
        .uncertainty_mm
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or_else(|| {
            failure(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "critical material has no admitted source realization allowance",
                0,
                (),
            )
        })?;
    let mut closed_boundary = prism.closed_capture_boundary(work, cancel)?;
    let mut whole_source = source.material_transport_is_proved_zero(work, cancel)?;
    let chart_bytes = scratch_limit
        .checked_sub(fixed_scratch.saturating_add(prism.heap_bytes()))
        .ok_or_else(|| {
            failure(
                GeometryErrorCode::ResourceLimit,
                "critical source chart capacity exhausted",
                0,
                scratch_limit,
            )
        })?;
    let pairing = ComponentVoidPairing {
        material_components: 1,
        bounded_voids: 0,
        tunnel_cycles: usize::from(prism.core_radius.is_some()),
    };
    let capture = CriticalCapture::new(definition, request, functional);
    let mut events = Vec::new();
    let mut edges = SourceEdgeIncidence::new(definition, MAX_EDGE_BYTES, work, cancel)?;
    edges.prepare_vertices(MAX_EDGE_BYTES, work, cancel)?;
    let vertex_before = work.consumed();
    let mut material_links = material_link::ClosedRadialLinks::new(
        source,
        &edges,
        &prism,
        request,
        chart_bytes,
        work,
        cancel,
    )
    .map_err(|error| {
        work.annotate_error(error, vertex_before, "closed material source vertex links")
    })?;
    let mut regular_flow = material_links
        .all_vertex_supports_outside_closed_slab()
        .then_some(0.0);
    for support in material_links.vertex_supports() {
        add_neighborhood(
            &mut events,
            &capture,
            NeighborhoodInput {
                face: None,
                height: functional.height(*support),
                root_proved: false,
                pairing,
            },
            work,
            cancel,
        )?;
    }
    let mut covered_face_cells = 0_u64;
    let mut covered_coedge_cells = 0_u64;
    let mut bound = prism.approximation;
    // Leave room for the later actual-source realization charge and its final
    // upward rounding. This target selects a witness, never changes policy.
    let witness_target_mm = Interval::point(down(request.native_shape_error_mm))
        .sub(Interval::point(source_allowance))
        .lo
        .max(0.0);
    let height_target = Interval {
        lo: request.height_interval_mm[0],
        hi: request.height_interval_mm[1],
    }
    .sub(functional.dot(
        [Interval {
            lo: -source_allowance,
            hi: source_allowance,
        }; 3],
    ));
    // A negative witness leaves even the existing realization/approximation
    // reserves on the rejected side. It never raises the acceptance policy.
    let rejection_target_mm = Interval::point(request.native_shape_error_mm)
        .add(Interval::point(source_allowance))
        .add(mul(
            Interval::point(3.0),
            Interval::point(prism.approximation),
        ))
        .hi;
    let mut lower = [f64::INFINITY; 2];
    let mut upper = [f64::NEG_INFINITY; 2];
    for (face_index, _) in definition.face_ids().iter().enumerate() {
        let face_before = work.consumed();
        let domain = source.face_domain(face_index, chart_bytes, work, cancel)?;
        (|| -> Result<(), GeometryError> {
            let surface = &domain.surface_operand();
            let source_chart = source_chart::SphereChart::new(surface, domain, work, cancel)?;
            let face_glued = capture_face_gluing(
                surface, domain.uv_domain(), functional, request, face_index, work, cancel,
            )?;
            whole_source &= face_glued;
            let mut stack = Vec::with_capacity(usize::from(MAX_DEPTH) + 1);
            let mut spans_seen = 0;
            let mut active_span = domain.uv_domain();
            let mut propose_negative = true;
            loop {
            let proposal = (|| -> Result<Option<SourceFNegativeProposal>, GeometryError> {
            let mut spans = surface_parameter_boxes(surface, domain.uv_domain(), work, cancel)?
                .skip(spans_seen);
            while let Some((uv, depth, height_preconditioned)) =
                stack.pop().or_else(|| spans.next().map(|uv| {
                    active_span = uv;
                    spans_seen += 1;
                    (uv, 0_u8, false)
                }))
            {
                work.charge(1, cancel)?;
                let span_smooth = surface_parameter_span_smooth(surface, uv, work, cancel)?;
                let (uv, jet, height_preconditioned) = if !whole_source && !height_preconditioned {
                    let Some((clipped, jet)) = super::slab::clip_height_box(
                        surface, uv, functional, [height_target.lo, height_target.hi], work, cancel,
                    )? else {
                        closed_boundary.excluded_face_cells += 1;
                        continue;
                    };
                    let h = functional.height(jet.position);
                    let preconditioned = height_target.lo <= h.lo && h.hi <= height_target.hi;
                    (clipped, jet, preconditioned)
                } else {
                    // The optional whole-source theorem retains its full
                    // domain. Otherwise only whole-image height containment
                    // is inherited; a merely contracted necessary preimage
                    // must be tightened again after parameter refinement.
                    let jet = if span_smooth {
                        surface_enclosure(surface, uv, work, cancel)?
                    } else {
                        crate::parametric::surface_piecewise_enclosure(surface, uv, work, cancel)?
                    };
                    (uv, jet, height_preconditioned)
                };
                // The allowance-expanded carrier excludes its entire trimmed
                // subset from this closed capture; no winding query is needed.
                // Keep full jets: annular meridian/convexity obligations below
                // still consume their second derivatives.
                let height = functional.height(jet.position.map(|value|
                    value.add(Interval { lo: -source_allowance, hi: source_allowance })));
                let active = overlaps(height, request.height_interval_mm);
                if !active && !whole_source {
                    closed_boundary.excluded_face_cells += 1;
                    continue;
                }
                // Qualify each ORIGINAL extracted source span before adaptive
                // refinement. Subdivision cannot erase an unproved nested C0
                // interface and turn two locally convex pieces into a global
                // convex shell. Explicit root-carrier interfaces were proved
                // by exact projective gluing above; this additionally follows
                // every actual nested sweep/profile carrier.
                if !span_smooth && !face_glued {
                    if domain.classify(uv, work, cancel)? == TrimClassification::Outside { continue; }
                    if !active {
                        whole_source = false;
                        closed_boundary.excluded_face_cells += 1;
                        continue;
                    }
                    return Err(failure(GeometryErrorCode::AnalysisIndeterminate,
                        "original source span has an unresolved nested/reflected C0 curvature interface", face_index, uv));
                }
                let n = match &source_chart {
                    Some(chart) => chart.normal(jet.position)?,
                    None => normal(&jet, domain.orientation()),
                };
                let (radial_degree, convex) = prism.patch_incidence(
                    &jet, n, source_chart.as_ref().map(source_chart::SphereChart::convex),
                )?;
                let regular = length(n)?.lo > 0.0;
                let metric = if active { prism.boundary_distance(jet.position, witness_target_mm, work, cancel)? } else { 0.0 };
                let needs_refinement = !regular || radial_degree.lo <= 0.0
                    || metric > request.native_shape_error_mm;
                // A successful full-carrier cell also covers its complete
                // original trimmed subset, including trim/cap contact. Asking
                // winding first repeatedly subdivides intersecting trim arcs
                // even when no trim exclusion is needed. Only unresolved
                // cells need that exclusion before refinement or refusal.
                // Preserve the optional whole-source convexity theorem by
                // excluding irrelevant nonconvex carrier cells separately.
                if (needs_refinement || whole_source && !convex)
                    && domain.classify(uv, work, cancel)? == TrimClassification::Outside
                {
                    continue;
                }
                whole_source &= convex;
                if propose_negative && active && regular && radial_degree.lo > 0.0
                    && metric > request.native_shape_error_mm
                    && let Some(separation) = prism.exterior_separation(
                        jet.position, rejection_target_mm, work, cancel,
                    )?
                {
                    let point_uv = uv.map(|range| [range[0] * 0.5 + range[1] * 0.5; 2]);
                    let point = surface_enclosure(surface, point_uv, work, cancel)?.position;
                    let point_height = functional.height(point.map(|value| value.add(
                        Interval { lo: -source_allowance, hi: source_allowance },
                    )));
                    if point_height.lo >= request.height_interval_mm[0]
                        && point_height.hi <= request.height_interval_mm[1]
                    {
                        return Ok(Some(SourceFNegativeProposal {
                            cell_uv: uv, depth, height_preconditioned,
                            point_uv, position: point, separation,
                        }));
                    }
                }
                if needs_refinement {
                    if !active && (radial_degree.hi <= 0.0 || depth >= MAX_DEPTH) {
                        // The optional whole-source theorem failed, but this
                        // complete original cell is proved outside BOTH cuts.
                        whole_source = false;
                        closed_boundary.excluded_face_cells += 1;
                        continue;
                    }
                    if depth >= MAX_DEPTH {
                        let obligation = if !regular { "source chart rank is unresolved" }
                            else if radial_degree.lo <= 0.0 { "source material/void radial degree is not positive" }
                            else { "whole native boundary cell exceeds candidate-prism shape policy" };
                        return Err(failure(if metric > request.native_shape_error_mm { GeometryErrorCode::ShapeErrorExceeded }
                            else { GeometryErrorCode::AnalysisIndeterminate }, obligation, face_index,
                            (uv, request.height_interval_mm, request.sample_height_mm)));
                    }
                    work.charge(2, cancel)?;
                    let children = match split_metric(uv, depth, face_index, &jet) {
                        Ok(children) => children,
                        Err(error) if !active && error.code == GeometryErrorCode::AnalysisIndeterminate => {
                            whole_source = false;
                            closed_boundary.excluded_face_cells += 1;
                            continue;
                        }
                        Err(error) => return Err(error),
                    };
                    for child in children { stack.push((child, depth + 1, height_preconditioned)); }
                    continue;
                }
                covered_face_cells += 1;
                if !active { continue; }
                include_flow_rate(&mut regular_flow,
                    material_face_flow_rate(n, functional.direction, prism.height_dual[2])?);
                bound = bound.max(metric);
                let projected = project(jet.position, section.frame);
                for axis in 0..2 { lower[axis] = lower[axis].min(projected[axis].lo); upper[axis] = upper[axis].max(projected[axis].hi); }
                match stationary_root(surface, uv, &jet, functional, work, cancel)? {
                    RootIsolation::Excluded => {}
                    RootIsolation::Unique(root) => {
                        let member = domain.classify(root, work, cancel)?;
                        let root_height = functional.height(surface_enclosure(surface, root, work, cancel)?.position);
                        if member != TrimClassification::Outside {
                            add_neighborhood(&mut events, &capture, NeighborhoodInput {
                                face: Some(face_index),
                                height: root_height,
                                root_proved: member == TrimClassification::Inside,
                                pairing,
                            }, work, cancel)?;
                        }
                    }
                    RootIsolation::ConvexCriticalCover => {
                        // Final acceptance additionally proves the COMPLETE
                        // closed radial map has degree one. This conservative
                        // original cell retains every potential height event;
                        // neither root existence nor convexity is inferred.
                        add_neighborhood(&mut events, &capture, NeighborhoodInput {
                            face: Some(face_index),
                            height,
                            root_proved: false,
                            pairing,
                        }, work, cancel)?;
                    }
                }
            }
            Ok(None)
            })()?;
            let Some(proposal) = proposal else { break; };
            // Multi-face reference qualification shares this immutable native
            // owner and never acquires a foreign surface mutex.
            let midpoint = active_span.map(|range| [range[0] * 0.5 + range[1] * 0.5; 2]);
            let candidates = [
                proposal.point_uv,
                [proposal.point_uv[0], midpoint[1]],
                [midpoint[0], proposal.point_uv[1]],
            ];
            for (index, &point_uv) in candidates.iter().enumerate() {
                if candidates[..index].contains(&point_uv) { continue; }
                let (point, separation) = if index == 0 {
                    (proposal.position, proposal.separation)
                } else {
                    // Coordinate-centered proposals stay in this already
                    // admitted original span, not a fitted/source family.
                    let point = surface_enclosure(surface, point_uv, work, cancel)?.position;
                    let height = functional.height(point.map(|value| value.add(
                        Interval { lo: -source_allowance, hi: source_allowance },
                    )));
                    if height.lo < request.height_interval_mm[0]
                        || height.hi > request.height_interval_mm[1] { continue; }
                    let Some(separation) = prism.exterior_separation(
                        point, rejection_target_mm, work, cancel,
                    )? else { continue; };
                    (point, separation)
                };
                if material_link::source_f_point_is_retained(
                    source, &domain, face_index, point_uv, point, &edges,
                    chart_bytes, work, cancel,
                )? {
                    return Err(failure(GeometryErrorCode::ShapeErrorExceeded,
                        "retained original F point has proved exterior candidate-prism separation",
                        face_index, (point_uv, request.height_interval_mm, separation)));
                }
            }
            // Keep all completed cells. Resume only the uncommitted cell and
            // its stack; rebuilding the borrowed span cursor copies no knots.
            propose_negative = false;
            stack.push((proposal.cell_uv, proposal.depth, proposal.height_preconditioned));
            }
            Ok(())
        })().map_err(|error| work.annotate_error(error, face_before,
            format_args!("closed material original face cover; source_face={face_index} scope={:?} sample={}",
                request.height_interval_mm, request.sample_height_mm)))?;
    }
    // All original incidences remain mandatory. Only a WHOLE edge-cell height
    // exclusion removes its geometric link from the clipped boundary; cells
    // touching either CLOSED cut retain the complete two-sided material link.
    for uses in edges.values() {
        let edge_before = work.consumed();
        let first = uses.first;
        let second = uses.second.ok_or_else(|| {
            failure(
                GeometryErrorCode::SingularGeometry,
                "source boundary complex has an unpaired edge",
                first.face,
                (first.wire, first.index),
            )
        })?;
        let first_domain = source.face_domain(first.face, chart_bytes, work, cancel)?;
        let second_domain = source.face_domain(
            second.face,
            chart_bytes,
            work,
            cancel,
        )?;
        let edge = first_domain.coedges()[first.coedge].edge;
        let central_range = material_links.central_spatial_range(
            edge,
            first_domain
                .original_coedge_correspondence(first.coedge)?.spatial_range(),
            [first.face, first.wire, first.index],
        )?;
        let paired_scratch = chart_bytes;
        let material_link = material_link::PairedMaterialLink::new(
            definition,
            &first_domain,
            first,
            &second_domain,
            second,
            central_range,
            paired_scratch,
            work,
            cancel,
        )?;
        let excluded_link_enclosure = if material_link.is_none() {
            let a = crate::parametric::surface_piecewise_enclosure(
                    &first_domain.surface_operand(),
                    first_domain.uv_domain(),
                    work,
                    cancel,
                )
                .map(|jet| jet.position)?;
            let b = crate::parametric::surface_piecewise_enclosure(
                    &second_domain.surface_operand(),
                    second_domain.uv_domain(),
                    work,
                    cancel,
                )
                .map(|jet| jet.position)?;
            Some(std::array::from_fn(|axis| {
                a[axis].hull(b[axis]).add(Interval {
                    lo: -source_allowance,
                    hi: source_allowance,
                })
            }))
        } else {
            None
        };
        // Both selected source traces share the same sealed original spatial gauge.
        let partition_bytes = chart_bytes;
        let source_cuts = original_coedge_parameter_cuts(
            [
                (&first_domain, first.coedge),
                (&second_domain, second.coedge),
            ],
            partition_bytes,
            work,
            cancel,
        )?;
        let range = first_domain.original_coedge_correspondence(first.coedge)?.spatial_range();
        // Events grow while the source operator remains borrowed. Reserve all
        // coexistent events, knots and sample storage before preparing it.
        let occupied_scratch = fixed_scratch
            + prism.heap_bytes()
            + source_cuts.capacity() * std::mem::size_of::<f64>();
        let remaining = scratch_limit.checked_sub(occupied_scratch).ok_or_else(|| failure(
            GeometryErrorCode::ResourceLimit,
            "critical scratch capacity exhausted before original spatial curve capture",
            first.face, range,
        ))?;
        first_domain.with_original_spatial_curve(
            first.coedge, remaining, work, cancel, |curve, work, cancel| {
            // A C0 original edge can carry a height event even when each
            // smooth side has nonzero derivative. It cannot advertise the
            // complete regular height-flow theorem merely from side bounds.
            if regular_flow.is_some()
                && !crate::parametric::curve_parameter_span_smooth(curve, range, work, cancel)?
            {
                regular_flow = None;
            }
            // Original physical coedge endpoints are zero-dimensional source
            // strata too. Monotone face/edge derivatives do not exclude a
            // polyhedral corner event. Use actual bounded 3D endpoint tubes,
            // never a UV parameter or a nearby stored/sampled vertex.
            for parameter in range {
                let height = functional.height(
                    crate::parametric::curve_enclosure(curve, (parameter, parameter), work, cancel)?.position,
                );
                add_neighborhood(&mut events, &capture, NeighborhoodInput {
                    face: Some(first.face),
                    height,
                    root_proved: false,
                    pairing,
                }, work, cancel)?;
            }
            for span in crate::parametric::curve_parameter_spans(curve, range, work, cancel)? {
                let span = [span[0].max(central_range[0]), span[1].min(central_range[1])];
                if span[0] > span[1] { continue; }
                let mut stack = Vec::with_capacity(usize::from(MAX_DEPTH) + 1);
                let first_cut = source_cuts.partition_point(|cut| *cut <= span[0]);
                let last_cut = source_cuts.partition_point(|cut| *cut < span[1]).max(first_cut);
                let mut segment_start = span[0];
                for segment_end in source_cuts[first_cut..last_cut].iter().copied()
                    .chain(std::iter::once(span[1]))
                {
                    stack.push(((segment_start, segment_end), 0_u8));
                    segment_start = segment_end;
                while let Some(((lo, hi), depth)) = stack.pop() {
                    work.charge(1, cancel)?;
                    let jet = crate::parametric::curve_enclosure(curve, (lo, hi), work, cancel)?;
                    let swept = match &material_link {
                        Some(link) => link.swept_enclosure([lo, hi], &jet, work, cancel)?,
                        None => excluded_link_enclosure.expect("unproved link keeps whole-face enclosure"),
                    };
                    let h = functional.height(swept);
                    let active = overlaps(h, request.height_interval_mm);
                    if !active && !whole_source {
                        closed_boundary.excluded_coedge_cells += 1;
                        continue;
                    }
                    let proved = match &material_link {
                        Some(link) => link.cell([lo, hi], &jet, &prism, work, cancel)?,
                        None => Err(material_link::MaterialLinkRefusal::ConstructorSourceEmbedding),
                    };
                    let proved = match proved {
                        Ok(proved) => proved,
                        Err(stage) => {
                            if !active {
                                whole_source = false;
                                closed_boundary.excluded_coedge_cells += 1;
                                continue;
                            }
                            let mid = lo * 0.5 + hi * 0.5;
                            if material_link.is_none() || depth >= MAX_DEPTH || mid <= lo || mid >= hi {
                                return Err(failure(GeometryErrorCode::AnalysisIndeterminate,
                                    "original paired material collar has no complete positive radial link", first.face,
                                    (stage, first.wire, first.index, second.face, [lo, hi],
                                        request.height_interval_mm, request.sample_height_mm)));
                            }
                            work.charge(2, cancel)?;
                            stack.push(((lo, mid), depth + 1));
                            stack.push(((mid, hi), depth + 1));
                            continue;
                        }
                    };
                    whole_source &= proved.convex_link;
                    material_links.record(&proved, work, cancel)?;
                    covered_coedge_cells += 1;
                    if !active { continue; }
                    include_flow_rate(&mut regular_flow,
                        proved.regular_flow_rate(jet.derivative, functional.direction, prism.height_dual[2])?);
                    if proved.source_error_mm != 0.0 {
                        // Transport can introduce height criticality in the
                        // collar interior. Keep its ENTIRE closed height image;
                        // original edge derivatives cannot exclude that event.
                        add_neighborhood(&mut events, &capture, NeighborhoodInput {
                            face: Some(first.face),
                            height: functional.height(proved.position),
                            root_proved: false,
                            pairing,
                        }, work, cancel)?;
                    }
                    let derivative = functional.dot(jet.derivative);
                    if !derivative.contains_zero() { continue; }
                    let second_derivative = functional.dot(jet.second_derivative);
                    let mut root_proved = false;
                    let mut neighborhood = h;
                    // Scalar interval Newton and a strict derivative sign change
                    // isolate a true critical root of the selected ORIGINAL
                    // spatial trace. Source UV derivatives grant no root proof.
                    if !second_derivative.contains_zero()
                        && crate::parametric::curve_parameter_span_smooth(curve, [lo, hi], work, cancel)?
                    {
                        let mid = lo * 0.5 + hi * 0.5;
                        let value = functional.dot(
                            crate::parametric::curve_enclosure(curve, (mid, mid), work, cancel)?.derivative,
                        );
                        let newton = Interval::point(mid).sub(value.div(second_derivative)?);
                        if let Some(root) = intersection(Interval { lo, hi }, newton) {
                            let a = functional.dot(
                                crate::parametric::curve_enclosure(curve, (lo, lo), work, cancel)?.derivative,
                            );
                            let b = functional.dot(
                                crate::parametric::curve_enclosure(curve, (hi, hi), work, cancel)?.derivative,
                            );
                            root_proved = strictly_contains(Interval { lo, hi }, newton)
                                && ((a.hi < 0.0 && b.lo > 0.0) || (a.lo > 0.0 && b.hi < 0.0));
                            neighborhood = functional.height(
                                crate::parametric::curve_enclosure(curve, (root.lo, root.hi), work, cancel)?.position,
                            );
                        } else { continue; }
                    }
                    add_neighborhood(&mut events, &capture, NeighborhoodInput {
                        face: Some(first.face),
                        height: neighborhood,
                        root_proved,
                        pairing,
                    }, work, cancel)?;
                }
                }
            }
            Ok(())
        }).map_err(|error| work.annotate_error(error, edge_before,
            format_args!("closed material paired coedge cover; source_faces={}/{} source_coedges={}/{}",
                first.face, second.face, first.coedge, second.coedge)))?;
    }
    // Local radial signs are insufficient: a second disconnected sheet or a
    // nonunit toroidal covering has the same local signs. The COMPLETE regular
    // fiber census counts all original faces and excludes every corrected
    // collar/vertex/cap support before granting total degree one.
    let degree_before = work.consumed();
    material_links
        .degree_one(source, &prism, request, chart_bytes, work, cancel)
        .map_err(|error| {
            work.annotate_error(
                error,
                degree_before,
                "closed material complete source fiber degree",
            )
        })?;
    let realization_error = material_links.source_error_mm;
    let vertical_reverse_bound = up(prism.vertical_return + realization_error);
    let reverse_bound = if let Some(rate) = regular_flow {
        let height_norm = length(functional.direction)?;
        let vertical_norm = Interval::point(prism.height_vertical_norm);
        let gap = Interval::point(actual_sample_height.hi)
            .sub(Interval::point(request.height_interval_mm[0]))
            .hi
            .max(
                Interval::point(request.height_interval_mm[1])
                    .sub(Interval::point(actual_sample_height.lo))
                    .hi,
            );
        // The raw sample is within E of its actual realization. Its height
        // change costs |h|E, and its transverse projection costs |h||dual_z|E.
        let seed_error = mul(
            mul(Interval::point(realization_error), height_norm),
            add(vertical_norm, Interval::point(rate)),
        );
        let flow_bound = add(
            Interval::point(prism.approximation),
            add(mul(Interval::point(rate), Interval::point(gap)), seed_error),
        )
        .hi;
        vertical_reverse_bound.min(flow_bound)
    } else {
        vertical_reverse_bound
    };
    bound = up(bound + realization_error).max(reverse_bound);
    let height_error = mul(
        Interval::point(realization_error),
        length(request.frame.z_axis.map(Interval::point))?,
    )
    .hi;
    if down(actual_sample_height.lo - height_error) <= request.height_interval_mm[0]
        || up(actual_sample_height.hi + height_error) >= request.height_interval_mm[1]
    {
        return Err(failure(
            GeometryErrorCode::AnalysisIndeterminate,
            "source realization moves native sample support into a closed cut",
            0,
            actual_sample_height,
        ));
    }
    // Both one-way boundary motion and reverse prism coverage are charged.
    if bound > request.native_shape_error_mm {
        return Err(failure(
            GeometryErrorCode::ShapeErrorExceeded,
            "source realization and native prism exceed shape policy",
            0,
            bound,
        ));
    }
    for (axis, row) in [section.frame.x_axis, section.frame.y_axis]
        .into_iter()
        .enumerate()
    {
        let error = mul(
            Interval::point(realization_error),
            length(row.map(Interval::point))?,
        )
        .hi;
        lower[axis] = down(lower[axis] - error);
        upper[axis] = up(upper[axis] + error);
    }
    let occupied = Some(occupied_projection(section, lower, upper, work, cancel)?);
    Ok(Some(CriticalSlabCertificate {
        boundary_motion_bound_mm: up(bound),
        occupied,
        event_neighborhoods: events,
        capture,
        pairing,
        covered_face_cells,
        covered_coedge_cells,
        regular_flow_speed: regular_flow,
        source_realization_error_mm: realization_error,
        boundary_scope: Some(if whole_source {
            SourceBoundaryScope::WholeSource
        } else {
            SourceBoundaryScope::ClosedCapture(closed_boundary)
        }),
    }))
}

fn occupied_projection(
    section: &NativeSection,
    lower: [f64; 2],
    upper: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<NativeSection, GeometryError> {
    if !lower.into_iter().chain(upper).all(f64::is_finite) {
        return Err(failure(
            GeometryErrorCode::AnalysisIndeterminate,
            "whole-source occupied projection has no active boundary enclosure",
            0,
            lower,
        ));
    }
    work.charge(5, cancel)?;
    let frame = section.frame;
    let scale = lower
        .into_iter()
        .chain(upper)
        .chain(frame.origin_mm)
        .map(f64::abs)
        .fold(1.0_f64, f64::max);
    let gram_error = dot(
        frame.x_axis.map(Interval::point),
        frame.x_axis.map(Interval::point),
    )
    .sub(Interval::point(1.0))
    .abs_upper()
        + dot(
            frame.y_axis.map(Interval::point),
            frame.y_axis.map(Interval::point),
        )
        .sub(Interval::point(1.0))
        .abs_upper()
        + 2.0
            * dot(
                frame.x_axis.map(Interval::point),
                frame.y_axis.map(Interval::point),
            )
            .abs_upper();
    let reserve = up(scale * (gram_error * 8.0 + f64::EPSILON * 64.0));
    let min = lower.map(|v| down(v - reserve));
    let max = upper.map(|v| up(v + reserve));
    let points_mm = [
        [min[0], min[1]],
        [max[0], min[1]],
        [max[0], max[1]],
        [min[0], max[1]],
        [min[0], min[1]],
    ]
    .into_iter()
    .map(|p| {
        std::array::from_fn(|axis| {
            frame.origin_mm[axis] + p[0] * frame.x_axis[axis] + p[1] * frame.y_axis[axis]
        })
    })
    .collect();
    let mut certificate = section.certificate.clone();
    certificate.authority = crate::SectionAuthority::ConservativeSlabEnvelope;
    Ok(NativeSection {
        plane: section.plane,
        frame,
        loops: vec![SectionLoopMm {
            is_hole: false,
            points_mm,
        }],
        loop_source_faces: Vec::new(),
        loop_spans: Vec::new(),
        sampling_tolerance_mm: section.sampling_tolerance_mm,
        boolean_tolerance_mm: section.boolean_tolerance_mm,
        certificate,
    })
}

#[cfg(test)]
mod tests;
