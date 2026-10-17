// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! A sufficient, representation-independent critical-neighborhood proof. Native
//! face charts and oriented coedges form the complex; boxes are only a covering.
//! The closed capture is bounded by original material and actual halfspace cuts.
//! Convexity proves a disk complex, or convex meridian fibers with positive
//! radial degree about a separated represented core prove an annular complex.
//! Whole-source convexity is separate, strictly stronger evidence.

pub(crate) mod material_link;
pub(crate) use material_link::PreparedSourceVertexPlans;
pub(super) use material_link::{ClosedRadialMaterialProduct, RadialRegionKind};
pub(super) mod ray_index;
mod source_chart;

use super::slab::HeightFunctional;
use super::source_incidence::SourceEdgeIncidence;
use super::{GeometryWorkBudget, SlabRequest};
#[cfg(test)]
use crate::kernel::prepare_face_trim_domain_with_byte_limit;
use crate::kernel::{PreparedFaceTrimDomain, TrimClassification};
use crate::parametric::{
    Interval, SurfaceEnclosure, down, surface_crease_interfaces, surface_enclosure,
    surface_parameter_boxes, surface_parameter_span_smooth, up,
};
use crate::section::OriginalTrimSource;
use crate::{Definition, GeometryError, NativeSection, PlaneFrameMm, SectionLoopMm};
use monstertruck_modeling::{BoundedCurve, Curve, Surface};
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
    pub(super) source: OriginalTrimSource<'source, 'domains>,
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
    // Parameter widths have unrelated units. Refine the larger physical
    // variation, not a contracted straight direction merely to alternate axes.
    // This changes scheduling only; every child retains the full proof tests.
    let variation = |derivative: &[Interval; 3], range: [f64; 2]| {
        derivative
            .iter()
            .map(|value| value.abs_upper())
            .fold(0.0_f64, f64::max)
            * (range[1] - range[0])
    };
    let u = variation(&jet.du, uv[0]);
    let v = variation(&jet.dv, uv[1]);
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

/// At a degree-multiplicity knot prove R=mu*L+lambda*H in homogeneous
/// coordinates, with mu strictly positive. Projectivization removes lambda*H;
/// positive transverse reparameterization removes mu. The two regular charts
/// therefore have an exact common tangent plane, without recognizing a family
/// or declaring a small computed derivative jump to be zero.
pub(super) fn interface_regular_join(
    surface: &Surface,
    interface: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    if interface.iter().all(|axis| axis[0] != axis[1]) {
        // A nonrepresentable reflected knot is an actual interval band, not
        // either endpoint masquerading as the original exact source knot.
        return Ok(false);
    }
    let axis = usize::from(interface[1][0] == interface[1][1]);
    let parameter = interface[axis][0];
    let (knots, degree, count, other_count): (&[f64], usize, usize, usize) = match surface {
        Surface::BsplineSurface(s) => {
            if axis == 0 {
                (
                    s.knot_vector_u().as_ref(),
                    s.udegree(),
                    s.control_points().len(),
                    s.control_points()[0].len(),
                )
            } else {
                (
                    s.knot_vector_v().as_ref(),
                    s.vdegree(),
                    s.control_points()[0].len(),
                    s.control_points().len(),
                )
            }
        }
        Surface::NurbsSurface(s) => {
            if axis == 0 {
                (
                    s.knot_vector_u().as_ref(),
                    s.udegree(),
                    s.control_points().len(),
                    s.control_points()[0].len(),
                )
            } else {
                (
                    s.knot_vector_v().as_ref(),
                    s.vdegree(),
                    s.control_points()[0].len(),
                    s.control_points().len(),
                )
            }
        }
        _ => return Ok(false),
    };
    let start = knots.partition_point(|value| *value < parameter);
    let end = knots.partition_point(|value| *value <= parameter);
    if degree == 0 || end - start != degree || start == 0 || end >= knots.len() {
        return Ok(false);
    }
    let Some(shared) = end.checked_sub(degree + 1) else {
        return Ok(false);
    };
    if shared == 0 || shared + 1 >= count {
        return Ok(false);
    }
    let Some(left_span) = ExactCoefficient::difference(parameter, knots[start - 1]) else {
        return Ok(false);
    };
    let Some(right_span) = ExactCoefficient::difference(knots[end], parameter) else {
        return Ok(false);
    };
    let control = |index: usize, other: usize| -> [f64; 4] {
        let (u, v) = if axis == 0 {
            (index, other)
        } else {
            (other, index)
        };
        match surface {
            Surface::BsplineSurface(s) => {
                let p = s.control_point(u, v);
                [p.x, p.y, p.z, 1.0]
            }
            Surface::NurbsSurface(s) => (*s.control_point(u, v)).into(),
            _ => unreachable!(),
        }
    };
    let derivative = |other: usize,
                      coordinate: usize,
                      left: bool,
                      work: &mut GeometryWorkBudget|
     -> Result<Option<ExactCoefficient>, GeometryError> {
        let center = control(shared, other)[coordinate];
        let neighbor = control(if left { shared - 1 } else { shared + 1 }, other)[coordinate];
        let Some(difference) = (if left {
            ExactCoefficient::difference(center, neighbor)
        } else {
            ExactCoefficient::difference(neighbor, center)
        }) else {
            return Ok(None);
        };
        difference.product(if left { &right_span } else { &left_span }, work, cancel)
    };
    coefficient_tangent_join(
        other_count,
        |other| control(shared, other),
        derivative,
        work,
        cancel,
    )
}

/// Coefficientwise equality with CONSTANT positive projective tangent scale.
/// This is a sufficient polynomial identity, not interpolation at control points.
fn coefficient_tangent_join(
    other_count: usize,
    control: impl Fn(usize) -> [f64; 4],
    derivative: impl Fn(
        usize,
        usize,
        bool,
        &mut GeometryWorkBudget,
    ) -> Result<Option<ExactCoefficient>, GeometryError>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let Some(reference) = (0..other_count).find(|&other| control(other)[3] != 0.0) else {
        return Ok(false);
    };
    let h = control(reference);
    let weight = ExactCoefficient::point(h[3]);
    let Some(lw) = derivative(reference, 3, true, work)? else {
        return Ok(false);
    };
    let Some(rw) = derivative(reference, 3, false, work)? else {
        return Ok(false);
    };
    let mut coefficients = None;
    for (coordinate, value) in h.iter().take(3).enumerate() {
        let Some(l) = derivative(reference, coordinate, true, work)? else {
            return Ok(false);
        };
        let Some(r) = derivative(reference, coordinate, false, work)? else {
            return Ok(false);
        };
        let point = ExactCoefficient::point(*value);
        let Some(mut determinant) = l.product(&weight, work, cancel)? else {
            return Ok(false);
        };
        let Some(term) = lw.product(&point, work, cancel)? else {
            return Ok(false);
        };
        if determinant.subtract(&term).is_none() {
            return Ok(false);
        }
        if determinant.sign() == 0 {
            continue;
        }
        let Some(mut mu) = r.product(&weight, work, cancel)? else {
            return Ok(false);
        };
        let Some(term) = rw.product(&point, work, cancel)? else {
            return Ok(false);
        };
        if mu.subtract(&term).is_none() || mu.sign() != determinant.sign() {
            return Ok(false);
        }
        let Some(mut lambda) = l.product(&rw, work, cancel)? else {
            return Ok(false);
        };
        let Some(term) = lw.product(&r, work, cancel)? else {
            return Ok(false);
        };
        if lambda.subtract(&term).is_none() {
            return Ok(false);
        }
        coefficients = Some((determinant, mu, lambda));
        break;
    }
    let Some((determinant, mu, lambda)) = coefficients else {
        return Ok(false);
    };
    for other in 0..other_count {
        work.charge(1, cancel)?;
        let center = control(other);
        for (coordinate, value) in center.iter().enumerate() {
            let Some(l) = derivative(other, coordinate, true, work)? else {
                return Ok(false);
            };
            let Some(r) = derivative(other, coordinate, false, work)? else {
                return Ok(false);
            };
            let Some(mut identity) = determinant.product(&r, work, cancel)? else {
                return Ok(false);
            };
            let Some(first) = mu.product(&l, work, cancel)? else {
                return Ok(false);
            };
            let Some(second) = lambda.product(&ExactCoefficient::point(*value), work, cancel)?
            else {
                return Ok(false);
            };
            if identity.subtract(&first).is_none()
                || identity.subtract(&second).is_none()
                || identity.len != 0
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Prove a closed rational/B-spline chart's end identification by the same
/// exact homogeneous G1 identity used at internal knots. Endpoint rows may have
/// different homogeneous scales; no coordinate tolerance closes a seam.
fn periodic_regular_join(
    surface: &Surface,
    axis: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let (knots, degree, count, other_count): (&[f64], usize, usize, usize) = match surface {
        Surface::BsplineSurface(s) => {
            if axis == 0 {
                (
                    s.knot_vector_u().as_ref(),
                    s.udegree(),
                    s.control_points().len(),
                    s.control_points()[0].len(),
                )
            } else {
                (
                    s.knot_vector_v().as_ref(),
                    s.vdegree(),
                    s.control_points()[0].len(),
                    s.control_points().len(),
                )
            }
        }
        Surface::NurbsSurface(s) => {
            if axis == 0 {
                (
                    s.knot_vector_u().as_ref(),
                    s.udegree(),
                    s.control_points().len(),
                    s.control_points()[0].len(),
                )
            } else {
                (
                    s.knot_vector_v().as_ref(),
                    s.vdegree(),
                    s.control_points()[0].len(),
                    s.control_points().len(),
                )
            }
        }
        _ => return Ok(false),
    };
    if degree == 0
        || count < 3
        || knots.len() <= count
        || knots[..=degree].iter().any(|&k| k != knots[degree])
        || knots[count..].iter().any(|&k| k != knots[count])
    {
        return Ok(false);
    }
    let control = |index: usize, other: usize| -> [f64; 4] {
        let (u, v) = if axis == 0 {
            (index, other)
        } else {
            (other, index)
        };
        match surface {
            Surface::BsplineSurface(s) => {
                let p = s.control_point(u, v);
                [p.x, p.y, p.z, 1.0]
            }
            Surface::NurbsSurface(s) => (*s.control_point(u, v)).into(),
            _ => unreachable!(),
        }
    };
    let Some(reference) = (0..other_count).find(|&other| control(0, other)[3] != 0.0) else {
        return Ok(false);
    };
    let scale = control(count - 1, reference)[3] / control(0, reference)[3];
    if !scale.is_finite() || scale <= 0.0 {
        return Ok(false);
    }
    let scale = ExactCoefficient::point(scale);
    for other in 0..other_count {
        for coordinate in 0..4 {
            let Some(mut identity) = ExactCoefficient::point(control(0, other)[coordinate])
                .product(&scale, work, cancel)?
            else {
                return Ok(false);
            };
            if identity
                .subtract(&ExactCoefficient::point(
                    control(count - 1, other)[coordinate],
                ))
                .is_none()
                || identity.sign() != 0
            {
                return Ok(false);
            }
        }
    }
    let Some(left_span) = ExactCoefficient::difference(knots[count], knots[count - 1]) else {
        return Ok(false);
    };
    let Some(right_span) = ExactCoefficient::difference(knots[degree + 1], knots[degree]) else {
        return Ok(false);
    };
    if left_span.sign() <= 0 || right_span.sign() <= 0 {
        return Ok(false);
    }
    let derivative = |other: usize,
                      coordinate: usize,
                      left: bool,
                      work: &mut GeometryWorkBudget|
     -> Result<Option<ExactCoefficient>, GeometryError> {
        let (a, b) = if left { (count - 1, count - 2) } else { (1, 0) };
        let Some(difference) = ExactCoefficient::difference(
            control(a, other)[coordinate],
            control(b, other)[coordinate],
        ) else {
            return Ok(None);
        };
        let Some(difference) =
            difference.product(if left { &right_span } else { &left_span }, work, cancel)?
        else {
            return Ok(None);
        };
        if left {
            Ok(Some(difference))
        } else {
            difference.product(&scale, work, cancel)
        }
    };
    coefficient_tangent_join(
        other_count,
        |other| control(count - 1, other),
        derivative,
        work,
        cancel,
    )
}

/// Exact homogeneous linear identities survive bounded source trim/pose/sense
/// wrappers. Restriction and reversal do not change a constant coordinate;
/// affine/projective chart coefficients are pulled back without rounding them.
fn chart_linear_identity(
    curve: &monstertruck_modeling::Curve2D,
    coefficients: &[ExactCoefficient; 3],
    depth: u8,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    use monstertruck_modeling::Curve2D;
    work.charge(1, cancel)?;
    if depth >= 16 {
        return Ok(false);
    } // bounded expansion scratch, not a guessed identity
    let point_identity = |point: [f64; 3],
                          work: &mut GeometryWorkBudget|
     -> Result<bool, GeometryError> {
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
    match curve {
        Curve2D::Line(line) => Ok(point_identity([line.0.x, line.0.y, 1.0], work)?
            && point_identity([line.1.x, line.1.y, 1.0], work)?),
        Curve2D::Polyline(curve) => {
            for point in curve.iter() {
                if !point_identity([point.x, point.y, 1.0], work)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        Curve2D::BsplineCurve(curve) => {
            for point in curve.control_points() {
                if !point_identity([point.x, point.y, 1.0], work)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        Curve2D::NurbsCurve(curve) => {
            for point in curve.control_points() {
                if !point_identity([point.x, point.y, point.z], work)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        Curve2D::TrimmedCurve(curve) => {
            let matrix = curve.transform();
            let columns = [matrix.x, matrix.y, matrix.z];
            let mut pulled = std::array::from_fn(|_| ExactCoefficient::point(0.0));
            for axis in 0..3 {
                for row in 0..3 {
                    let Some(product) = coefficients[row].product(
                        &ExactCoefficient::point(columns[axis][row]),
                        work,
                        cancel,
                    )?
                    else {
                        return Ok(false);
                    };
                    for &term in &product.terms[..product.len] {
                        if pulled[axis].push(term).is_none() {
                            return Ok(false);
                        }
                    }
                }
            }
            chart_linear_identity(
                curve.entity().curve().as_ref(),
                &pulled,
                depth + 1,
                work,
                cancel,
            )
        }
        _ => Ok(false),
    }
}

fn spline_seam_join(
    surface: &Surface,
    domain: &PreparedFaceTrimDomain<'_>,
    first: usize,
    second: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    use monstertruck_modeling::Curve2D;
    // A nominal constant coordinate is source authority only without a source
    // chart uncertainty tube. Otherwise the full normal-union proof is required.
    if domain.source_chart_error_mm() != 0.0 {
        return Ok(false);
    }
    let constant = |curve: &Curve2D,
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
        chart_linear_identity(curve, &coefficients, 0, work, cancel)
    };
    for axis in 0..2 {
        let (knots, degree, count): (&[f64], usize, usize) = match surface {
            Surface::BsplineSurface(s) => {
                if axis == 0 {
                    (
                        s.knot_vector_u().as_ref(),
                        s.udegree(),
                        s.control_points().len(),
                    )
                } else {
                    (
                        s.knot_vector_v().as_ref(),
                        s.vdegree(),
                        s.control_points()[0].len(),
                    )
                }
            }
            Surface::NurbsSurface(s) => {
                if axis == 0 {
                    (
                        s.knot_vector_u().as_ref(),
                        s.udegree(),
                        s.control_points().len(),
                    )
                } else {
                    (
                        s.knot_vector_v().as_ref(),
                        s.vdegree(),
                        s.control_points()[0].len(),
                    )
                }
            }
            _ => return Ok(false),
        };
        let a = domain.native().coedges()[first].curve;
        let b = domain.native().coedges()[second].curve;
        let opposite = (constant(a, axis, knots[degree], work)?
            && constant(b, axis, knots[count], work)?)
            || (constant(b, axis, knots[degree], work)? && constant(a, axis, knots[count], work)?);
        if opposite && periodic_regular_join(surface, axis, work, cancel)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// A coefficient-structural smoothness fact, not an operation permission.
/// Arbitrary spline/sweep profiles are intentionally absent: smooth patch
/// interiors do not establish the link of an identified endpoint.
fn intrinsically_smooth_profile(curve: &Curve, depth: u8) -> bool {
    if depth >= 16 {
        return false;
    }
    match curve {
        Curve::Line(_) => true,
        Curve::ParameterCurve(pcurve) => {
            matches!(pcurve.curve(), monstertruck_modeling::Curve2D::Conic(_))
                && matches!(pcurve.surface().as_ref(), Surface::Plane(_))
        }
        Curve::TrimmedCurve(curve) => {
            let matrix = curve.transform();
            matrix.x.w == 0.0
                && matrix.y.w == 0.0
                && matrix.z.w == 0.0
                && matrix.w.w == 1.0
                && intrinsically_smooth_profile(curve.entity().curve().as_ref(), depth + 1)
        }
        _ => false,
    }
}

fn intrinsically_smooth_carrier(surface: &Surface) -> bool {
    match surface {
        Surface::Plane(_) | Surface::SphericalSurface(_) | Surface::ToroidalSurface(_) => true,
        Surface::RevolutionSurface(surface) => {
            intrinsically_smooth_profile(surface.entity().entity_curve(), 0)
        }
        Surface::ExtrusionSurface(surface) => {
            intrinsically_smooth_profile(surface.entity_curve(), 0)
        }
        _ => false,
    }
}

/// One original paired native occurrence link, shared by critical topology and
/// height regularity. This is geometric G1 continuation, not height transversality.
pub(super) fn same_face_source_seam_join(
    surface: &Surface,
    domain: &PreparedFaceTrimDomain<'_>,
    first: usize,
    second: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(1, cancel)?;
    let coedges = domain.native().coedges();
    let (Some(a), Some(b)) = (coedges.get(first), coedges.get(second)) else {
        return Ok(false);
    };
    if first == second
        || a.identity.face_id != b.identity.face_id
        || a.identity.face_id != domain.native().face().id()
        || a.identity.edge_id != b.identity.edge_id
        || a.identity.orientation == b.identity.orientation
    {
        return Ok(false);
    }
    match surface {
        Surface::BsplineSurface(_) | Surface::NurbsSurface(_) => {
            spline_seam_join(surface, domain, first, second, work, cancel)
        }
        _ => Ok(intrinsically_smooth_carrier(surface)),
    }
}

/// This view can only be created while borrowing the ACTUAL owning native face's
/// surface mutex. An unrelated surface cannot be attached to a retained domain.
pub(super) struct OriginalSourceFace<'a, 'd> {
    surface: &'a Surface,
    domain: &'a PreparedFaceTrimDomain<'d>,
    bounds: [[f64; 2]; 2],
}
pub(super) fn with_original_source_face<R>(
    domain: &PreparedFaceTrimDomain<'_>,
    bounds: [[f64; 2]; 2],
    visit: impl FnOnce(OriginalSourceFace<'_, '_>) -> Result<R, GeometryError>,
) -> Result<R, GeometryError> {
    domain.native().face().with_surface(|surface| {
        visit(OriginalSourceFace {
            surface,
            domain,
            bounds,
        })
    })
}
impl<'a, 'd> OriginalSourceFace<'a, 'd> {
    pub fn surface(&self) -> &'a Surface {
        self.surface
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
    surface: &'a Surface,
    domain: &'a PreparedFaceTrimDomain<'d>,
    bounds: [[f64; 2]; 2],
}
impl<'a, 'd> GluedSourceFace<'a, 'd> {
    fn prove(
        surface: &'a Surface,
        domain: &'a PreparedFaceTrimDomain<'d>,
        bounds: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        if !critical_face_gluing(surface, bounds, work, cancel)? {
            return Ok(None);
        }
        Ok(Some(Self {
            surface,
            domain,
            bounds,
        }))
    }
    pub fn surface(&self) -> &Surface {
        self.surface
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
        let mut native = monstertruck_modeling::ValidatedGeometryBudget::new(
            usize::try_from(work.remaining()).unwrap_or(usize::MAX),
        );
        let result = monstertruck_modeling::validated_surface_geometric_normal(
            self.surface,
            uv,
            &mut native,
            cancel,
        );
        crate::parametric::debit_native_work(work, native.consumed(), cancel)?;
        match result {
            Ok(normal) => Ok(normal.map(|value| Interval {
                lo: value.lower,
                hi: value.upper,
            })),
            Err(monstertruck_modeling::ValidatedGeometryError::NonSmooth)
                if one_sided_tensor_joint(self.surface, uv, work, cancel)? =>
            {
                // The positive original coefficient G1 theorem identifies all
                // incident tangent planes. One-sided nonzero native jets supply
                // their common NORMAL DIRECTION, never a fictitious C1 jet.
                Ok(normal(
                    &surface_enclosure(self.surface, uv, work, cancel)?,
                    true,
                ))
            }
            Err(cause) => Err(crate::parametric::native_error(cause)),
        }
    }
}

fn one_sided_tensor_joint(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let axes: [(&[f64], usize, usize); 2] = match surface {
        Surface::BsplineSurface(s) => [
            (
                s.knot_vector_u().as_ref(),
                s.udegree(),
                s.control_points().len(),
            ),
            (
                s.knot_vector_v().as_ref(),
                s.vdegree(),
                s.control_points()[0].len(),
            ),
        ],
        Surface::NurbsSurface(s) => [
            (
                s.knot_vector_u().as_ref(),
                s.udegree(),
                s.control_points().len(),
            ),
            (
                s.knot_vector_v().as_ref(),
                s.vdegree(),
                s.control_points()[0].len(),
            ),
        ],
        _ => return Ok(false),
    };
    let mut joint = false;
    for (axis, (knots, degree, count)) in axes.into_iter().enumerate() {
        let levels = usize::BITS - knots.len().leading_zeros();
        work.charge(4 * u64::from(levels) + 1, cancel)?;
        let (lo, hi) = (uv[axis][0], uv[axis][1]);
        let after = knots.partition_point(|knot| *knot <= lo);
        if knots.get(after).is_some_and(|knot| *knot < hi) {
            return Ok(false); // Never choose one side of a box crossing interiors.
        }
        if lo == hi && knots[degree] < lo && lo < knots[count] {
            let first = knots.partition_point(|knot| *knot < lo);
            joint |= after.saturating_sub(first) >= degree;
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
    surface: &Surface,
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    if surface_parameter_span_smooth(surface, uv, work, cancel)? {
        return Ok(true);
    }
    if !matches!(
        surface,
        Surface::BsplineSurface(_) | Surface::NurbsSurface(_)
    ) {
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
    surface: &Surface,
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
    surface: &Surface,
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
    surface: &Surface,
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
    surface: &Surface,
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
        let center: [f64; 3] = std::array::from_fn(|axis| {
            section.loops[outer].points_mm[..outer_count]
                .iter()
                .map(|point| point[axis] / outer_count as f64)
                .sum()
        });
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
            let b = vertices[next];
            let side = orient(a, b, [Interval::point(0.0); 2]);
            let signed = if inner { side.neg() } else { side };
            let edge_length = length([b[0].sub(a[0]), b[1].sub(a[1]), Interval::point(0.0)])?;
            if signed.lo <= 0.0 || edge_length.hi <= 0.0 {
                return Err(failure(
                    GeometryErrorCode::AnalysisIndeterminate,
                    "actual native loop has no separated radial degree",
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

/// Numeric proposal only; original F membership is checked after releasing the
/// native surface mutex. No borrowed kernel object crosses that boundary.
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
    let face = domain.native().face();
    let source_chart =
        face.with_surface(|surface| source_chart::SphereChart::new(surface, domain, work, cancel))?;
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
            // Partition its ENTIRE position tube at every native surface knot
            // before requesting one-sided jets. A projective G1 join can have
            // a genuine parameter derivative jump, so evaluating a single jet
            // across it would refuse an otherwise exact regular source seam.
            let compatible = face.with_surface(
                |surface| -> Result<Option<PositionNormalEnclosure>, GeometryError> {
                    let mut result: Option<PositionNormalEnclosure> = None;
                    for patch in surface_parameter_boxes(surface, uv, work, cancel)? {
                        let jet = surface_enclosure(surface, patch, work, cancel)?;
                        if (0..3)
                            .any(|axis| intersection(position[axis], jet.position[axis]).is_none())
                        {
                            continue;
                        }
                        let n = match &source_chart {
                            Some(chart) => chart.normal(jet.position)?,
                            None => normal(&jet, face.orientation()),
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
                },
            )?;
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
                    .and_then(|n| {
                        inventory
                            .continuation_spans
                            .checked_mul(2)
                            .and_then(|m| n.checked_add(m))
                    })
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
    if definition.solid.boundaries().len() != 1 {
        return Ok(None);
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
    let mut whole_source = definition.material_transport_is_proved_zero(work, cancel)?;
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
    for (face_index, face) in definition.solid.face_iter().enumerate() {
        let face_before = work.consumed();
        let domain = source.face_domain(face_index, chart_bytes, work, cancel)?;
        (|| -> Result<(), GeometryError> {
            let (source_chart, face_glued) = face.with_surface(|surface| -> Result<_, GeometryError> {
                Ok((
                    source_chart::SphereChart::new(surface, &domain, work, cancel)?,
                    capture_face_gluing(surface, domain.uv_bounds(), functional, request, face_index, work, cancel)?,
                ))
            })?;
            whole_source &= face_glued;
            let mut stack = Vec::with_capacity(usize::from(MAX_DEPTH) + 1);
            let mut spans_seen = 0;
            let mut active_span = domain.uv_bounds();
            let mut propose_negative = true;
            loop {
            let proposal = face.with_surface(|surface| -> Result<Option<SourceFNegativeProposal>, GeometryError> {
            let mut spans = surface_parameter_boxes(surface, domain.uv_bounds(), work, cancel)?
                .skip(spans_seen);
            while let Some((uv, depth, height_preconditioned)) =
                stack.pop().or_else(|| spans.next().map(|uv| {
                    active_span = uv;
                    spans_seen += 1;
                    (uv, 0_u8, false)
                }))
            {
                work.charge(1, cancel)?;
                let span_smooth = depth != 0 || surface_parameter_span_smooth(surface, uv, work, cancel)?;
                let (uv, jet, height_preconditioned) = if !whole_source && !height_preconditioned {
                    let Some((clipped, jet)) = super::slab::clip_height_box(
                        surface, uv, functional, [height_target.lo, height_target.hi], work, cancel,
                    )? else {
                        closed_boundary.excluded_face_cells += 1;
                        continue;
                    };
                    let h = functional.height(jet.position);
                    let preconditioned = clipped != uv
                        || (height_target.lo <= h.lo && h.hi <= height_target.hi);
                    (clipped, jet, preconditioned)
                } else {
                    // The optional whole-source theorem always keeps its full
                    // original domain. Descendants of a contracted box already
                    // inherit that necessary height preimage; do not solve the
                    // same cut with fresh native strip queries at every split.
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
                if !span_smooth {
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
                    None => normal(&jet, face.orientation()),
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
            })?;
            let Some(proposal) = proposal else { break; };
            // Multi-face reference qualification can revisit this face. The
            // native surface guard must already be gone before entering it.
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
                    let point = face.with_surface(|surface|
                        surface_enclosure(surface, point_uv, work, cancel)
                    )?.position;
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
                    chart_bytes.checked_sub(domain.owned_bytes()).ok_or_else(|| failure(
                        GeometryErrorCode::ResourceLimit,
                        "source-F counterexample exceeds coexistent domain capacity",
                        face_index, point_uv,
                    ))?, work, cancel,
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
            chart_bytes.saturating_sub(first_domain.owned_bytes()),
            work,
            cancel,
        )?;
        let first_face = first_domain.native().face();
        let second_face = second_domain.native().face();
        let edge = &first_face.absolute_boundaries()[first.wire][first.index];
        let central_range = material_links.central_native_range(
            edge.id(),
            first_domain
                .original_coedge_correspondence(first.coedge)?
                .native_range(),
            [first.face, first.wire, first.index],
        )?;
        let paired_scratch = chart_bytes
            .checked_sub(first_domain.owned_bytes())
            .and_then(|bytes| bytes.checked_sub(second_domain.owned_bytes()))
            .ok_or_else(|| {
                failure(
                    GeometryErrorCode::ResourceLimit,
                    "paired source ownership exceeds coexistent domain admission",
                    first.face,
                    central_range,
                )
            })?;
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
            let a = first_face.with_surface(|surface| {
                crate::parametric::surface_piecewise_enclosure(
                    surface,
                    first_domain.uv_bounds(),
                    work,
                    cancel,
                )
                .map(|jet| jet.position)
            })?;
            let b = second_face.with_surface(|surface| {
                crate::parametric::surface_piecewise_enclosure(
                    surface,
                    second_domain.uv_bounds(),
                    work,
                    cancel,
                )
                .map(|jet| jet.position)
            })?;
            Some(std::array::from_fn(|axis| {
                a[axis].hull(b[axis]).add(Interval {
                    lo: -source_allowance,
                    hi: source_allowance,
                })
            }))
        } else {
            None
        };
        // Visit before borrowing this edge's curve: the inventory visitor
        // acquires the same original coefficient owner.
        let partition_bytes = chart_bytes
            .checked_sub(
                first_domain
                    .owned_bytes()
                    .saturating_add(second_domain.owned_bytes()),
            )
            .ok_or_else(|| {
                failure(
                    GeometryErrorCode::ResourceLimit,
                    "original coedge domains exhaust partition scratch",
                    first.face,
                    (),
                )
            })?;
        let source_cuts = original_coedge_parameter_cuts(
            [
                (&first_domain, first.coedge),
                (&second_domain, second.coedge),
            ],
            partition_bytes,
            work,
            cancel,
        )?;
        edge.with_curve(|curve| -> Result<(), GeometryError> {
            let range = curve.range_tuple();
            // Events grow while this native owner remains live: reserve their
            // complete bounded capacity, not just the capacity before capture.
            let occupied_scratch = fixed_scratch
                + prism.heap_bytes()
                + first_domain.owned_bytes() + second_domain.owned_bytes()
                + source_cuts.capacity() * std::mem::size_of::<f64>();
            let remaining = scratch_limit.checked_sub(occupied_scratch).ok_or_else(|| failure(
                GeometryErrorCode::ResourceLimit, "critical scratch capacity exhausted before native curve capture", first.face, range))?;
            let prepared = definition.prepare_boundary_curve(edge.id(), curve, range, remaining, work, cancel)?;
            // A C0 original edge can carry a height event even when each
            // smooth side has nonzero derivative. It cannot advertise the
            // complete regular height-flow theorem merely from side bounds.
            if regular_flow.is_some() && !prepared.is_smooth_on(range, work, cancel)? {
                regular_flow = None;
            }
            // Original physical coedge endpoints are zero-dimensional source
            // strata too. Monotone face/edge derivatives do not exclude a
            // polyhedral corner event. Use actual bounded 3D endpoint tubes,
            // never a UV parameter or a nearby stored/sampled vertex.
            for parameter in [range.0, range.1] {
                let height = functional.height(prepared.jet((parameter, parameter), work, cancel)?.position);
                add_neighborhood(&mut events, &capture, NeighborhoodInput {
                    face: Some(first.face),
                    height,
                    root_proved: false,
                    pairing,
                }, work, cancel)?;
            }
            for span in prepared.parameter_intervals() {
                let span = (span.0.max(central_range[0]), span.1.min(central_range[1]));
                if span.0 > span.1 { continue; }
                let mut stack = Vec::with_capacity(usize::from(MAX_DEPTH) + 1);
                let first_cut = source_cuts.partition_point(|cut| *cut <= span.0);
                let last_cut = source_cuts.partition_point(|cut| *cut < span.1).max(first_cut);
                let mut segment_start = span.0;
                for segment_end in source_cuts[first_cut..last_cut].iter().copied()
                    .chain(std::iter::once(span.1))
                {
                    stack.push(((segment_start, segment_end), 0_u8));
                    segment_start = segment_end;
                while let Some(((lo, hi), depth)) = stack.pop() {
                    work.charge(1, cancel)?;
                    let jet = prepared.jet((lo, hi), work, cancel)?;
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
                    // isolate a true critical root of the ACTUAL native 3D edge.
                    // Source UV derivative estimates play no role here.
                    if !second_derivative.contains_zero() && prepared.is_smooth_on((lo, hi), work, cancel)? {
                        let mid = lo * 0.5 + hi * 0.5;
                        let value = functional.dot(prepared.jet((mid, mid), work, cancel)?.derivative);
                        let newton = Interval::point(mid).sub(value.div(second_derivative)?);
                        if let Some(root) = intersection(Interval { lo, hi }, newton) {
                            let a = functional.dot(prepared.jet((lo, lo), work, cancel)?.derivative);
                            let b = functional.dot(prepared.jet((hi, hi), work, cancel)?.derivative);
                            root_proved = strictly_contains(Interval { lo, hi }, newton)
                                && ((a.hi < 0.0 && b.lo > 0.0) || (a.lo > 0.0 && b.hi < 0.0));
                            neighborhood = functional.height(prepared.jet((root.lo, root.hi), work, cancel)?.position);
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
        sampling_tolerance_mm: section.sampling_tolerance_mm,
        boolean_tolerance_mm: section.boolean_tolerance_mm,
        certificate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use monstertruck_modeling::{Plane, Point3};

    #[test]
    fn material_height_flow_uses_the_actual_oblique_dual_and_strict_submersion() {
        let height = [0.0, 0.0, 2.0].map(Interval::point);
        let vertical = [0.25, 0.0, 0.5].map(Interval::point);
        let face = material_face_flow_rate([1.0, 0.0, 0.0].map(Interval::point), height, vertical)
            .unwrap()
            .unwrap();
        assert!(face >= 0.25 && face < 0.250000000001);
        let edge = material_edge_flow_rate([0.0, 0.0, 4.0].map(Interval::point), height, vertical)
            .unwrap()
            .unwrap();
        assert!(edge >= 0.25 && edge < 0.250000000001);
        assert!(
            material_face_flow_rate(height, height, vertical)
                .unwrap()
                .is_none()
        );
        assert!(
            material_edge_flow_rate([1.0, 0.0, 0.0].map(Interval::point), height, vertical)
                .unwrap()
                .is_none()
        );
        // A genuinely almost-critical face has a large rate, never a clamped
        // horizontal normal, zero source correction, or exact invariance.
        let steep = material_face_flow_rate(
            [1.0e-12, 0.0, 1.0].map(Interval::point),
            [0.0, 0.0, 1.0].map(Interval::point),
            [0.0, 0.0, 1.0].map(Interval::point),
        )
        .unwrap()
        .unwrap();
        assert!(steep >= 1.0e12);
    }

    fn annular_metric_candidate() -> SamplePrism {
        let request = horizontal_request([-0.1, 0.1], 0.0);
        let dual = [
            [1.0, 0.0, 0.0].map(Interval::point),
            [0.0, 1.0, 0.0].map(Interval::point),
            [0.0, 0.0, 1.0].map(Interval::point),
        ];
        let vertices = [
            [-10.0, -10.0],
            [10.0, -10.0],
            [10.0, 10.0],
            [-10.0, 10.0],
            [-3.0, 0.0],
            [0.0, 3.0],
            [3.0, 0.0],
            [0.0, -3.0],
        ]
        .into_iter()
        .map(|p| p.map(Interval::point))
        .collect::<Vec<_>>();
        let index = SamplePrism::ray_index(
            &vertices,
            4,
            MAX_SAMPLE_BYTES,
            &mut GeometryWorkBudget::new(100_000),
            &AtomicBool::new(false),
        )
        .unwrap();
        SamplePrism {
            vertices,
            ray_index: Some(index),
            outer_count: 4,
            center: [0.0; 3],
            frame: request.frame,
            dual,
            height_dual: dual,
            height_vertical_norm: 1.0,
            core_radius: Some(6.0),
            convex_disk: false,
            inner_radius_upper: 3.0,
            range: request.height_interval_mm,
            source_height: HeightFunctional::from_frame(request.frame),
            approximation: 0.0,
            vertical_return: 0.1,
        }
    }

    #[test]
    fn convex_box_distance_allows_different_actual_boundary_witnesses() {
        let mut prism = annular_metric_candidate();
        prism.ray_index = None;
        prism.core_radius = None;
        prism.vertices = [[4.0, 0.0], [0.0, 4.0], [-4.0, 0.0], [0.0, -4.0]]
            .into_iter()
            .map(|p| p.map(Interval::point))
            .collect();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        prism.convex_disk =
            SamplePrism::convex_radial_polygon(&prism.vertices, &mut work, &cancel).unwrap();
        let bound = prism
            .boundary_distance(
                [[3.8, 4.01], [-0.2, 0.2], [-0.02, 0.02]].map(|[lo, hi]| Interval { lo, hi }),
                0.2,
                &mut work,
                &cancel,
            )
            .unwrap();
        // The two exterior corners use different sides of the actual diamond.
        let actual_exterior_distance = (4.01 + 0.2 - 4.0) / 2.0_f64.sqrt();
        assert!(bound >= actual_exterior_distance && bound < 0.15, "{bound}");
    }

    #[test]
    fn convex_box_distance_keeps_cap_bound_when_all_corners_are_on_boundary() {
        let mut prism = annular_metric_candidate();
        prism.ray_index = None;
        prism.core_radius = None;
        prism.vertices.truncate(prism.outer_count);
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        prism.convex_disk =
            SamplePrism::convex_radial_polygon(&prism.vertices, &mut work, &cancel).unwrap();
        let bound = prism
            .boundary_distance(
                [[-10.0, 10.0], [-10.0, 10.0], [-0.1, 0.1]].map(|[lo, hi]| Interval { lo, hi }),
                0.15,
                &mut work,
                &cancel,
            )
            .unwrap();
        // Every corner has zero boundary distance, but the center is 0.1 mm
        // from either cap. Filled-set convexity alone would miss this maximum.
        assert!(bound >= 0.1 && bound < 0.101, "{bound}");
    }

    #[test]
    fn convex_radial_predicate_rejects_twice_wound_positive_turn_cycle() {
        let vertices = [[4.0, 0.0], [0.0, 4.0], [-4.0, 0.0], [0.0, -4.0]];
        let twice = vertices
            .iter()
            .cycle()
            .take(8)
            .map(|p| p.map(Interval::point))
            .collect::<Vec<_>>();
        assert!(
            !SamplePrism::convex_radial_polygon(
                &twice,
                &mut GeometryWorkBudget::new(100),
                &AtomicBool::new(false),
            )
            .unwrap()
        );
    }

    #[test]
    fn convex_disk_metric_never_assigns_cap_witnesses_to_exterior_angles() {
        let mut prism = annular_metric_candidate();
        prism.ray_index = None;
        prism.core_radius = None;
        prism.outer_count = 8;
        prism.vertices = (0..prism.outer_count)
            .map(|index| {
                let angle = std::f64::consts::TAU * index as f64 / prism.outer_count as f64;
                [angle.cos(), angle.sin()].map(Interval::point)
            })
            .collect();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        prism.convex_disk =
            SamplePrism::convex_radial_polygon(&prism.vertices, &mut work, &cancel).unwrap();
        for index in 0..64 {
            // Includes the angular wrap, original vertices and the reference
            // ray: ambiguous closed comparisons must retain the full scan.
            let angle = std::f64::consts::TAU * index as f64 / 64.0;
            let exterior = prism
                .boundary_distance(
                    [1.5 * angle.cos(), 1.5 * angle.sin(), 0.0].map(Interval::point),
                    0.15,
                    &mut work,
                    &cancel,
                )
                .unwrap();
            // The entire polygon lies in the unit disk, independently of
            // which finite polygon side supplies the returned upper bound.
            assert!(
                exterior.is_finite() && exterior >= 0.49,
                "{index}: {exterior}"
            );
            let interior = prism
                .boundary_distance(
                    [0.25 * angle.cos(), 0.25 * angle.sin(), 0.0].map(Interval::point),
                    0.15,
                    &mut work,
                    &cancel,
                )
                .unwrap();
            assert!(interior >= 0.1 && interior < 0.101, "{index}: {interior}");
        }
    }

    #[test]
    fn disk_side_witness_covers_complete_segment_and_clamps_endpoints() {
        let mut prism = annular_metric_candidate();
        prism.ray_index = None;
        prism.vertices.truncate(prism.outer_count);
        prism.core_radius = None;
        let mut work = GeometryWorkBudget::new(100_000);
        let cancel = AtomicBool::new(false);
        let along_side = prism
            .boundary_distance(
                [
                    Interval::point(10.0),
                    Interval { lo: -9.0, hi: 9.0 },
                    Interval::point(0.0),
                ],
                0.01,
                &mut work,
                &cancel,
            )
            .unwrap();
        assert!(
            along_side <= 0.01,
            "the entire box is on one actual side: {along_side}"
        );
        let over_endpoint = prism
            .boundary_distance(
                [
                    Interval::point(10.0),
                    Interval { lo: 9.9, hi: 10.1 },
                    Interval::point(0.0),
                ],
                0.2,
                &mut work,
                &cancel,
            )
            .unwrap();
        assert!(over_endpoint >= 0.09999 && over_endpoint < 0.10001);
    }

    #[test]
    fn disk_side_witness_preserves_closed_height_and_sampling_reserves() {
        let mut prism = annular_metric_candidate();
        prism.ray_index = None;
        prism.vertices.truncate(prism.outer_count);
        prism.core_radius = None;
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let near_cut = prism
            .boundary_distance(
                [10.0, -10.0, 0.100001].map(Interval::point),
                0.01,
                &mut work,
                &cancel,
            )
            .unwrap();
        assert!(near_cut >= 0.999e-6 && near_cut < 1.1e-6);
        let outside = prism
            .boundary_distance(
                [10.0, -10.0, 0.3].map(Interval::point),
                0.01,
                &mut work,
                &cancel,
            )
            .unwrap();
        assert!(outside >= 0.19999 && outside < 0.20001);
        prism.approximation = 0.004;
        let reserved = prism
            .boundary_distance(
                [10.0, 0.0, 0.0].map(Interval::point),
                0.01,
                &mut work,
                &cancel,
            )
            .unwrap();
        assert!(reserved >= 0.012 && reserved < 0.012001);
    }

    #[test]
    fn annular_ray_edge_functional_contracts_at_square_corner_and_closed_cut() {
        let prism = annular_metric_candidate();
        let mut work = GeometryWorkBudget::new(100_000);
        let cancel = AtomicBool::new(false);
        let distance = prism
            .boundary_distance(
                [10.0, -10.0, 0.100001].map(Interval::point),
                0.0,
                &mut work,
                &cancel,
            )
            .unwrap();
        assert!(
            distance < 1.1e-6,
            "same-ray edge witness must contract: {distance}"
        );
        let outside = prism
            .boundary_distance(
                [10.0, -10.0, 0.3].map(Interval::point),
                0.0,
                &mut work,
                &cancel,
            )
            .unwrap();
        assert!(
            outside >= 0.19999 && outside < 0.20001,
            "the side witness must account the actual closed-cut height: {outside}"
        );
        let unresolved = prism
            .boundary_distance(
                [
                    Interval { lo: -2.0, hi: 20.0 },
                    Interval {
                        lo: -20.0,
                        hi: -2.0,
                    },
                    Interval::point(0.0),
                ],
                0.0,
                &mut work,
                &cancel,
            )
            .unwrap();
        assert!(
            unresolved.is_finite() && unresolved > 1.0,
            "an unresolved denominator retains the complete broad fallback"
        );
    }

    #[test]
    fn prism_closed_cut_witness_uses_the_oblique_height_dual() {
        let mut prism = annular_metric_candidate();
        prism.frame.x_axis = [1.0, 0.0, 0.25];
        prism.dual[2] = [-0.25, 0.0, 1.0].map(Interval::point);
        prism.height_dual = prism.dual;
        prism.height_vertical_norm = length(prism.height_dual[2]).unwrap().hi;
        let z = 0.100001;
        let distance = prism
            .boundary_distance(
                [-10.0 - 0.25 * z, -10.0, z].map(Interval::point),
                0.0,
                &mut GeometryWorkBudget::new(100_000),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert!(
            distance > 1.02e-6 && distance < 1.1e-6,
            "a normal-only cap shift is not inside this oblique corner: {distance}"
        );
    }

    #[test]
    fn annular_cap_witness_uses_actual_hole_not_its_enclosing_disk() {
        let prism = annular_metric_candidate();
        let cap_z = prism.range[0];
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        // Outside the actual diamond hole, but inside its enclosing disk.
        let cap = prism
            .boundary_distance(
                [2.0, 2.0, cap_z].map(Interval::point),
                0.0,
                &mut work,
                &cancel,
            )
            .unwrap();
        assert!(
            cap < 1e-10,
            "actual closed cap must remain available: {cap}"
        );
        let hole = prism
            .boundary_distance(
                [1.0, 1.0, cap_z].map(Interval::point),
                0.0,
                &mut work,
                &cancel,
            )
            .unwrap();
        assert!(
            hole > 0.1,
            "a hole point must not acquire a cap witness: {hole}"
        );
    }

    fn tilted_frame() -> PlaneFrameMm {
        PlaneFrameMm {
            origin_mm: [0.0; 3],
            x_axis: [0.8, 0.0, -0.6],
            y_axis: [0.0, 1.0, 0.0],
            z_axis: [0.6, 0.0, 0.8],
        }
    }
    fn source(name: &str) -> Definition {
        let (_, solid, _) = crate::parity_fixtures::recipes()
            .into_iter()
            .find(|(candidate, _, _)| *candidate == name)
            .unwrap();
        let text = crate::parity_fixtures::original(name, solid);
        crate::import_step(text.as_bytes(), &AtomicBool::new(false)).unwrap()
    }
    #[test]
    fn native_rational_and_polynomial_caps_share_the_critical_complex() {
        let cancel = AtomicBool::new(false);
        for name in [
            "native-parity-bspline-box.step",
            "native-parity-nurbs-box.step",
        ] {
            let definition = source(name);
            let mut work = GeometryWorkBudget::new(30_000_000);
            let prepared = crate::prepare_material_source(
                &definition,
                crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
                &cancel,
                &mut work,
            )
            .unwrap();
            // The bottom-right edge changes source face incidence at height 12.
            // Both encodings take the same oriented patch/trim construction.
            let result = prepared
                .analyze_slab(
                    SlabRequest {
                        frame: tilted_frame(),
                        height_interval_mm: [11.98, 12.02],
                        sample_height_mm: 12.001,
                        native_shape_error_mm: 0.1,
                    },
                    crate::MAX_PREPARED_SECTION_BYTES.saturating_sub(prepared.retained_bytes()),
                    &mut work,
                    &cancel,
                )
                .unwrap();
            assert_eq!(result.section.loops.len(), 1);
            assert!(
                result
                    .critical
                    .as_ref()
                    .is_some_and(CriticalSlabCertificate::has_native_convex_section_topology_proof)
            );
            assert!(
                !result.event_neighborhoods().is_empty(),
                "physical corner strata remain in the critical cover"
            );
            assert!(result.boundary_motion_bound_mm <= 0.1);
            assert!(
                result
                    .occupied
                    .as_ref()
                    .is_some_and(|s| !s.is_native_intersection())
            );
        }
    }
    #[test]
    fn imported_rational_torus_has_generic_terminal_metric_and_cycle_incidence() {
        let cancel = AtomicBool::new(false);
        let definition = source("native-parity-nurbs-torus.step");
        let mut work = GeometryWorkBudget::new(100_000_000);
        let prepared = crate::prepare_material_source(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &cancel,
            &mut work,
        )
        .unwrap();
        let frame = PlaneFrameMm {
            origin_mm: [0.0; 3],
            x_axis: [1.0, 0.0, 0.0],
            y_axis: [0.0, 1.0, 0.0],
            z_axis: [0.0, 0.0, 1.0],
        };
        let request = SlabRequest {
            frame,
            height_interval_mm: [1.999, 2.001],
            sample_height_mm: 1.99901,
            native_shape_error_mm: 0.05,
        };
        let result = prepared
            .analyze_slab(
                request,
                crate::MAX_PREPARED_SECTION_BYTES.saturating_sub(prepared.retained_bytes()),
                &mut work,
                &cancel,
            )
            .unwrap();
        assert_eq!(result.section.loops.len(), 2);
        assert_eq!(result.critical_material_component_count(), Some(1));
        assert_eq!(result.critical_bounded_void_count(), Some(0));
        assert_eq!(result.critical_tunnel_cycle_count(), Some(1));
        assert!(
            !result
                .critical
                .as_ref()
                .unwrap()
                .has_native_convex_section_topology_proof(),
            "fiberwise convex toroidal meridians are not a globally convex original source"
        );
        assert!(!result.event_neighborhoods().is_empty());
        assert!(result.boundary_motion_bound_mm <= request.native_shape_error_mm);
        assert_eq!(result.request().sample_height_mm, request.sample_height_mm);
        assert!(
            result
                .occupied
                .as_ref()
                .is_some_and(|s| !s.is_native_intersection())
        );
    }
    #[test]
    fn source_normal_matching_uses_independent_uv_and_native_edge_parameters() {
        let cancel = AtomicBool::new(false);
        let definition = source("native-parity-nurbs-torus-independent-parameters.step");
        let mut work = GeometryWorkBudget::new(10_000_000);
        let domain = prepare_face_trim_domain_with_byte_limit(
            &definition,
            0,
            MAX_SCRATCH_BYTES / 2,
            &mut work,
            &cancel,
        )
        .unwrap();
        let face = domain.native().face();
        for (index, coedge) in domain.native().coedges().iter().enumerate() {
            let edge = &face.absolute_boundaries()[coedge.identity.wire_index]
                [coedge.identity.coedge_index];
            edge.with_curve(|curve| {
                let range = curve.range_tuple();
                // Source pcurves retain [0,2]/[2,4]; spatial source leaders
                // retain [10,14]/[14,18]. No numeric parameter is shared.
                assert!(range.0 >= 10.0);
                assert!(coedge.oriented_range.into_iter().all(|value| value <= 4.0));
                let prepared = definition
                    .prepare_boundary_curve(
                        edge.id(),
                        curve,
                        range,
                        MAX_SCRATCH_BYTES / 2,
                        &mut work,
                        &cancel,
                    )
                    .unwrap();
                let (lo, hi) = prepared.parameter_intervals().next().unwrap();
                let t = lo + (hi - lo) * 0.37;
                let child = prepared
                    .jet((t, t + (hi - lo) * 0.005), &mut work, &cancel)
                    .unwrap();
                let n = coedge_normal_enclosure(&domain, index, child.position, &mut work, &cancel)
                    .unwrap();
                assert!(length(n).unwrap().lo > 0.0);
                let p = child.position.map(Interval::mid);
                let rho = p[0].hypot(p[1]);
                let outward = [p[0] * (rho - 6.0) / rho, p[1] * (rho - 6.0) / rho, p[2]];
                assert!(dot(n, outward.map(Interval::point)).lo > 0.0);
            });
            let complete = coedge_normal_enclosure(
                &domain,
                index,
                [Interval {
                    lo: -10.0,
                    hi: 10.0,
                }; 3],
                &mut work,
                &cancel,
            )
            .unwrap();
            assert!(
                complete
                    .into_iter()
                    .any(|axis| axis.lo < 0.0 && axis.hi > 0.0),
                "all position-compatible portions of the source half-cycle must survive"
            );
            let disjoint = coedge_normal_enclosure(
                &domain,
                index,
                [Interval {
                    lo: 100.0,
                    hi: 101.0,
                }; 3],
                &mut work,
                &cancel,
            )
            .unwrap_err();
            assert_eq!(
                disjoint.code,
                GeometryErrorCode::ParameterCorrespondenceUnavailable
            );
        }
    }
    #[test]
    fn imported_exact_tensor_proves_internal_and_periodic_projective_g1_identities() {
        let cancel = AtomicBool::new(false);
        let definition = source("native-parity-nurbs-torus.step");
        let mut work = GeometryWorkBudget::new(10_000_000);
        let domain = prepare_face_trim_domain_with_byte_limit(
            &definition,
            0,
            MAX_SCRATCH_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        domain.native().face().with_surface(|surface| {
            assert!(matches!(surface, Surface::NurbsSurface(_)));
            for axis in 0..2 {
                assert!(periodic_regular_join(surface, axis, &mut work, &cancel).unwrap());
            }
            let mut joins = 0;
            let source_domain = domain.certified_uv_domain().expect("independently proved original source chart domain");
            assert!(critical_face_gluing(surface, source_domain, &mut work, &cancel).unwrap(),
                "the complete exact rational torus is admitted by coefficient identities, not a knot-count shortcut");
            for interface in surface_crease_interfaces(surface, source_domain, &mut work, &cancel).unwrap() {
                assert!(interface_regular_join(surface, interface, &mut work, &cancel).unwrap());
                let fixed_axis = usize::from(interface[1][0] == interface[1][1]);
                let mut uncertain_band = interface;
                uncertain_band[fixed_axis] = [down(interface[fixed_axis][0]), up(interface[fixed_axis][1])];
                assert!(!interface_regular_join(surface, uncertain_band, &mut work, &cancel).unwrap(),
                    "a reflected knot enclosure is not either nominal endpoint");
                joins += 1;
            }
            // Three internal knot lines in each axis, each split into four
            // one-sided spans by the other axis's original knot partition.
            assert_eq!(joins, 24);
        });
    }
    #[test]
    fn original_one_degree_tilted_cylinder_retains_its_terminal_slab() {
        let cancel = AtomicBool::new(false);
        let definition = crate::import_step(
            include_bytes!(
                "../../../../fixtures/geometry/independent-exporters/occt-cylinder-mm.step"
            ),
            &cancel,
        )
        .unwrap();
        let angle = std::f64::consts::PI / 180.0;
        let (sine, cosine) = angle.sin_cos();
        let frame = PlaneFrameMm {
            origin_mm: [0.0; 3],
            x_axis: [cosine, 0.0, -sine],
            y_axis: [0.0, 1.0, 0.0],
            z_axis: [sine, 0.0, cosine],
        };
        let terminal = 8.0 * cosine + 5.0 * sine;
        let mut work = GeometryWorkBudget::new(100_000_000);
        let prepared = crate::prepare_material_source(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &cancel,
            &mut work,
        )
        .unwrap();
        let request = SlabRequest {
            frame,
            height_interval_mm: [terminal - 0.001, terminal + 0.001],
            sample_height_mm: terminal - 0.00099,
            native_shape_error_mm: 0.02,
        };
        let result = prepared
            .analyze_slab(
                request,
                crate::MAX_PREPARED_SECTION_BYTES.saturating_sub(prepared.retained_bytes()),
                &mut work,
                &cancel,
            )
            .unwrap();
        assert_eq!(result.critical_material_component_count(), Some(1));
        assert_eq!(result.critical_tunnel_cycle_count(), Some(0));
        assert!(
            !result
                .critical
                .as_ref()
                .unwrap()
                .has_native_convex_section_topology_proof(),
            "bounded material transport is not an exact whole-source convexity theorem"
        );
        assert!(result.boundary_motion_bound_mm <= 0.02);
        assert!(!result.event_neighborhoods().is_empty());
        assert_eq!(result.request().sample_height_mm, request.sample_height_mm);
    }
    #[test]
    fn trimmed_uv_pose_and_sense_preserve_only_exact_chart_linear_identities() {
        use monstertruck_modeling::{
            Curve2D, Invertible, Line, Matrix3, Point2, Processor, TrimmedCurve,
        };
        let basis = Curve2D::Line(Line(Point2::new(0.0, 1.0), Point2::new(0.0, 2.0)));
        let mut wrapped = Processor::with_transform(
            TrimmedCurve::new(Box::new(basis), (0.2, 0.8)),
            Matrix3::new(0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 4.0, -3.0, 1.0),
        );
        wrapped.invert();
        let curve = Curve2D::TrimmedCurve(wrapped);
        let coefficients = [0.0, 1.0, 3.0].map(ExactCoefficient::point);
        let mut work = GeometryWorkBudget::new(10_000);
        let cancel = AtomicBool::new(false);
        assert!(chart_linear_identity(&curve, &coefficients, 0, &mut work, &cancel).unwrap());
        let not_identical = [0.0, 1.0, 3.0 + f64::EPSILON * 2.0].map(ExactCoefficient::point);
        assert!(!chart_linear_identity(&curve, &not_identical, 0, &mut work, &cancel).unwrap());
    }
    #[test]
    fn diagonal_stationary_cover_retains_a_whole_critical_line_and_excludes_nonroots() {
        use monstertruck_modeling::{
            BsplineCurve, BsplineSurface, ExtrusionSurface, KnotVector, Vector3,
        };
        // Equivalent native encodings of z=(2u-1)^2 retain the complete
        // stationary line u=1/2 even though the full Hessian is singular.
        let surface = Surface::BsplineSurface(BsplineSurface::new(
            (KnotVector::bezier_knot(2), KnotVector::bezier_knot(1)),
            [0.0, 0.5, 1.0]
                .into_iter()
                .zip([1.0, -1.0, 1.0])
                .map(|(x, z)| {
                    [-1000.0, 1000.0]
                        .into_iter()
                        .map(|y| Point3::new(x, y, z))
                        .collect()
                })
                .collect(),
        ));
        let height = HeightFunctional {
            direction: [0.0, 0.0, 1.0].map(Interval::point),
            offset: Interval::point(0.0),
        };
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let uv = [[0.0, 0.6], [0.0, 1.0]];
        let jet = surface_enclosure(&surface, uv, &mut work, &cancel).unwrap();
        let root = necessary_stationary_domain(&surface, uv, &jet, height, &mut work, &cancel)
            .unwrap()
            .expect("the whole original critical line remains");
        assert!(root[0][0] <= 0.5 && 0.5 <= root[0][1]);
        assert!(root[0][0] >= uv[0][0] && root[0][1] <= uv[0][1]);
        assert_eq!(
            root[1], uv[1],
            "a necessary cover never drops part of the critical line"
        );
        // Tensor interval dependency may prevent contraction; that is not
        // evidence of exclusion. The direct profile supplies a sharp diagonal.
        let surface = Surface::ExtrusionSurface(ExtrusionSurface::by_extrusion(
            Curve::BsplineCurve(BsplineCurve::new(
                KnotVector::bezier_knot(2),
                vec![
                    Point3::new(0.0, -1000.0, 1.0),
                    Point3::new(0.5, -1000.0, -1.0),
                    Point3::new(1.0, -1000.0, 1.0),
                ],
            )),
            Vector3::new(0.0, 2000.0, 0.0),
        ));
        let jet = surface_enclosure(&surface, uv, &mut work, &cancel).unwrap();
        assert!(!height.dot(jet.duu).contains_zero());
        let root = necessary_stationary_domain(&surface, uv, &jet, height, &mut work, &cancel)
            .unwrap()
            .expect("the same original critical line remains");
        assert!(root[0][0] <= 0.5 && 0.5 <= root[0][1] && root[0][1] - root[0][0] < 1e-8);
        assert_eq!(root[1], uv[1], "constant-height generator stays complete");
        let critical_height = height.height(
            surface_enclosure(&surface, root, &mut work, &cancel)
                .unwrap()
                .position,
        );
        assert!(critical_height.contains_zero() && critical_height.width() < 1e-8);
        let nonroot = [[0.0, 0.2], [0.0, 1.0]];
        let jet = surface_enclosure(&surface, nonroot, &mut work, &cancel).unwrap();
        assert!(
            necessary_stationary_domain(&surface, nonroot, &jet, height, &mut work, &cancel,)
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn mixed_stationary_cover_retains_true_saddles_and_original_pole_corners() {
        use monstertruck_modeling::{BsplineSurface, KnotVector, Matrix4, Processor, Sphere};
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let height = HeightFunctional {
            direction: [0.0, 0.0, 1.0].map(Interval::point),
            offset: Interval::point(0.0),
        };
        // h=8+(u-1/2)(v-1/2) has a genuine interior saddle, with
        // zero diagonal Hessians. Its nonsingular mixed entry supplies
        // necessary contraction, while Krawczyk separately proves existence.
        let saddle = Surface::BsplineSurface(BsplineSurface::new(
            (KnotVector::bezier_knot(1), KnotVector::bezier_knot(1)),
            vec![
                vec![Point3::new(0.0, 0.0, 8.25), Point3::new(0.0, 1.0, 7.75)],
                vec![Point3::new(1.0, 0.0, 7.75), Point3::new(1.0, 1.0, 8.25)],
            ],
        ));
        let uv = [[0.4, 0.6]; 2];
        let jet = surface_enclosure(&saddle, uv, &mut work, &cancel).unwrap();
        let root = necessary_stationary_domain(&saddle, uv, &jet, height, &mut work, &cancel)
            .unwrap()
            .expect("the true saddle remains covered");
        assert!(
            root.iter()
                .all(|axis| { axis[0] <= 0.5 && 0.5 <= axis[1] && axis[1] - axis[0] < 1e-10 })
        );
        let proved = proved_stationary_height(&saddle, uv, &jet, height, &mut work, &cancel)
            .unwrap()
            .expect("independent inclusion proves the interior saddle");
        assert!(proved.lo <= 8.0 && 8.0 <= proved.hi);
        let nonroot = [[0.6, 0.9], [0.4, 0.6]];
        let jet = surface_enclosure(&saddle, nonroot, &mut work, &cancel).unwrap();
        assert!(
            necessary_stationary_domain(&saddle, nonroot, &jet, height, &mut work, &cancel)
                .unwrap()
                .is_none()
        );

        // A horizontal-axis sphere gives h=8+2 sin(u)sin(v). The original
        // closed chart corner u=v=0 is retained; its long longitude strip
        // must not emit the height of the entire near-pole search cell.
        let pole = Surface::SphericalSurface(Processor::with_transform(
            Sphere::new(Point3::new(0.0, 0.0, 0.0), 2.0),
            Matrix4::new(
                1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 8.0, 1.0,
            ),
        ));
        let uv = [[0.0, 4e-7], [0.0, std::f64::consts::FRAC_PI_4]];
        let jet = surface_enclosure(&pole, uv, &mut work, &cancel).unwrap();
        assert!(height.dot(jet.duu).contains_zero() && height.dot(jet.dvv).contains_zero());
        assert!(!height.dot(jet.duv).contains_zero());
        assert!(height.height(jet.position).width() > 1e-7);
        let root = necessary_stationary_domain(&pole, uv, &jet, height, &mut work, &cancel)
            .unwrap()
            .expect("the original pole corner remains covered");
        for axis in 0..2 {
            assert_eq!(root[axis][0], 0.0);
            assert!(root[axis][1] < 1e-12 && root[axis][1] <= uv[axis][1]);
        }
        let bound = height.height(
            surface_enclosure(&pole, root, &mut work, &cancel)
                .unwrap()
                .position,
        );
        assert!(bound.lo <= 8.0 && 8.0 <= bound.hi && bound.width() < 1e-12);
    }
    #[test]
    fn a_piecewise_hessian_cannot_exclude_a_true_source_knot_cusp() {
        use monstertruck_modeling::{BsplineSurface, KnotVector};
        // Actual native height is u²-|u|+v². Its smooth-piece Hessians are
        // 2I, yet its gradient jumps at u=0. Newton from the right derivative
        // would jump outside this box and falsely exclude the source cusp.
        let xs = [-1.0, -0.5, 0.0, 0.5, 1.0];
        let hu = [0.0, -0.5, 0.0, -0.5, 0.0];
        let ys = [-1.0, 0.0, 1.0];
        let hv = [1.0, -1.0, 1.0];
        let controls = xs
            .into_iter()
            .enumerate()
            .map(|(i, x)| {
                ys.into_iter()
                    .enumerate()
                    .map(|(j, y)| Point3::new(x, y, hu[i] + hv[j]))
                    .collect()
            })
            .collect();
        let surface = Surface::BsplineSurface(BsplineSurface::new(
            (
                KnotVector::from(vec![-1.0, -1.0, -1.0, 0.0, 0.0, 1.0, 1.0, 1.0]),
                KnotVector::from(vec![-1.0, -1.0, -1.0, 1.0, 1.0, 1.0]),
            ),
            controls,
        ));
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        assert!(!critical_face_gluing(&surface, [[-1.0, 1.0]; 2], &mut work, &cancel).unwrap());
        let uv = [[-0.125, 0.125]; 2];
        let jet = crate::parametric::surface_piecewise_enclosure(&surface, uv, &mut work, &cancel)
            .unwrap();
        let height = HeightFunctional {
            direction: [0.0, 0.0, 1.0].map(Interval::point),
            offset: Interval::point(0.0),
        };
        assert!(matches!(
            stationary_root(&surface, uv, &jet, height, &mut work, &cancel).unwrap(),
            RootIsolation::ConvexCriticalCover
        ));
        assert_eq!(
            necessary_stationary_domain(&surface, uv, &jet, height, &mut work, &cancel).unwrap(),
            Some(uv),
            "a discontinuous source gradient cannot justify a Newton contraction"
        );
        let monotone = HeightFunctional {
            direction: [0.0, 1.0, 0.0].map(Interval::point),
            offset: Interval::point(0.0),
        };
        assert!(
            matches!(
                stationary_root(&surface, uv, &jet, monotone, &mut work, &cancel).unwrap(),
                RootIsolation::Excluded
            ),
            "gradient-only exclusion remains valid"
        );
    }
    #[test]
    fn nested_folded_profile_cannot_gain_global_convexity_by_partitioning() {
        use monstertruck_modeling::{
            BsplineSurface, Curve2D, ExtrusionSurface, KnotVector, Line, ParameterCurve, Point2,
            Vector3,
        };
        let carrier = Surface::BsplineSurface(BsplineSurface::new(
            (
                KnotVector::from(vec![0.0, 0.0, 0.5, 1.0, 1.0]),
                KnotVector::from(vec![0.0, 0.0, 1.0, 1.0]),
            ),
            vec![
                vec![Point3::new(0.0, 0.0, 0.0), Point3::new(0.0, 1.0, 0.0)],
                vec![Point3::new(0.5, 0.25, 0.0), Point3::new(0.5, 1.25, 0.0)],
                vec![Point3::new(1.0, 0.0, 0.0), Point3::new(1.0, 1.0, 0.0)],
            ],
        ));
        let profile = Curve::ParameterCurve(ParameterCurve::new(
            Curve2D::Line(Line(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0))),
            Box::new(carrier),
        ));
        let surface =
            Surface::ExtrusionSurface(ExtrusionSurface::by_extrusion(profile, Vector3::unit_z()));
        assert!(
            !critical_face_gluing(
                &surface,
                [[0.0, 1.0]; 2],
                &mut GeometryWorkBudget::new(100_000),
                &AtomicBool::new(false)
            )
            .unwrap(),
            "a hidden original carrier crease is not an original topological coedge"
        );
    }
    #[test]
    fn rounded_axis_length_is_not_an_exact_native_rotation_identity() {
        let mut work = GeometryWorkBudget::new(1000);
        let cancel = AtomicBool::new(false);
        assert!(exact_unit_vector_identity([0.0, 0.0, 1.0], &mut work, &cancel).unwrap());
        assert_eq!(0.6_f64 * 0.6 + 0.8_f64 * 0.8, 1.0);
        assert!(!exact_unit_vector_identity([0.6, 0.0, 0.8], &mut work, &cancel).unwrap());
    }
    #[test]
    fn exact_expansion_never_rounds_a_residual_or_underflow_product_to_an_identity() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100);
        let next = f64::from_bits(1.0_f64.to_bits() + 1);
        assert_eq!(ExactCoefficient::difference(next, 1.0).unwrap().sign(), 1);
        let tiny = ExactCoefficient::point(2.0_f64.powi(-600));
        assert!(tiny.product(&tiny, &mut work, &cancel).unwrap().is_none());
        let mut residual = ExactCoefficient::point(1.0);
        residual.subtract(&ExactCoefficient::point(next)).unwrap();
        assert_eq!(residual.sign(), -1);
    }
    #[test]
    fn a_nonzero_g1_coefficient_residual_is_not_a_small_jump_certificate() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(10_000);
        let center = |_| [0.0, 0.0, 0.0, 1.0];
        let derivative =
            |other: usize, coordinate: usize, left: bool, _: &mut GeometryWorkBudget| {
                let value = if coordinate == 0 {
                    1.0
                } else if !left && other == 1 && coordinate == 1 {
                    f64::EPSILON
                } else {
                    0.0
                };
                Ok(Some(ExactCoefficient::point(value)))
            };
        assert!(!coefficient_tangent_join(2, center, derivative, &mut work, &cancel).unwrap());
    }
    #[test]
    fn krawczyk_proves_native_morse_root_and_excludes_no_root_patch() {
        let cancel = AtomicBool::new(false);
        let surface = Surface::BsplineSurface(monstertruck_modeling::BsplineSurface::new(
            (
                monstertruck_modeling::KnotVector::bezier_knot(2),
                monstertruck_modeling::KnotVector::bezier_knot(2),
            ),
            (0..3)
                .map(|u| {
                    (0..3)
                        .map(|v| {
                            Point3::new(
                                u as f64 * 0.5,
                                v as f64 * 0.5,
                                [0.25, -0.25, 0.25][u] + [0.25, -0.25, 0.25][v],
                            )
                        })
                        .collect()
                })
                .collect(),
        ));
        let frame = PlaneFrameMm {
            origin_mm: [0.0; 3],
            x_axis: [1.0, 0.0, 0.0],
            y_axis: [0.0, 1.0, 0.0],
            z_axis: [0.0, 0.0, 1.0],
        };
        let mut work = GeometryWorkBudget::new(1_000_000);
        let uv = [[0.1, 0.9], [0.1, 0.9]];
        let jet = surface_enclosure(&surface, uv, &mut work, &cancel).unwrap();
        assert!(matches!(
            stationary_root(
                &surface,
                uv,
                &jet,
                HeightFunctional::from_frame(frame),
                &mut work,
                &cancel
            )
            .unwrap(),
            RootIsolation::Unique(_)
        ));
        let uv = [[0.01, 0.1], [0.01, 0.1]];
        let jet = surface_enclosure(&surface, uv, &mut work, &cancel).unwrap();
        assert!(matches!(
            stationary_root(
                &surface,
                uv,
                &jet,
                HeightFunctional::from_frame(frame),
                &mut work,
                &cancel
            )
            .unwrap(),
            RootIsolation::Excluded
        ));
    }
    #[test]
    fn flat_native_rank_zero_is_not_a_fake_isolated_root() {
        let cancel = AtomicBool::new(false);
        let surface = Surface::Plane(Plane::new(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
        ));
        let frame = PlaneFrameMm {
            origin_mm: [0.0; 3],
            x_axis: [1.0, 0.0, 0.0],
            y_axis: [0.0, 1.0, 0.0],
            z_axis: [0.0, 0.0, 1.0],
        };
        let mut work = GeometryWorkBudget::new(1_000_000);
        let uv = [[0.0, 1.0], [0.0, 1.0]];
        let jet = surface_enclosure(&surface, uv, &mut work, &cancel).unwrap();
        assert!(locally_convex(&jet, normal(&jet, true)));
        assert!(matches!(
            stationary_root(
                &surface,
                uv,
                &jet,
                HeightFunctional::from_frame(frame),
                &mut work,
                &cancel
            )
            .unwrap(),
            RootIsolation::ConvexCriticalCover
        ));
    }
    #[test]
    fn enclosed_direction_is_used_in_both_gradient_and_hessian() {
        let functional = HeightFunctional {
            direction: [
                Interval {
                    lo: -0.01,
                    hi: 0.01,
                },
                Interval::point(0.0),
                Interval::point(1.0),
            ],
            offset: Interval::point(0.0),
        };
        assert!(
            functional
                .dot([
                    Interval::point(1.0),
                    Interval::point(0.0),
                    Interval::point(0.0)
                ])
                .contains_zero()
        );
        let projected = functional.height([
            Interval::point(10.0),
            Interval::point(0.0),
            Interval::point(2.0),
        ]);
        assert!(projected.lo <= 1.9 && projected.hi >= 2.1);
    }

    fn horizontal_request(range: [f64; 2], sample: f64) -> SlabRequest {
        SlabRequest {
            frame: PlaneFrameMm {
                origin_mm: [0.0; 3],
                x_axis: [1.0, 0.0, 0.0],
                y_axis: [0.0, 1.0, 0.0],
                z_axis: [0.0, 0.0, 1.0],
            },
            height_interval_mm: range,
            sample_height_mm: sample,
            native_shape_error_mm: 0.1,
        }
    }

    fn horizontal_section(
        source: &crate::PreparedSectionSource<'_>,
        height: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> NativeSection {
        source
            .section_with_byte_limit(
                crate::PlaneMm {
                    origin_mm: [0.0, 0.0, height],
                    normal: [0.0, 0.0, 1.0],
                },
                cancel,
                (crate::MAX_PREPARED_SECTION_BYTES - source.retained_bytes()) as u32,
                work,
            )
            .unwrap()
    }

    fn horizontal_critical(
        source: &crate::PreparedSectionSource<'_>,
        request: SlabRequest,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<CriticalSlabCertificate, GeometryError> {
        let section = horizontal_section(source, request.sample_height_mm, work, cancel);
        let height = super::super::slab::native_sample_height_band(
            source.definition(),
            request.frame,
            &section,
            work,
            cancel,
        )?;
        certify_critical_slab(
            CriticalSlabQuery {
                source: source.trim_source(),
                request: &request,
                section: &section,
                functional: HeightFunctional::from_frame(request.frame),
                actual_sample_height: height,
            },
            crate::MAX_PREPARED_SECTION_BYTES
                .checked_sub(source.retained_bytes())
                .and_then(|remaining| remaining.checked_sub(section.retained_bytes()))
                .ok_or_else(|| {
                    GeometryError::new(
                        GeometryErrorCode::ResourceLimit,
                        "independent critical proof input exceeds remaining query capacity",
                    )
                })?,
            work,
            cancel,
        )?
        .ok_or_else(|| {
            GeometryError::new(
                GeometryErrorCode::AnalysisIndeterminate,
                "independent critical property has no represented native disk",
            )
        })
    }

    #[test]
    fn closed_capture_excludes_original_pocket_only_when_its_whole_height_is_disjoint() {
        let cancel = AtomicBool::new(false);
        for name in [
            "native-parity-blind-pocket.step",
            "native-parity-blind-pocket-nurbs-floor.step",
        ] {
            let definition = source(name);
            let mut work = GeometryWorkBudget::new(100_000_000);
            let prepared = crate::prepare_section_source(
                &definition,
                crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
                &cancel,
                &mut work,
            )
            .unwrap();
            // The SAME immutable source has real void-facing walls above Z=4.
            // Below that floor its actual closed material intersection is a disk.
            let request = horizontal_request([1.99, 2.01], 2.0);
            let proof = horizontal_critical(&prepared, request, &mut work, &cancel).unwrap();
            assert_eq!(proof.definition_id(), definition.id());
            assert_eq!(
                proof.request().height_interval_mm,
                request.height_interval_mm
            );
            assert_eq!(proof.material_component_count(), 1);
            assert_eq!(proof.bounded_void_count(), 0);
            assert!(
                !proof.has_native_convex_section_topology_proof(),
                "the actual upper pocket forbids a whole-source convex claim"
            );
            let Some(SourceBoundaryScope::ClosedCapture(boundary)) = &proof.boundary_scope else {
                panic!("the original upper pocket needs explicit height-exclusion authority");
            };
            assert!(boundary.excluded_face_cells > 0);
            assert!(boundary.excluded_coedge_cells > 0);
            assert!(proof.boundary_motion_bound_mm <= request.native_shape_error_mm);
            assert!(proof.covered_cells().0 > boundary.excluded_face_cells);
            // Intersecting the pocket, or merely touching its floor at the
            // CLOSED endpoint, must not reuse those previous exclusions.
            for range in [[3.99, 4.01], [3.99, 4.0]] {
                let failure = horizontal_critical(
                    &prepared,
                    horizontal_request(range, 3.995),
                    &mut work,
                    &cancel,
                )
                .unwrap_err();
                assert_eq!(failure.code, GeometryErrorCode::AnalysisIndeterminate);
                assert!(failure.message.contains("radial degree"), "{failure:?}");
            }
        }
    }

    #[test]
    fn actual_whole_source_convexity_is_not_replaced_by_capture_exclusions() {
        let cancel = AtomicBool::new(false);
        let definition = source("native-parity-nurbs-box.step");
        let mut work = GeometryWorkBudget::new(100_000_000);
        let prepared = crate::prepare_section_source(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &cancel,
            &mut work,
        )
        .unwrap();
        let proof = horizontal_critical(
            &prepared,
            horizontal_request([1.99, 2.01], 2.0),
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(
            proof.has_native_convex_section_topology_proof(),
            "all original box faces and links, not only this section, were proved"
        );
    }

    #[test]
    fn original_halfspace_cap_degree_requires_a_strictly_interior_native_witness() {
        let cancel = AtomicBool::new(false);
        let definition = source("native-parity-blind-pocket.step");
        let mut work = GeometryWorkBudget::new(100_000_000);
        let prepared = crate::prepare_section_source(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &cancel,
            &mut work,
        )
        .unwrap();
        let request = horizontal_request([1.9, 2.1], 2.0);
        let section = horizontal_section(&prepared, 2.0, &mut work, &cancel);
        let sample_height = super::super::slab::native_sample_height_band(
            &definition,
            request.frame,
            &section,
            &mut work,
            &cancel,
        )
        .unwrap();
        let mut prism = SamplePrism::new(
            &section,
            &request,
            HeightFunctional::from_frame(request.frame),
            sample_height,
            &mut work,
            &cancel,
        )
        .unwrap();
        prism.closed_capture_boundary(&mut work, &cancel).unwrap();
        let height = prism
            .source_height
            .height(prism.center.map(Interval::point));
        for range in [[height.lo, height.hi + 0.1], [height.lo - 0.1, height.hi]] {
            prism.range = range;
            let failure = prism
                .closed_capture_boundary(&mut work, &cancel)
                .unwrap_err();
            assert_eq!(failure.code, GeometryErrorCode::AnalysisIndeterminate);
        }
    }

    #[test]
    fn closed_caps_enclose_the_whole_native_core_not_only_its_center() {
        let cancel = AtomicBool::new(false);
        let definition = source("native-parity-nurbs-torus.step");
        let mut work = GeometryWorkBudget::new(100_000_000);
        let prepared = crate::prepare_section_source(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &cancel,
            &mut work,
        )
        .unwrap();
        let request = horizontal_request([-0.01, 0.01], 0.0);
        let section = horizontal_section(&prepared, 0.0, &mut work, &cancel);
        let band = super::super::slab::native_sample_height_band(
            &definition,
            request.frame,
            &section,
            &mut work,
            &cancel,
        )
        .unwrap();
        let mut prism = SamplePrism::new(
            &section,
            &request,
            HeightFunctional::from_frame(request.frame),
            band,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(prism.core_radius.unwrap() > 1.0);
        prism.closed_capture_boundary(&mut work, &cancel).unwrap();
        // Tilt the ACTUAL cut covector relative to the captured native core.
        // The center remains interior, but part of this same core leaves it.
        prism.source_height.direction = [0.1, 0.0, 1.0].map(Interval::point);
        let center_height = prism
            .source_height
            .height(prism.center.map(Interval::point));
        prism.range = [center_height.lo - 0.01, center_height.hi + 0.01];
        let failure = prism
            .closed_capture_boundary(&mut work, &cancel)
            .unwrap_err();
        assert_eq!(failure.code, GeometryErrorCode::AnalysisIndeterminate);
        assert!(failure.message.contains("strictly interior"));
    }

    #[test]
    fn complete_radial_material_link_preserves_a_genuine_nonconvex_star() {
        use monstertruck_io::step::load::step_geometry::Curve3D;
        use monstertruck_modeling::builder;
        let vertices = builder::vertices([
            (0.0, 0.0, 0.0),
            (20.0, 0.0, 0.0),
            (20.0, 20.0, 0.0),
            (12.0, 20.0, 0.0),
            (10.0, 18.0, 0.0),
            (8.0, 20.0, 0.0),
            (0.0, 20.0, 0.0),
        ]);
        let wire = (0..vertices.len())
            .map(|index| {
                builder::line::<Curve3D>(&vertices[index], &vertices[(index + 1) % vertices.len()])
            })
            .collect();
        let solid = crate::parity_fixtures::extruded(vec![wire], 4.0);
        let text = crate::parity_fixtures::original("original-reentrant-star.step", solid);
        let cancel = AtomicBool::new(false);
        let definition = crate::import_step(text.as_bytes(), &cancel).unwrap();
        let mut work = GeometryWorkBudget::new(100_000_000);
        let prepared = crate::prepare_section_source(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &cancel,
            &mut work,
        )
        .unwrap();
        let proof = horizontal_critical(
            &prepared,
            horizontal_request([1.99, 2.01], 2.0),
            &mut work,
            &cancel,
        )
        .unwrap();
        assert_eq!(proof.material_component_count(), 1);
        assert_eq!(proof.bounded_void_count(), 0);
        assert_eq!(proof.tunnel_cycle_count(), 0);
        assert!(!proof.has_native_convex_section_topology_proof());
        assert!(proof.boundary_motion_bound_mm <= 0.1);
        let section = horizontal_section(&prepared, 2.0, &mut work, &cancel);
        assert_eq!(section.loops.len(), 1);
        let points = &section.loops[0].points_mm;
        let area = points
            .windows(2)
            .map(|pair| pair[0][0] * pair[1][1] - pair[1][0] * pair[0][1])
            .sum::<f64>()
            .abs()
            * 0.5;
        assert!(
            (area - 396.0).abs() <= 0.01,
            "the actual V notch must remain: {area}"
        );
    }

    #[test]
    fn original_housing_layer_one_has_a_complete_paired_material_link() {
        let cancel = AtomicBool::new(false);
        let definition = crate::import_step(
            include_bytes!(
                "../../../../fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step"
            ),
            &cancel,
        )
        .unwrap();
        let mut work = GeometryWorkBudget::new(100_000_000);
        let prepared = crate::prepare_section_source(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &cancel,
            &mut work,
        )
        .unwrap();
        let proof = horizontal_critical(
            &prepared,
            horizontal_request(
                [0.09999992857142809, 0.1499998928571424],
                0.12499991071428523,
            ),
            &mut work,
            &cancel,
        )
        .unwrap();
        assert_eq!(proof.material_component_count(), 1);
        assert_eq!(proof.bounded_void_count(), 0);
        assert_eq!(proof.tunnel_cycle_count(), 0);
        assert!(
            !proof.has_native_convex_section_topology_proof(),
            "the unchanged upper blind pocket is not a globally convex source"
        );
        assert!(proof.boundary_motion_bound_mm <= 0.1);
        assert!(proof.occupied.is_some());
        assert_eq!(
            proof.material_realization_identity(),
            crate::kernel::MaterialRealizationIdentity::new(&definition)
        );
    }
}
