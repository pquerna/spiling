// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! A sufficient, representation-independent critical-neighborhood proof. Native
//! face charts and oriented coedges form the complex; boxes are only a covering.
//! The closed capture is bounded by original material and actual halfspace cuts.
//! Convexity proves a disk complex, or convex meridian fibers with positive
//! radial degree about a separated represented core prove an annular complex.
//! Whole-source convexity is separate, strictly stronger evidence.

mod source_chart;

use super::slab::HeightFunctional;
use super::source_incidence::SourceEdgeIncidence;
use super::{GeometryWorkBudget, SlabRequest};
#[cfg(test)]
use crate::kernel::prepare_face_trim_domain_with_byte_limit;
use crate::kernel::{PreparedFaceTrimDomain, TrimClassification};
use crate::parametric::{
    Interval, SurfaceEnclosure, down, same_surface_coefficients as coefficient_equal,
    surface_crease_interfaces, surface_enclosure, surface_parameter_boxes,
    surface_parameter_span_smooth, up,
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
    boundary_scope: Option<SourceBoundaryScope>,
}
impl CriticalSlabCertificate {
    pub(super) fn definition_id(&self) -> &DefinitionId {
        &self.capture.definition_id
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
    let axis = usize::from(uv[1][1] - uv[1][0] > uv[0][1] - uv[0][0]);
    let mid = uv[axis][0] * 0.5 + uv[axis][1] * 0.5;
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

/// A bounded error-free expansion used only for coefficient IDENTITIES. It
/// never turns a small numerical residual into a zero. Products outside the
/// exponent range in which their FMA remainder is representable are refused.
#[derive(Clone)]
struct ExactCoefficient {
    terms: [f64; 128],
    len: usize,
}
impl ExactCoefficient {
    fn point(value: f64) -> Self {
        let mut result = Self {
            terms: [0.0; 128],
            len: 0,
        };
        if value != 0.0 {
            result.terms[0] = value;
            result.len = 1;
        }
        result
    }
    fn sign(&self) -> i8 {
        if self.len == 0 {
            0
        } else if self.terms[self.len - 1] > 0.0 {
            1
        } else {
            -1
        }
    }
    fn push(&mut self, value: f64) -> Option<()> {
        if value == 0.0 {
            return Some(());
        }
        let mut carry = value;
        let mut length = 0;
        for index in 0..self.len {
            let other = self.terms[index];
            let sum = carry + other;
            if !sum.is_finite() {
                return None;
            }
            let virtual_other = sum - carry;
            let residual = (carry - (sum - virtual_other)) + (other - virtual_other);
            if residual != 0.0 {
                self.terms[length] = residual;
                length += 1;
            }
            carry = sum;
        }
        if carry != 0.0 {
            if length == self.terms.len() {
                return None;
            }
            self.terms[length] = carry;
            length += 1;
        }
        self.len = length;
        Some(())
    }
    fn subtract(&mut self, other: &Self) -> Option<()> {
        for term in &other.terms[..other.len] {
            self.push(-term)?;
        }
        Some(())
    }
    fn difference(a: f64, b: f64) -> Option<Self> {
        let mut result = Self::point(a);
        result.push(-b)?;
        Some(result)
    }
    fn product(
        &self,
        other: &Self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        let mut result = Self::point(0.0);
        for &a in &self.terms[..self.len] {
            for &b in &other.terms[..other.len] {
                work.charge(1, cancel)?;
                let ea = ((a.abs().to_bits() >> 52) & 0x7ff) as i32;
                let eb = ((b.abs().to_bits() >> 52) & 0x7ff) as i32;
                if ea == 0 || eb == 0 || !(-900..=900).contains(&(ea + eb - 2046)) {
                    return Ok(None);
                }
                let value = a * b;
                let remainder = a.mul_add(b, -value);
                if !value.is_finite()
                    || !remainder.is_finite()
                    || result.push(remainder).is_none()
                    || result.push(value).is_none()
                {
                    return Ok(None);
                }
            }
        }
        Ok(Some(result))
    }
}
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
    outer_count: usize,
    center: [f64; 3],
    frame: PlaneFrameMm,
    dual: [[Interval; 3]; 3],
    core_radius: Option<f64>,
    inner_radius_upper: f64,
    range: [f64; 2],
    source_height: HeightFunctional,
    approximation: f64,
    vertical_return: f64,
}
impl SamplePrism {
    fn new(
        section: &NativeSection,
        request: &SlabRequest,
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
        if count
            .checked_mul(std::mem::size_of::<[Interval; 2]>())
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
        let dual = [cross(y, z), cross(z, x), cross(x, y)]
            .map(|column| column.map(|value| value.div(determinant)))
            .map(|column| -> Result<[Interval; 3], GeometryError> {
                let [a, b, c] = column;
                Ok([a?, b?, c?])
            });
        let [a, b, c] = dual;
        let dual = [a?, b?, c?];
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
        let direction_length = length(request.frame.z_axis.map(Interval::point))?;
        if direction_length.lo <= 0.0 {
            return Err(failure(
                GeometryErrorCode::ConditioningFailure,
                "height direction has zero norm",
                0,
                request.frame.z_axis,
            ));
        }
        let plane_drift = actual.sub(Interval::point(actual.mid())).abs_upper();
        let plane_return = Interval::point(plane_drift).div(direction_length)?.hi;
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
        let source_height = HeightFunctional::from_frame(request.frame);
        let vertical = actual
            .sub(Interval::point(request.height_interval_mm[0]))
            .hull(Interval::point(request.height_interval_mm[1]).sub(actual))
            .abs_upper();
        let vertical_return =
            up(Interval::point(vertical).div(direction_length)?.hi + approximation);
        Ok(Self {
            vertices,
            outer_count,
            center,
            frame,
            dual,
            core_radius,
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
        if edge + 1 == self.outer_count {
            0
        } else if edge + 1 == self.vertices.len() {
            self.outer_count
        } else {
            edge + 1
        }
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
        let relative =
            std::array::from_fn(|axis| jet.position[axis].sub(Interval::point(self.center[axis])));
        let Some(core_radius) = self.core_radius else {
            return Ok((
                dot(n, relative),
                source_convex.unwrap_or_else(|| locally_convex(jet, n)),
            ));
        };
        let p = project(jet.position, self.frame);
        let rho = length([p[0], p[1], Interval::point(0.0)])?;
        let unresolved = || {
            (
                Interval {
                    lo: f64::NEG_INFINITY,
                    hi: f64::INFINITY,
                },
                false,
            )
        };
        if rho.lo <= 0.0 {
            return Ok(unresolved());
        }
        let cosine = p[0].div(rho)?;
        let sine = p[1].div(rho)?;
        let radial: [Interval; 3] = std::array::from_fn(|axis| {
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
        let radial_degree = dot(n, q);
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
            return Ok(unresolved());
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

    /// Every witness is on the actual polygon prism. In annular mode side
    /// witnesses use the SAME azimuth as the source point, as required by the
    /// convex-meridian artificial-cap covering proof.
    fn boundary_distance(
        &self,
        position: [Interval; 3],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<f64, GeometryError> {
        let p = project(position, self.frame);
        let rho = length([p[0], p[1], Interval::point(0.0)])?;
        let mut inside = true;
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
                break;
            }
        }
        if self.core_radius.is_some() && rho.lo <= self.inner_radius_upper {
            inside = false;
        }
        let height = self.source_height.height(position);
        let mut distance = f64::INFINITY;
        if inside {
            let vertical = height
                .sub(Interval::point(self.range[0]))
                .abs_upper()
                .min(height.sub(Interval::point(self.range[1])).abs_upper());
            distance = Interval::point(vertical)
                .div(length(self.source_height.direction)?)?
                .hi;
        }
        if self.core_radius.is_some() && rho.lo > 0.0 {
            let cosine = p[0].div(rho)?;
            let sine = p[1].div(rho)?;
            let radial: [Interval; 3] = std::array::from_fn(|axis| {
                add(
                    mul(self.dual[0][axis], cosine),
                    mul(self.dual[1][axis], sine),
                )
            });
            let scale = length(radial)?.hi;
            let mut loop_bounds = [0.0_f64; 2];
            let mut candidates = [0_usize; 2];
            for edge in 0..self.vertices.len() {
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
                    continue;
                }
                let support = mul(sign, orient(a, b, [Interval::point(0.0); 2]));
                let edge_length = length([b[0].sub(a[0]), b[1].sub(a[1]), Interval::point(0.0)])?;
                let low = Interval::point(support.lo.max(0.0))
                    .div(Interval::point(edge_length.hi))?
                    .lo;
                let high = length([a[0], a[1], Interval::point(0.0)])?
                    .hi
                    .max(length([b[0], b[1], Interval::point(0.0)])?.hi);
                let reserve = Interval::point(rho.sub(Interval { lo: low, hi: high }).abs_upper())
                    .mul(Interval::point(scale))
                    .hi;
                loop_bounds[inner] = loop_bounds[inner].max(reserve);
                candidates[inner] += 1;
            }
            // Each actual star loop has a ray intersection. Enumerate every
            // interval-compatible edge: MAX, not min, bounds the unknown feasible
            // intersection even when a source cell straddles a sampled vertex.
            for index in 0..2 {
                if candidates[index] > 0 {
                    distance = distance.min(loop_bounds[index]);
                }
            }
        } else if self.core_radius.is_none() {
            for edge in 0..self.vertices.len() {
                work.charge(1, cancel)?;
                let a = self.vertices[edge];
                let b = self.vertices[self.next_vertex(edge)];
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
                let difference = std::array::from_fn(|axis| {
                    add(
                        mul(p[0].sub(q[0]), self.dual[0][axis]),
                        mul(p[1].sub(q[1]), self.dual[1][axis]),
                    )
                });
                distance = distance.min(length(difference)?.hi);
            }
        }
        // The cap material is convex globally (disk complex) or in each proven
        // meridian fiber (cycle complex). Native sampling transfers that exact
        // source region to its actual polygon in both directions.
        let scale = position
            .into_iter()
            .map(Interval::abs_upper)
            .fold(1.0_f64, f64::max);
        Ok(up(distance
            + 3.0 * self.approximation
            + up(scale * f64::EPSILON * 128.0)))
    }
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

struct NeighborhoodInput {
    face: usize,
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
            face,
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
            face,
            [height.lo, height.hi],
        ));
    }
    work.charge(1, cancel)?;
    events.try_reserve(1).map_err(|_| {
        failure(
            GeometryErrorCode::ResourceLimit,
            "critical neighborhood allocation refused",
            face,
            events.len(),
        )
    })?;
    events.push(CriticalEventNeighborhood {
        capture: Arc::clone(capture),
        height_bracket_mm: [height.lo, height.hi],
        source_face: Some(face),
        stationary_root_proved: root_proved,
        incoming_regular_endpoint: false,
        pairing,
    });
    Ok(())
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
    let fixed_scratch =
        MAX_EVENT_BYTES + MAX_EDGE_BYTES + RESERVED_STACK_BYTES + capture_bytes + 4096;
    let sample_bytes = section
        .loops
        .iter()
        .map(|boundary| boundary.points_mm.len().saturating_sub(1))
        .sum::<usize>()
        .saturating_mul(std::mem::size_of::<[Interval; 2]>());
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
    let prism = SamplePrism::new(section, request, actual_sample_height, work, cancel)?;
    let mut closed_boundary = prism.closed_capture_boundary(work, cancel)?;
    let mut whole_source = true;
    let chart_bytes = scratch_limit
        .checked_sub(
            fixed_scratch.saturating_add(
                prism
                    .vertices
                    .capacity()
                    .saturating_mul(std::mem::size_of::<[Interval; 2]>()),
            ),
        )
        .ok_or_else(|| {
            failure(
                GeometryErrorCode::ResourceLimit,
                "critical source chart capacity exhausted",
                0,
                scratch_limit,
            )
        })?;
    if prism.vertical_return > request.native_shape_error_mm {
        return Err(failure(
            GeometryErrorCode::ShapeErrorExceeded,
            "reverse prism/cap distance exceeds native shape policy",
            0,
            prism.vertical_return,
        ));
    }
    let pairing = ComponentVoidPairing {
        material_components: 1,
        bounded_voids: 0,
        tunnel_cycles: usize::from(prism.core_radius.is_some()),
    };
    let capture = CriticalCapture::new(definition, request, functional);
    let mut events = Vec::new();
    let edges = SourceEdgeIncidence::new(definition, MAX_EDGE_BYTES, work, cancel)?;
    let mut covered_face_cells = 0_u64;
    let mut covered_coedge_cells = 0_u64;
    let mut bound = prism.vertical_return;
    let mut lower = [f64::INFINITY; 2];
    let mut upper = [f64::NEG_INFINITY; 2];
    for (face_index, face) in definition.solid.face_iter().enumerate() {
        let domain = source.face_domain(face_index, chart_bytes, work, cancel)?;
        face.with_surface(|surface| -> Result<(), GeometryError> {
            let source_chart = source_chart::SphereChart::new(surface, &domain, work, cancel)?;
            let face_glued = capture_face_gluing(surface, domain.uv_bounds(), functional, request, face_index, work, cancel)?;
            whole_source &= face_glued;
            let mut spans = surface_parameter_boxes(surface, domain.uv_bounds(), work, cancel)?;
            let mut stack = Vec::with_capacity(usize::from(MAX_DEPTH) + 1);
            while let Some((uv, depth)) = stack.pop().or_else(|| spans.next().map(|uv| (uv, 0_u8))) {
                work.charge(1, cancel)?;
                if domain.classify(uv, work, cancel)? == TrimClassification::Outside { continue; }
                let span_smooth = depth != 0 || surface_parameter_span_smooth(surface, uv, work, cancel)?;
                let jet = if span_smooth {
                    surface_enclosure(surface, uv, work, cancel)?
                } else {
                    crate::parametric::surface_piecewise_enclosure(surface, uv, work, cancel)?
                };
                let height = functional.height(jet.position);
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
                let metric = if active { prism.boundary_distance(jet.position, work, cancel)? } else { 0.0 };
                if !regular || radial_degree.lo <= 0.0 || !convex || metric > request.native_shape_error_mm {
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
                            else if !convex { "source normal-curvature form is not convex" }
                            else { "whole native boundary cell exceeds candidate-prism shape policy" };
                        return Err(failure(if metric > request.native_shape_error_mm { GeometryErrorCode::ShapeErrorExceeded }
                            else { GeometryErrorCode::AnalysisIndeterminate }, obligation, face_index,
                            (uv, request.height_interval_mm, request.sample_height_mm)));
                    }
                    work.charge(2, cancel)?;
                    let children = match split(uv, depth, face_index) {
                        Ok(children) => children,
                        Err(error) if !active && error.code == GeometryErrorCode::AnalysisIndeterminate => {
                            whole_source = false;
                            closed_boundary.excluded_face_cells += 1;
                            continue;
                        }
                        Err(error) => return Err(error),
                    };
                    for child in children { stack.push((child, depth + 1)); }
                    continue;
                }
                covered_face_cells += 1;
                if !active { continue; }
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
                                face: face_index,
                                height: root_height,
                                root_proved: member == TrimClassification::Inside,
                                pairing,
                            }, work, cancel)?;
                        }
                    }
                    RootIsolation::ConvexCriticalCover => {
                        // Positive source radial degree and convex meridian
                        // fibers prove every covered stratum attaches to the
                        // represented component/core, not a hidden void. Root
                        // existence is NOT claimed for a conservative cover.
                        add_neighborhood(&mut events, &capture, NeighborhoodInput {
                            face: face_index,
                            height,
                            root_proved: false,
                            pairing,
                        }, work, cancel)?;
                    }
                }
            }
            Ok(())
        })?;
    }
    // All original incidences remain mandatory. Only a WHOLE edge-cell height
    // exclusion removes its geometric link from the clipped boundary; cells
    // touching either CLOSED cut retain the complete nonreentrant link proof.
    for uses in edges.values() {
        let first = uses.first;
        let second = uses.second.ok_or_else(|| {
            failure(
                GeometryErrorCode::SingularGeometry,
                "source boundary complex has an unpaired edge",
                first.face,
                (first.wire, first.index),
            )
        })?;
        if first.face == second.face {
            let domain = source.face_domain(first.face, chart_bytes, work, cancel)?;
            let smooth = domain.native().face().with_surface(|surface| {
                same_face_source_seam_join(
                    surface,
                    &domain,
                    first.coedge,
                    second.coedge,
                    work,
                    cancel,
                )
            })?;
            if smooth {
                continue;
            }
        }
        let first_domain = source.face_domain(first.face, chart_bytes, work, cancel)?;
        let second_domain = source.face_domain(
            second.face,
            chart_bytes.saturating_sub(first_domain.owned_bytes()),
            work,
            cancel,
        )?;
        let first_face = first_domain.native().face();
        let second_face = second_domain.native().face();
        // Different face uses (including inverted ones) can share the same surface mutex.
        // Still run the coefficient proof so work, cancellation and typed refusals are unchanged.
        let equal = if first_face.id() == second_face.id() {
            first_face.with_surface(|surface| coefficient_equal(surface, surface, work, cancel))
        } else if second.face < first.face {
            // Acquire distinct carriers in original source-face order, retaining logical lhs/rhs.
            second_face.with_surface(|b| {
                first_face.with_surface(|a| coefficient_equal(a, b, work, cancel))
            })
        } else {
            first_face.with_surface(|a| {
                second_face.with_surface(|b| coefficient_equal(a, b, work, cancel))
            })
        }?;
        if first.face != second.face
            && equal
            && first_face.orientation() == second_face.orientation()
        {
            // Identical spline carriers still need a proof at a periodic end;
            // coefficient equality alone says nothing about endpoint gluing.
            let smooth = first_face.with_surface(intrinsically_smooth_carrier);
            if smooth {
                continue;
            }
        }
        let edge = &first_face.absolute_boundaries()[first.wire][first.index];
        edge.with_curve(|curve| -> Result<(), GeometryError> {
            let range = curve.range_tuple();
            // Events grow while this native owner remains live: reserve their
            // complete bounded capacity, not just the capacity before capture.
            let occupied_scratch = fixed_scratch
                + prism.vertices.capacity() * std::mem::size_of::<[Interval; 2]>()
                + first_domain.owned_bytes() + second_domain.owned_bytes();
            let remaining = scratch_limit.checked_sub(occupied_scratch).ok_or_else(|| failure(
                GeometryErrorCode::ResourceLimit, "critical scratch capacity exhausted before native curve capture", first.face, range))?;
            let prepared = definition.prepare_boundary_curve(edge.id(), curve, range, remaining, work, cancel)?;
            // Original physical coedge endpoints are zero-dimensional source
            // strata too. Monotone face/edge derivatives do not exclude a
            // polyhedral corner event. Use actual bounded 3D endpoint tubes,
            // never a UV parameter or a nearby stored/sampled vertex.
            for parameter in [range.0, range.1] {
                let height = functional.height(prepared.jet((parameter, parameter), work, cancel)?.position);
                add_neighborhood(&mut events, &capture, NeighborhoodInput {
                    face: first.face,
                    height,
                    root_proved: false,
                    pairing,
                }, work, cancel)?;
            }
            for span in prepared.parameter_intervals() {
                let mut stack = Vec::with_capacity(usize::from(MAX_DEPTH) + 1);
                stack.push((span, 0_u8));
                while let Some(((lo, hi), depth)) = stack.pop() {
                    work.charge(1, cancel)?;
                    let jet = prepared.jet((lo, hi), work, cancel)?;
                    let h = functional.height(jet.position);
                    let active = overlaps(h, request.height_interval_mm);
                    if !active && !whole_source {
                        closed_boundary.excluded_coedge_cells += 1;
                        continue;
                    }
                    let n1 = coedge_normal_enclosure(&first_domain, first.coedge, jet.position, work, cancel)?;
                    let n2 = coedge_normal_enclosure(&second_domain, second.coedge, jet.position, work, cancel)?;
                    let tangent = if first.orientation { jet.derivative } else { jet.derivative.map(Interval::neg) };
                    let dihedral = dot(n2, cross(n1, tangent));
                    if dihedral.hi > 0.0 {
                        if !active && (dihedral.lo > 0.0 || depth >= MAX_DEPTH) {
                            whole_source = false;
                            closed_boundary.excluded_coedge_cells += 1;
                            continue;
                        }
                        let mid = lo * 0.5 + hi * 0.5;
                        if depth >= MAX_DEPTH || mid <= lo || mid >= hi {
                            if !active {
                                whole_source = false;
                                closed_boundary.excluded_coedge_cells += 1;
                                continue;
                            }
                            return Err(failure(GeometryErrorCode::AnalysisIndeterminate,
                                "oriented source trim link cannot exclude a reentrant material/void attachment", first.face,
                                (first.wire, first.index, second.face, [lo, hi], [dihedral.lo, dihedral.hi],
                                    request.height_interval_mm, request.sample_height_mm)));
                        }
                        work.charge(2, cancel)?;
                        stack.push(((lo, mid), depth + 1)); stack.push(((mid, hi), depth + 1));
                        continue;
                    }
                    covered_coedge_cells += 1;
                    if !active { continue; }
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
                        face: first.face,
                        height: neighborhood,
                        root_proved,
                        pairing,
                    }, work, cancel)?;
                }
            }
            Ok(())
        })?;
    }
    // This is the CLOSED original material intersection, not a polygon cap:
    // all retained source patches/links are locally convex and point outward
    // with positive radial degree. Each halfspace cap is flat with the proved
    // positive degree above; its source links are intersections of those SAME
    // locally convex material sides with the actual closed halfspace.
    //
    // Radial projection of each resulting closed component is locally
    // degree-positive, hence covers the point/core represented by the native
    // sample. A disjoint extra component or inward void boundary cannot have
    // this degree about that same separated witness. Convexity then gives the
    // disk (or each meridian disk around the core), and the sample calibrates
    // the represented component/cycle. Artificial caps grant neither a native
    // sample nor cross-height coface incidence; those remain independent.
    let occupied = Some(occupied_projection(section, lower, upper, work, cancel)?);
    Ok(Some(CriticalSlabCertificate {
        boundary_motion_bound_mm: up(bound),
        occupied,
        event_neighborhoods: events,
        capture,
        pairing,
        covered_face_cells,
        covered_coedge_cells,
        regular_flow_speed: None,
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
            let prepared = crate::prepare_section_source(
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
        let prepared = crate::prepare_section_source(
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
        let prepared = crate::prepare_section_source(
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
            result
                .critical
                .as_ref()
                .unwrap()
                .has_native_convex_section_topology_proof(),
            "actual whole-source convexity survives this original cap/face branch transition"
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
            crate::MAX_PREPARED_SECTION_BYTES - source.retained_bytes() - section.retained_bytes(),
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
        let mut prism =
            SamplePrism::new(&section, &request, sample_height, &mut work, &cancel).unwrap();
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
        let mut prism = SamplePrism::new(&section, &request, band, &mut work, &cancel).unwrap();
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
    fn positive_face_degree_cannot_mask_an_active_reentrant_original_source_link() {
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
        let failure = horizontal_critical(
            &prepared,
            horizontal_request([1.99, 2.01], 2.0),
            &mut work,
            &cancel,
        )
        .unwrap_err();
        assert_eq!(failure.code, GeometryErrorCode::AnalysisIndeterminate);
        assert!(
            failure.message.contains("oriented source trim link"),
            "{failure:?}"
        );
    }
}
