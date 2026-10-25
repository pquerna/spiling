// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Stratified material height exclusion and bounded native-section continuation.
//! Search cells are not geometry: material bounds use complete selected source
//! coefficient intervals, and every returned sample is a native section.
//!
//! A regular core requires a proper stratified submersion of every retained
//! original face, physical coedge and vertex over the whole slab. Exact
//! coefficient identities and complete trimmed oriented covers, not a family
//! name, identify horizontal endpoint faces. Incoming faces belong to the core
//! closure; outgoing faces are separately retained CLOSED occupancy strata.
//! A curved section's finite polygon error remains nonzero even for an exact
//! source product.
//! Departing endpoint strata additionally retain the real native collar
//! complex: a directed adjacent source-M receipt is not an endpoint reverse
//! coverage proof. The regular core's metric and its incidence remain separate.
//!
//! Interior horizontal event bands instead require the original native collar
//! complex. In that mode Core is CLOSED C intersected with the vertical sweep
//! of the actual native filled sample, not the sampled polygon. Its metric is
//! not a reverse bound from all C: transition material still requires actual
//! source-M target coverage and the atomic union ledger. Nonempty native guide
//! bands must be enclosed by the original request interval. Every coexisting
//! retained sample, proof buffer and native capture shares the same byte cap;
//! every alternative uses the same cumulative work/cancellation token.

use super::critical::GluedSourceFace;
use super::source_incidence::SourceEdgeIncidence;
use super::{
    AnalysisPolicy, GeometryWorkBudget, HeightAnalysis, InvariantHeightDomain, SlabAnalysis,
    SlabRequest,
};
use crate::kernel::{PreparedFaceTrimDomain, TrimClassification};
use crate::parametric::{
    CurveEvaluator, Interval, PreparedValidatedCurve, SurfaceEvaluator, down,
    surface_crease_interfaces, surface_enclosure, surface_parameter_boxes, up,
};
use crate::{Definition, GeometryError, NativeSection, PlaneFrameMm, PlaneMm, SectionLoopMm};
use solidcore::surface::surface::{Surface, SurfaceKind};
use solidcore::{EdgeId, VertexId};
use spiling_contracts::geometry::GeometryErrorCode;
#[cfg(test)]
use spiling_contracts::geometry::MAX_SECTION_BYTES;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;

const MAX_DEPTH: u8 = 64;
const MAX_EVENT_BRACKETS: usize = 65_536;
pub(super) const MAX_SLAB_SCRATCH_BYTES: usize = 32 * 1024 * 1024;
const STACK_SCRATCH_BYTES: usize = 128 * 1024;
const HEIGHT_EVENT_SCRATCH_BYTES: usize = 2 * MAX_EVENT_BRACKETS * std::mem::size_of::<[f64; 2]>();

pub(super) fn remaining_scratch(occupied: usize) -> Result<usize, GeometryError> {
    crate::MAX_PREPARED_SECTION_BYTES
        .checked_sub(occupied)
        .map(|bytes| bytes.min(MAX_SLAB_SCRATCH_BYTES))
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "native slab coexistent shared capacity exhausted",
            )
        })
}
fn remaining_scratch_with_limit(limit: usize, occupied: usize) -> Result<usize, GeometryError> {
    limit
        .min(MAX_SLAB_SCRATCH_BYTES)
        .checked_sub(occupied)
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "native slab coexistent scratch capacity exhausted",
            )
        })
}
pub(super) fn trim_parameter_box(
    domain: &PreparedFaceTrimDomain<'_>,
) -> Result<[[f64; 2]; 2], GeometryError> {
    let uv = domain.uv_domain();
    if uv
        .iter()
        .any(|axis| !axis[0].is_finite() || !axis[1].is_finite() || axis[0] > axis[1])
    {
        return Err(indeterminate(
            "native trim support has no certified finite carrier domain",
        ));
    }
    Ok(uv)
}
#[derive(Clone, Copy)]
struct SeamHeightLink {
    smooth: bool,
    endpoints: [bool; 2],
    whole: Option<bool>,
}
struct HeightSourceLinks<'d> {
    incidence: SourceEdgeIncidence<'d>,
    edges: HashMap<EdgeId, SeamHeightLink>,
    vertices: HashMap<VertexId, bool>,
}

struct FaceHeightQuery<'q, 'a, 's> {
    face: usize,
    glued: &'q GluedSourceFace<'a, 's>,
    functional: HeightFunctional,
}
struct EdgeHeightQuery<'q, 'a, 's> {
    source: &'q FaceHeightQuery<'q, 'a, 's>,
    edge: EdgeId,
}
fn receipt_map_bytes<K, V>(map: &HashMap<K, V>) -> usize {
    map.capacity()
        .saturating_mul(std::mem::size_of::<(K, V)>() + 16)
        .saturating_mul(2)
}
impl<'d> HeightSourceLinks<'d> {
    fn new(
        definition: &'d Definition,
        max_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        let mut incidence = SourceEdgeIncidence::new(definition, max_bytes, work, cancel)?;
        incidence.prepare_vertices(max_bytes, work, cancel)?;
        Ok(Self {
            incidence,
            edges: HashMap::new(),
            vertices: HashMap::new(),
        })
    }
    fn fixed_bytes(&self) -> usize {
        self.incidence
            .owned_bytes()
            .saturating_add(
                std::mem::size_of::<Self>() - std::mem::size_of::<SourceEdgeIncidence<'d>>(),
            )
            .saturating_add(receipt_map_bytes(&self.vertices))
    }
    fn owned_bytes(&self) -> usize {
        self.fixed_bytes()
            .saturating_add(receipt_map_bytes(&self.edges))
    }
    fn edge_link(
        incidence: &SourceEdgeIncidence<'d>,
        cache: &mut HashMap<EdgeId, SeamHeightLink>,
        fixed_bytes: usize,
        query: &EdgeHeightQuery<'_, '_, '_>,
        max_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<SeamHeightLink, GeometryError> {
        let EdgeHeightQuery { source, edge } = *query;
        let FaceHeightQuery {
            face,
            glued,
            functional,
        } = *source;
        crate::check_cancel(cancel)?;
        if let Some(receipt) = cache.get(&edge) {
            return Ok(*receipt);
        }
        work.charge(1, cancel)?;
        let (surface, domain) = (glued.surface(), glued.domain());
        let uses = *incidence
            .get(edge)
            .ok_or_else(|| indeterminate("original seam edge is absent from native incidence"))?;
        let mut receipt = SeamHeightLink {
            smooth: false,
            endpoints: [false; 2],
            whole: Some(false),
        };
        if let Some(second) = uses.second
            && uses.first.face == face
            && second.face == face
        {
            receipt.smooth = super::critical::same_face_source_seam_join(
                surface,
                domain,
                uses.first.coedge,
                second.coedge,
                work,
                cancel,
            )?;
            if receipt.smooth {
                receipt.whole = None;
                for endpoint in 0..2 {
                    let mut regular = true;
                    for occurrence in [uses.first.coedge, second.coedge] {
                        let proof = domain.original_coedge_correspondence(occurrence)?;
                        let binding = proof.binding().ok_or_else(|| {
                            indeterminate(
                                "source seam endpoint lacks selected spatial/chart binding",
                            )
                        })?;
                        let parameter =
                            proof.uv_endpoint((endpoint != 0) == binding.spatial_forward);
                        let uv = domain.boundary_position_enclosure(
                            occurrence,
                            [parameter; 2],
                            work,
                            cancel,
                        )?;
                        if !source_height_transverse(glued, uv, functional, work, cancel)? {
                            regular = false;
                            break;
                        }
                    }
                    receipt.endpoints[endpoint] = regular;
                }
            }
        }
        let prospective = fixed_bytes.saturating_add(
            cache
                .len()
                .saturating_add(16)
                .saturating_mul(std::mem::size_of::<(EdgeId, SeamHeightLink)>() + 16)
                .saturating_mul(4),
        );
        if prospective > max_bytes {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                "source seam-height receipt capacity exhausted",
            ));
        }
        cache.try_reserve(1).map_err(|_| {
            error(
                GeometryErrorCode::ResourceLimit,
                "source seam-height receipt allocation refused",
            )
        })?;
        cache.insert(edge, receipt);
        if fixed_bytes.saturating_add(receipt_map_bytes(cache)) > max_bytes {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                "source seam-height retained capacity exhausted",
            ));
        }
        Ok(receipt)
    }
    fn whole_edge_regular(
        &mut self,
        query: &FaceHeightQuery<'_, '_, '_>,
        edge: EdgeId,
        max_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let FaceHeightQuery {
            glued, functional, ..
        } = *query;
        let domain = glued.domain();
        let fixed = self.fixed_bytes();
        let receipt = Self::edge_link(
            &self.incidence,
            &mut self.edges,
            fixed,
            &EdgeHeightQuery {
                source: query,
                edge,
            },
            max_bytes,
            work,
            cancel,
        )?;
        if let Some(whole) = receipt.whole {
            return Ok(whole);
        }
        let uses = self.incidence.get(edge).unwrap();
        let second = uses.second.unwrap();
        let mut whole = true;
        for occurrence in [uses.first.coedge, second.coedge] {
            for span in domain.boundary_parameter_intervals(occurrence)? {
                let uv = domain.boundary_position_enclosure(occurrence, span, work, cancel)?;
                if !source_height_transverse(glued, uv, functional, work, cancel)? {
                    whole = false;
                    break;
                }
            }
            if !whole {
                break;
            }
        }
        self.edges.get_mut(&edge).unwrap().whole = Some(whole);
        Ok(whole)
    }
    fn vertex_regular(
        &mut self,
        definition: &Definition,
        query: &FaceHeightQuery<'_, '_, '_>,
        vertex: VertexId,
        max_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let FaceHeightQuery {
            glued, functional, ..
        } = *query;
        crate::check_cancel(cancel)?;
        if let Some(regular) = self.vertices.get(&vertex) {
            return Ok(*regular);
        }
        let domain = glued.domain();
        let mut regular = self
            .incidence
            .vertex_link_is_cycle(definition, vertex, work, cancel)?;
        let fixed = self.fixed_bytes();
        if regular {
            for edge in self.incidence.vertex_edges(vertex) {
                let uses = *self.incidence.get(edge).unwrap();
                let receipt = Self::edge_link(
                    &self.incidence,
                    &mut self.edges,
                    fixed,
                    &EdgeHeightQuery {
                        source: query,
                        edge,
                    },
                    max_bytes,
                    work,
                    cancel,
                )?;
                let endpoint = if uses.vertices[0] == vertex { 0 } else { 1 };
                if !receipt.smooth || !receipt.endpoints[endpoint] {
                    regular = false;
                    break;
                }
            }
        }
        if regular {
            // Complete original endpoint connectors, not nominal endpoint samples.
            for joint in domain
                .original_joints()?
                .iter()
                .filter(|joint| joint.vertex_id == vertex)
            {
                work.charge(1, cancel)?;
                if !source_height_transverse(
                    glued,
                    joint.enclosure.map(|axis| [axis.lo, axis.hi]),
                    functional,
                    work,
                    cancel,
                )? {
                    regular = false;
                    break;
                }
            }
        }
        let prospective = self.owned_bytes().saturating_add(
            self.vertices
                .len()
                .saturating_add(16)
                .saturating_mul(std::mem::size_of::<(VertexId, bool)>() + 16)
                .saturating_mul(4),
        );
        if prospective > max_bytes {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                "source vertex-height receipt capacity exhausted",
            ));
        }
        self.vertices.try_reserve(1).map_err(|_| {
            error(
                GeometryErrorCode::ResourceLimit,
                "source vertex-height receipt allocation refused",
            )
        })?;
        self.vertices.insert(vertex, regular);
        if self.owned_bytes() > max_bytes {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                "source vertex-height retained capacity exhausted",
            ));
        }
        Ok(regular)
    }
}

/// Source geometric rank and nonzero height differential on the WHOLE UV tube,
/// including one-sided knot pieces and genuinely collapsed coordinate charts.
fn source_height_transverse(
    glued: &GluedSourceFace<'_, '_>,
    uv: [[f64; 2]; 2],
    functional: HeightFunctional,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let mut pending = [([[0.0; 2]; 2], 0u8); MAX_DEPTH as usize + 1];
    for patch in surface_parameter_boxes(glued.surface(), uv, work, cancel)? {
        let mut length = 1;
        pending[0] = (patch, 0);
        while length != 0 {
            work.charge(1, cancel)?;
            length -= 1;
            let (patch, depth) = pending[length];
            let regular = match glued.normal(patch, work, cancel) {
                Ok(normal) => {
                    let cross: [Interval; 3] = std::array::from_fn(|axis| {
                        let a = (axis + 1) % 3;
                        let b = (axis + 2) % 3;
                        normal[a]
                            .mul(functional.direction[b])
                            .sub(normal[b].mul(functional.direction[a]))
                    });
                    norm(normal)?.lo > 0. && norm(cross)?.lo > 0.
                }
                Err(cause) if cause.code == GeometryErrorCode::SingularGeometry => false,
                Err(cause) => return Err(cause),
            };
            if regular {
                continue;
            }
            let children = match split_uv(patch, depth) {
                Ok(children) => children,
                Err(cause) if cause.code == GeometryErrorCode::AnalysisIndeterminate => {
                    return Ok(false);
                }
                Err(cause) => return Err(cause),
            };
            work.charge(2, cancel)?;
            for child in children {
                pending[length] = (child, depth + 1);
                length += 1;
            }
        }
    }
    Ok(true)
}
fn height_source_links<'a, 'd>(
    slot: &'a mut Option<HeightSourceLinks<'d>>,
    definition: &'d Definition,
    max_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<&'a mut HeightSourceLinks<'d>, GeometryError> {
    crate::check_cancel(cancel)?;
    if slot.is_none() {
        *slot = Some(HeightSourceLinks::new(definition, max_bytes, work, cancel)?);
    }
    let links = slot.as_mut().unwrap();
    if links.owned_bytes() > max_bytes {
        return Err(error(
            GeometryErrorCode::ResourceLimit,
            "source-link coexistent capacity exhausted",
        ));
    }
    Ok(links)
}

fn edge_scratch(definition: &Definition, ordinal: usize) -> Result<usize, GeometryError> {
    let (_, face) = super::source_incidence::native_face(definition, ordinal)?;
    (0..face.loops.len()).try_fold(STACK_SCRATCH_BYTES, |sum, wire| {
        super::source_incidence::boundary_loop(definition, ordinal, wire)?
            .edges
            .len()
            .checked_mul(256)
            .and_then(|bytes| sum.checked_add(bytes))
            .ok_or_else(|| {
                error(
                    GeometryErrorCode::ResourceLimit,
                    "native edge incidence scratch capacity overflowed",
                )
            })
    })
}

fn error(code: GeometryErrorCode, message: &'static str) -> GeometryError {
    GeometryError::new(code, message)
}
fn indeterminate(message: &'static str) -> GeometryError {
    error(GeometryErrorCode::AnalysisIndeterminate, message)
}
fn height_face_context(
    mut error: GeometryError,
    face_index: usize,
    operation: &str,
) -> GeometryError {
    error.message = format!(
        "height-event face {face_index} / {operation}: {}",
        error.message
    );
    error
}
fn dot(a: [Interval; 3], b: [f64; 3]) -> Interval {
    (0..3).fold(Interval::point(0.0), |s, i| {
        s.add(a[i].mul(Interval::point(b[i])))
    })
}
fn height(position: [Interval; 3], frame: PlaneFrameMm) -> Interval {
    dot(
        std::array::from_fn(|i| position[i].sub(Interval::point(frame.origin_mm[i]))),
        frame.z_axis,
    )
}
#[derive(Clone, Copy)]
pub(crate) struct HeightFunctional {
    pub(crate) direction: [Interval; 3],
    pub(crate) offset: Interval,
}
impl HeightFunctional {
    pub(crate) fn from_frame(frame: PlaneFrameMm) -> Self {
        Self {
            direction: frame.z_axis.map(Interval::point),
            offset: tight_dot(frame.origin_mm, frame.z_axis).neg(),
        }
    }
    pub(crate) fn dot(self, vector: [Interval; 3]) -> Interval {
        (0..3).fold(Interval::point(0.0), |s, i| {
            tight_add(s, tight_mul(vector[i], self.direction[i]))
        })
    }
    pub(crate) fn height(self, position: [Interval; 3]) -> Interval {
        tight_add(self.dot(position), self.offset)
    }
    pub(crate) fn vertex(self, position: [f64; 3]) -> Interval {
        self.height(position.map(Interval::point))
    }
}
pub(super) const HEIGHT_CONTRACTION_SCRATCH_BYTES: usize =
    crate::parametric::surface_functional_scratch_bytes::<1>()
        + std::mem::size_of::<crate::parametric::SurfaceFunctional>()
        + 2 * std::mem::size_of::<crate::parametric::SurfaceFunctionalEnclosure>()
        + std::mem::size_of::<crate::parametric::SurfaceEnclosure>()
        + std::mem::size_of::<([[f64; 2]; 2], HeightFunctional, [f64; 2])>();

/// Contract the necessary height preimage, then return a complete spatial jet.
pub(super) fn clip_height_box(
    surface: &(impl SurfaceEvaluator + ?Sized),
    uv: [[f64; 2]; 2],
    functional: HeightFunctional,
    heights: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<([[f64; 2]; 2], crate::parametric::SurfaceEnclosure)>, GeometryError> {
    let Some(uv) = clip_height_parameter_box(surface, uv, functional, heights, work, cancel)?
    else {
        return Ok(None);
    };
    let jet = crate::parametric::surface_piecewise_enclosure(surface, uv, work, cancel)?;
    Ok(Some((uv, jet)))
}

/// Interval Newton contraction in a height-monotone UV direction. Evaluation
/// on the midpoint *strip* retains every value of the other parameter, so this
/// is a whole-domain exclusion/contractor, not a sampled height test.
/// Continuous-source images and one-sided first derivatives suffice; this
/// scalar mean-value bound grants no C1, Hessian, normal or regularity authority.
pub(super) fn clip_height_parameter_box(
    surface: &(impl SurfaceEvaluator + ?Sized),
    mut uv: [[f64; 2]; 2],
    functional: HeightFunctional,
    heights: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[[f64; 2]; 2]>, GeometryError> {
    let scalar = crate::parametric::SurfaceFunctional {
        direction: functional.direction,
        origin: [Interval::point(0.0); 3],
        offset: functional.offset,
    };
    let [mut bound] =
        crate::parametric::surface_functional_enclosures(surface, uv, &[scalar], work, cancel)?;
    for axis in 0..2 {
        let h = bound.position;
        if h.hi < heights[0] || h.lo > heights[1] {
            return Ok(None);
        }
        if heights[0] <= h.lo && h.hi <= heights[1] {
            break;
        }
        let derivative = if axis == 0 { bound.du } else { bound.dv };
        if derivative.contains_zero() {
            continue;
        }
        let mid = uv[axis][0] * 0.5 + uv[axis][1] * 0.5;
        let mut strip = uv;
        strip[axis] = [mid, mid];
        let [value] = crate::parametric::surface_functional_enclosures(
            surface,
            strip,
            &[scalar],
            work,
            cancel,
        )?;
        let height = value.position;
        let contracted = Interval::point(mid).add(
            Interval {
                lo: heights[0],
                hi: heights[1],
            }
            .sub(height)
            .div(derivative)?,
        );
        let previous = uv[axis];
        uv[axis][0] = uv[axis][0].max(contracted.lo);
        uv[axis][1] = uv[axis][1].min(contracted.hi);
        if uv[axis][0] > uv[axis][1] {
            return Ok(None);
        }
        if uv[axis] != previous {
            [bound] = crate::parametric::surface_functional_enclosures(
                surface,
                uv,
                &[scalar],
                work,
                cancel,
            )?;
        }
    }
    if bound.position.hi < heights[0] || bound.position.lo > heights[1] {
        return Ok(None);
    }
    Ok(Some(uv))
}
// Error-free transforms preserve a zero-width event only when binary64
// operations really are exact. They are not magnitude-based snapping.
fn tight_add(a: Interval, b: Interval) -> Interval {
    if a.lo == 0.0 && a.hi == 0.0 {
        return b;
    }
    if b.lo == 0.0 && b.hi == 0.0 {
        return a;
    }
    if a.lo != a.hi || b.lo != b.hi {
        return a.add(b);
    }
    let sum = a.lo + b.lo;
    let shifted = sum - a.lo;
    let residual = (a.lo - (sum - shifted)) + (b.lo - shifted);
    if sum.is_finite() && residual == 0.0 {
        Interval::point(sum)
    } else {
        a.add(b)
    }
}
fn tight_mul(a: Interval, b: Interval) -> Interval {
    if (a.lo == 0.0 && a.hi == 0.0) || (b.lo == 0.0 && b.hi == 0.0) {
        return Interval::point(0.0);
    }
    if a.lo == 1.0 && a.hi == 1.0 {
        return b;
    }
    if b.lo == 1.0 && b.hi == 1.0 {
        return a;
    }
    if a.lo != a.hi || b.lo != b.hi {
        return a.mul(b);
    }
    let product = a.lo * b.lo;
    // A zero FMA residual is not evidence when a 106-bit product's tail can
    // itself underflow. Keep all possible tail bits above the subnormal floor.
    let exact_tail_floor = f64::MIN_POSITIVE * 18_014_398_509_481_984.0;
    if product.is_finite()
        && product.abs() >= exact_tail_floor
        && a.lo.mul_add(b.lo, -product) == 0.0
    {
        Interval::point(product)
    } else {
        a.mul(b)
    }
}
fn tight_div(a: Interval, b: Interval) -> Result<Interval, GeometryError> {
    if a.lo == a.hi && b.lo == b.hi && b.lo != 0.0 {
        let quotient = a.lo / b.lo;
        let exact_tail_floor = f64::MIN_POSITIVE * 18_014_398_509_481_984.0;
        let safe =
            a.lo == 0.0 || (a.lo.abs() >= exact_tail_floor && quotient.abs() >= f64::MIN_POSITIVE);
        if quotient.is_finite() && safe && (-quotient).mul_add(b.lo, a.lo) == 0.0 {
            return Ok(Interval::point(quotient));
        }
    }
    a.div(b)
}
fn equal_coefficient_height(
    first: [f64; 4],
    other: [f64; 4],
    functional: HeightFunctional,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    if functional
        .direction
        .iter()
        .any(|coefficient| coefficient.lo != coefficient.hi)
    {
        return Ok(false);
    }
    super::critical::exact_homogeneous_height_identity(
        first,
        other,
        functional.direction.map(|coefficient| coefficient.lo),
        work,
        cancel,
    )
}
pub(super) fn constant_spline_height(
    surface: &(impl SurfaceEvaluator + ?Sized),
    functional: HeightFunctional,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Interval>, GeometryError> {
    if let Some(native) = surface.native_surface() {
        return constant_surface_height_inner(native, functional, 0, work, cancel);
    }
    let original = surface.original_surface().ok_or_else(|| {
        indeterminate("selected surface has no actual coefficient operand for height identity")
    })?;
    constant_original_surface_height(original, functional, 0, work, cancel)
}
fn constant_surface_height_inner(
    surface: &solidcore::surface::surface::Surface,
    functional: HeightFunctional,
    depth: u8,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Interval>, GeometryError> {
    use solidcore::surface::surface::SurfaceKind;
    work.charge(1, cancel)?;
    if depth >= MAX_DEPTH {
        return Err(indeterminate(
            "native carrier height identity nesting exhausted",
        ));
    }
    match surface.kind() {
        SurfaceKind::Step(_) => {
            if let Some(operand) = native_step_operand(surface)
                && matches!(
                    operand.kind(),
                    SurfaceKind::Plane(_) | SurfaceKind::BSpline(_)
                )
            {
                return constant_surface_height_inner(operand, functional, depth + 1, work, cancel);
            }
            // STEP analytic operands and stored NURBS charts are distinct
            // laws. Prove constancy in the actual wrapper's full active chart,
            // never borrow a fitted control net from its analytic child.
            let domain = surface.natural_uv_domain();
            let lo = domain.min();
            let hi = domain.max();
            let [bound] = crate::parametric::surface_functional_enclosures(
                surface,
                [[lo.x, hi.x], [lo.y, hi.y]],
                &[crate::parametric::SurfaceFunctional {
                    direction: functional.direction,
                    origin: [Interval::point(0.0); 3],
                    offset: functional.offset,
                }],
                work,
                cancel,
            )?;
            return Ok((bound.du.lo == 0.0
                && bound.du.hi == 0.0
                && bound.dv.lo == 0.0
                && bound.dv.hi == 0.0)
                .then_some(bound.position));
        }
        SurfaceKind::CurveBounded(bound) => {
            return constant_surface_height_inner(
                bound.basis(),
                functional,
                depth + 1,
                work,
                cancel,
            );
        }
        SurfaceKind::Plane(plane) => {
            let p = plane.position();
            let u = p.x_axis();
            let v = p.y_axis();
            // Native scale multiplies these axes; exact perpendicularity to
            // both axes proves constancy for every finite UV scale.
            for axis in [u, v] {
                if !equal_coefficient_height(
                    [0.0, 0.0, 0.0, 1.0],
                    [axis.x, axis.y, axis.z, 1.0],
                    functional,
                    work,
                    cancel,
                )? {
                    return Ok(None);
                }
            }
            let o = p.origin();
            return Ok(Some(functional.vertex([o.x, o.y, o.z])));
        }
        _ => {}
    }
    let Some(spline) = crate::parametric::native_surface_nurbs(surface)? else {
        return Ok(None);
    };
    let first = spline
        .cpt(0, 0)
        .map_err(|_| indeterminate("native surface has no coefficients"))?;
    let first_w = if spline.is_rational() { first.w } else { 1.0 };
    if first_w == 0.0 {
        return Ok(None);
    }
    let first_h = [first.x, first.y, first.z, first_w];
    for i in 0..=spline.n() {
        for j in 0..=spline.m() {
            work.charge(1, cancel)?;
            let c = spline
                .cpt(i, j)
                .map_err(|_| indeterminate("native surface coefficient missing"))?;
            let w = if spline.is_rational() { c.w } else { 1.0 };
            if w == 0.0 || (w > 0.0) != (first_w > 0.0) {
                return Ok(None);
            }
            if !equal_coefficient_height(first_h, [c.x, c.y, c.z, w], functional, work, cancel)? {
                return Ok(None);
            }
        }
    }
    Ok(Some(tight_add(
        tight_div(
            functional.dot([first.x, first.y, first.z].map(Interval::point)),
            Interval::point(first_w),
        )?,
        functional.offset,
    )))
}
/// Whole original carrier identity under the actual caller functional. This is
/// also used by placed strata; a near-horizontal source frame is not authority.
pub(super) fn constant_source_face_height(
    definition: &Definition,
    face_index: usize,
    functional: HeightFunctional,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Interval>, GeometryError> {
    work.charge(face_index as u64 + 1, cancel)?;
    let face = *definition
        .face_ids()
        .get(face_index)
        .ok_or_else(|| indeterminate("source face constancy mapping is incomplete"))?;
    constant_original_surface_height(
        definition.original_source().face(face)?.carrier.as_ref(),
        functional,
        0,
        work,
        cancel,
    )
}
pub(super) fn constant_native_curve_height(
    curve: &(impl CurveEvaluator + ?Sized),
    functional: HeightFunctional,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Interval>, GeometryError> {
    if let Some(native) = curve.native_curve() {
        return constant_native_curve_height_inner(native, functional, 0, work, cancel);
    }
    // The provider's selected operator may be S(gamma(t)), not curve_3d.
    // Preserve affine identities using its unrounded composed coefficients.
    if functional
        .direction
        .iter()
        .all(|value| value.lo == value.hi)
        && let Some(coefficients) = curve.exact_affine_coefficients(work, cancel)?
    {
        use crate::exact_coefficient::ExactCoefficient as E;
        if coefficients.denominator.sign() == 0 {
            return Ok(None);
        }
        let mut derivative = E::point(0.0);
        let mut position = E::point(0.0);
        for axis in 0..3 {
            let direction = E::point(functional.direction[axis].lo);
            let Some(d) = coefficients.derivative[axis].product(&direction, work, cancel)? else {
                return Ok(None);
            };
            let Some(p) = coefficients.position[axis].product(&direction, work, cancel)? else {
                return Ok(None);
            };
            for &term in &d.terms[..d.len] {
                if derivative.push(term).is_none() {
                    return Ok(None);
                }
            }
            for &term in &p.terms[..p.len] {
                if position.push(term).is_none() {
                    return Ok(None);
                }
            }
        }
        if derivative.sign() != 0 {
            return Ok(None);
        }
        let mut offset = functional.offset;
        if offset.lo == offset.hi {
            // Recenter before rounding the rational quotient; large absolute
            // source positions must not destroy an exact relative height.
            let Some(projected) =
                coefficients
                    .denominator
                    .product(&E::point(offset.lo), work, cancel)?
            else {
                return Ok(None);
            };
            for &term in &projected.terms[..projected.len] {
                if position.push(term).is_none() {
                    return Ok(None);
                }
            }
            offset = Interval::point(0.0);
        }
        let enclose = |coefficient: &E| -> Result<Interval, GeometryError> {
            let mut result = Interval::point(0.0);
            for &term in &coefficient.terms[..coefficient.len] {
                work.charge(1, cancel)?;
                result = tight_add(result, Interval::point(term));
            }
            Ok(result)
        };
        let denominator = enclose(&coefficients.denominator)?;
        if denominator.contains_zero() {
            return Ok(None);
        }
        let height = tight_add(tight_div(enclose(&position)?, denominator)?, offset);
        if height.lo != height.hi && functional.offset.lo == functional.offset.hi {
            let middle = height.lo * 0.5 + height.hi * 0.5;
            for candidate in [middle, down(middle), up(middle)] {
                if candidate.is_finite()
                    && exact_affine_curve_height_at(
                        curve, 0.0, functional, candidate, work, cancel,
                    )?
                {
                    return Ok(Some(Interval::point(candidate)));
                }
            }
        }
        return Ok(Some(height));
    }
    let original = curve.original_curve().ok_or_else(|| {
        indeterminate("selected curve has no actual coefficient operand for height identity")
    })?;
    if let Some(height) = constant_original_curve_height(original, functional, 0, work, cancel)? {
        return Ok(Some(height));
    }
    // A selected PCurve can be horizontal on a non-horizontal carrier without
    // possessing a rational exact-affine receipt. Prove constancy on the
    // COMPLETE actual operator: C0 plus an identically annihilated first hull,
    // never a midpoint, small derivative or an unselected curve_3d.
    let full = curve.active_range();
    if !curve.continuous(full, work, cancel)? {
        return Ok(None);
    }
    let first = curve.tensor(full, 1, work, cancel)?;
    let derivative = functional.dot(first[1]);
    if derivative.lo != 0.0 || derivative.hi != 0.0 {
        return Ok(None);
    }
    // Only the completed constant-image proof licenses this tight anchor.
    let position = curve.tensor([full[0]; 2], 0, work, cancel)?[0];
    Ok(Some(functional.height(position)))
}
fn constant_native_curve_height_inner(
    curve: &solidcore::curve::curve::Curve,
    functional: HeightFunctional,
    depth: u8,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Interval>, GeometryError> {
    use solidcore::curve::curve::CurveKind;
    work.charge(1, cancel)?;
    if depth >= MAX_DEPTH {
        return Err(indeterminate(
            "native curve height identity nesting exhausted",
        ));
    }
    match curve.kind() {
        CurveKind::Line(line) => {
            let c = line.canonical_unit();
            if !equal_coefficient_height(
                [0.0, 0.0, 0.0, 1.0],
                [c.vector.x, c.vector.y, c.vector.z, 1.0],
                functional,
                work,
                cancel,
            )? {
                return Ok(None);
            }
            return Ok(Some(functional.vertex([c.point.x, c.point.y, c.point.z])));
        }
        CurveKind::CrvOnSurf(curve) => {
            return constant_surface_height_inner(
                curve.surface(),
                functional,
                depth + 1,
                work,
                cancel,
            );
        }
        _ => {}
    }
    let Some(spline) = crate::parametric::native_curve_nurbs(curve)? else {
        return Ok(None);
    };
    let first = spline
        .cpts()
        .first()
        .ok_or_else(|| indeterminate("native curve has no coefficients"))?;
    let first_w = if spline.is_rational() { first.w } else { 1.0 };
    if first_w == 0.0 {
        return Ok(None);
    }
    for c in spline.cpts() {
        work.charge(1, cancel)?;
        let w = if spline.is_rational() { c.w } else { 1.0 };
        if w == 0.0 || (w > 0.0) != (first_w > 0.0) {
            return Ok(None);
        }
        if !equal_coefficient_height(
            [first.x, first.y, first.z, first_w],
            [c.x, c.y, c.z, w],
            functional,
            work,
            cancel,
        )? {
            return Ok(None);
        }
    }
    Ok(Some(tight_add(
        tight_div(
            functional.dot([first.x, first.y, first.z].map(Interval::point)),
            Interval::point(first_w),
        )?,
        functional.offset,
    )))
}

pub(super) fn exact_affine_curve_height_at(
    curve: &(impl CurveEvaluator + ?Sized),
    parameter: f64,
    functional: HeightFunctional,
    target: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    use crate::exact_coefficient::ExactCoefficient as E;
    work.charge(1, cancel)?;
    if functional
        .direction
        .iter()
        .chain([&functional.offset])
        .any(|v| v.lo != v.hi)
    {
        return Ok(false);
    }
    let Some(coefficients) = curve.exact_affine_coefficients(work, cancel)? else {
        return Ok(false);
    };
    if coefficients.denominator.sign() == 0 {
        return Ok(false);
    }
    let mut sum = E::point(0.0);
    for ((mut position, derivative), direction) in coefficients
        .position
        .into_iter()
        .zip(coefficients.derivative)
        .zip(functional.direction)
    {
        let Some(delta) = derivative.product(&E::point(parameter), work, cancel)? else {
            return Ok(false);
        };
        for &term in &delta.terms[..delta.len] {
            if position.push(term).is_none() {
                return Ok(false);
            }
        }
        let Some(projected) = position.product(&E::point(direction.lo), work, cancel)? else {
            return Ok(false);
        };
        if sum.subtract(&projected).is_none() {
            return Ok(false);
        }
    }
    let Some(offset) = E::difference(functional.offset.lo, target) else {
        return Ok(false);
    };
    let Some(offset) = offset.product(&coefficients.denominator, work, cancel)? else {
        return Ok(false);
    };
    if sum.subtract(&offset).is_none() {
        return Ok(false);
    }
    Ok(sum.sign() == 0)
}

pub(super) fn original_endpoint_height(
    curve: &(impl CurveEvaluator + ?Sized),
    parameter: f64,
    declared_point: [f64; 3],
    functional: HeightFunctional,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Interval, GeometryError> {
    let declared = functional.vertex(declared_point);
    if declared.lo == declared.hi
        && exact_affine_curve_height_at(curve, parameter, functional, declared.lo, work, cancel)?
    {
        return Ok(declared);
    }
    if declared.lo == declared.hi {
        let mut exact = true;
        for axis in 0..3 {
            let direction = functional.direction[axis];
            if direction.lo == 0.0 && direction.hi == 0.0 {
                continue;
            }
            if !crate::parametric::curve_endpoint_coordinate_equal(
                curve,
                parameter,
                axis,
                declared_point[axis],
                work,
                cancel,
            )? {
                exact = false;
                break;
            }
        }
        if exact {
            return Ok(declared);
        }
    }
    // Both actual C(t) and the bound original V remain CLOSED strata. A
    // rounded endpoint tensor never becomes an exact scalar cap identity.
    Ok(declared.hull(functional.height(curve.tensor([parameter; 2], 0, work, cancel)?[0])))
}

fn original_control_height(
    mut controls: impl Iterator<Item = ([f64; 3], f64)>,
    functional: HeightFunctional,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Interval>, GeometryError> {
    let (first, first_weight) = controls
        .next()
        .ok_or_else(|| indeterminate("original height identity has no control coefficients"))?;
    for value in first.into_iter().chain([first_weight]) {
        Interval::new(value, value)?;
    }
    if first_weight == 0.0 {
        return Ok(None);
    }
    for (point, weight) in controls {
        work.charge(1, cancel)?;
        for value in point.into_iter().chain([weight]) {
            Interval::new(value, value)?;
        }
        if weight == 0.0 || (weight > 0.0) != (first_weight > 0.0) {
            return Ok(None);
        }
        // Original STEP spline controls are Euclidean, unlike native
        // homogeneous controls. Do not round point*weight before the identity.
        if !equal_coefficient_height(
            [first[0], first[1], first[2], 1.0],
            [point[0], point[1], point[2], 1.0],
            functional,
            work,
            cancel,
        )? {
            return Ok(None);
        }
    }
    Ok(Some(functional.vertex(first)))
}

fn original_cross_height_zero(
    a: [f64; 3],
    b: [f64; 3],
    functional: HeightFunctional,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    use crate::exact_coefficient::ExactCoefficient;
    if functional.direction.iter().any(|v| v.lo != v.hi) {
        return Ok(false);
    }
    let mut sum = ExactCoefficient::point(0.0);
    for k in 0..3 {
        let i = (k + 1) % 3;
        let j = (k + 2) % 3;
        for (left, right, sign) in [(a[i], b[j], 1.0), (a[j], b[i], -1.0)] {
            work.charge(1, cancel)?;
            for value in [left, right, functional.direction[k].lo] {
                Interval::new(value, value)?;
            }
            let Some(product) = ExactCoefficient::point(left).product(
                &ExactCoefficient::point(right),
                work,
                cancel,
            )?
            else {
                return Ok(false);
            };
            let Some(term) = product.product(
                &ExactCoefficient::point(sign * functional.direction[k].lo),
                work,
                cancel,
            )?
            else {
                return Ok(false);
            };
            if sum.subtract(&term).is_none() {
                return Ok(false);
            }
        }
    }
    Ok(sum.sign() == 0)
}

fn original_placement_height_constant(
    placement: &solidcraft_step::geom::Placement,
    functional: HeightFunctional,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let x = placement.ref_direction;
    Ok(equal_coefficient_height(
        [0.0, 0.0, 0.0, 1.0],
        [x[0], x[1], x[2], 1.0],
        functional,
        work,
        cancel,
    )? && original_cross_height_zero(placement.axis, x, functional, work, cancel)?)
}

fn constant_original_curve_height(
    curve: &solidcraft_step::geom::Curve,
    functional: HeightFunctional,
    depth: u8,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Interval>, GeometryError> {
    use solidcraft_step::geom::Curve;
    work.charge(1, cancel)?;
    if depth >= MAX_DEPTH {
        return Err(indeterminate(
            "original curve height identity nesting exhausted",
        ));
    }
    match curve {
        Curve::Line {
            origin, direction, ..
        } => {
            let d = *direction;
            Ok(equal_coefficient_height(
                [0.0, 0.0, 0.0, 1.0],
                [d[0], d[1], d[2], 1.0],
                functional,
                work,
                cancel,
            )?
            .then(|| functional.vertex(*origin)))
        }
        Curve::Circle { position, .. }
        | Curve::Ellipse { position, .. }
        | Curve::Parabola { position, .. }
        | Curve::Hyperbola { position, .. } => Ok(original_placement_height_constant(
            position, functional, work, cancel,
        )?
        .then(|| functional.vertex(position.origin))),
        Curve::BSpline(spline) => {
            if spline
                .weights
                .as_ref()
                .is_some_and(|weights| weights.len() != spline.points.len())
            {
                return Err(indeterminate(
                    "original curve height coefficient census mismatch",
                ));
            }
            original_control_height(
                spline.points.iter().enumerate().map(|(i, point)| {
                    (
                        *point,
                        spline.weights.as_ref().map_or(1.0, |weights| weights[i]),
                    )
                }),
                functional,
                work,
                cancel,
            )
        }
        Curve::Polyline(points) => original_control_height(
            points.iter().map(|point| (*point, 1.0)),
            functional,
            work,
            cancel,
        ),
        Curve::Trimmed { basis, .. } => {
            constant_original_curve_height(basis, functional, depth + 1, work, cancel)
        }
        Curve::SurfaceCurve { curve_3d, .. } => {
            if let Some(pcurve) = crate::kernel::source_operand::preferred_source_pcurve(curve)? {
                constant_original_surface_height(
                    &pcurve.surface,
                    functional,
                    depth + 1,
                    work,
                    cancel,
                )
            } else {
                constant_original_curve_height(curve_3d, functional, depth + 1, work, cancel)
            }
        }
        _ => Ok(None),
    }
}

fn constant_original_surface_height(
    surface: &solidcraft_step::geom::Surface,
    functional: HeightFunctional,
    depth: u8,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Interval>, GeometryError> {
    use solidcraft_step::geom::Surface;
    work.charge(1, cancel)?;
    if depth >= MAX_DEPTH {
        return Err(indeterminate(
            "original carrier height identity nesting exhausted",
        ));
    }
    match surface {
        Surface::Plane { position } => Ok(original_placement_height_constant(
            position, functional, work, cancel,
        )?
        .then(|| functional.vertex(position.origin))),
        Surface::BSpline(spline) => {
            if spline.weights.as_ref().is_some_and(|weights| {
                weights.len() != spline.points.len()
                    || weights
                        .iter()
                        .zip(&spline.points)
                        .any(|(a, b)| a.len() != b.len())
            }) {
                return Err(indeterminate(
                    "original surface height coefficient census mismatch",
                ));
            }
            original_control_height(
                spline.points.iter().enumerate().flat_map(|(i, row)| {
                    row.iter().enumerate().map(move |(j, point)| {
                        (
                            *point,
                            spline.weights.as_ref().map_or(1.0, |weights| weights[i][j]),
                        )
                    })
                }),
                functional,
                work,
                cancel,
            )
        }
        Surface::Extrusion {
            profile, direction, ..
        } => {
            let d = *direction;
            if !equal_coefficient_height(
                [0.0, 0.0, 0.0, 1.0],
                [d[0], d[1], d[2], 1.0],
                functional,
                work,
                cancel,
            )? {
                return Ok(None);
            }
            constant_original_curve_height(profile, functional, depth + 1, work, cancel)
        }
        Surface::Revolution { profile, axis } => {
            if !super::critical::exact_unit_vector_identity(axis.direction, work, cancel)? {
                return Ok(None);
            }
            for k in 0..3 {
                let i = (k + 1) % 3;
                let j = (k + 2) % 3;
                let mut perpendicular = [0.0, 0.0, 0.0, 1.0];
                perpendicular[i] = axis.direction[j];
                perpendicular[j] = -axis.direction[i];
                if !equal_coefficient_height(
                    [0.0, 0.0, 0.0, 1.0],
                    perpendicular,
                    functional,
                    work,
                    cancel,
                )? {
                    return Ok(None);
                }
            }
            constant_original_curve_height(profile, functional, depth + 1, work, cancel)
        }
        Surface::RectangularTrimmed { basis, .. } => {
            constant_original_surface_height(basis, functional, depth + 1, work, cancel)
        }
        _ => Ok(None),
    }
}
fn flat_native_surface_height(
    surface: &(impl SurfaceEvaluator + ?Sized),
    frame: PlaneFrameMm,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Interval>, GeometryError> {
    if let Some(h) = flat_native_plane_height(surface, frame) {
        return Ok(Some(h));
    }
    constant_spline_height(surface, HeightFunctional::from_frame(frame), work, cancel)
}
fn vertex_height(point: [f64; 3], frame: PlaneFrameMm) -> Interval {
    (0..3).fold(Interval::point(0.0), |sum, i| {
        tight_add(
            sum,
            tight_mul(
                tight_add(
                    Interval::point(point[i]),
                    Interval::point(-frame.origin_mm[i]),
                ),
                Interval::point(frame.z_axis[i]),
            ),
        )
    })
}
fn flat_native_plane_height(
    surface: &(impl SurfaceEvaluator + ?Sized),
    frame: PlaneFrameMm,
) -> Option<Interval> {
    let SurfaceKind::Plane(plane) = surface.native_surface()?.kind() else {
        return None;
    };
    let axis = (0..3).find(|&i| frame.z_axis[i].abs() == 1.0)?;
    if (0..3).any(|i| i != axis && frame.z_axis[i] != 0.0) {
        return None;
    }
    let position = plane.position();
    let u = position.x_axis();
    let v = position.y_axis();
    let o = position.origin();
    let u = [u.x, u.y, u.z];
    let v = [v.x, v.y, v.z];
    (u[axis] == 0.0 && v[axis] == 0.0).then(|| vertex_height([o.x, o.y, o.z], frame))
}
fn tight_dot(a: [f64; 3], b: [f64; 3]) -> Interval {
    (0..3).fold(Interval::point(0.0), |s, i| {
        tight_add(s, tight_mul(Interval::point(a[i]), Interval::point(b[i])))
    })
}
/// Native torus parameter semantics, not a descriptive family permission.
/// An aligned height is c+a*sin(v). Its only interior critical values are
/// c+-a, including the complete stationary circles.
fn torus_stationary_values(
    surface: &(impl SurfaceEvaluator + ?Sized),
    frame: PlaneFrameMm,
) -> Option<([Interval; 2], usize)> {
    if let Some(mut original) = surface.original_surface() {
        for _ in 0..MAX_DEPTH {
            match original {
                solidcraft_step::geom::Surface::RectangularTrimmed { basis, .. } => {
                    original = basis
                }
                solidcraft_step::geom::Surface::Torus {
                    position,
                    major_radius,
                    minor_radius,
                } => {
                    if major_radius <= minor_radius {
                        return None;
                    }
                    let x = position.ref_direction;
                    let z = position.axis;
                    let y = tight_cross_points(z, x);
                    let a = tight_dot(x, frame.z_axis);
                    let b = (0..3).fold(Interval::point(0.0), |s, k| {
                        tight_add(s, tight_mul(y[k], Interval::point(frame.z_axis[k])))
                    });
                    if a.lo != 0.0 || a.hi != 0.0 || b.lo != 0.0 || b.hi != 0.0 {
                        return None;
                    }
                    let c = tight_dot(z, frame.z_axis);
                    if c.contains_zero() {
                        return None;
                    }
                    let offset = vertex_height(position.origin, frame);
                    let amplitude = tight_mul(c, Interval::point(*minor_radius));
                    return Some((
                        [
                            tight_add(offset, amplitude),
                            tight_add(offset, amplitude.neg()),
                        ],
                        1,
                    ));
                }
                _ => return None,
            }
        }
        return None;
    }
    let surface = native_step_operand(surface.native_surface()?)?;
    let SurfaceKind::Torus(torus) = surface.kind() else {
        return None;
    };
    if torus.major_radius() <= torus.minor_radius() {
        return None;
    }
    let p = torus.base().position();
    let x = p.x_axis();
    let y = p.y_axis();
    let z = p.z_axis();
    let a = tight_dot([x.x, x.y, x.z], frame.z_axis);
    let b = tight_dot([y.x, y.y, y.z], frame.z_axis);
    if a.lo != 0.0 || a.hi != 0.0 || b.lo != 0.0 || b.hi != 0.0 {
        return None;
    }
    let c = tight_dot([z.x, z.y, z.z], frame.z_axis);
    if c.contains_zero() {
        return None;
    }
    let o = p.origin();
    let offset = vertex_height([o.x, o.y, o.z], frame);
    let amplitude = tight_mul(c, Interval::point(torus.minor_radius()));
    // The literal STEP torus chart has its meridional sine on V. No
    // descriptive family or NURBS parameter inversion establishes this index.
    Some((
        [
            tight_add(offset, amplitude),
            tight_add(offset, amplitude.neg()),
        ],
        1,
    ))
}
fn native_step_operand(mut surface: &Surface) -> Option<&Surface> {
    let mut step = false;
    for _ in 0..MAX_DEPTH {
        match surface.kind() {
            SurfaceKind::Step(wrapper) => {
                surface = wrapper.surface()?;
                step = true;
            }
            SurfaceKind::CurveBounded(bound) => surface = bound.basis(),
            _ => return step.then_some(surface),
        }
    }
    None
}
fn tight_cross_points(a: [f64; 3], b: [f64; 3]) -> [Interval; 3] {
    std::array::from_fn(|k| {
        let i = (k + 1) % 3;
        let j = (k + 2) % 3;
        tight_add(
            tight_mul(Interval::point(a[i]), Interval::point(b[j])),
            tight_mul(Interval::point(a[j]), Interval::point(b[i])).neg(),
        )
    })
}
fn cross(a: [Interval; 3], b: [Interval; 3]) -> [Interval; 3] {
    [
        a[1].mul(b[2]).sub(a[2].mul(b[1])),
        a[2].mul(b[0]).sub(a[0].mul(b[2])),
        a[0].mul(b[1]).sub(a[1].mul(b[0])),
    ]
}
fn norm(a: [Interval; 3]) -> Result<Interval, GeometryError> {
    // Addition's outward padding can make an exact sum of zeros negative.
    let squared = a
        .into_iter()
        .fold(Interval::point(0.0), |s, x| s.add(x.square()));
    Interval {
        lo: squared.lo.max(0.0),
        hi: squared.hi.max(0.0),
    }
    .sqrt()
}
fn overlaps(a: Interval, b: [f64; 2]) -> bool {
    a.lo <= b[1] && a.hi >= b[0]
}
fn validate(frame: PlaneFrameMm, range: [f64; 2], uncertainty: f64) -> Result<(), GeometryError> {
    frame.validate()?;
    if !range.iter().all(|x| x.is_finite())
        || range[0] >= range[1]
        || !uncertainty.is_finite()
        || uncertainty <= 0.0
    {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "height analysis requires a finite increasing interval and positive uncertainty",
        ));
    }
    Ok(())
}
fn split_uv(uv: [[f64; 2]; 2], depth: u8) -> Result<[[[f64; 2]; 2]; 2], GeometryError> {
    let axis = usize::from(uv[1][1] - uv[1][0] > uv[0][1] - uv[0][0]);
    split_uv_axis(uv, depth, axis)
}
fn split_uv_axis(
    uv: [[f64; 2]; 2],
    depth: u8,
    axis: usize,
) -> Result<[[[f64; 2]; 2]; 2], GeometryError> {
    if depth >= MAX_DEPTH {
        return Err(indeterminate(
            "height patch could not be resolved within the bounded subdivision depth",
        ));
    }
    let mid = uv[axis][0] * 0.5 + uv[axis][1] * 0.5;
    if mid <= uv[axis][0] || mid >= uv[axis][1] {
        return Err(indeterminate(
            "height patch subdivision lost parameter resolution",
        ));
    }
    let mut children = [uv; 2];
    children[0][axis][1] = mid;
    children[1][axis][0] = mid;
    Ok(children)
}
fn add_event(
    events: &mut Vec<[f64; 2]>,
    value: Interval,
    range: [f64; 2],
    uncertainty: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    if !value.lo.is_finite() || !value.hi.is_finite() {
        return Err(error(
            GeometryErrorCode::ConditioningFailure,
            "critical-height arithmetic is nonfinite",
        ));
    }
    if !overlaps(value, range) {
        return Ok(());
    }
    if value.width() > uncertainty {
        return Err(indeterminate(
            "critical-height stratum exceeds the requested height uncertainty",
        ));
    }
    if events.len() >= MAX_EVENT_BRACKETS {
        return Err(error(
            GeometryErrorCode::ResourceLimit,
            "critical-height bracket capacity exhausted",
        ));
    }
    work.charge(1, cancel)?;
    events.try_reserve(1).map_err(|_| {
        error(
            GeometryErrorCode::ResourceLimit,
            "critical-height bracket allocation refused",
        )
    })?;
    // Brackets enclose the entire critical value; clipping to the query range
    // would discard the uncertainty at a range endpoint.
    events.push([value.lo, value.hi]);
    Ok(())
}

/// Retain actual source-owned collar, edge-end and joint-star supports.
/// The provider evaluates the full unnormalized scalar covector; only the
/// affine height offset is applied here. No whole-face blanket or fabricated
/// root subdivisions replace these physical supports.
fn material_transport_height_cover(
    source: super::material_source::MaterialSourceView<'_, '_>,
    functional: HeightFunctional,
    height_interval_mm: [f64; 2],
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Vec<super::SourceTransportCover>, GeometryError> {
    let mut covers = super::critical::material_link::source_transport_supports(
        source,
        functional.direction,
        None,
        remaining_bytes,
        work,
        cancel,
    )?;
    if covers.len() > MAX_EVENT_BRACKETS {
        return Err(error(
            GeometryErrorCode::ResourceLimit,
            "material transport support count exceeds cap",
        ));
    }
    remaining_scratch_with_limit(
        remaining_bytes,
        covers
            .capacity()
            .checked_mul(std::mem::size_of::<super::SourceTransportCover>())
            .ok_or_else(|| {
                error(
                    GeometryErrorCode::ResourceLimit,
                    "material transport support capacity overflow",
                )
            })?,
    )?;
    for cover in &mut covers {
        work.charge(1, cancel)?;
        let base =
            Interval::new(cover.base_interval[0], cover.base_interval[1])?.add(functional.offset);
        let corrected = Interval::new(cover.corrected_interval[0], cover.corrected_interval[1])?
            .add(functional.offset);
        let base = Interval::new(base.lo, base.hi)?;
        let corrected = Interval::new(corrected.lo, corrected.hi)?;
        Interval::new(
            cover.displacement_interval[0],
            cover.displacement_interval[1],
        )?;
        if !cover.source_error_mm.is_finite() || cover.source_error_mm < 0.0 {
            return Err(error(
                GeometryErrorCode::ConditioningFailure,
                "invalid material transport physical error",
            ));
        }
        cover.base_interval = [base.lo, base.hi];
        cover.corrected_interval = [corrected.lo, corrected.hi];
    }
    covers.retain(|cover| {
        cover.base_interval[0].min(cover.corrected_interval[0]) <= height_interval_mm[1]
            && height_interval_mm[0] <= cover.base_interval[1].max(cover.corrected_interval[1])
    });
    Ok(covers)
}

/// Source-owned proof that the complete correction support misses this exact
/// CLOSED range. Scalar displacement zero alone cannot construct this receipt:
/// an active XY correction can still change physical continuation speed.
struct MaterialTransportExclusion<'d> {
    definition_id: &'d spiling_contracts::geometry::DefinitionId,
    material_realization: crate::kernel::MaterialRealizationIdentity,
    frame: PlaneFrameMm,
    range: [f64; 2],
}
fn exclude_material_transport<'d>(
    source: super::material_source::MaterialSourceView<'_, 'd>,
    frame: PlaneFrameMm,
    range: [f64; 2],
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<MaterialTransportExclusion<'d>>, GeometryError> {
    let supports = material_transport_height_cover(
        source,
        HeightFunctional::from_frame(frame),
        range,
        remaining_bytes,
        work,
        cancel,
    )?;
    if !supports.is_empty() {
        return Ok(None);
    }
    let definition = source.definition();
    Ok(Some(MaterialTransportExclusion {
        definition_id: definition.id(),
        material_realization: crate::kernel::MaterialRealizationIdentity::new(definition),
        frame,
        range,
    }))
}

/// Complete conservative isolation of selected original critical-height strata.
/// Material completeness additionally retains the complete source realization's
/// collar/cone transport support on HeightCapture. Those supports are not
/// isolated roots and do not authorize regular continuation through their bands.
pub fn analyze_height_domain(
    definition: &Definition,
    frame: PlaneFrameMm,
    height_interval_mm: [f64; 2],
    policy: AnalysisPolicy,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<HeightAnalysis, GeometryError> {
    analyze_height_domain_with_total_limit(
        definition,
        frame,
        height_interval_mm,
        policy,
        MAX_SLAB_SCRATCH_BYTES,
        work,
        cancel,
    )
}
fn analyze_height_domain_with_total_limit(
    definition: &Definition,
    frame: PlaneFrameMm,
    height_interval_mm: [f64; 2],
    policy: AnalysisPolicy,
    total_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<HeightAnalysis, GeometryError> {
    work.charge(1, cancel)?;
    let prepared = super::material_source::prepare_solid(definition, total_bytes, work, cancel)?;
    let remaining_bytes = remaining_scratch_with_limit(
        total_bytes.min(MAX_SLAB_SCRATCH_BYTES),
        prepared.retained_bytes(),
    )?;
    analyze_height_domain_with_scratch_limit(
        prepared.source_view()?,
        frame,
        height_interval_mm,
        policy,
        remaining_bytes,
        work,
        cancel,
    )
}
pub(crate) struct HeightDomainQuery<'s, 'd> {
    pub(crate) source: super::material_source::MaterialSourceView<'s, 'd>,
    pub(crate) frame: PlaneFrameMm,
    pub(crate) functional: HeightFunctional,
    pub(crate) height_interval_mm: [f64; 2],
    pub(crate) policy: AnalysisPolicy,
}

/// `remaining_bytes` excludes the already reserved pinned definition proof and
/// other coexisting buffers. It covers event arrays, trim/curve captures,
/// source incidence tables and subdivision stacks; never debit pinned twice.
pub(crate) fn analyze_height_domain_with_scratch_limit(
    source: super::material_source::MaterialSourceView<'_, '_>,
    frame: PlaneFrameMm,
    height_interval_mm: [f64; 2],
    policy: AnalysisPolicy,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<HeightAnalysis, GeometryError> {
    analyze_height_functional(
        &HeightDomainQuery {
            source,
            frame,
            functional: HeightFunctional::from_frame(frame),
            height_interval_mm,
            policy,
        },
        remaining_bytes,
        work,
        cancel,
        true,
    )
}

/// Scratch-only boundary: the caller has already reserved the pinned proof.
pub(crate) fn analyze_height_domain_enclosed_with_scratch_limit(
    query: &HeightDomainQuery<'_, '_>,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<HeightAnalysis, GeometryError> {
    let HeightDomainQuery {
        frame, functional, ..
    } = *query;
    let nominal = HeightFunctional::from_frame(frame);
    let exact_nominal = (0..3).all(|i| {
        functional.direction[i].lo == frame.z_axis[i]
            && functional.direction[i].hi == frame.z_axis[i]
    }) && nominal.offset.lo == nominal.offset.hi
        && functional.offset.lo == nominal.offset.lo
        && functional.offset.hi == nominal.offset.hi;
    analyze_height_functional(query, remaining_bytes, work, cancel, exact_nominal)
}
fn analyze_height_functional(
    query: &HeightDomainQuery<'_, '_>,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    nominal_fast_paths: bool,
) -> Result<HeightAnalysis, GeometryError> {
    let HeightDomainQuery {
        source,
        frame,
        functional,
        height_interval_mm,
        policy,
    } = *query;
    let definition = source.definition();
    work.charge(1, cancel)?;
    validate(
        frame,
        height_interval_mm,
        policy.maximum_height_uncertainty_mm,
    )?;
    for coefficient in functional.direction.into_iter().chain([functional.offset]) {
        Interval::new(coefficient.lo, coefficient.hi)?;
    }
    if norm(functional.direction)?.lo <= 0.0 {
        return Err(error(
            GeometryErrorCode::ConditioningFailure,
            "enclosed height direction is not proved nonzero",
        ));
    }
    if !policy.maximum_shape_error_mm.is_finite() || policy.maximum_shape_error_mm <= 0.0 {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "native shape uncertainty must be positive and finite",
        ));
    }
    let scratch_limit = remaining_scratch_with_limit(
        remaining_bytes.min(MAX_SLAB_SCRATCH_BYTES),
        definition.id().allocated_bytes(),
    )?;
    remaining_scratch_with_limit(
        scratch_limit,
        HEIGHT_EVENT_SCRATCH_BYTES + STACK_SCRATCH_BYTES,
    )?;
    // The optional exact boundary-flow certificate runs before event output.
    // Only a full direct-query allowance enables this optional proof, but its
    // trim/curve captures still share the allowance remaining after pinning.
    let full_scratch = MAX_SLAB_SCRATCH_BYTES
        .checked_sub(source.retained_owner_bytes())
        .and_then(|bytes| bytes.checked_sub(definition.id().allocated_bytes()));
    let material_transport_zero = source.material_transport_is_proved_zero(work, cancel)?;
    let invariance =
        if material_transport_zero && nominal_fast_paths && Some(scratch_limit) == full_scratch {
            narrow_invariance_with_scratch_limit(
                source,
                frame,
                height_interval_mm,
                scratch_limit,
                work,
                cancel,
            )?
        } else {
            None
        };
    let scratch_limit = remaining_scratch_with_limit(
        scratch_limit,
        invariance
            .as_ref()
            .map_or(0, |_| definition.id().allocated_bytes()),
    )?;
    let mut events = Vec::new();
    let mut proved_events = Vec::new();
    let transport_before = work.consumed();
    let transport_support = if material_transport_zero {
        Vec::new()
    } else {
        material_transport_height_cover(
            source,
            functional,
            height_interval_mm,
            remaining_scratch_with_limit(
                scratch_limit,
                HEIGHT_EVENT_SCRATCH_BYTES + STACK_SCRATCH_BYTES,
            )?,
            work,
            cancel,
        )
        .map_err(|error| {
            work.annotate_error(error, transport_before, "material height transport support")
        })?
    };
    let scratch_limit = remaining_scratch_with_limit(
        scratch_limit,
        transport_support.capacity() * std::mem::size_of::<super::SourceTransportCover>(),
    )?;
    let source_before = work.consumed();
    if let Some(round) = if nominal_fast_paths {
        source_round_continuation(
            source,
            frame,
            remaining_scratch_with_limit(scratch_limit, HEIGHT_EVENT_SCRATCH_BYTES)?,
            work,
            cancel,
        )
        .map_err(|error| {
            work.annotate_error(
                error,
                source_before,
                "material height census round continuation after source transport",
            )
        })?
    } else {
        None
    } {
        for value in round_height_support(&round) {
            add_event(
                &mut events,
                value,
                height_interval_mm,
                policy.maximum_height_uncertainty_mm,
                work,
                cancel,
            )?;
            add_event(
                &mut proved_events,
                value,
                height_interval_mm,
                policy.maximum_height_uncertainty_mm,
                work,
                cancel,
            )?;
        }
        return Ok(HeightAnalysis {
            event_brackets_mm: events,
            proved_event_brackets_mm: proved_events,
            invariance: None,
            capture: super::HeightCapture {
                definition_id: definition.id().clone(),
                material_realization: crate::kernel::MaterialRealizationIdentity::new(definition),
                frame,
                height_interval_mm,
                policy,
                pose: None,
                transport_support,
                transport_support_complete: true,
            },
        });
    }
    let mut source_links: Option<HeightSourceLinks<'_>> = None;
    for index in 0..definition.face_ids().len() {
        let face_before = work.consumed();
        work.charge(1, cancel).map_err(|error| {
            work.annotate_error(
                error,
                face_before,
                format_args!("material height census source_face={index} begin face"),
            )
        })?;
        let face_reserved =
            HEIGHT_EVENT_SCRATCH_BYTES.saturating_add(edge_scratch(definition, index)?);
        let reserved = face_reserved.saturating_add(
            source_links
                .as_ref()
                .map_or(0, HeightSourceLinks::owned_bytes),
        );
        let domain = source
            .face_domain(
                index,
                remaining_scratch_with_limit(scratch_limit, reserved)?,
                work,
                cancel,
            )
            .map_err(|error| {
                work.annotate_error(
                    error,
                    face_before,
                    format_args!(
                        "material height census source_face={index} prepare original trim domain"
                    ),
                )
            })?;
        let bounded_uv = trim_parameter_box(&domain)?;
        let mut stage = "source face qualification";
        let mut curve_owner: Option<(usize, usize)> = None;
        let mut curve_window: Option<(f64, f64)> = None;
        let mut surface_window = bounded_uv;
        let mut vertex_point: Option<[f64; 3]> = None;
        super::critical::with_original_source_face(&domain, bounded_uv, |original_face| -> Result<(), GeometryError> {
        let surface = original_face.surface();
        let flat_height = if nominal_fast_paths { flat_native_surface_height(surface, frame, work, cancel)? } else { constant_spline_height(surface, functional, work, cancel)? };
        let glued = if flat_height.is_none() { original_face.prove_gluing(work, cancel)? } else { None };
        if flat_height.is_none() && glued.is_none() {
            if !overlaps(functional.height(crate::parametric::surface_piecewise_enclosure(surface, bounded_uv, work, cancel)?.position),
                height_interval_mm) {
                return Ok(());
            }
            return Err(indeterminate("height-event census lacks whole-source C1/G1 or complete internal C0 knot incidence"));
        }
        let torus_values = if nominal_fast_paths { torus_stationary_values(surface, frame) } else { None };
        let mut torus_candidates = [false; 2];
        let mut torus_proved = [false; 2];
        if let Some(h) = flat_height {
            add_event(&mut events, h, height_interval_mm, policy.maximum_height_uncertainty_mm, work, cancel)?;
            add_event(&mut proved_events, h, height_interval_mm, policy.maximum_height_uncertainty_mm, work, cancel)?;
        }
        let mut spans = surface_parameter_boxes(surface, bounded_uv, work, cancel)?;
        let mut stack = Vec::new();
        while let Some((uv, depth)) = stack.pop().or_else(|| if flat_height.is_none() { spans.next().map(|uv| (uv, 0u8)) } else { None }) {
            stage = "source face interior";
            surface_window = uv;
            work.charge(1, cancel)?;
            // Carrier height/gradient exclusion is independent of trim
            // membership. Do not refine winding near a source boundary for a
            // cell which cannot contain an interior height event.
            let jet = match surface_enclosure(surface, uv, work, cancel) {
                Ok(jet) => jet,
                Err(error) => {
                    if !matches!(error.code, GeometryErrorCode::Cancelled | GeometryErrorCode::ResourceLimit) {
                        // An excluded trim cell need not have an evaluable
                        // carrier (for example across a rational pole).
                        let classification = domain.classify(uv, work, cancel).map_err(|error| {
                            height_face_context(error, index, &format!("classify original trim cell {uv:?}"))
                        })?;
                        if matches!(classification, TrimClassification::Outside) { continue; }
                    }
                    return Err(height_face_context(error, index, &format!("enclose carrier cell {uv:?}")));
                }
            };
            let h = functional.height(jet.position);
            if !overlaps(h, height_interval_mm) { continue; }
            let projected_du = functional.dot(jet.du);
            let projected_dv = functional.dot(jet.dv);
            if !projected_du.contains_zero() || !projected_dv.contains_zero() { continue; }
            let classification = domain.classify(uv, work, cancel).map_err(|error| {
                height_face_context(error, index, &format!("classify original trim cell {uv:?}"))
            })?;
            if matches!(classification, TrimClassification::Outside) { continue; }
            // This also covers collapsed pole charts, horizontal patches and
            // nonisolated critical sets: none is normalized into a fake normal.
            if h.width() <= policy.maximum_height_uncertainty_mm {
                let Some(critical_uv) = super::critical::necessary_stationary_domain(
                    surface, uv, &jet, functional, work, cancel,
                )? else { continue; };
                let contracted_jet = if critical_uv == uv {
                    None
                } else {
                    Some(surface_enclosure(surface, critical_uv, work, cancel)?)
                };
                let jet = contracted_jet.as_ref().unwrap_or(&jet);
                let uv = critical_uv;
                let critical_height = functional.height(jet.position);
                let h = Interval {
                    lo: h.lo.max(critical_height.lo),
                    hi: h.hi.min(critical_height.hi),
                };
                if h.lo > h.hi { continue; }
                if let Some((values, phase_axis)) = torus_values {
                    let pi = Interval { lo: down(std::f64::consts::PI), hi: up(std::f64::consts::PI) };
                    for (event, value) in values.into_iter().enumerate() {
                        if !overlaps(value, [h.lo, h.hi]) { continue; }
                        torus_candidates[event] = true;
                        add_event(&mut events, value, height_interval_mm, policy.maximum_height_uncertainty_mm, work, cancel)?;
                        // One feasible strip is an existence witness for this
                        // critical circle. Artificial periodic seams need not
                        // themselves be classified as physical boundaries.
                        let phase = pi.mul(Interval::point(if event == 0 { 0.5 } else { 1.5 }));
                        let bounds = domain.uv_domain()[phase_axis];
                        let first = ((bounds[0] - phase.hi) / std::f64::consts::TAU).floor() as i32;
                        let last = ((bounds[1] - phase.lo) / std::f64::consts::TAU).ceil() as i32;
                        if last.saturating_sub(first) > 8 { return Err(indeterminate("periodic critical-height branch ownership is unbounded")); }
                        for turn in first..=last {
                            let lifted = phase.add(pi.mul(Interval::point(2.0 * f64::from(turn))));
                            if lifted.lo < bounds[0] || lifted.hi > bounds[1] { continue; }
                            let other = 1 - phase_axis;
                            let mid = uv[other][0] * 0.5 + uv[other][1] * 0.5;
                            let mut root = [[mid, mid]; 2];
                            root[phase_axis] = [lifted.lo, lifted.hi];
                            if matches!(domain.classify(root, work, cancel)?, TrimClassification::Inside) {
                                torus_proved[event] = true;
                                add_event(&mut proved_events, value, height_interval_mm, policy.maximum_height_uncertainty_mm, work, cancel)?;
                            }
                        }
                    }
                } else {
                    add_event(&mut events, h, height_interval_mm, policy.maximum_height_uncertainty_mm, work, cancel)?;
                    if matches!(classification, TrimClassification::Inside)
                        && let Some(root_height) = super::critical::proved_stationary_height(surface, uv, jet, functional, work, cancel)? {
                            add_event(&mut events, root_height, height_interval_mm, policy.maximum_height_uncertainty_mm, work, cancel)?;
                            add_event(&mut proved_events, root_height, height_interval_mm, policy.maximum_height_uncertainty_mm, work, cancel)?;
                    }
                }
            } else {
                work.charge(2, cancel)?;
                // Match directional-bound search: do not refine a constant-
                // height generator merely because its parameter is wider.
                // This only orders a complete partition; no proof is skipped.
                let priority = [projected_du, projected_dv].map(|derivative| {
                    derivative.lo.abs().max(derivative.hi.abs())
                });
                let priority = [
                    priority[0] * (uv[0][1] - uv[0][0]),
                    priority[1] * (uv[1][1] - uv[1][0]),
                ];
                let children = if priority.iter().all(|value| value.is_finite())
                    && priority.into_iter().any(|value| value > 0.0)
                {
                    split_uv_axis(uv, depth, usize::from(priority[1] > priority[0]))?
                } else {
                    split_uv(uv, depth)?
                };
                for child in children { stack.push((child, depth + 1)); }
            }
        }
        // Potential C0 interfaces are lower-dimensional strata. Multiplicity
        // alone does not prove an actual geometric crease or event.
        for crease in surface_crease_interfaces(surface, bounded_uv, work, cancel)? {
            stage = "source face crease";
            surface_window = crease;
            if crease.iter().all(|axis| axis[0] != axis[1]) {
                return Err(indeterminate("source reflected knot band has no proved fixed chart coordinate"));
            }
            let moving_axis = usize::from(crease[0][0] == crease[0][1]);
            for endpoint in crease[moving_axis] {
                let mut uv = crease;
                uv[moving_axis] = [endpoint, endpoint];
                if !matches!(domain.classify(uv, work, cancel)?, TrimClassification::Outside) {
                    let jet = surface_enclosure(surface, uv, work, cancel)?;
                    let height = functional.height(jet.position);
                    // This excludes only the artificial parameter-interface
                    // endpoint. Original physical coedges/vertices are still
                    // inventoried independently below, including true corners.
                    if overlaps(height, height_interval_mm)
                        && let Some(glued) = glued.as_ref()
                        && source_height_transverse(glued, uv, functional, work, cancel)? {
                        continue;
                    }
                    add_event(&mut events, height, height_interval_mm, policy.maximum_height_uncertainty_mm, work, cancel)?;
                }
            }
            let mut cells = vec![(crease, 0u8)];
            while let Some((uv, depth)) = cells.pop() {
                work.charge(1, cancel)?;
                if matches!(domain.classify(uv, work, cancel)?, TrimClassification::Outside) { continue; }
                let jet = surface_enclosure(surface, uv, work, cancel)?;
                let h = functional.height(jet.position);
                let tangent = if moving_axis == 0 { jet.du } else { jet.dv };
                if !overlaps(h, height_interval_mm) || !functional.dot(tangent).contains_zero() { continue; }
                if h.width() <= policy.maximum_height_uncertainty_mm {
                    // The whole original face is positively G1 here. A
                    // nonzero geometric height differential on this COMPLETE
                    // UV cell excludes a parameter-crease alias, not a source
                    // trim or a parallel-normal critical stratum.
                    if let Some(glued) = glued.as_ref()
                        && source_height_transverse(glued, uv, functional, work, cancel)? {
                        continue;
                    }
                    add_event(&mut events, h, height_interval_mm, policy.maximum_height_uncertainty_mm, work, cancel)?;
                } else {
                    work.charge(2, cancel)?;
                    for child in split_uv(uv, depth)? { cells.push((child, depth + 1)); }
                }
            }
        }
        for (flat, coedge) in domain.coedges().iter().enumerate() {
                let identity = coedge.identity();
                let source_proof = domain.original_coedge_correspondence(flat)?;
                let binding = source_proof.binding().ok_or_else(|| indeterminate(
                    "original coedge has no selected source vertex binding",
                ))?;
                let active = source_proof.spatial_range();
                let curve_range = (active[0], active[1]);
                let mut census = |curve: &dyn CurveEvaluator,
                    work: &mut GeometryWorkBudget, cancel: &AtomicBool| -> Result<(), GeometryError> {
                stage = "source coedge census";
                curve_owner = Some((identity.wire_index, identity.coedge_index));
                curve_window = Some(curve_range);
                vertex_point = None;
                work.charge(1, cancel)?;
                let constant = constant_native_curve_height(curve, functional, work, cancel)?;
                if let Some(h) = constant {
                    let mut vertices_share_height = true;
                    for point in binding.source_vertex_points_mm {
                        work.charge(1, cancel)?;
                        if functional.vertex(point) != h { vertices_share_height = false; break; }
                    }
                    let mut curve_regular = false;
                    let mut proved_physical_boundary = true;
                    if flat_height.is_none() && overlaps(h, height_interval_mm) {
                        let allowance = remaining_scratch_with_limit(scratch_limit,
                            face_reserved)?;
                        let links = height_source_links(&mut source_links, definition, allowance, work, cancel)?;
                        if links.incidence.get(coedge.edge).is_some_and(|uses| {
                            uses.second.is_some_and(|second| uses.first.face == second.face)
                        }) {
                            // An unresolved chart-seam differential is only a
                            // candidate, never an asserted physical critical.
                            proved_physical_boundary = false;
                        }
                        if links.whole_edge_regular(&FaceHeightQuery { face: index, glued: glued.as_ref().unwrap(), functional }, coedge.edge, allowance, work, cancel)?
                        {
                            curve_regular = true;
                        }
                    }
                    if !curve_regular {
                        add_event(&mut events, h, height_interval_mm, policy.maximum_height_uncertainty_mm, work, cancel)?;
                        if proved_physical_boundary {
                            add_event(&mut proved_events, h, height_interval_mm, policy.maximum_height_uncertainty_mm, work, cancel)?;
                        }
                    }
                    if vertices_share_height { return Ok(()); }
                }
                for (endpoint, vertex) in coedge.vertices.into_iter().enumerate() {
                    stage = "source vertex census";
                    let parameter = source_proof.spatial_endpoint(endpoint != 0);
                    let point = binding.source_vertex_points_mm[endpoint];
                    vertex_point = Some(point);
                    let height = original_endpoint_height(curve, parameter, point, functional, work, cancel)?;
                    if flat_height.is_none() && overlaps(height, height_interval_mm) {
                        let allowance = remaining_scratch_with_limit(scratch_limit,
                            face_reserved)?;
                        if height_source_links(&mut source_links, definition, allowance, work, cancel)?
                            .vertex_regular(definition, &FaceHeightQuery { face: index, glued: glued.as_ref().unwrap(), functional }, vertex, allowance, work, cancel)?
                        {
                            continue;
                        }
                    }
                    add_event(&mut events, height, height_interval_mm, policy.maximum_height_uncertainty_mm, work, cancel)?;
                }
                if flat_height.is_none() && constant.is_none() {
                    stage = "source coedge height isolation";
                    vertex_point = None;
                    (|| -> Result<(), GeometryError> {
                        let occupied = face_reserved
                            .saturating_add(source_links.as_ref().map_or(0, HeightSourceLinks::owned_bytes));
                        let prepared = crate::parametric::prepare_curve_enclosure(curve, curve_range, remaining_scratch_with_limit(scratch_limit, occupied)?, work, cancel)?;
                        isolate_curve_events(&OriginalCurveSpan { curve: &prepared, range: curve_range }, &HeightEventQuery { functional, heights: height_interval_mm, uncertainty: policy.maximum_height_uncertainty_mm }, &mut events, &mut proved_events, work, cancel)
                    })().map_err(|error| height_face_context(error, index, "isolate original coedge heights"))?;
                }
                Ok(())
                };
                domain.with_original_spatial_curve(
                    flat, remaining_scratch_with_limit(scratch_limit, face_reserved)?,
                    work, cancel, &mut census,
                )?;
        }
        if (0..2).any(|i| torus_candidates[i] && !torus_proved[i]) {
            return Err(indeterminate("analytic critical-height value lacks a feasible selected-source trim witness"));
        }
        Ok(())
        }).map_err(|error| work.annotate_error(error, face_before, format_args!(
            "material height census stage={stage} source_face={index} wire_coedge={curve_owner:?} original_curve_window={curve_window:?} uv_window={surface_window:?} vertex_mm={vertex_point:?} transport_work_before={transport_before}"
        )))?;
    }
    events.sort_unstable_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    // Only identical enclosures are deduplicated. Proximity is not proof that
    // different critical strata are one event, so uncertain neighbors survive.
    events.dedup();
    proved_events.sort_unstable_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    proved_events.dedup();
    Ok(HeightAnalysis {
        event_brackets_mm: events,
        proved_event_brackets_mm: proved_events,
        invariance,
        capture: super::HeightCapture {
            definition_id: definition.id().clone(),
            material_realization: crate::kernel::MaterialRealizationIdentity::new(definition),
            frame,
            height_interval_mm,
            policy,
            pose: None,
            transport_support,
            transport_support_complete: true,
        },
    })
}

struct HeightEventQuery {
    functional: HeightFunctional,
    heights: [f64; 2],
    uncertainty: f64,
}
struct OriginalCurveSpan<'q, 'c> {
    curve: &'q PreparedValidatedCurve<'c>,
    range: (f64, f64),
}

/// Necessary height cover of derivative roots in a separately qualified C1
/// original coedge span. Interval Newton contracts parameters, not the source
/// support; no existence or endpoint-incidence claim follows from this bound.
fn necessary_curve_critical_height(
    curve: &PreparedValidatedCurve<'_>,
    range: (f64, f64),
    jet: &crate::parametric::CurveEnclosure,
    functional: HeightFunctional,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Interval>, GeometryError> {
    work.charge(1, cancel)?;
    let height = functional.height(jet.position);
    let second = functional.dot(jet.second_derivative);
    if second.contains_zero() {
        return Ok(Some(height));
    }
    let midpoint = range.0 * 0.5 + range.1 * 0.5;
    let value = functional.dot(curve.jet([midpoint, midpoint], work, cancel)?.derivative);
    let newton = Interval::point(midpoint).sub(value.div(second)?);
    let root = (range.0.max(newton.lo), range.1.min(newton.hi));
    if root.0 > root.1 {
        return Ok(None);
    }
    let root_height = functional.height(curve.jet([root.0, root.1], work, cancel)?.position);
    let bound = Interval {
        lo: height.lo.max(root_height.lo),
        hi: height.hi.min(root_height.hi),
    };
    Ok((bound.lo <= bound.hi).then_some(bound))
}

/// A literal parameter knot is incidence, not automatically a physical height
/// critical point. Prove a regular same-sign crossing from BOTH complete
/// one-sided neighborhoods; prove a turn from opposite signs or continuous
/// height bracketing. Unknown limits retain only a conservative candidate.
fn inventory_curve_joint_height(
    span: &OriginalCurveSpan<'_, '_>,
    knot: f64,
    query: &HeightEventQuery,
    events: &mut Vec<[f64; 2]>,
    proved: &mut Vec<[f64; 2]>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    let OriginalCurveSpan {
        curve,
        range: neighbors,
    } = *span;
    let HeightEventQuery {
        functional,
        heights,
        uncertainty,
    } = *query;
    let height = functional.height(curve.position_bounds([knot, knot], work, cancel)?);
    if !overlaps(height, heights) {
        return Ok(());
    }
    let (mut lower, mut upper) = neighbors;
    for depth in 0..=MAX_DEPTH {
        work.charge(1, cancel)?;
        let left = curve.jet([lower, knot], work, cancel)?;
        let right = curve.jet([knot, upper], work, cancel)?;
        let dl = functional.dot(left.derivative);
        let dr = functional.dot(right.derivative);
        if (dl.lo > 0. && dr.lo > 0.) || (dl.hi < 0. && dr.hi < 0.) {
            return Ok(());
        }
        if (dl.lo > 0. && dr.hi < 0.) || (dl.hi < 0. && dr.lo > 0.) {
            add_event(events, height, heights, uncertainty, work, cancel)?;
            add_event(proved, height, heights, uncertainty, work, cancel)?;
            return Ok(());
        }
        let cover = functional.height(std::array::from_fn(|axis| {
            left.position[axis].hull(right.position[axis])
        }));
        if cover.width() <= uncertainty {
            let a = functional.height(curve.position_bounds([lower, lower], work, cancel)?);
            let b = functional.height(curve.position_bounds([upper, upper], work, cancel)?);
            let turning =
                (height.lo > a.hi && height.lo > b.hi) || (height.hi < a.lo && height.hi < b.lo);
            // A maximum/minimum exists inside the closed continuous joined
            // interval by strict endpoint bracketing. Its WHOLE height cover,
            // not the knot's nominal value, owns this existence receipt.
            add_event(
                events,
                if turning { cover } else { height },
                heights,
                uncertainty,
                work,
                cancel,
            )?;
            if turning {
                add_event(proved, cover, heights, uncertainty, work, cancel)?;
            }
            return Ok(());
        }
        let next_lower = lower * 0.5 + knot * 0.5;
        let next_upper = knot * 0.5 + upper * 0.5;
        if depth == MAX_DEPTH
            || next_lower <= lower
            || next_lower >= knot
            || next_upper <= knot
            || next_upper >= upper
        {
            return Err(indeterminate(
                "original C0 joint height incidence could not be isolated",
            ));
        }
        lower = next_lower;
        upper = next_upper;
    }
    Err(indeterminate(
        "original C0 joint height incidence could not be isolated",
    ))
}

fn isolate_curve_events(
    source: &OriginalCurveSpan<'_, '_>,
    query: &HeightEventQuery,
    events: &mut Vec<[f64; 2]>,
    proved: &mut Vec<[f64; 2]>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    let OriginalCurveSpan {
        curve,
        range: original_range,
    } = *source;
    let HeightEventQuery {
        functional,
        heights,
        ..
    } = *query;
    if curve.is_smooth_on([original_range.0, original_range.1], work, cancel)? {
        for span in curve.parameter_intervals() {
            isolate_smooth_curve_span_events(
                &OriginalCurveSpan {
                    curve,
                    range: (span[0], span[1]),
                },
                query,
                false,
                events,
                proved,
                work,
                cancel,
            )?;
        }
        return Ok(());
    }
    // Whole-position exclusion does not need a differentiability claim.
    if !overlaps(
        functional.height(curve.position_bounds(
            [original_range.0, original_range.1],
            work,
            cancel,
        )?),
        heights,
    ) {
        return Ok(());
    }
    // The complete explicit-knot visitor analytically qualifies every original
    // between-knot span. Do not re-map those exact closed intervals through
    // Native's wider recursive C1 query, which can re-cross a reflected endpoint.
    let mut lower = original_range.0;
    let mut pending = None;
    let complete = curve.visit_explicit_knots(
        [original_range.0, original_range.1],
        work,
        cancel,
        &mut |next, work, cancel| {
            if let Some(knot) = pending {
                inventory_curve_joint_height(
                    &OriginalCurveSpan {
                        curve,
                        range: (lower, next),
                    },
                    knot,
                    query,
                    events,
                    proved,
                    work,
                    cancel,
                )?;
                isolate_smooth_curve_span_events(
                    &OriginalCurveSpan {
                        curve,
                        range: (lower, knot),
                    },
                    query,
                    true,
                    events,
                    proved,
                    work,
                    cancel,
                )?;
                lower = knot;
            }
            pending = Some(next);
            Ok(())
        },
    )?;
    if !complete {
        return Err(indeterminate(
            "height-event census lacks whole-original-coedge C1 or complete internal C0 knot incidence",
        ));
    }
    if let Some(knot) = pending {
        inventory_curve_joint_height(
            &OriginalCurveSpan {
                curve,
                range: (lower, original_range.1),
            },
            knot,
            query,
            events,
            proved,
            work,
            cancel,
        )?;
        isolate_smooth_curve_span_events(
            &OriginalCurveSpan {
                curve,
                range: (lower, knot),
            },
            query,
            true,
            events,
            proved,
            work,
            cancel,
        )?;
        lower = knot;
    }
    isolate_smooth_curve_span_events(
        &OriginalCurveSpan {
            curve,
            range: (lower, original_range.1),
        },
        query,
        true,
        events,
        proved,
        work,
        cancel,
    )
}

/// The caller proved this COMPLETE original span C1. Its closed endpoints may
/// be actual C0 joints with independently inventoried position/height strata.
fn isolate_smooth_curve_span_events(
    source: &OriginalCurveSpan<'_, '_>,
    query: &HeightEventQuery,
    one_sided_endpoints: bool,
    events: &mut Vec<[f64; 2]>,
    proved: &mut Vec<[f64; 2]>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    let OriginalCurveSpan { curve, range: span } = *source;
    let HeightEventQuery {
        functional,
        heights,
        uncertainty,
    } = *query;
    let before = work.consumed();
    let mut window = span;
    let result = (|| {
        let mut stack = vec![(span, 0u8)];
        while let Some(((lo, hi), depth)) = stack.pop() {
            window = (lo, hi);
            work.charge(1, cancel)?;
            let jet = curve.jet([lo, hi], work, cancel)?;
            let h = functional.height(jet.position);
            if !overlaps(h, heights) || !functional.dot(jet.derivative).contains_zero() {
                continue;
            }
            let mut root_height = None;
            if h.width() <= uncertainty {
                let Some(h) = necessary_curve_critical_height(
                    curve,
                    (lo, hi),
                    &jet,
                    functional,
                    work,
                    cancel,
                )?
                else {
                    continue;
                };
                // A strict sign change proves an interior height extremum by
                // continuity, rather than labelling a zero-containing box a root.
                // Enlarge inside the SAME retained owner/span to catch a root
                // lying on an artificial binary subdivision boundary.
                let expanded = (
                    down(lo - (hi - lo)).max(span.0),
                    up(hi + (hi - lo)).min(span.1),
                );
                let left_range = if one_sided_endpoints && expanded.0 == span.0 {
                    (expanded.0, expanded.0.next_up().min(expanded.1))
                } else {
                    (expanded.0, expanded.0)
                };
                let right_range = if one_sided_endpoints && expanded.1 == span.1 {
                    (expanded.1.next_down().max(expanded.0), expanded.1)
                } else {
                    (expanded.1, expanded.1)
                };
                let left = functional.dot(
                    curve
                        .jet([left_range.0, left_range.1], work, cancel)?
                        .derivative,
                );
                let right = functional.dot(
                    curve
                        .jet([right_range.0, right_range.1], work, cancel)?
                        .derivative,
                );
                if (left.hi < 0.0 && right.lo > 0.0) || (left.lo > 0.0 && right.hi < 0.0) {
                    let expanded_jet = curve.jet([expanded.0, expanded.1], work, cancel)?;
                    root_height = Some(
                    necessary_curve_critical_height(
                        curve,
                        expanded,
                        &expanded_jet,
                        functional,
                        work,
                        cancel,
                    )?
                    .ok_or_else(|| {
                        indeterminate(
                            "necessary original-coedge root cover contradicts a proved sign change",
                        )
                    })?,
                );
                }
                if root_height.is_none_or(|root| root.width() <= uncertainty) {
                    add_event(events, h, heights, uncertainty, work, cancel)?;
                    if let Some(root) = root_height {
                        add_event(events, root, heights, uncertainty, work, cancel)?;
                        add_event(proved, root, heights, uncertainty, work, cancel)?;
                    }
                    continue;
                }
            }
            let mid = lo * 0.5 + hi * 0.5;
            if depth >= MAX_DEPTH || mid <= lo || mid >= hi {
                return Err(indeterminate(
                    "trim critical-height roots could not be isolated",
                ));
            }
            work.charge(2, cancel)?;
            stack.push(((mid, hi), depth + 1));
            stack.push(((lo, mid), depth + 1));
        }
        Ok(())
    })();
    result.map_err(|error| work.annotate_error(
        error, before, format_args!("material height census smooth original coedge original_parameter_bounds={span:?} curve_window={window:?}"),
    ))
}

const ENDPOINT_PROOF_BYTES: usize = 8 * 1024 * 1024;
#[derive(Clone, Copy)]
pub(super) struct EndpointFaceProof {
    face: usize,
    endpoint: usize,
    covered_cells: u64,
    outgoing: bool,
}
impl EndpointFaceProof {
    pub(super) fn source_face(&self) -> usize {
        self.face
    }
    pub(super) fn endpoint(&self) -> usize {
        self.endpoint
    }
    pub(super) fn is_outgoing(&self) -> bool {
        self.outgoing
    }
}
struct EndpointDecomposition {
    faces: Vec<EndpointFaceProof>,
    strata: Vec<super::NativeEndpointStratum>,
    stratum_buffer_bytes: usize,
}
impl EndpointDecomposition {
    fn retained_bytes(&self) -> usize {
        self.faces
            .capacity()
            .saturating_mul(std::mem::size_of::<EndpointFaceProof>())
            .saturating_add(
                self.strata
                    .capacity()
                    .saturating_mul(std::mem::size_of::<super::NativeEndpointStratum>()),
            )
            .saturating_add(self.stratum_buffer_bytes)
    }
}
/// A compact original-edge graph, not reconstructed positional welding.
pub(super) fn original_edge_uses(
    definition: &Definition,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<HashMap<EdgeId, FlowEdgeUses>, GeometryError> {
    let mut edges: HashMap<_, FlowEdgeUses> = HashMap::new();
    for face_index in 0..definition.face_ids().len() {
        let (_, face) = super::source_incidence::native_face(definition, face_index)?;
        let orientation =
            super::source_incidence::face_orientation(definition, face_index, work, cancel)?;
        for wire_index in 0..face.loops.len() {
            let wire = super::source_incidence::boundary_loop(definition, face_index, wire_index)?;
            for (index, edge) in wire.edges.iter().enumerate() {
                work.charge(1, cancel)?;
                let use_ = FlowCoedgeUse {
                    face: face_index,
                    wire: wire_index,
                    index,
                    orientation: edge.forward == orientation,
                };
                if let Some(uses) = edges.get_mut(&edge.edge) {
                    if uses.second.is_some() || uses.first.orientation == use_.orientation {
                        return Err(error(
                            GeometryErrorCode::SingularGeometry,
                            "original source coedge is not a two-sided oriented manifold incidence",
                        ));
                    }
                    uses.second = Some(use_);
                } else {
                    let bytes = edges
                        .len()
                        .saturating_add(16)
                        .saturating_mul(4)
                        .saturating_mul(std::mem::size_of::<(EdgeId, FlowEdgeUses)>());
                    if bytes > ENDPOINT_PROOF_BYTES {
                        return Err(error(
                            GeometryErrorCode::ResourceLimit,
                            "source endpoint incidence capacity exhausted",
                        ));
                    }
                    edges.try_reserve(1).map_err(|_| {
                        error(
                            GeometryErrorCode::ResourceLimit,
                            "source endpoint incidence allocation refused",
                        )
                    })?;
                    edges.insert(
                        edge.edge,
                        FlowEdgeUses {
                            first: use_,
                            second: None,
                        },
                    );
                }
            }
        }
    }
    for uses in edges.values() {
        work.charge(1, cancel)?;
        if uses.second.is_none() {
            return Err(error(
                GeometryErrorCode::SingularGeometry,
                "source endpoint incidence has an unpaired coedge",
            ));
        }
    }
    Ok(edges)
}
/// Classify complete original horizontal faces at exact endpoints. Incoming
/// faces belong to the core closure; only outgoing faces become extra strata.
fn decompose_endpoint_faces(
    source: super::material_source::MaterialSourceView<'_, '_>,
    request: SlabRequest,
    resident_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<EndpointDecomposition, GeometryError> {
    let definition = source.definition();
    let mut result = EndpointDecomposition {
        faces: Vec::new(),
        strata: Vec::new(),
        stratum_buffer_bytes: 0,
    };
    let mut incidence = None;
    let functional = HeightFunctional::from_frame(request.frame);
    for (face_index, &face) in definition.face_ids().iter().enumerate() {
        work.charge(1, cancel)?;
        let Some(h) =
            constant_source_face_height(definition, face_index, functional, work, cancel)?
        else {
            continue;
        };
        if h.lo != h.hi {
            continue;
        }
        let endpoint = if h.lo == request.height_interval_mm[0] {
            0
        } else if h.lo == request.height_interval_mm[1] {
            1
        } else {
            continue;
        };
        if incidence.is_none() {
            remaining_scratch(
                resident_bytes.saturating_add(2 * ENDPOINT_PROOF_BYTES + STACK_SCRATCH_BYTES),
            )?;
            incidence = Some(original_edge_uses(definition, work, cancel)?);
        }
        let incidence = incidence
            .as_ref()
            .ok_or_else(|| indeterminate("source endpoint incidence is unavailable"))?;
        let domain = source.face_domain(
            face_index,
            remaining_scratch(
                resident_bytes.saturating_add(2 * ENDPOINT_PROOF_BYTES + STACK_SCRATCH_BYTES),
            )?,
            work,
            cancel,
        )?;
        let surface = &domain.surface_operand();
        let face_orientation = domain.orientation();
        let mut cover = Vec::new();
        let mut position: Option<[Interval; 3]> = None;
        let mut normal_sign = None;
        let oriented = (|| -> Result<bool, GeometryError> {
            let mut spans =
                surface_parameter_boxes(surface, trim_parameter_box(&domain)?, work, cancel)?;
            let mut stack = Vec::new();
            stack
                .try_reserve_exact(MAX_DEPTH as usize + 2)
                .map_err(|_| {
                    error(
                        GeometryErrorCode::ResourceLimit,
                        "endpoint face cover stack allocation refused",
                    )
                })?;
            while let Some((uv, depth)) = stack.pop().or_else(|| spans.next().map(|uv| (uv, 0u8))) {
                work.charge(1, cancel)?;
                if matches!(
                    domain.classify(uv, work, cancel)?,
                    TrimClassification::Outside
                ) {
                    continue;
                }
                let jet = surface_enclosure(surface, uv, work, cancel)?;
                let normal = cross(jet.du, jet.dv);
                let vertical = functional.dot(if face_orientation {
                    normal
                } else {
                    normal.map(Interval::neg)
                });
                let sign = if vertical.lo > 0.0 {
                    Some(1i8)
                } else if vertical.hi < 0.0 {
                    Some(-1i8)
                } else {
                    None
                };
                let Some(sign) = sign else {
                    if depth >= MAX_DEPTH {
                        return Ok(false);
                    }
                    for child in split_uv(uv, depth)? {
                        stack.push((child, depth + 1));
                    }
                    continue;
                };
                if normal_sign.is_some_and(|previous| previous != sign) {
                    return Ok(false);
                }
                normal_sign = Some(sign);
                let occupied = result.retained_bytes().saturating_add(
                    cover
                        .len()
                        .saturating_add(16)
                        .saturating_mul(2 * std::mem::size_of::<[[f64; 2]; 2]>()),
                );
                if occupied > ENDPOINT_PROOF_BYTES {
                    return Err(error(
                        GeometryErrorCode::ResourceLimit,
                        "retained endpoint face cover capacity exhausted",
                    ));
                }
                cover.try_reserve(1).map_err(|_| {
                    error(
                        GeometryErrorCode::ResourceLimit,
                        "retained endpoint face cover allocation refused",
                    )
                })?;
                cover.push(uv);
                position = Some(match position {
                    Some(previous) => {
                        std::array::from_fn(|axis| previous[axis].hull(jet.position[axis]))
                    }
                    None => jet.position,
                });
            }
            Ok(true)
        })()?;
        if !oriented {
            continue;
        }
        let (Some(position), Some(normal_sign)) = (position, normal_sign) else {
            continue;
        };
        let mut conforming = true;
        for (flat, coedge) in domain.coedges().iter().enumerate() {
            work.charge(1, cancel)?;
            let identity = coedge.identity();
            let edge_height = domain.with_original_spatial_curve(
                flat,
                remaining_scratch(
                    resident_bytes.saturating_add(2 * ENDPOINT_PROOF_BYTES + STACK_SCRATCH_BYTES),
                )?,
                work,
                cancel,
                |curve, work, cancel| constant_native_curve_height(curve, functional, work, cancel),
            )?;
            if edge_height
                .is_none_or(|edge_height| edge_height.lo != h.lo || edge_height.hi != h.hi)
            {
                conforming = false;
                break;
            }
            let binding = domain
                .original_coedge_correspondence(flat)?
                .binding()
                .ok_or_else(|| indeterminate("endpoint coedge lacks original vertex binding"))?;
            for point in binding.source_vertex_points_mm {
                work.charge(1, cancel)?;
                let vertex_height = functional.vertex(point);
                if vertex_height.lo != h.lo || vertex_height.hi != h.hi {
                    conforming = false;
                    break;
                }
            }
            if !conforming {
                break;
            }
            let uses = incidence.get(&identity.edge_id).ok_or_else(|| {
                indeterminate("endpoint coedge is missing its original native incidence")
            })?;
            let second = uses
                .second
                .ok_or_else(|| indeterminate("endpoint coedge has no original neighboring face"))?;
            let same_use = |use_: FlowCoedgeUse| {
                use_.face == face_index
                    && use_.wire == identity.wire_index
                    && use_.index == identity.coedge_index
            };
            if !same_use(uses.first) && !same_use(second) {
                return Err(indeterminate(
                    "endpoint coedge source-use correspondence is inconsistent",
                ));
            }
            // Existence of the retained original face is only genealogy.
            // The borrowed MaterialSourceView separately owns the complete
            // geometric use correspondence; raw wire indices select no source
            // pcurve branch or repaired-boundary association.
            definition.original_source().face(face)?;
        }
        if !conforming {
            continue;
        }
        let outgoing = (endpoint == 0 && normal_sign > 0) || (endpoint == 1 && normal_sign < 0);
        let covered_cells = cover.len() as u64;
        let info = definition
            .faces
            .get(face_index)
            .ok_or_else(|| indeterminate("endpoint source face identity is unavailable"))?;
        let pending = result
            .retained_bytes()
            .saturating_add(
                cover
                    .capacity()
                    .saturating_mul(std::mem::size_of::<[[f64; 2]; 2]>()),
            )
            .saturating_add(
                result
                    .faces
                    .len()
                    .saturating_add(16)
                    .saturating_mul(2 * std::mem::size_of::<EndpointFaceProof>()),
            )
            .saturating_add(
                result
                    .strata
                    .len()
                    .saturating_add(16)
                    .saturating_mul(2 * std::mem::size_of::<super::NativeEndpointStratum>()),
            )
            .saturating_add(definition.id().allocated_bytes())
            .saturating_add(info.face_id.allocated_bytes());
        if pending > ENDPOINT_PROOF_BYTES {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                "retained endpoint capture capacity exhausted",
            ));
        }
        if outgoing {
            result.strata.try_reserve(1).map_err(|_| {
                error(
                    GeometryErrorCode::ResourceLimit,
                    "retained endpoint stratum allocation refused",
                )
            })?;
            let stratum = super::NativeEndpointStratum {
                definition_id: definition.id().clone(),
                material_realization: crate::kernel::MaterialRealizationIdentity::new(definition),
                face_id: info.face_id.clone(),
                face_index,
                request,
                placed: None,
                endpoint: if endpoint == 0 {
                    super::NativeOwnedEndpoint::Lower
                } else {
                    super::NativeOwnedEndpoint::Upper
                },
                height_mm: h.lo,
                source_position_bounds_mm: position.map(|axis| [axis.lo, axis.hi]),
                world_xy_bounds_mm: None,
                outward_height_sign: normal_sign,
                patch_cover: cover,
            };
            result.stratum_buffer_bytes = result
                .stratum_buffer_bytes
                .saturating_add(stratum.retained_bytes());
            result.strata.push(stratum);
        }
        result.faces.try_reserve(1).map_err(|_| {
            error(
                GeometryErrorCode::ResourceLimit,
                "retained endpoint face incidence allocation refused",
            )
        })?;
        result.faces.push(EndpointFaceProof {
            face: face_index,
            endpoint,
            covered_cells,
            outgoing,
        });
        if result.retained_bytes() > ENDPOINT_PROOF_BYTES {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                "retained endpoint proof capacity exhausted",
            ));
        }
    }
    Ok(result)
}

/// Exact vertical boundary flow, independent of finish eligibility, outward
/// cylinder fitting, and descriptive families. Both bore and exterior walls
/// participate. Nonzero or uncertain interval residuals never establish reuse.
#[cfg(test)]
fn narrow_invariance(
    definition: &Definition,
    frame: PlaneFrameMm,
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<InvariantHeightDomain>, GeometryError> {
    let prepared =
        super::material_source::prepare_solid(definition, MAX_SLAB_SCRATCH_BYTES, work, cancel)?;
    let source = prepared.source_view()?;
    work.charge(1, cancel)?;
    narrow_invariance_with_scratch_limit(
        source,
        frame,
        range,
        remaining_scratch_with_limit(MAX_SLAB_SCRATCH_BYTES, prepared.retained_bytes())?,
        work,
        cancel,
    )
}
fn narrow_invariance_with_scratch_limit(
    source: super::material_source::MaterialSourceView<'_, '_>,
    frame: PlaneFrameMm,
    range: [f64; 2],
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<InvariantHeightDomain>, GeometryError> {
    if frame.z_axis != [0.0, 0.0, 1.0] && frame.z_axis != [0.0, 0.0, -1.0] {
        return Ok(None);
    }
    match source_vertical_boundary_flow(source, frame, range, remaining_bytes, work, cancel) {
        Err(e)
            if matches!(
                e.code,
                GeometryErrorCode::AnalysisIndeterminate
                    | GeometryErrorCode::UnsupportedGeometry
                    | GeometryErrorCode::ConditioningFailure
                    | GeometryErrorCode::ParameterCorrespondenceUnavailable
            ) =>
        {
            Ok(None)
        }
        result => result,
    }
}
#[derive(Clone, Copy)]
pub(super) struct FlowCoedgeUse {
    pub(super) face: usize,
    pub(super) wire: usize,
    pub(super) index: usize,
    pub(super) orientation: bool,
}
pub(super) struct FlowEdgeUses {
    pub(super) first: FlowCoedgeUse,
    pub(super) second: Option<FlowCoedgeUse>,
}
/// Complete selected source boundary flow; native IDs supply incidence only.
/// `remaining_bytes` excludes the source owner and all caller-retained buffers.
pub(super) fn source_vertical_boundary_flow(
    source: super::material_source::MaterialSourceView<'_, '_>,
    frame: PlaneFrameMm,
    range: [f64; 2],
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<InvariantHeightDomain>, GeometryError> {
    let definition = source.definition();
    const EDGE_BYTES: usize = 8 * 1024 * 1024;
    work.charge(1, cancel)?;
    let scratch_limit = remaining_bytes.min(MAX_SLAB_SCRATCH_BYTES);
    remaining_scratch_with_limit(scratch_limit, EDGE_BYTES + STACK_SCRATCH_BYTES)?;
    if !source.material_transport_is_proved_zero(work, cancel)? {
        return Ok(None);
    }
    let edges = original_edge_uses(definition, work, cancel)?;
    if receipt_map_bytes(&edges) > EDGE_BYTES {
        return Err(error(
            GeometryErrorCode::ResourceLimit,
            "native vertical-flow incidence capacity exhausted",
        ));
    }
    let functional = HeightFunctional::from_frame(frame);
    let mut cap_levels = [None, None];
    let mut side_cells = 0u64;
    for face_index in 0..definition.face_ids().len() {
        work.charge(1, cancel)?;
        let domain = source.face_domain(
            face_index,
            remaining_scratch_with_limit(scratch_limit, EDGE_BYTES + STACK_SCRATCH_BYTES)?,
            work,
            cancel,
        )?;
        let surface = &domain.surface_operand();
        let compatible = (|| -> Result<bool, GeometryError> {
            if let Some(h) = flat_native_surface_height(surface, frame, work, cancel)? {
                if h.lo != h.hi || !h.lo.is_finite() {
                    return Ok(false);
                }
                if cap_levels[0].is_none() {
                    cap_levels[0] = Some(h.lo);
                } else if cap_levels[0] != Some(h.lo) && cap_levels[1] != Some(h.lo) {
                    if cap_levels[1].is_some() {
                        return Ok(false);
                    }
                    cap_levels[1] = Some(h.lo);
                }
                return Ok(true);
            }
            let bounded_uv = trim_parameter_box(&domain)?;
            let mut spans = surface_parameter_boxes(surface, bounded_uv, work, cancel)?;
            let mut stack = Vec::new();
            stack
                .try_reserve_exact(MAX_DEPTH as usize + 2)
                .map_err(|_| {
                    error(
                        GeometryErrorCode::ResourceLimit,
                        "native vertical-flow stack allocation refused",
                    )
                })?;
            while let Some((uv, depth)) = stack.pop().or_else(|| spans.next().map(|uv| (uv, 0u8))) {
                work.charge(1, cancel)?;
                if matches!(
                    domain.classify(uv, work, cancel)?,
                    TrimClassification::Outside
                ) {
                    continue;
                }
                let jet = surface_enclosure(surface, uv, work, cancel)?;
                // Tight products preserve structural zero factors. Ordinary
                // interval cross products would pad an exact 0-0 identity.
                let nz = tight_add(
                    tight_mul(jet.du[0], jet.dv[1]),
                    tight_mul(jet.du[1], jet.dv[0]).neg(),
                );
                if nz.lo != 0.0 || nz.hi != 0.0 {
                    return Ok(false);
                }
                if norm(cross(jet.du, jet.dv))?.lo > 0.0 {
                    side_cells += 1;
                } else {
                    if depth >= MAX_DEPTH {
                        return Ok(false);
                    }
                    for child in split_uv(uv, depth)? {
                        stack.push((child, depth + 1));
                    }
                }
            }
            Ok(true)
        })()?;
        if !compatible {
            return Ok(None);
        }
    }
    // Two proved cap levels bound the complete flow cylinder. A third level is
    // a genuine potential change, not an event that may be ignored for reuse.
    let [Some(a), Some(b)] = cap_levels else {
        return Ok(None);
    };
    let interval = [range[0].max(a.min(b)), range[1].min(a.max(b))];
    if side_cells == 0 || interval[0] >= interval[1] {
        return Ok(None);
    }
    let x = HeightFunctional {
        direction: [
            Interval::point(1.0),
            Interval::point(0.0),
            Interval::point(0.0),
        ],
        offset: Interval::point(0.0),
    };
    let y = HeightFunctional {
        direction: [
            Interval::point(0.0),
            Interval::point(1.0),
            Interval::point(0.0),
        ],
        offset: Interval::point(0.0),
    };
    for uses in edges.values() {
        work.charge(1, cancel)?;
        let Some(second) = uses.second else {
            return Ok(None);
        };
        let first = uses.first;
        let remaining =
            remaining_scratch_with_limit(scratch_limit, EDGE_BYTES + STACK_SCRATCH_BYTES)?;
        let first_domain = source.face_domain(first.face, remaining, work, cancel)?;
        let second_domain = source.face_domain(second.face, remaining, work, cancel)?;
        let first_surface = first_domain.surface_operand();
        let second_surface = second_domain.surface_operand();
        // Native edge identity supplies incidence, not carrier equality.
        // Artificial cuts cancel only by the selected original operands and
        // their material sense, never a native proxy or approximate normal fit.
        if first_domain.orientation() == second_domain.orientation() {
            let same = crate::parametric::same_surface_coefficients(
                &first_surface,
                &second_surface,
                work,
                cancel,
            )?;
            if same {
                continue;
            }
        }
        let mut first_flat = None;
        for (flat, coedge) in first_domain.coedges().iter().enumerate() {
            work.charge(1, cancel)?;
            let identity = coedge.identity();
            if identity.wire_index == first.wire && identity.coedge_index == first.index {
                first_flat = Some(flat);
                break;
            }
        }
        let first_flat = first_flat.ok_or_else(|| {
            error(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "source vertical-flow edge use has no selected original coedge",
            )
        })?;
        let active = first_domain
            .original_coedge_correspondence(first_flat)?
            .spatial_range();
        let curve_range = (active[0], active[1]);
        let compatible = first_domain.with_original_spatial_curve(
            first_flat,
            remaining,
            work,
            cancel,
            |curve, work, cancel| -> Result<bool, GeometryError> {
                if let Some(h) = constant_native_curve_height(curve, functional, work, cancel)?
                    && (h.hi <= interval[0] || h.lo >= interval[1])
                {
                    return Ok(true);
                }
                if constant_native_curve_height(curve, x, work, cancel)?.is_some()
                    && constant_native_curve_height(curve, y, work, cancel)?.is_some()
                {
                    return Ok(true);
                }
                let prepared = crate::parametric::prepare_curve_enclosure(
                    curve,
                    curve_range,
                    remaining_scratch_with_limit(scratch_limit, EDGE_BYTES + STACK_SCRATCH_BYTES)?,
                    work,
                    cancel,
                )?;
                for span in prepared.parameter_intervals() {
                    work.charge(1, cancel)?;
                    let jet = prepared.jet(span, work, cancel)?;
                    let h = functional.height(jet.position);
                    if h.hi <= interval[0] || h.lo >= interval[1] {
                        continue;
                    }
                    if jet.derivative[0].lo != 0.0
                        || jet.derivative[0].hi != 0.0
                        || jet.derivative[1].lo != 0.0
                        || jet.derivative[1].hi != 0.0
                    {
                        return Ok(false);
                    }
                }
                Ok(true)
            },
        )?;
        if !compatible {
            return Ok(None);
        }
    }
    Ok(Some(InvariantHeightDomain {
        definition_id: definition.id().clone(),
        material_realization: crate::kernel::MaterialRealizationIdentity::new(definition),
        frame,
        height_interval_mm: interval,
        pose: None,
    }))
}

/// Whole-original-source authority: a regular vertical product closed by two
/// original disk caps. Unlike a regular-band receipt, this excludes void births
/// even when the requested sample is outside the source's closed support.
pub(super) struct NativeClosedDiskProduct<'a> {
    _definition_id: &'a spiling_contracts::geometry::DefinitionId,
    _material_realization: crate::kernel::MaterialRealizationIdentity,
}

pub(super) fn native_closed_disk_product<'a>(
    definition: &'a Definition,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<NativeClosedDiskProduct<'a>>, GeometryError> {
    let prepared =
        super::material_source::prepare_solid(definition, remaining_bytes, work, cancel)?;
    let source = prepared.source_view()?;
    let remaining_bytes = remaining_scratch_with_limit(remaining_bytes, prepared.retained_bytes())?;
    work.charge(1, cancel)?;
    if definition.brep().shells().len() != 1 {
        return Ok(None);
    }
    let frame = PlaneFrameMm {
        origin_mm: [0.0; 3],
        x_axis: [1.0, 0.0, 0.0],
        y_axis: [0.0, 1.0, 0.0],
        z_axis: [0.0, 0.0, 1.0],
    };
    // This visits every original trimmed face, not only slab-active boundary
    // cells. Its coefficient identities and opposed original coedge pairing
    // prove a product between the two exact cap levels.
    let Some(product) = narrow_invariance_with_scratch_limit(
        source,
        frame,
        [-f64::MAX, f64::MAX],
        remaining_bytes,
        work,
        cancel,
    )?
    else {
        return Ok(None);
    };
    let mut caps = [false; 2];
    for index in 0..definition.face_ids().len() {
        work.charge(1, cancel)?;
        let (_, record) = super::source_incidence::native_face(definition, index)?;
        let Some(height) = constant_source_face_height(
            definition,
            index,
            HeightFunctional::from_frame(frame),
            work,
            cancel,
        )?
        else {
            continue;
        };
        let Some(endpoint) = product
            .height_interval_mm
            .iter()
            .position(|level| height.lo == *level && height.hi == *level)
        else {
            return Ok(None);
        };
        // A single original regular planar trim wire bounds a disk. An annular
        // cap, split cap complex or additional floor needs its own incidence
        // proof; neither a sample without Hole loops nor a genus guess suffices.
        if caps[endpoint] || record.loops.len() != 1 {
            return Ok(None);
        }
        let domain = source.face_domain(
            index,
            remaining_scratch_with_limit(remaining_bytes, STACK_SCRATCH_BYTES)?,
            work,
            cancel,
        )?;
        let surface = &domain.surface_operand();
        let orientation = domain.orientation();
        let accepted = (|| -> Result<bool, GeometryError> {
            let uv = trim_parameter_box(&domain)?;
            if !super::critical::critical_face_gluing(surface, uv, work, cancel)? {
                return Ok(false);
            }
            let mut spans = surface_parameter_boxes(surface, uv, work, cancel)?;
            let mut stack = Vec::new();
            stack
                .try_reserve_exact(MAX_DEPTH as usize + 2)
                .map_err(|_| {
                    error(
                        GeometryErrorCode::ResourceLimit,
                        "native disk-cap stack allocation refused",
                    )
                })?;
            while let Some((uv, depth)) = stack.pop().or_else(|| spans.next().map(|uv| (uv, 0u8))) {
                work.charge(1, cancel)?;
                if matches!(
                    domain.classify(uv, work, cancel)?,
                    TrimClassification::Outside
                ) {
                    continue;
                }
                let jet = surface_enclosure(surface, uv, work, cancel)?;
                let mut normal = cross(jet.du, jet.dv)[2];
                if !orientation {
                    normal = normal.neg();
                }
                if (endpoint == 0 && normal.hi < 0.0) || (endpoint == 1 && normal.lo > 0.0) {
                    continue;
                }
                if depth >= MAX_DEPTH {
                    return Ok(false);
                }
                for child in split_uv(uv, depth)? {
                    stack.push((child, depth + 1));
                }
            }
            Ok(true)
        })()?;
        if !accepted {
            return Ok(None);
        }
        caps[endpoint] = true;
    }
    Ok(caps
        .into_iter()
        .all(|cap| cap)
        .then_some(NativeClosedDiskProduct {
            _definition_id: definition.id(),
            _material_realization: crate::kernel::MaterialRealizationIdentity::new(definition),
        }))
}

#[derive(Clone, Copy, PartialEq)]
struct RoundCarrier {
    center: [f64; 3],
    radius: f64,
    major_radius: Option<f64>,
    columns: [[f64; 3]; 3],
    translation: [f64; 3],
}
struct RoundContinuation<'a> {
    definition_id: &'a spiling_contracts::geometry::DefinitionId,
    material_realization: crate::kernel::MaterialRealizationIdentity,
    source_face_count: usize,
    carrier: RoundCarrier,
    offset: Interval,
    height_coefficient: Interval,
    axial_column: [f64; 3],
    radial_columns: [[f64; 3]; 2],
}
pub(super) struct RoundIncidenceProof<'a> {
    definition_id: &'a spiling_contracts::geometry::DefinitionId,
    material_realization: crate::kernel::MaterialRealizationIdentity,
    source_face_count: usize,
    tunnel_cycles: usize,
    native_convex_source: bool,
}
impl RoundIncidenceProof<'_> {
    pub(super) fn material_realization_identity(
        &self,
    ) -> crate::kernel::MaterialRealizationIdentity {
        self.material_realization
    }
    pub(super) fn definition_id(&self) -> &spiling_contracts::geometry::DefinitionId {
        self.definition_id
    }
    pub(super) fn material_component_count(&self) -> usize {
        1
    }
    pub(super) fn bounded_void_count(&self) -> usize {
        0
    }
    pub(super) fn tunnel_cycle_count(&self) -> usize {
        self.tunnel_cycles
    }
    pub(super) fn source_face_count(&self) -> usize {
        self.source_face_count
    }
    pub(super) fn has_native_convex_source_proof(&self) -> bool {
        self.native_convex_source
    }
}
impl<'a> RoundContinuation<'a> {
    fn incidence_proof(&self) -> RoundIncidenceProof<'a> {
        RoundIncidenceProof {
            definition_id: self.definition_id,
            material_realization: self.material_realization,
            source_face_count: self.source_face_count,
            tunnel_cycles: usize::from(self.carrier.major_radius.is_some()),
            // The complete positively bounded affine sphere was proved from
            // the entire original closed co-carrier patch complex above.
            native_convex_source: self.carrier.major_radius.is_none(),
        }
    }
}
fn round_carrier(surface: &(impl SurfaceEvaluator + ?Sized)) -> Option<(RoundCarrier, bool)> {
    if let Some(mut original) = surface.original_surface() {
        for _ in 0..MAX_DEPTH {
            let (position, radius, major_radius) = match original {
                solidcraft_step::geom::Surface::RectangularTrimmed { basis, .. } => {
                    original = basis;
                    continue;
                }
                solidcraft_step::geom::Surface::Sphere { position, radius } => {
                    (position, *radius, None)
                }
                solidcraft_step::geom::Surface::Torus {
                    position,
                    major_radius,
                    minor_radius,
                } => (position, *minor_radius, Some(*major_radius)),
                _ => return None,
            };
            let y = tight_cross_points(position.axis, position.ref_direction);
            // The retained affine carrier uses literal binary64 columns.
            // An inexact original cross product cannot be rounded into one.
            if y.iter().any(|value| value.lo != value.hi) {
                return None;
            }
            let columns = [
                position.ref_direction,
                y.map(|value| value.lo),
                position.axis,
            ];
            let determinant = dot(
                cross(
                    columns[0].map(Interval::point),
                    columns[1].map(Interval::point),
                ),
                columns[2],
            );
            if determinant.contains_zero() {
                return None;
            }
            return Some((
                RoundCarrier {
                    center: [0.0; 3],
                    radius,
                    major_radius,
                    columns,
                    translation: position.origin,
                },
                determinant.lo > 0.0,
            ));
        }
        return None;
    }
    let surface = native_step_operand(surface.native_surface()?)?;
    let (radius, major_radius, placement) = match surface.kind() {
        SurfaceKind::Sphere(s) => (s.radius(), None, s.base().position()),
        SurfaceKind::Torus(t) => (
            t.minor_radius(),
            Some(t.major_radius()),
            t.base().position(),
        ),
        _ => return None,
    };
    let x = placement.x_axis();
    let y = placement.y_axis();
    let z = placement.z_axis();
    let columns = [[x.x, x.y, x.z], [y.x, y.y, y.z], [z.x, z.y, z.z]];
    let determinant = dot(
        cross(
            columns[0].map(Interval::point),
            columns[1].map(Interval::point),
        ),
        columns[2],
    );
    if determinant.contains_zero() {
        return None;
    }
    let o = placement.origin();
    Some((
        RoundCarrier {
            center: [0.0; 3],
            radius,
            major_radius,
            columns,
            translation: [o.x, o.y, o.z],
        },
        determinant.lo > 0.0,
    ))
}
/// The admitted closed oriented shell, exact selected co-carrier equality,
/// source trim feasibility and nonzero affine determinant establish a complete carrier,
/// not permission from a family label. A regular closed same-carrier patch
/// complex is open and closed in the connected carrier: opposite edge uses
/// continue across chart/face subdivisions rather than leaving physical trims.
fn source_round_continuation<'a>(
    source: super::material_source::MaterialSourceView<'_, 'a>,
    frame: PlaneFrameMm,
    scratch_limit: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<RoundContinuation<'a>>, GeometryError> {
    let definition = source.definition();
    if definition.brep().shells().len() != 1 {
        return Ok(None);
    }
    let mut capture = None;
    let mut source_face_count = 0;
    for ordinal in 0..definition.face_ids().len() {
        work.charge(1, cancel)?;
        let domain = source.face_domain(
            ordinal,
            remaining_scratch_with_limit(scratch_limit, STACK_SCRATCH_BYTES)?,
            work,
            cancel,
        )?;
        let Some((carrier, orientation)) = round_carrier(&domain.surface_operand()) else {
            return Ok(None);
        };
        if carrier.radius <= 0.0 || carrier.major_radius.is_some_and(|r| r <= carrier.radius) {
            return Err(error(
                GeometryErrorCode::SingularGeometry,
                "round continuation requires a regular sphere or ring torus",
            ));
        }
        if orientation != domain.orientation() {
            return Ok(None);
        }
        if capture.is_some_and(|previous| previous != carrier) {
            return Ok(None);
        }
        capture = Some(carrier);
        source_face_count += 1;
    }
    let Some(carrier) = capture else {
        return Ok(None);
    };
    for index in 0..source_face_count {
        let domain = source.face_domain(
            index,
            remaining_scratch_with_limit(scratch_limit, STACK_SCRATCH_BYTES)?,
            work,
            cancel,
        )?;
        let mut cells = vec![(trim_parameter_box(&domain)?, 0u8)];
        let mut feasible = false;
        while let Some((uv, depth)) = cells.pop() {
            work.charge(1, cancel)?;
            match domain.classify(uv, work, cancel)? {
                TrimClassification::Outside => {}
                TrimClassification::Inside => {
                    feasible = true;
                    break;
                }
                TrimClassification::Boundary => {
                    // This query needs one strictly feasible original trim
                    // witness, not a complete cover. Certify the proposed
                    // point against the whole original boundary before chasing
                    // a seam/pole corner down an unresolved depth-first arm.
                    let point = uv.map(|bounds| {
                        let middle = bounds[0] * 0.5 + bounds[1] * 0.5;
                        [middle, middle]
                    });
                    if matches!(
                        domain.classify(point, work, cancel)?,
                        TrimClassification::Inside
                    ) {
                        feasible = true;
                        break;
                    }
                    if depth >= MAX_DEPTH {
                        continue;
                    }
                    work.charge(2, cancel)?;
                    for child in split_uv(uv, depth).map_err(|error| {
                        height_face_context(
                            error,
                            index,
                            &format!("closed round original trim {uv:?}"),
                        )
                    })? {
                        cells.push((child, depth + 1));
                    }
                }
            }
        }
        if !feasible {
            return Err(indeterminate(
                "closed round carrier contains an unresolved trimmed face",
            ));
        }
    }
    let determinant = dot(
        cross(
            carrier.columns[0].map(Interval::point),
            carrier.columns[1].map(Interval::point),
        ),
        carrier.columns[2],
    );
    if determinant.contains_zero() {
        return Err(error(
            GeometryErrorCode::ConditioningFailure,
            "round carrier affine map is not proved nonsingular",
        ));
    }
    let row = carrier
        .columns
        .map(|column| tight_dot(column, frame.z_axis));
    let axis = if carrier.major_radius.is_some() {
        if row[0].lo != 0.0 || row[0].hi != 0.0 || row[1].lo != 0.0 || row[1].hi != 0.0 {
            return Ok(None);
        }
        2
    } else {
        let mut nonzero = (0..3).filter(|&i| row[i].lo != 0.0 || row[i].hi != 0.0);
        let Some(axis) = nonzero.next() else {
            return Ok(None);
        };
        if nonzero.next().is_some() {
            return Ok(None);
        }
        axis
    };
    if row[axis].contains_zero() {
        return Err(error(
            GeometryErrorCode::ConditioningFailure,
            "round carrier height map is singular",
        ));
    }
    let offset = (0..3).fold(vertex_height(carrier.translation, frame), |s, i| {
        tight_add(s, tight_mul(row[i], Interval::point(carrier.center[i])))
    });
    let other = [(axis + 1) % 3, (axis + 2) % 3];
    Ok(Some(RoundContinuation {
        definition_id: definition.id(),
        material_realization: crate::kernel::MaterialRealizationIdentity::new(definition),
        source_face_count,
        carrier,
        offset,
        height_coefficient: row[axis],
        axial_column: carrier.columns[axis],
        radial_columns: [carrier.columns[other[0]], carrier.columns[other[1]]],
    }))
}
fn round_height_support(round: &RoundContinuation<'_>) -> [Interval; 2] {
    let amplitude = tight_mul(
        round.height_coefficient,
        Interval::point(round.carrier.radius),
    );
    let a = tight_add(round.offset, amplitude);
    let b = tight_add(round.offset, amplitude.neg());
    if a.lo <= b.lo { [a, b] } else { [b, a] }
}
fn round_slab_motion(
    round: &RoundContinuation<'_>,
    frame: PlaneFrameMm,
    range: [f64; 2],
    sample: Interval,
) -> Result<f64, GeometryError> {
    // The queried prism may extend beyond a terminal support bracket. Its
    // excess is genuine vertical Hausdorff error, not a reason to discard the
    // material cap or relabel the requested strictly interior sample.
    let r = Interval::point(round.carrier.radius);
    let local = Interval {
        lo: range[0],
        hi: range[1],
    }
    .sub(round.offset)
    .div(round.height_coefficient)?;
    let at_sample = sample.sub(round.offset).div(round.height_coefficient)?;
    if at_sample.lo <= -r.lo || at_sample.hi >= r.lo {
        return Err(indeterminate(
            "round native sample is not proved strictly regular",
        ));
    }
    let radius_at = |z: Interval| -> Result<Interval, GeometryError> {
        let squared = r.square().sub(z.square());
        // Only |z| <= r contains material. Intersect the squared-radius range
        // with nonnegative values; this is an enclosure, not a pole/root claim.
        Interval {
            lo: squared.lo.max(0.0),
            hi: squared.hi.max(0.0),
        }
        .sqrt()
    };
    let slab_radius = radius_at(local)?;
    let sample_radius = radius_at(at_sample)?;
    let growth = up(slab_radius.hi - sample_radius.lo).max(0.0);
    let [a, b] = round.radial_columns;
    let aa = dot(a.map(Interval::point), a);
    let bb = dot(b.map(Interval::point), b);
    let ab = dot(a.map(Interval::point), b);
    let discriminant = aa
        .sub(bb)
        .square()
        .add(ab.square().mul(Interval::point(4.0)));
    let radial_scale = aa
        .add(bb)
        .add(discriminant.sqrt()?)
        .mul(Interval::point(0.5))
        .sqrt()?
        .hi;
    let axial_planar = norm(cross(
        round.axial_column.map(Interval::point),
        frame.z_axis.map(Interval::point),
    ))?
    .div(norm(frame.z_axis.map(Interval::point))?)?
    .hi;
    let axial_change = local.sub(at_sample).abs_upper();
    let shear = Interval::point(axial_change)
        .mul(Interval::point(axial_planar))
        .hi;
    let radial_return = Interval::point(growth)
        .mul(Interval::point(radial_scale))
        .hi;
    // This is a SOLID/BODY-BOUNDARY prism metric, not just XY contour drift.
    // Points inside the sample footprint can return vertically to a source
    // fiber boundary or an artificial slab cap. Only footprint growth beyond
    // the actual sample requires a radial return. An interior sample biased
    // toward the regular end of a terminal cap therefore has O(height) error.
    let vertical = sample
        .sub(Interval::point(range[0]))
        .hull(Interval::point(range[1]).sub(sample))
        .div(norm(frame.z_axis.map(Interval::point))?)?
        .abs_upper();
    Ok(up(vertical.max(radial_return) + shear))
}
fn broad_height(bounds: &[Interval; 3], frame: PlaneFrameMm) -> Interval {
    (0..8).fold(
        Interval {
            lo: f64::INFINITY,
            hi: f64::NEG_INFINITY,
        },
        |range, mask| {
            let point = std::array::from_fn(|axis| {
                if mask & (1 << axis) == 0 {
                    bounds[axis].lo
                } else {
                    bounds[axis].hi
                }
            });
            range.hull(vertex_height(point, frame))
        },
    )
}
/// Enclose the height functional over the entire returned native plane/tube,
/// including stored normal renormalization and achieved plane uncertainty.
pub(super) fn native_sample_height_band(
    definition: &Definition,
    frame: PlaneFrameMm,
    section: &NativeSection,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Interval, GeometryError> {
    let actual = section.plane;
    let mut extent = 0.0_f64;
    for mask in 0..8 {
        work.charge(1, cancel)?;
        let delta = std::array::from_fn(|i| {
            let coordinate = if mask & (1 << i) == 0 {
                definition.bounds.min[i]
            } else {
                definition.bounds.max[i]
            };
            Interval::point(coordinate).sub(Interval::point(actual.origin_mm[i]))
        });
        extent = extent.max(norm(delta)?.hi);
    }
    let m = actual.normal.map(Interval::point);
    let n = frame.z_axis.map(Interval::point);
    let k = dot(n, actual.normal).div(dot(m, actual.normal))?;
    let residual = std::array::from_fn(|axis| n[axis].sub(k.mul(m[axis])));
    // The band is centered at h(plane.origin), so extent is relative to that
    // same origin, not relative to the world/source coordinate origin.
    let drift = norm(residual)?.mul(Interval::point(extent));
    let plane_error = Interval::point(k.abs_upper())
        .mul(norm(m)?)
        .mul(Interval::point(section.achieved_plane_error_mm()));
    let error = drift.add(plane_error).hi;
    let center = vertex_height(actual.origin_mm, frame);
    Ok(Interval {
        lo: down(center.lo - error),
        hi: up(center.hi + error),
    })
}
struct SlabSourceQuery<'q, 's, 'd> {
    source: super::material_source::MaterialSourceView<'s, 'd>,
    request: &'q SlabRequest,
    source_retained_bytes: usize,
}

/// A transition collar uses the explicitly scoped core C intersected with the
/// vertical sweep of the actual native filled sample S. This does NOT assert
/// a reverse distance from all CLOSED C to the current candidate prism.
fn recover_critical_slab(
    query: &SlabSourceQuery<'_, '_, '_>,
    section: &NativeSection,
    capture: super::SlabCapture,
    endpoints: EndpointDecomposition,
    unresolved: GeometryError,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SourceSlabProof, GeometryError> {
    let SlabSourceQuery {
        source,
        request,
        source_retained_bytes,
    } = *query;
    let definition = source.definition();
    let sample_height = Interval {
        lo: capture.source_sample_height_band_mm[0],
        hi: capture.source_sample_height_band_mm[1],
    };
    let sample_inside = sample_height.lo >= request.height_interval_mm[0]
        && sample_height.hi <= request.height_interval_mm[1];
    let approximation = up(section.achieved_chord_error_mm()
        + section.achieved_plane_error_mm()
        + section.boolean_tolerance_mm);
    let vertical = sample_height
        .sub(Interval::point(request.height_interval_mm[0]))
        .hull(Interval::point(request.height_interval_mm[1]).sub(sample_height))
        .abs_upper();
    let vertical = Interval::point(vertical)
        .div(norm(request.frame.z_axis.map(Interval::point))?)?
        .hi;
    let one_sided = if section.loops.is_empty() {
        0.0
    } else {
        norm([
            Interval::point(approximation),
            Interval::point(vertical),
            Interval::point(0.0),
        ])?
        .hi
    };
    if sample_inside || section.loops.is_empty() {
        // Retain every already-proved original endpoint face, the immutable
        // sample, and the new complete native complex under one cumulative cap.
        let reserved = source_retained_bytes
            .saturating_add(endpoints.retained_bytes())
            .saturating_add(section.retained_bytes())
            .saturating_add(capture.definition_id.allocated_bytes())
            .saturating_add(STACK_SCRATCH_BYTES)
            .saturating_add(4096);
        if let Some(complex) = super::collar::certify_native_collar_complex(
            source,
            request,
            HeightFunctional::from_frame(request.frame),
            remaining_scratch(reserved)?,
            work,
            cancel,
        )? {
            // A completed collar already supports this conservative metric.
            // Refine an overlarge proposal instead of constructing a stronger
            // critical cover just to seek a tighter bound. A failed collar
            // remains indeterminate; it never authorizes refinement.
            if one_sided > request.native_shape_error_mm {
                return Err(error(
                    GeometryErrorCode::ShapeErrorExceeded,
                    "certified native collar slab shape bound exceeds policy",
                ));
            }
            let occupied = Some(occupied_envelope_from_definition_bounds(
                definition,
                section,
                remaining_scratch(reserved.saturating_add(complex.retained_bytes()))?,
                work,
                cancel,
            )?);
            return Ok(SourceSlabProof {
                occupied,
                boundary_motion_bound_mm: one_sided,
                invariance: None,
                critical: None,
                endpoint_strata: endpoints.strata,
                collar_complex: Some(complex),
                capture,
            });
        }
    }
    if !endpoints.strata.is_empty() {
        return Err(indeterminate(
            "material core has unresolved non-endpoint critical incidence and no proved transition collar; outgoing faces cannot authorize a closed-target metric",
        ));
    }
    // Release incoming temporary covers before the complete closed critical
    // proof. Work already consumed is never reset by trying this stronger path.
    drop(endpoints);
    let Some(mut critical) = super::critical::certify_critical_slab(
        super::critical::CriticalSlabQuery {
            source,
            request,
            section,
            functional: HeightFunctional::from_frame(request.frame),
            actual_sample_height: sample_height,
        },
        remaining_scratch(
            source_retained_bytes
                .saturating_add(section.retained_bytes())
                .saturating_add(capture.definition_id.allocated_bytes()),
        )?,
        work,
        cancel,
    )?
    else {
        return Err(unresolved);
    };
    let bound = critical.boundary_motion_bound_mm;
    if !bound.is_finite() {
        return Err(error(
            GeometryErrorCode::ConditioningFailure,
            "critical slab bound is nonfinite",
        ));
    }
    if bound > request.native_shape_error_mm {
        return Err(error(
            GeometryErrorCode::ShapeErrorExceeded,
            "certified critical slab shape bound exceeds policy",
        ));
    }
    let occupied = match critical.occupied.take() {
        Some(occupied) => Some(occupied),
        None => Some(occupied_envelope(
            section,
            bound,
            remaining_scratch(
                source_retained_bytes
                    .saturating_add(section.retained_bytes())
                    .saturating_add(capture.definition_id.allocated_bytes())
                    .saturating_add(critical.retained_bytes()),
            )?,
            work,
            cancel,
        )?),
    };
    Ok(SourceSlabProof {
        occupied,
        boundary_motion_bound_mm: bound,
        invariance: None,
        critical: Some(critical),
        endpoint_strata: Vec::new(),
        collar_complex: None,
        capture,
    })
}

/// Endpoint-only transition material needs the same real original native
/// complex as an interior event band. A regular core certificate alone cannot
/// authorize a directed adjacent source-M leaf to cover departing CLOSED C.
fn certify_endpoint_transition_complex(
    source: super::material_source::MaterialSourceView<'_, '_>,
    request: &SlabRequest,
    endpoints: &EndpointDecomposition,
    resident_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<super::collar::NativeCollarComplex>, GeometryError> {
    if endpoints.strata.is_empty() {
        return Ok(None);
    }
    let occupied = resident_bytes
        .saturating_add(endpoints.retained_bytes())
        .saturating_add(STACK_SCRATCH_BYTES)
        .saturating_add(4096);
    let complex = super::collar::certify_native_collar_complex(
        source,
        request,
        HeightFunctional::from_frame(request.frame),
        remaining_scratch(occupied)?,
        work,
        cancel,
    )?
    .ok_or_else(|| {
        indeterminate("outgoing original endpoint strata lack a complete native transition complex")
    })?;
    Ok(Some(complex))
}

fn certified_empty_slab(
    section: &NativeSection,
    definition: &Definition,
    request: SlabRequest,
    source_retained_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SourceSlabProof, GeometryError> {
    if !section.loops.is_empty() {
        return Err(error(
            GeometryErrorCode::NativeIntersectionFailure,
            "native section contradicts a proved empty slab",
        ));
    }
    let remaining = remaining_scratch(
        source_retained_bytes
            .saturating_add(section.retained_bytes())
            .saturating_add(2 * definition.id().allocated_bytes()),
    )?;
    let capture = super::SlabCapture::new(definition, request, section, work, cancel)?;
    let occupied = Some(occupied_envelope(section, 0.0, remaining, work, cancel)?);
    // Only a whole CLOSED-slab support exclusion authorizes exact empty reuse.
    // A regular empty sample or endpoint-only contact is not this witness.
    let invariance = Some(InvariantHeightDomain {
        definition_id: definition.id().clone(),
        material_realization: crate::kernel::MaterialRealizationIdentity::new(definition),
        frame: request.frame,
        height_interval_mm: request.height_interval_mm,
        pose: None,
    });
    Ok(SourceSlabProof {
        occupied,
        boundary_motion_bound_mm: 0.0,
        invariance,
        critical: None,
        endpoint_strata: Vec::new(),
        collar_complex: None,
        capture,
    })
}

/// Source proof parts independent of the immutable native sample they certify.
pub(super) struct SourceSlabProof {
    occupied: Option<NativeSection>,
    boundary_motion_bound_mm: f64,
    invariance: Option<InvariantHeightDomain>,
    endpoint_strata: Vec<super::NativeEndpointStratum>,
    critical: Option<super::critical::CriticalSlabCertificate>,
    collar_complex: Option<super::collar::NativeCollarComplex>,
    capture: super::SlabCapture,
}

impl SourceSlabProof {
    /// Selects sufficient-proof lanes only; this does not authorize a metric.
    pub(super) fn has_transition_material(&self) -> bool {
        self.collar_complex.is_some() || !self.endpoint_strata.is_empty()
    }

    /// Retained proof buffers, excluding the borrowed sample and inline self.
    pub(super) fn retained_bytes(&self) -> usize {
        let mut bytes = self.capture.definition_id.allocated_bytes();
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
                .saturating_mul(std::mem::size_of::<super::NativeEndpointStratum>()),
        );
        self.endpoint_strata.iter().fold(bytes, |bytes, stratum| {
            bytes.saturating_add(stratum.retained_bytes())
        })
    }

    pub(super) fn into_analysis(self, section: NativeSection) -> SlabAnalysis {
        let Self {
            occupied,
            boundary_motion_bound_mm,
            invariance,
            endpoint_strata,
            critical,
            collar_complex,
            capture,
        } = self;
        SlabAnalysis {
            section,
            occupied,
            boundary_motion_bound_mm,
            invariance,
            endpoint_strata,
            critical,
            collar_complex,
            radial_slab: None,
            endpoint_radial_products: [None; 2],
            capture,
        }
    }
}

/// One owned native sample, prepared once before selecting a sufficient proof.
pub(super) struct PreparedSlabSample<'s, 'd> {
    source_owner: &'s super::material_source::PreparedSolid<'d>,
    pub(super) section: NativeSection,
    pub(super) request: SlabRequest,
    pub(super) source_retained_bytes: usize,
    pub(super) material_transport_zero: bool,
    origin_error: f64,
    norm_squared: Interval,
}

pub(crate) fn analyze_slab(
    source: &super::material_source::PreparedSolid<'_>,
    request: SlabRequest,
    query_byte_limit: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SlabAnalysis, GeometryError> {
    let sample = prepare_slab_sample(source, request, query_byte_limit, work, cancel)?;
    let proof = analyze_slab_sample(source, &sample, work, cancel)?;
    Ok(proof.into_analysis(sample.section))
}

pub(super) fn prepare_slab_sample<'s, 'd>(
    source: &'s super::material_source::PreparedSolid<'d>,
    request: SlabRequest,
    query_byte_limit: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<PreparedSlabSample<'s, 'd>, GeometryError> {
    validate(
        request.frame,
        request.height_interval_mm,
        request.native_shape_error_mm,
    )?;
    let range = request.height_interval_mm;
    if !request.sample_height_mm.is_finite()
        || request.sample_height_mm <= range[0]
        || request.sample_height_mm >= range[1]
    {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "slab sample must lie strictly inside its height interval",
        ));
    }
    let definition = source.definition();
    let material_transport_zero = source
        .source_view()?
        .material_transport_is_proved_zero(work, cancel)?;
    let n = request.frame.z_axis;
    let norm_squared = dot(n.map(Interval::point), n);
    let factor = Interval::point(request.sample_height_mm).div(norm_squared)?;
    let origin_bounds: [Interval; 3] = std::array::from_fn(|i| {
        Interval::point(request.frame.origin_mm[i]).add(factor.mul(Interval::point(n[i])))
    });
    let origin = origin_bounds.map(Interval::mid);
    let origin_error = norm(std::array::from_fn(|i| {
        origin_bounds[i].sub(Interval::point(origin[i]))
    }))?
    .hi;
    let plane = PlaneMm {
        origin_mm: origin,
        normal: n,
    }
    .normalized()?;
    work.charge(1, cancel)?;
    // This is only a precision proposal. The actual captured plane/tube must
    // still pass the closed-slab height-functional gate below.
    let sample = Interval::point(request.sample_height_mm);
    let clearance = sample
        .sub(Interval::point(range[0]))
        .lo
        .min(Interval::point(range[1]).sub(sample).lo);
    let maximum_plane_error_mm = spiling_contracts::geometry::SECTION_PLANE_TOLERANCE_MM.min(
        Interval::point(clearance)
            .mul(Interval::point(0.5))
            .div(norm_squared.sqrt()?)?
            .lo,
    );
    if maximum_plane_error_mm <= 0.0 {
        return Err(indeterminate(
            "original slab sample has no representable normal-error allowance",
        ));
    }
    // The caller already charged all prepared sources/proofs and other live
    // results. Keep that occupied capacity in every subsequent slab admission;
    // local scratch objects still retain their independent 32 MiB cap.
    let section_bytes = query_byte_limit.min(
        crate::MAX_PREPARED_SECTION_BYTES
            .checked_sub(source.retained_bytes())
            .ok_or_else(|| {
                error(
                    GeometryErrorCode::ResourceLimit,
                    "native slab source exceeds shared capacity",
                )
            })?,
    );
    let source_retained_bytes = crate::MAX_PREPARED_SECTION_BYTES - section_bytes;
    // Keep the established first uniform sampling proposal. Its actual native
    // chord bound, not this proposal, is charged with complete 3D reference
    // displacement in the same NativeSection. Normalization may use the
    // declared sampling ledger; actual placed/world and native material gates
    // remain independent and may still refuse the resulting candidate.
    let sampling_chord_error_mm = spiling_contracts::geometry::SECTION_SAMPLING_TOLERANCE_MM * 0.5;
    let section = source.section_for_query(
        &crate::section::SectionQuery {
            plane,
            maximum_chord_error_mm: sampling_chord_error_mm,
            maximum_plane_error_mm,
        },
        cancel,
        section_bytes as u32,
        work,
    )?;
    // This exact native reference reaches source proof, world placement and
    // planner output; the native section owns its chord/plane/winding receipts.
    Ok(PreparedSlabSample {
        source_owner: source,
        section,
        request,
        source_retained_bytes,
        material_transport_zero,
        origin_error,
        norm_squared,
    })
}

pub(super) fn analyze_slab_sample<'d>(
    source: &super::material_source::PreparedSolid<'d>,
    sample: &PreparedSlabSample<'_, 'd>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SourceSlabProof, GeometryError> {
    let PreparedSlabSample {
        source_owner,
        ref section,
        request,
        source_retained_bytes,
        material_transport_zero,
        origin_error,
        norm_squared,
    } = *sample;
    let definition = source.definition();
    if !std::ptr::eq(source_owner, source) || section.definition_id() != definition.id() {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "prepared slab sample belongs to another original source",
        ));
    }
    let range = request.height_interval_mm;
    let n = request.frame.z_axis;
    let source = source.source_view()?;
    work.charge(
        section
            .loops
            .iter()
            .map(|boundary| boundary.points_mm.len() as u64)
            .sum(),
        cancel,
    )?;
    let resident_bytes = source_retained_bytes
        .saturating_add(section.retained_bytes())
        .saturating_add(definition.id().allocated_bytes());
    let remaining = remaining_scratch(resident_bytes.saturating_add(STACK_SCRATCH_BYTES))?;
    if section.loops.is_empty() {
        let positions = super::placed::material_position_bounds(source, remaining, work, cancel)?;
        let broad = broad_height(&positions, request.frame);
        if range[1] < broad.lo || range[0] > broad.hi {
            return certified_empty_slab(
                section,
                definition,
                request,
                source_retained_bytes,
                work,
                cancel,
            );
        }
    }
    let transport_exclusion = if material_transport_zero {
        Some(MaterialTransportExclusion {
            definition_id: definition.id(),
            material_realization: crate::kernel::MaterialRealizationIdentity::new(definition),
            frame: request.frame,
            range,
        })
    } else {
        exclude_material_transport(
            source,
            request.frame,
            range,
            remaining_scratch(resident_bytes.saturating_add(STACK_SCRATCH_BYTES))?,
            work,
            cancel,
        )?
    };
    let endpoints = if transport_exclusion.is_some() {
        decompose_endpoint_faces(source, request, resident_bytes, work, cancel)?
    } else {
        // Raw horizontal caps are not caps of a potentially transported face.
        // The complete closed-source recovery must establish their ownership.
        EndpointDecomposition {
            faces: Vec::new(),
            strata: Vec::new(),
            stratum_buffer_bytes: 0,
        }
    };
    let invariance = if material_transport_zero && endpoints.faces.is_empty() {
        narrow_invariance_with_scratch_limit(
            source,
            request.frame,
            range,
            remaining_scratch(resident_bytes.saturating_add(endpoints.retained_bytes()))?,
            work,
            cancel,
        )?
        .filter(|certificate| {
            certificate.height_interval_mm[0] <= range[0]
                && certificate.height_interval_mm[1] >= range[1]
        })
    } else {
        None
    };
    let round = if material_transport_zero && invariance.is_none() && endpoints.faces.is_empty() {
        source_round_continuation(
            source,
            request.frame,
            remaining_scratch(resident_bytes.saturating_add(endpoints.retained_bytes()))?,
            work,
            cancel,
        )?
    } else {
        None
    };
    if let Some(round) = &round {
        let support = round_height_support(round);
        if range[1] < support[0].lo || range[0] > support[1].hi {
            return certified_empty_slab(
                section,
                definition,
                request,
                source_retained_bytes,
                work,
                cancel,
            );
        }
    }
    let capture = super::SlabCapture::new(definition, request, section, work, cancel)?;
    if !section.loops.is_empty()
        && (capture.source_sample_height_band_mm[0] < range[0]
            || capture.source_sample_height_band_mm[1] > range[1])
    {
        return Err(GeometryError::new(
            GeometryErrorCode::AnalysisIndeterminate,
            format!(
                "actual native source sample plane height {:?} is not proved inside original closed slab {:?} (native plane error {} mm)",
                capture.source_sample_height_band_mm,
                range,
                section.achieved_plane_error_mm(),
            ),
        ));
    }
    let regular = if let Some(product) = &invariance {
        work.charge(1, cancel)?;
        if !product.matches_original_material(definition)
            || product.frame() != request.frame
            || product.pose().is_some()
            || product.height_interval_mm()[0] > range[0]
            || product.height_interval_mm()[1] < range[1]
        {
            return Err(error(
                GeometryErrorCode::InvalidGeometry,
                "source product proof does not bind this entire original slab",
            ));
        }
        // This is the opaque whole-source coefficient/topology proof above,
        // not an observed equal section. No second native query is necessary.
        // Its proof did not retain adaptive cell tallies: zero counts here do
        // not claim that an unvisited source cell is a finite root cover.
        Some(RegularContinuation {
            definition_id: definition.id(),
            material_realization: product.material_realization_identity(),
            frame: request.frame,
            range,
            speed: 0.0,
            covered_face_cells: 0,
            covered_coedge_cells: 0,
        })
    } else if round.is_none() {
        let regular = regular_boundary_speed(
            source,
            transport_exclusion.as_ref(),
            &endpoints,
            resident_bytes,
            work,
            cancel,
        );
        match regular {
            Ok(regular) => Some(regular),
            Err(unresolved) if unresolved.code == GeometryErrorCode::AnalysisIndeterminate => {
                return recover_critical_slab(
                    &SlabSourceQuery {
                        source,
                        request: &request,
                        source_retained_bytes,
                    },
                    section,
                    capture,
                    endpoints,
                    unresolved,
                    work,
                    cancel,
                );
            }
            Err(failure) => return Err(failure),
        }
    } else {
        None
    };
    // Proper stratified continuation carries an empty native sample to an
    // empty entire regular interval, not merely an empty sampled plane.
    if section.loops.is_empty() && round.is_none() {
        if endpoints.faces.iter().any(|proof| !proof.outgoing) {
            return Err(error(
                GeometryErrorCode::NativeIntersectionFailure,
                "empty sample contradicts an incoming original endpoint face and proved core continuation",
            ));
        }
        if endpoints.strata.is_empty() {
            let occupied = Some(occupied_envelope(
                section,
                0.0,
                remaining_scratch(resident_bytes.saturating_add(endpoints.retained_bytes()))?,
                work,
                cancel,
            )?);
            return Ok(SourceSlabProof {
                occupied,
                boundary_motion_bound_mm: 0.0,
                invariance: None,
                critical: None,
                endpoint_strata: Vec::new(),
                collar_complex: None,
                capture,
            });
        }
        let proof = regular.as_ref().ok_or_else(|| {
            indeterminate("empty material core lacks an original-source continuation proof")
        })?;
        let occupied = Some(occupied_envelope_with_strata(
            section,
            0.0,
            &endpoints.strata,
            remaining_scratch(resident_bytes.saturating_add(endpoints.retained_bytes()))?,
            work,
            cancel,
        )?);
        let critical = Some(
            super::critical::CriticalSlabCertificate::from_proved_regular_core(
                super::critical::ProvedSlabSample {
                    definition,
                    request: &request,
                    section,
                    boundary_motion_bound_mm: 0.0,
                },
                proof,
                &endpoints.strata,
                &endpoints.faces,
                remaining_scratch(
                    resident_bytes
                        .saturating_add(endpoints.retained_bytes())
                        .saturating_add(occupied.as_ref().map_or(0, NativeSection::retained_bytes)),
                )?,
                work,
                cancel,
            )?,
        );
        let collar_complex = certify_endpoint_transition_complex(
            source,
            &request,
            &endpoints,
            resident_bytes
                .saturating_add(occupied.as_ref().map_or(0, NativeSection::retained_bytes))
                .saturating_add(
                    critical
                        .as_ref()
                        .map_or(0, super::critical::CriticalSlabCertificate::retained_bytes),
                ),
            work,
            cancel,
        )?;
        return Ok(SourceSlabProof {
            occupied,
            boundary_motion_bound_mm: 0.0,
            invariance: None,
            critical,
            endpoint_strata: endpoints.strata,
            collar_complex,
            capture,
        });
    }
    let speed = regular.as_ref().map_or(0.0, |regular| regular.speed);
    let actual_height = Interval {
        lo: capture.source_sample_height_band_mm[0],
        hi: capture.source_sample_height_band_mm[1],
    };
    let distance = actual_height
        .sub(Interval::point(range[0]))
        .hull(Interval::point(range[1]).sub(actual_height))
        .abs_upper();
    let motion = if let Some(round) = &round {
        let expected_holes = usize::from(round.carrier.major_radius.is_some());
        if section.loops.len() != expected_holes + 1
            || section
                .loops
                .iter()
                .filter(|boundary| boundary.is_hole)
                .count()
                != expected_holes
        {
            return Err(error(
                GeometryErrorCode::NativeIntersectionFailure,
                "native round section disagrees with certified complete-carrier incidence",
            ));
        }
        round_slab_motion(round, request.frame, range, actual_height)?
    } else {
        let vertical_coefficient =
            dot(section.plane.normal.map(Interval::point), n).div(norm_squared)?;
        let transverse = norm(cross(
            section.plane.normal.map(Interval::point),
            n.map(Interval::point),
        ))?
        .div(norm(n.map(Interval::point))?)?;
        if down(vertical_coefficient.lo - up(transverse.hi * speed)) <= 0.0 {
            return Err(error(
                GeometryErrorCode::ConditioningFailure,
                "native sample plane is not proved transverse to slab continuation",
            ));
        }
        Interval::point(speed).mul(Interval::point(distance)).hi
    };
    let approximation = up(section.achieved_chord_error_mm()
        + section.achieved_plane_error_mm()
        + section.boolean_tolerance_mm
        + origin_error);
    let bound = up(motion + approximation);
    if !bound.is_finite() {
        return Err(error(
            GeometryErrorCode::ConditioningFailure,
            "slab motion bound is nonfinite",
        ));
    }
    if bound > request.native_shape_error_mm {
        return recover_critical_slab(
            &SlabSourceQuery {
                source,
                request: &request,
                source_retained_bytes,
            },
            section,
            capture,
            endpoints,
            error(
                GeometryErrorCode::ShapeErrorExceeded,
                "certified native slab shape bound exceeds the requested policy",
            ),
            work,
            cancel,
        );
    }
    // At a proved round critical stratum the actual source branches close on
    // a ring/pole; sample approximation tubes, not event separation, stay apart.
    certify_loop_separation(
        section,
        if round.is_some() {
            approximation
        } else {
            bound
        },
        work,
        cancel,
    )?;
    let output_resident = resident_bytes
        .saturating_add(endpoints.retained_bytes())
        .saturating_add(
            invariance
                .as_ref()
                .map_or(0, |_| definition.id().allocated_bytes()),
        );
    let mut occupied = Some(occupied_envelope_with_strata(
        section,
        bound,
        &endpoints.strata,
        remaining_scratch(output_resident)?,
        work,
        cancel,
    )?);
    let critical = if let Some(round) = &round {
        let mut brackets = [[0.0; 2]; 2];
        let mut bracket_count = 0;
        for event in round_height_support(round)
            .into_iter()
            .filter(|event| overlaps(*event, range))
        {
            brackets[bracket_count] = [event.lo, event.hi];
            bracket_count += 1;
        }
        let remaining = remaining_scratch(
            output_resident
                .saturating_add(occupied.as_ref().map_or(0, NativeSection::retained_bytes)),
        )?;
        let mut certificate = super::critical::CriticalSlabCertificate::from_proved_round(
            super::critical::ProvedSlabSample {
                definition,
                request: &request,
                section,
                boundary_motion_bound_mm: bound,
            },
            &round.incidence_proof(),
            &brackets[..bracket_count],
            occupied.take(),
            remaining,
            work,
            cancel,
        )?;
        occupied = certificate.occupied.take();
        Some(certificate)
    } else if let Some(proof) = &regular {
        Some(
            super::critical::CriticalSlabCertificate::from_proved_regular_core(
                super::critical::ProvedSlabSample {
                    definition,
                    request: &request,
                    section,
                    boundary_motion_bound_mm: bound,
                },
                proof,
                &endpoints.strata,
                &endpoints.faces,
                remaining_scratch(
                    output_resident
                        .saturating_add(occupied.as_ref().map_or(0, NativeSection::retained_bytes)),
                )?,
                work,
                cancel,
            )?,
        )
    } else {
        None
    };
    let collar_complex = certify_endpoint_transition_complex(
        source,
        &request,
        &endpoints,
        resident_bytes
            .saturating_add(occupied.as_ref().map_or(0, NativeSection::retained_bytes))
            .saturating_add(
                critical
                    .as_ref()
                    .map_or(0, super::critical::CriticalSlabCertificate::retained_bytes),
            ),
        work,
        cancel,
    )?;
    Ok(SourceSlabProof {
        occupied,
        boundary_motion_bound_mm: bound,
        invariance,
        critical,
        endpoint_strata: endpoints.strata,
        collar_complex,
        capture,
    })
}

/// A compact stratified submersion has complete one-to-one continuation of its
/// material components and oriented loops on the OPEN interval. Exact oriented
/// endpoint-face incidence separately identifies its closure and extra strata.
/// Face normals alone never replace original edge/vertex continuation.
pub(super) struct RegularContinuation<'a> {
    definition_id: &'a spiling_contracts::geometry::DefinitionId,
    material_realization: crate::kernel::MaterialRealizationIdentity,
    frame: PlaneFrameMm,
    range: [f64; 2],
    speed: f64,
    covered_face_cells: u64,
    covered_coedge_cells: u64,
}
impl RegularContinuation<'_> {
    pub(super) fn material_realization_identity(
        &self,
    ) -> crate::kernel::MaterialRealizationIdentity {
        self.material_realization
    }
    pub(super) fn definition_id(&self) -> &spiling_contracts::geometry::DefinitionId {
        self.definition_id
    }
    pub(super) fn frame(&self) -> PlaneFrameMm {
        self.frame
    }
    pub(super) fn range(&self) -> [f64; 2] {
        self.range
    }
    pub(super) fn speed(&self) -> f64 {
        self.speed
    }
    pub(super) fn covered_cells(&self) -> (u64, u64) {
        (self.covered_face_cells, self.covered_coedge_cells)
    }
}
fn regular_boundary_speed<'a>(
    source: super::material_source::MaterialSourceView<'_, 'a>,
    transport_exclusion: Option<&MaterialTransportExclusion<'_>>,
    endpoints: &EndpointDecomposition,
    resident_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<RegularContinuation<'a>, GeometryError> {
    let definition = source.definition();
    let Some(exclusion) = transport_exclusion else {
        return Err(indeterminate(
            "active material transport requires corrected geometric continuation, not a raw speed",
        ));
    };
    if exclusion.definition_id != definition.id()
        || exclusion.material_realization
            != crate::kernel::MaterialRealizationIdentity::new(definition)
    {
        return Err(error(
            GeometryErrorCode::ParameterCorrespondenceUnavailable,
            "regular continuation changed its source-owned transport exclusion",
        ));
    }
    let frame = exclusion.frame;
    let range = exclusion.range;
    let mut speed = 0.0_f64;
    let mut covered_face_cells = 0u64;
    let mut covered_coedge_cells = 0u64;
    let n = frame.z_axis.map(Interval::point);
    let n_length = norm(n)?;
    let mut cap_faces = HashSet::new();
    let mut cap_edges = HashSet::new();
    let mut cap_vertices = HashMap::new();
    for proof in &endpoints.faces {
        work.charge(proof.face as u64 + 1, cancel)?;
        let domain =
            source.face_domain(proof.face, remaining_scratch(resident_bytes)?, work, cancel)?;
        let count = domain.coedges().len();
        let reserved = resident_bytes
            .saturating_add(endpoints.retained_bytes())
            .saturating_add(STACK_SCRATCH_BYTES)
            .saturating_add(cap_faces.len().saturating_add(16).saturating_mul(128))
            .saturating_add(
                cap_edges
                    .len()
                    .saturating_add(count)
                    .saturating_add(16)
                    .saturating_mul(256),
            )
            .saturating_add(
                cap_vertices
                    .len()
                    .saturating_add(2 * count)
                    .saturating_add(16)
                    .saturating_mul(256),
            );
        remaining_scratch(reserved)?;
        cap_faces.try_reserve(1).map_err(|_| {
            error(
                GeometryErrorCode::ResourceLimit,
                "core endpoint face allocation refused",
            )
        })?;
        cap_edges.try_reserve(count).map_err(|_| {
            error(
                GeometryErrorCode::ResourceLimit,
                "core endpoint edge allocation refused",
            )
        })?;
        cap_vertices.try_reserve(2 * count).map_err(|_| {
            error(
                GeometryErrorCode::ResourceLimit,
                "core endpoint vertex allocation refused",
            )
        })?;
        cap_faces.insert(domain.face());
        for coedge in domain.coedges() {
            work.charge(1, cancel)?;
            cap_edges.insert(coedge.edge);
            for vertex in coedge.vertices {
                let mask = cap_vertices.entry(vertex).or_insert(0u8);
                *mask |= 1 << proof.endpoint;
            }
        }
        covered_face_cells += proof.covered_cells;
    }
    for (index, &face) in definition.face_ids().iter().enumerate() {
        work.charge(1, cancel)?;
        if cap_faces.contains(&face) {
            continue;
        }
        if constant_source_face_height(
            definition,
            index,
            HeightFunctional::from_frame(frame),
            work,
            cancel,
        )?
        .is_some_and(|h| overlaps(h, range))
        {
            return Err(indeterminate(
                "slab contains an original horizontal critical face, not a regular continuation",
            ));
        }
        let reserved = resident_bytes
            .saturating_add(edge_scratch(definition, index)?)
            .saturating_add(endpoints.retained_bytes())
            .saturating_add(
                (cap_faces.capacity() + cap_edges.capacity() + cap_vertices.capacity())
                    .saturating_mul(64),
            );
        let domain = source.face_domain(index, remaining_scratch(reserved)?, work, cancel)?;
        let surface = &domain.surface_operand();
        let bounded_uv = trim_parameter_box(&domain)?;
        (|| -> Result<(), GeometryError> {
            if !super::critical::critical_face_gluing(surface, bounded_uv, work, cancel)? {
                if !overlaps(
                    height(
                        surface_enclosure(surface, bounded_uv, work, cancel)?.position,
                        frame,
                    ),
                    range,
                ) {
                    return Ok(());
                }
                return Err(indeterminate(
                    "regular source continuation has unresolved whole-face internal C0 gluing",
                ));
            }
            let mut spans = surface_parameter_boxes(surface, bounded_uv, work, cancel)?;
            let mut stack = Vec::new();
            stack
                .try_reserve_exact(MAX_DEPTH as usize + 2)
                .map_err(|_| {
                    error(
                        GeometryErrorCode::ResourceLimit,
                        "core surface continuation stack allocation refused",
                    )
                })?;
            while let Some((uv, depth)) = stack.pop().or_else(|| spans.next().map(|uv| (uv, 0u8))) {
                work.charge(1, cancel)?;
                if matches!(
                    domain.classify(uv, work, cancel)?,
                    TrimClassification::Outside
                ) {
                    continue;
                }
                let jet = surface_enclosure(surface, uv, work, cancel)?;
                if !overlaps(height(jet.position, frame), range) {
                    continue;
                }
                let normal = cross(jet.du, jet.dv);
                let transverse = norm(cross(normal, n))?;
                if transverse.lo > 0.0 && n_length.lo > 0.0 {
                    let rate = dot(normal, frame.z_axis)
                        .mul(Interval::point(1.0))
                        .div(transverse.mul(n_length))?
                        .abs_upper();
                    // Refine overly broad rates before charging the user's shape
                    // budget. A nonzero lower bound is a proof, not a sample check.
                    if rate.is_finite() && (depth >= 8 || rate <= 1.0) {
                        speed = speed.max(rate);
                        covered_face_cells += 1;
                        continue;
                    }
                }
                work.charge(2, cancel)?;
                for child in split_uv(uv, depth)? {
                    stack.push((child, depth + 1));
                }
            }
            for crease in surface_crease_interfaces(surface, bounded_uv, work, cancel)? {
                if super::critical::interface_regular_join(surface, crease, work, cancel)? {
                    continue;
                }
                if crease.iter().all(|axis| axis[0] != axis[1]) {
                    return Err(indeterminate(
                        "slab source reflected knot band has no proved fixed chart coordinate",
                    ));
                }
                let moving_axis = usize::from(crease[0][0] == crease[0][1]);
                for endpoint in crease[moving_axis] {
                    let mut uv = crease;
                    uv[moving_axis] = [endpoint, endpoint];
                    if !matches!(
                        domain.classify(uv, work, cancel)?,
                        TrimClassification::Outside
                    ) && overlaps(
                        height(
                            surface_enclosure(surface, uv, work, cancel)?.position,
                            frame,
                        ),
                        range,
                    ) {
                        return Err(indeterminate(
                            "slab contains an unresolved spline-interface junction",
                        ));
                    }
                }
                let mut cells = Vec::new();
                cells
                    .try_reserve_exact(MAX_DEPTH as usize + 2)
                    .map_err(|_| {
                        error(
                            GeometryErrorCode::ResourceLimit,
                            "core spline-interface stack allocation refused",
                        )
                    })?;
                cells.push((crease, 0u8));
                while let Some((uv, depth)) = cells.pop() {
                    work.charge(1, cancel)?;
                    if matches!(
                        domain.classify(uv, work, cancel)?,
                        TrimClassification::Outside
                    ) {
                        continue;
                    }
                    let jet = surface_enclosure(surface, uv, work, cancel)?;
                    if !overlaps(height(jet.position, frame), range) {
                        continue;
                    }
                    let tangent = if moving_axis == 0 { jet.du } else { jet.dv };
                    let vertical = dot(tangent, frame.z_axis);
                    if !vertical.contains_zero() {
                        let denominator = Interval {
                            lo: if vertical.lo > 0.0 {
                                vertical.lo
                            } else {
                                -vertical.hi
                            },
                            hi: vertical.abs_upper(),
                        };
                        let rate = norm(cross(tangent, n))?.div(denominator.mul(n_length))?.hi;
                        if rate.is_finite() {
                            speed = speed.max(rate);
                            continue;
                        }
                    }
                    work.charge(2, cancel)?;
                    for child in split_uv(uv, depth)? {
                        cells.push((child, depth + 1));
                    }
                }
            }
            let mut seen = HashSet::new();
            seen.try_reserve(domain.coedges().len()).map_err(|_| {
                error(
                    GeometryErrorCode::ResourceLimit,
                    "core original edge identity allocation refused",
                )
            })?;
            for (flat, coedge) in domain.coedges().iter().enumerate() {
                work.charge(1, cancel)?;
                if cap_edges.contains(&coedge.edge) || !seen.insert(coedge.edge) {
                    continue;
                }
                let proof = domain.original_coedge_correspondence(flat)?;
                let binding = proof.binding().ok_or_else(|| {
                    indeterminate("regular coedge lacks its selected source vertex binding")
                })?;
                let curve_range = proof.spatial_range();
                domain.with_original_spatial_curve(
                    flat,
                    remaining_scratch(reserved)?,
                    work,
                    cancel,
                    |curve, work, cancel| -> Result<(), GeometryError> {
                        // A paired periodic use is not automatically a smooth seam.
                        // Prove its actual 3D tangent flow as for every physical edge.
                        for (endpoint, vertex) in coedge.vertices.into_iter().enumerate() {
                            let h = original_endpoint_height(
                                curve,
                                proof.spatial_endpoint(endpoint != 0),
                                binding.source_vertex_points_mm[endpoint],
                                HeightFunctional::from_frame(frame),
                                work,
                                cancel,
                            )?;
                            let terminal_vertex = h.lo == h.hi
                                && cap_vertices.get(&vertex).is_some_and(|mask| {
                                    (mask & 1 != 0 && h.lo == range[0])
                                        || (mask & 2 != 0 && h.lo == range[1])
                                });
                            if overlaps(h, range) && !terminal_vertex {
                                return Err(indeterminate(
                                    "slab contains a native trim corner or singular endpoint",
                                ));
                            }
                        }
                        let prepared = crate::parametric::prepare_curve_enclosure(
                            curve,
                            (curve_range[0], curve_range[1]),
                            remaining_scratch(reserved)?,
                            work,
                            cancel,
                        )?;
                        if !prepared.is_smooth_on(curve_range, work, cancel)? {
                            if !overlaps(
                                height(prepared.position_bounds(curve_range, work, cancel)?, frame),
                                range,
                            ) {
                                return Ok(());
                            }
                            return Err(indeterminate(
                                "active original coedge has no whole-range C1 continuity proof",
                            ));
                        }
                        let mut spans = prepared.parameter_intervals();
                        let mut curves = Vec::new();
                        curves
                            .try_reserve_exact(MAX_DEPTH as usize + 2)
                            .map_err(|_| {
                                error(
                                    GeometryErrorCode::ResourceLimit,
                                    "core edge continuation stack allocation refused",
                                )
                            })?;
                        while let Some(([lo, hi], depth)) = curves
                            .pop()
                            .or_else(|| spans.next().map(|range| (range, 0u8)))
                        {
                            work.charge(1, cancel)?;
                            let jet = prepared.jet([lo, hi], work, cancel)?;
                            if !overlaps(height(jet.position, frame), range) {
                                continue;
                            }
                            let vertical = dot(jet.derivative, frame.z_axis);
                            if !vertical.contains_zero() {
                                let rate = norm(cross(jet.derivative, n))?
                                    .div(
                                        Interval {
                                            lo: if vertical.lo > 0.0 {
                                                vertical.lo
                                            } else {
                                                -vertical.hi
                                            },
                                            hi: vertical.abs_upper(),
                                        }
                                        .mul(n_length),
                                    )?
                                    .hi;
                                if rate.is_finite() {
                                    speed = speed.max(rate);
                                    covered_coedge_cells += 1;
                                    continue;
                                }
                            }
                            let mid = lo * 0.5 + hi * 0.5;
                            if depth >= MAX_DEPTH || mid <= lo || mid >= hi {
                                return Err(indeterminate(
                                    "slab crosses an unresolved trim critical-height event",
                                ));
                            }
                            work.charge(2, cancel)?;
                            curves.push(([lo, mid], depth + 1));
                            curves.push(([mid, hi], depth + 1));
                        }
                        Ok(())
                    },
                )?;
            }
            Ok(())
        })()?;
    }
    Ok(RegularContinuation {
        definition_id: definition.id(),
        material_realization: crate::kernel::MaterialRealizationIdentity::new(definition),
        frame,
        range,
        speed,
        covered_face_cells,
        covered_coedge_cells,
    })
}

fn point_segment_distance_squared(p: [f64; 3], a: [f64; 3], b: [f64; 3]) -> Interval {
    let d: [Interval; 3] =
        std::array::from_fn(|i| Interval::point(b[i]).sub(Interval::point(a[i])));
    let w: [Interval; 3] =
        std::array::from_fn(|i| Interval::point(p[i]).sub(Interval::point(a[i])));
    let length = d
        .into_iter()
        .fold(Interval::point(0.0), |s, x| s.add(x.square()));
    let projection = (0..3).fold(Interval::point(0.0), |s, i| s.add(w[i].mul(d[i])));
    let t = match projection.div(length) {
        Ok(t) => Interval {
            lo: t.lo.clamp(0.0, 1.0),
            hi: t.hi.clamp(0.0, 1.0),
        },
        Err(_) => {
            return Interval {
                lo: 0.0,
                hi: w
                    .into_iter()
                    .fold(Interval::point(0.0), |sum, x| sum.add(x.square()))
                    .hi,
            };
        }
    };
    (0..3).fold(Interval::point(0.0), |s, i| {
        s.add(w[i].sub(t.mul(d[i])).square())
    })
}
fn project_point(point: [f64; 3], frame: PlaneFrameMm) -> [Interval; 2] {
    let relative =
        std::array::from_fn(|i| Interval::point(point[i]).sub(Interval::point(frame.origin_mm[i])));
    [dot(relative, frame.x_axis), dot(relative, frame.y_axis)]
}
fn orientation(a: [Interval; 2], b: [Interval; 2], c: [Interval; 2]) -> Interval {
    b[0].sub(a[0])
        .mul(c[1].sub(a[1]))
        .sub(b[1].sub(a[1]).mul(c[0].sub(a[0])))
}
fn separated_segments(a: &[[f64; 3]], b: &[[f64; 3]], frame: PlaneFrameMm) -> bool {
    let a = [project_point(a[0], frame), project_point(a[1], frame)];
    let b = [project_point(b[0], frame), project_point(b[1], frame)];
    for axis in 0..2 {
        if a[0][axis].hi.max(a[1][axis].hi) < b[0][axis].lo.min(b[1][axis].lo)
            || b[0][axis].hi.max(b[1][axis].hi) < a[0][axis].lo.min(a[1][axis].lo)
        {
            return true;
        }
    }
    let same_side = |first: Interval, second: Interval| {
        (first.lo > 0.0 && second.lo > 0.0) || (first.hi < 0.0 && second.hi < 0.0)
    };
    same_side(orientation(a[0], a[1], b[0]), orientation(a[0], a[1], b[1]))
        || same_side(orientation(b[0], b[1], a[0]), orientation(b[0], b[1], a[1]))
}
fn certify_loop_separation(
    section: &NativeSection,
    radius: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    let residual = spiling_contracts::geometry::SECTION_PLANE_TOLERANCE_MM;
    let required = Interval::point(up(radius + residual))
        .mul(Interval::point(2.0))
        .square()
        .hi;
    for (i, a) in section.loops.iter().enumerate() {
        for b in &section.loops[i + 1..] {
            for edge_a in a.points_mm.windows(2) {
                for edge_b in b.points_mm.windows(2) {
                    work.charge(1, cancel)?;
                    if !separated_segments(edge_a, edge_b, section.frame) {
                        return Err(indeterminate(
                            "distinct sampled native loops cannot be proved nonintersecting",
                        ));
                    }
                    // A nonintersecting pair has its planar minimum at an
                    // endpoint. Include the native plane residual twice so
                    // slightly noncoplanar sampled endpoints cannot overstate
                    // the separation of the exact planar branches.
                    let distances = [
                        point_segment_distance_squared(edge_a[0], edge_b[0], edge_b[1]),
                        point_segment_distance_squared(edge_a[1], edge_b[0], edge_b[1]),
                        point_segment_distance_squared(edge_b[0], edge_a[0], edge_a[1]),
                        point_segment_distance_squared(edge_b[1], edge_a[0], edge_a[1]),
                    ];
                    if distances.iter().any(|d| d.lo <= required) {
                        return Err(indeterminate(
                            "distinct native loop continuation tubes cannot be proved disjoint",
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

/// A conservative occupied envelope, not another section of the source. Taking
/// the outward rectangle of every material loop and dropping holes deliberately
/// over-approximates their swept union; a disappearing hole is never promoted
/// into safe unoccupied support space. No display vertices are consulted.
pub(super) fn occupied_envelope(
    section: &NativeSection,
    radius: f64,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<NativeSection, GeometryError> {
    occupied_envelope_with_strata(section, radius, &[], remaining_bytes, work, cancel)
}
fn occupied_envelope_from_definition_bounds(
    definition: &Definition,
    section: &NativeSection,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<NativeSection, GeometryError> {
    // Import bounds enclose the original native carriers and trims. This is a
    // deliberately coarse outer C, not a sampled section or a collision proof.
    let position = std::array::from_fn(|axis| Interval {
        lo: definition.bounds.min[axis],
        hi: definition.bounds.max[axis],
    });
    occupied_envelope_with_position_boxes(
        section,
        0.0,
        std::iter::once(position),
        remaining_bytes,
        work,
        cancel,
    )
}
fn occupied_envelope_with_strata(
    section: &NativeSection,
    radius: f64,
    strata: &[super::NativeEndpointStratum],
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<NativeSection, GeometryError> {
    occupied_envelope_with_position_boxes(
        section,
        radius,
        strata.iter().map(|stratum| {
            stratum.source_position_bounds_mm.map(|axis| Interval {
                lo: axis[0],
                hi: axis[1],
            })
        }),
        remaining_bytes,
        work,
        cancel,
    )
}
fn occupied_envelope_with_position_boxes(
    section: &NativeSection,
    radius: f64,
    position_boxes: impl Iterator<Item = [Interval; 3]>,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<NativeSection, GeometryError> {
    let mut lower = [f64::INFINITY; 2];
    let mut upper = [f64::NEG_INFINITY; 2];
    let frame = section.frame;
    for position in position_boxes {
        work.charge(1, cancel)?;
        let relative =
            std::array::from_fn(|axis| position[axis].sub(Interval::point(frame.origin_mm[axis])));
        for (axis, direction) in [frame.x_axis, frame.y_axis].into_iter().enumerate() {
            let projection = dot(relative, direction);
            lower[axis] = lower[axis].min(projection.lo);
            upper[axis] = upper[axis].max(projection.hi);
        }
    }
    for boundary in &section.loops {
        if boundary.is_hole {
            continue;
        }
        for point in &boundary.points_mm {
            work.charge(1, cancel)?;
            let relative = std::array::from_fn(|i| {
                Interval::point(point[i]).sub(Interval::point(frame.origin_mm[i]))
            });
            for (axis, direction) in [frame.x_axis, frame.y_axis].into_iter().enumerate() {
                let projection = dot(relative, direction);
                let reserve = Interval::point(radius)
                    .mul(norm(direction.map(Interval::point))?)
                    .hi;
                lower[axis] = lower[axis].min(down(projection.lo - reserve));
                upper[axis] = upper[axis].max(up(projection.hi + reserve));
            }
        }
    }
    let output_bytes = section.definition_id().allocated_bytes().saturating_add(
        if lower.iter().all(|x| x.is_finite()) {
            std::mem::size_of::<SectionLoopMm>() + 5 * std::mem::size_of::<[f64; 3]>()
        } else {
            0
        },
    );
    remaining_scratch_with_limit(remaining_bytes, output_bytes)?;
    let loops = if lower.iter().all(|x| x.is_finite()) {
        work.charge(5, cancel)?;
        // Add representational roundoff before publishing the enclosing loop.
        let local_scale = lower
            .into_iter()
            .chain(upper)
            .map(f64::abs)
            .fold(1.0_f64, f64::max);
        let world_scale = frame
            .origin_mm
            .into_iter()
            .map(f64::abs)
            .fold(local_scale, f64::max);
        let gram_error = dot(frame.x_axis.map(Interval::point), frame.x_axis)
            .sub(Interval::point(1.0))
            .abs_upper()
            + dot(frame.y_axis.map(Interval::point), frame.y_axis)
                .sub(Interval::point(1.0))
                .abs_upper()
            + 2.0 * dot(frame.x_axis.map(Interval::point), frame.y_axis).abs_upper();
        let reserve = up(world_scale * f64::EPSILON * 64.0 + local_scale * gram_error * 4.0);
        let min = lower.map(|x| down(x - reserve));
        let max = upper.map(|x| up(x + reserve));
        let points_mm = [
            [min[0], min[1]],
            [max[0], min[1]],
            [max[0], max[1]],
            [min[0], max[1]],
            [min[0], min[1]],
        ]
        .into_iter()
        .map(|p| {
            std::array::from_fn(|i| {
                frame.origin_mm[i] + p[0] * frame.x_axis[i] + p[1] * frame.y_axis[i]
            })
        })
        .collect();
        vec![SectionLoopMm {
            is_hole: false,
            points_mm,
        }]
    } else {
        Vec::new()
    };
    let mut certificate = section.certificate.clone();
    certificate.authority = crate::SectionAuthority::ConservativeSlabEnvelope;
    Ok(NativeSection {
        plane: section.plane,
        frame,
        loops,
        loop_source_faces: Vec::new(),
        loop_spans: Vec::new(),
        sampling_tolerance_mm: section.sampling_tolerance_mm,
        boolean_tolerance_mm: section.boolean_tolerance_mm,
        certificate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn native_test_curve(
        controls: &[[f64; 4]],
        degree: usize,
        knots: &[f64],
    ) -> solidcore::curve::curve::Curve {
        use solidcore::nurbs::{b_spline_curve::BSplineCurve, crv_basic, math::CPoint};
        let raw = crv_basic::Curve::new(
            controls
                .iter()
                .map(|p| CPoint::new(p[0], p[1], p[2], p[3]))
                .collect(),
            degree,
            knots.to_vec(),
        )
        .unwrap();
        BSplineCurve::from_nurb(raw, 3, true).into()
    }
    fn frame() -> PlaneFrameMm {
        PlaneFrameMm {
            origin_mm: [0.0; 3],
            x_axis: [1.0, 0.0, 0.0],
            y_axis: [0.0, 1.0, 0.0],
            z_axis: [0.0, 0.0, 1.0],
        }
    }
    fn parity_definition(name: &str) -> Definition {
        super::super::test_source::recipe(name)
    }
    fn material_transport_zero(
        definition: &Definition,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let prepared = super::super::material_source::prepare_solid(
            definition,
            MAX_SLAB_SCRATCH_BYTES,
            work,
            cancel,
        )?;
        prepared
            .source_view()?
            .material_transport_is_proved_zero(work, cancel)
    }
    #[test]
    fn standalone_material_domains_borrow_one_definition_bound_owner() {
        let cancel = AtomicBool::new(false);
        let definition = crate::from_step_bytes_with_cancel(
            include_bytes!("../../../../fixtures/geometry/box-mm.step"),
            &cancel,
        )
        .unwrap();
        let other = crate::from_step_bytes_with_cancel(
            include_bytes!("../../../../fixtures/geometry/box-mm.step"),
            &cancel,
        )
        .unwrap();
        let mut preparation = GeometryWorkBudget::new(20_000_000);
        let prepared = super::super::material_source::prepare_solid(
            &definition,
            MAX_SLAB_SCRATCH_BYTES,
            &mut preparation,
            &cancel,
        )
        .unwrap();
        let foreign = super::super::material_source::prepare_solid(
            &other,
            MAX_SLAB_SCRATCH_BYTES,
            &mut preparation,
            &cancel,
        )
        .unwrap();
        let source = prepared.source_view().unwrap();
        assert!(std::ptr::eq(source.definition(), &definition));
        assert!(std::ptr::eq(
            foreign.source_view().unwrap().definition(),
            &other
        ));
        let retained = prepared.retained_bytes();
        let mut work = GeometryWorkBudget::new(64);
        let first = source.face_domain(0, 0, &mut work, &cancel).unwrap();
        for _ in 0..10 {
            let again = source.face_domain(0, 0, &mut work, &cancel).unwrap();
            assert!(std::ptr::eq(first, again));
            assert_eq!(prepared.retained_bytes(), retained);
        }
        let error =
            super::super::material_source::prepare_solid(&definition, 0, &mut preparation, &cancel)
                .err()
                .unwrap();
        assert_eq!(error.code, GeometryErrorCode::ResourceLimit);
        let error = source
            .face_domain(0, 0, &mut work, &AtomicBool::new(true))
            .err()
            .unwrap();
        assert_eq!(error.code, GeometryErrorCode::Cancelled);
    }

    #[test]
    fn slab_sample_cannot_cross_prepared_material_owners() {
        let cancel = AtomicBool::new(false);
        let definition = crate::from_step_bytes_with_cancel(
            include_bytes!("../../../../fixtures/geometry/box-mm.step"),
            &cancel,
        )
        .unwrap();
        let mut work = GeometryWorkBudget::new(20_000_000);
        let first = super::super::material_source::prepare_solid(
            &definition,
            MAX_SLAB_SCRATCH_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let other = super::super::material_source::prepare_solid(
            &definition,
            MAX_SLAB_SCRATCH_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let sample = prepare_slab_sample(
            &first,
            SlabRequest {
                frame: frame(),
                height_interval_mm: [2.0, 3.0],
                sample_height_mm: 2.5,
                native_shape_error_mm: 0.1,
            },
            MAX_SLAB_SCRATCH_BYTES - first.retained_bytes(),
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(std::ptr::eq(first.definition(), other.definition()));
        let error = analyze_slab_sample(&other, &sample, &mut work, &cancel)
            .err()
            .unwrap();
        assert_eq!(error.code, GeometryErrorCode::InvalidGeometry);
    }

    #[test]
    fn complete_position_box_preserves_one_sided_empty_height_exclusion() {
        let bounds = [
            Interval { lo: 2.0, hi: 3.0 },
            Interval { lo: -5.0, hi: -4.0 },
            Interval { lo: 10.0, hi: 12.0 },
        ];
        let height = broad_height(&bounds, frame());
        assert_eq!([height.lo, height.hi], [10.0, 12.0]);
        assert!(9.0 < height.lo);
        assert!(!(10.0 < height.lo), "closed endpoint contact is not empty");
    }

    #[test]
    fn affine_spline_census_keeps_all_original_corner_levels() {
        let cancel = AtomicBool::new(false);
        let frame = PlaneFrameMm {
            origin_mm: [0.0; 3],
            x_axis: [0.8, 0.0, -0.6],
            y_axis: [0.0, 1.0, 0.0],
            z_axis: [0.6, 0.0, 0.8],
        };
        let policy = AnalysisPolicy {
            maximum_height_uncertainty_mm: 0.01,
            maximum_shape_error_mm: 0.1,
        };
        // The source is 20 x 10 x 8. The y-directed original edges are
        // constant-height critical strata, even though every face has a
        // strictly nonzero interior height-gradient component.
        let levels = [0.0, 6.4, 12.0, 18.4];
        for name in [
            "native-parity-bspline-box.step",
            "native-parity-nurbs-box.step",
        ] {
            let definition = parity_definition(name);
            let mut preparation_work = GeometryWorkBudget::new(30_000_000);
            let source = super::super::material_source::prepare_solid(
                &definition,
                crate::MAX_PREPARED_SECTION_BYTES,
                &mut preparation_work,
                &cancel,
            )
            .unwrap();
            // Isolate census cost from import/Boolean-source preparation.
            let mut work = GeometryWorkBudget::new(1_000_000);
            let result = analyze_height_domain_with_scratch_limit(
                source.source_view().unwrap(),
                frame,
                [-1.0, 20.0],
                policy,
                crate::MAX_PREPARED_SECTION_BYTES.saturating_sub(source.retained_bytes()),
                &mut work,
                &cancel,
            )
            .unwrap();
            for brackets in [&result.event_brackets_mm, &result.proved_event_brackets_mm] {
                for level in levels {
                    assert!(
                        brackets.iter().any(|h| h[0] <= level && level <= h[1]),
                        "{name}: missing original corner height {level}: {brackets:?}",
                    );
                }
                assert!(
                    brackets.iter().all(|h| {
                        h[1] - h[0] <= policy.maximum_height_uncertainty_mm
                            && levels
                                .into_iter()
                                .any(|level| h[0] <= level && level <= h[1])
                    }),
                    "{name}: regular carrier cells introduced a nonphysical event: {brackets:?}"
                );
            }
        }
    }

    #[test]
    fn original_implicit_height_total_cap_reserves_proof_exactly_once() {
        let source = crate::corpus_fixtures::implicit_intersections(
            include_str!("../../../../fixtures/geometry/box-mm.step"),
            "implicit.step",
        );
        let cancel = AtomicBool::new(false);
        let definition = crate::from_step_bytes_with_cancel(source.as_bytes(), &cancel).unwrap();
        let pinned = definition.retained_query_proof_bytes();
        assert!(pinned > 0);
        let policy = AnalysisPolicy {
            maximum_height_uncertainty_mm: 0.01,
            maximum_shape_error_mm: 0.1,
        };
        let mut work = GeometryWorkBudget::new(20_000_000);
        let scratch = HEIGHT_EVENT_SCRATCH_BYTES + STACK_SCRATCH_BYTES;
        let failure = analyze_height_domain_with_total_limit(
            &definition,
            frame(),
            [3.99, 4.01],
            policy,
            pinned + scratch - 1,
            &mut work,
            &cancel,
        )
        .unwrap_err();
        assert_eq!(failure.code, GeometryErrorCode::ResourceLimit);
        // Same already-debited scratch boundary must not charge pinned again.
        let total = pinned + 16 * 1024 * 1024;
        let direct = analyze_height_domain_with_total_limit(
            &definition,
            frame(),
            [3.99, 4.01],
            policy,
            total,
            &mut work,
            &cancel,
        )
        .unwrap();
        let nested = analyze_height_domain_with_scratch_limit(
            super::super::material_source::prepare_solid(
                &definition,
                MAX_SLAB_SCRATCH_BYTES,
                &mut work,
                &cancel,
            )
            .unwrap()
            .source_view()
            .unwrap(),
            frame(),
            [3.99, 4.01],
            policy,
            total - pinned,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert_eq!(direct.event_brackets_mm, nested.event_brackets_mm);
        assert!(direct.event_brackets_mm.is_empty());
    }

    #[test]
    fn prepared_slab_rejects_exhausted_caller_capacity_before_query_growth() {
        let cancel = AtomicBool::new(false);
        let definition = crate::from_step_bytes_with_cancel(
            include_bytes!("../../../../fixtures/geometry/box-mm.step"),
            &cancel,
        )
        .unwrap();
        let mut work = GeometryWorkBudget::new(5_000_000);
        let source = super::super::material_source::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let request = SlabRequest {
            frame: frame(),
            height_interval_mm: [3.99, 4.01],
            sample_height_mm: 4.0,
            native_shape_error_mm: 0.1,
        };
        assert_eq!(
            analyze_slab(&source, request, 0, &mut work, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::ResourceLimit
        );
        let analysis = analyze_slab(
            &source,
            request,
            crate::MAX_PREPARED_SECTION_BYTES - source.retained_bytes(),
            &mut work,
            &cancel,
        )
        .unwrap();
        assert_eq!(analysis.section.loops.len(), 1);
        assert!(analysis.invariance.is_some());
        assert!(analysis.occupied.is_some());
    }
    #[test]
    fn varying_native_families_certify_full_regular_slabs_not_only_the_sample() {
        let cancel = AtomicBool::new(false);
        for (name, range, sample, expected_loops) in [
            ("native-parity-cone-frustum.step", [3.99, 4.01], 4.0, 1),
            ("native-parity-sphere-belt.step", [-0.01, 0.01], 0.0, 1),
            ("native-parity-ring-torus.step", [-0.01, 0.01], 0.0, 2),
        ] {
            let definition = parity_definition(name);
            let mut work = GeometryWorkBudget::new(20_000_000);
            let prepared = super::super::material_source::prepare_solid(
                &definition,
                crate::MAX_PREPARED_SECTION_BYTES,
                &mut work,
                &cancel,
            )
            .unwrap();
            let result = analyze_slab(
                &prepared,
                SlabRequest {
                    frame: frame(),
                    height_interval_mm: range,
                    sample_height_mm: sample,
                    native_shape_error_mm: 0.1,
                },
                crate::MAX_PREPARED_SECTION_BYTES - prepared.retained_bytes(),
                &mut work,
                &cancel,
            )
            .unwrap_or_else(|error| {
                panic!("{name}: regular native slab with bounded shape motion: {error}")
            });
            assert_eq!(result.section.loops.len(), expected_loops);
            assert!(result.invariance.is_none());
            assert!(
                result.boundary_motion_bound_mm > 0.0 && result.boundary_motion_bound_mm <= 0.1
            );
            assert!(
                result
                    .occupied
                    .as_ref()
                    .is_some_and(|occupied| !occupied.loops.is_empty())
            );
            drop(result);
            // The selected source geometry may be retained, but the identical
            // sample cannot authorize a wider CLOSED material interval.
            let wide = analyze_slab(
                &prepared,
                SlabRequest {
                    frame: frame(),
                    height_interval_mm: [sample - 1.25, sample + 1.25],
                    sample_height_mm: sample,
                    native_shape_error_mm: 0.1,
                },
                crate::MAX_PREPARED_SECTION_BYTES - prepared.retained_bytes(),
                &mut work,
                &cancel,
            )
            .unwrap_err();
            assert_eq!(
                wide.code,
                GeometryErrorCode::ShapeErrorExceeded,
                "{name}: {wide}"
            );
        }
    }
    #[test]
    fn stationary_original_coedge_endpoint_does_not_become_a_search_cell_height_event() {
        // Actual z=t² on the original closed [0,1] span. Its only derivative
        // root is the endpoint at height zero, not a micrometre search band.
        let original = native_test_curve(
            &[
                [0.0, 0.0, 0.0, 1.0],
                [0.5, 0.0, 0.0, 1.0],
                [1.0, 0.0, 1.0, 1.0],
            ],
            2,
            &[0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        );
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let prepared = crate::parametric::prepare_curve_enclosure(
            &original,
            (0.0, 1.0),
            MAX_SLAB_SCRATCH_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let mut events = Vec::new();
        let mut proved = Vec::new();
        isolate_curve_events(
            &OriginalCurveSpan {
                curve: &prepared,
                range: (0.0, 1.0),
            },
            &HeightEventQuery {
                functional: HeightFunctional::from_frame(frame()),
                heights: [-0.1, 1.1],
                uncertainty: 1e-6,
            },
            &mut events,
            &mut proved,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(
            events
                .iter()
                .any(|event| event[0] <= 0.0 && 0.0 <= event[1])
        );
        assert!(
            events
                .iter()
                .all(|event| event[0].abs().max(event[1].abs()) < 1e-12)
        );
        assert!(
            proved.is_empty(),
            "a one-sided endpoint is not an interior sign-change witness"
        );
    }
    #[test]
    fn height_only_coedge_census_cannot_drop_an_internal_v_height_knot() {
        let original = native_test_curve(
            &[
                [0.0, 0.0, 0.0, 1.0],
                [0.5, 0.0, 1.0, 1.0],
                [1.0, 0.0, 0.0, 1.0],
            ],
            1,
            &[0.0, 0.0, 0.5, 1.0, 1.0],
        );
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let prepared = crate::parametric::prepare_curve_enclosure(
            &original,
            (0.0, 1.0),
            MAX_SLAB_SCRATCH_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let mut events = Vec::new();
        let mut proved = Vec::new();
        assert!(
            !prepared
                .is_smooth_on([0.0, 1.0], &mut work, &cancel)
                .unwrap()
        );
        isolate_curve_events(
            &OriginalCurveSpan {
                curve: &prepared,
                range: (0.0, 1.0),
            },
            &HeightEventQuery {
                functional: HeightFunctional::from_frame(frame()),
                heights: [-0.1, 1.1],
                uncertainty: 0.01,
            },
            &mut events,
            &mut proved,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(events.iter().any(|event| event[0] <= 1. && 1. <= event[1]));
        assert!(
            proved.iter().any(|event| event[0] <= 1. && 1. <= event[1]),
            "the actual original C0 joint is inventoried, not lost between open-piece derivatives"
        );
        events.clear();
        proved.clear();
        isolate_curve_events(
            &OriginalCurveSpan {
                curve: &prepared,
                range: (0.0, 1.0),
            },
            &HeightEventQuery {
                functional: HeightFunctional::from_frame(frame()),
                heights: [2.0, 3.0],
                uncertainty: 0.01,
            },
            &mut events,
            &mut proved,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(
            events.is_empty(),
            "independent whole-position exclusion remains valid"
        );
    }
    #[test]
    fn parameter_c0_joint_with_same_sign_height_limits_is_not_a_critical_event() {
        let cancel = AtomicBool::new(false);
        for reflected in [false, true] {
            let controls = if reflected {
                [[2., 0., 3., 1.], [1., 0., 1., 1.], [0., 0., 0., 1.]]
            } else {
                [[0., 0., 0., 1.], [1., 0., 1., 1.], [2., 0., 3., 1.]]
            };
            let knot = if reflected { 6. } else { 3. };
            let original = native_test_curve(&controls, 1, &[2., 2., knot, 7., 7.]);
            let mut work = GeometryWorkBudget::new(1_000_000);
            let prepared = crate::parametric::prepare_curve_enclosure(
                &original,
                (2., 7.),
                MAX_SLAB_SCRATCH_BYTES,
                &mut work,
                &cancel,
            )
            .unwrap();
            assert!(!prepared.is_smooth_on([2., 7.], &mut work, &cancel).unwrap());
            let mut events = Vec::new();
            let mut proved = Vec::new();
            isolate_curve_events(
                &OriginalCurveSpan {
                    curve: &prepared,
                    range: (2., 7.),
                },
                &HeightEventQuery {
                    functional: HeightFunctional::from_frame(frame()),
                    heights: [0.9, 1.1],
                    uncertainty: 0.01,
                },
                &mut events,
                &mut proved,
                &mut work,
                &cancel,
            )
            .unwrap();
            assert!(
                events.is_empty() && proved.is_empty(),
                "original knot incidence does not promote a strictly monotone crossing to a critical stratum"
            );
        }
    }

    #[test]
    fn original_c0_joint_inventory_preserves_nonsymmetric_reflection_anchors_and_pose() {
        let cancel = AtomicBool::new(false);
        for reflected in [false, true] {
            // Native import stores the actual reflected coefficient law. Its
            // original 2+7 anchor must survive the narrower query restriction.
            let controls = if reflected {
                [[2., 0., 4., 1.], [1., 0., 5., 1.], [0., 0., 4., 1.]]
            } else {
                [[0., 0., 4., 1.], [1., 0., 5., 1.], [2., 0., 4., 1.]]
            };
            let knot = if reflected { 6. } else { 3. };
            let original = native_test_curve(&controls, 1, &[2., 2., knot, 7., 7.]);
            let mut work = GeometryWorkBudget::new(1_000_000);
            let prepared = crate::parametric::prepare_curve_enclosure(
                &original,
                (2.5, 6.25),
                MAX_SLAB_SCRATCH_BYTES,
                &mut work,
                &cancel,
            )
            .unwrap();
            assert!(
                !prepared
                    .is_smooth_on([2.5, 6.25], &mut work, &cancel)
                    .unwrap()
            );
            let mut knots = Vec::new();
            assert!(
                prepared
                    .visit_explicit_knots([2.5, 6.25], &mut work, &cancel, &mut |knot, _, _| {
                        knots.push(knot);
                        Ok(())
                    })
                    .unwrap()
            );
            assert_eq!(
                knots,
                vec![if reflected { 6. } else { 3. }],
                "the original 2+7 anchor is not the narrower outer restriction's anchor"
            );
            let mut events = Vec::new();
            let mut proved = Vec::new();
            isolate_curve_events(
                &OriginalCurveSpan {
                    curve: &prepared,
                    range: (2.5, 6.25),
                },
                &HeightEventQuery {
                    functional: HeightFunctional::from_frame(frame()),
                    heights: [3.9, 5.1],
                    uncertainty: 0.01,
                },
                &mut events,
                &mut proved,
                &mut work,
                &cancel,
            )
            .unwrap();
            assert!(events.iter().any(|event| event[0] <= 5. && 5. <= event[1]));
            assert!(proved.iter().any(|event| event[0] <= 5. && 5. <= event[1]));
            assert_eq!(
                prepared
                    .visit_explicit_knots(
                        [2.5, 6.25],
                        &mut GeometryWorkBudget::new(0),
                        &cancel,
                        &mut |_, _, _| Ok(())
                    )
                    .unwrap_err()
                    .code,
                GeometryErrorCode::ResourceLimit
            );
            assert_eq!(
                prepared
                    .visit_explicit_knots(
                        [2.5, 6.25],
                        &mut work,
                        &AtomicBool::new(true),
                        &mut |_, _, _| Ok(())
                    )
                    .unwrap_err()
                    .code,
                GeometryErrorCode::Cancelled
            );
        }
    }

    #[test]
    fn uncertain_mapped_original_c0_knot_band_is_not_rounded_into_incidence() {
        use solidcraft_step::geom::{BSplineCurve, Curve, CurveForm, KnotSpec, Logical};
        let original = Curve::BSpline(Box::new(BSplineCurve {
            degree: 1,
            points: vec![[0., 0., 0.], [1., 0., 1.], [2., 0., 0.]],
            weights: None,
            knots: vec![0.1, 0.2, 0.4],
            multiplicities: vec![2, 1, 2],
            form: CurveForm::PolylineForm,
            closed: Logical::False,
            self_intersect: Logical::False,
            knot_spec: KnotSpec::Unspecified,
        }));
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let source = crate::kernel::source_operand::PreparedOriginalCurve::new(
            &original,
            [0.1, 0.4],
            MAX_SLAB_SCRATCH_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let prepared = crate::parametric::prepare_curve_enclosure(
            &source,
            (0.1, 0.4),
            MAX_SLAB_SCRATCH_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(
            !prepared
                .is_smooth_on([0.1, 0.4], &mut work, &cancel)
                .unwrap()
        );
        let mut knots = Vec::new();
        assert!(
            prepared
                .visit_explicit_knots([0.1, 0.4], &mut work, &cancel, &mut |knot, _, _| {
                    knots.push(knot);
                    Ok(())
                },)
                .unwrap()
        );
        assert_eq!(
            knots,
            vec![0.2],
            "source knots stay in the actual original gauge"
        );
        let mut events = Vec::new();
        let mut proved = Vec::new();
        isolate_curve_events(
            &OriginalCurveSpan {
                curve: &prepared,
                range: (0.1, 0.4),
            },
            &HeightEventQuery {
                functional: HeightFunctional::from_frame(frame()),
                heights: [-0.1, 1.1],
                uncertainty: 0.01,
            },
            &mut events,
            &mut proved,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(
            events
                .iter()
                .any(|height| height[0] <= 1.0 && height[1] >= 1.0)
        );
        assert!(
            proved
                .iter()
                .any(|height| height[0] <= 1.0 && height[1] >= 1.0)
        );

        // A different, actual native coefficient chart has its own literal
        // knot. It is not an original source operator or an established gauge.
        let native = native_test_curve(
            &[[2., 0., 0., 1.], [1., 0., 1., 1.], [0., 0., 0., 1.]],
            1,
            &[0.1, 0.1, 0.3, 0.4, 0.4],
        );
        assert!(
            native
                .source_parameter_image([0.1, 0.4], &mut work, &cancel)
                .unwrap()
                .is_none()
        );
        let reflected = Interval::point(0.1)
            .add(Interval::point(0.4))
            .sub(Interval::point(0.2));
        assert!(
            reflected.lo < reflected.hi && reflected.lo <= 0.3 && reflected.hi >= 0.3,
            "the inverse expression is an enclosure, never rounded original incidence"
        );
        assert_eq!(
            prepared
                .visit_explicit_knots(
                    [0.1, 0.4],
                    &mut GeometryWorkBudget::new(0),
                    &cancel,
                    &mut |_, _, _| Ok(()),
                )
                .unwrap_err()
                .code,
            GeometryErrorCode::ResourceLimit
        );
        assert_eq!(
            prepared
                .visit_explicit_knots(
                    [0.1, 0.4],
                    &mut work,
                    &AtomicBool::new(true),
                    &mut |_, _, _| Ok(()),
                )
                .unwrap_err()
                .code,
            GeometryErrorCode::Cancelled
        );
    }

    #[test]
    fn original_torus_self_seam_and_closed_vertex_links_are_height_transverse_at_zero() {
        let definition = parity_definition("native-parity-nurbs-torus.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(30_000_000);
        let prepared_source = super::super::material_source::prepare_solid(
            &definition,
            MAX_SLAB_SCRATCH_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let source = prepared_source.source_view().unwrap();
        let domain = source
            .face_domain(0, MAX_SLAB_SCRATCH_BYTES, &mut work, &cancel)
            .unwrap();
        let allowance = MAX_SLAB_SCRATCH_BYTES - prepared_source.retained_bytes();
        let mut links = HeightSourceLinks::new(&definition, allowance, &mut work, &cancel).unwrap();
        let functional = HeightFunctional::from_frame(frame());
        let mut constant = 0;
        super::super::critical::with_original_source_face(&domain, trim_parameter_box(&domain).unwrap(),
            |original_face| -> Result<(), GeometryError> {
            let glued = original_face.prove_gluing(&mut work, &cancel)?.unwrap();
            let joint = glued.normal([[1., 1.], [0., 0.]], &mut work, &cancel)?;
            assert!(norm(joint)?.lo > 0., "the original positive G1 joint has a regular one-sided normal");
            let crossing = glued.normal([[0.9, 1.1], [0., 0.]], &mut work, &cancel).unwrap_err();
            assert_eq!(crossing.code, GeometryErrorCode::SingularGeometry);
            assert!(crossing.message.contains("NonSmooth"),
                "a positive-width query crossing both original knot interiors never borrows a one-sided normal");
            for coedge in domain.coedges() {
                if constant_native_curve_height(definition.edge_curve(coedge.edge)
                    .expect("imported native seam edge has a carrier"), functional, &mut work, &cancel)?.is_some() {
                    assert!(links.whole_edge_regular(&FaceHeightQuery { face: 0, glued: &glued, functional }, coedge.edge, allowance, &mut work, &cancel)?);
                    constant += 1;
                }
                for vertex in coedge.vertices {
                    assert!(links.vertex_regular(&definition, &FaceHeightQuery { face: 0, glued: &glued, functional }, vertex,
                        allowance, &mut work, &cancel)?,
                        "all original incident uses, actual closed vertex cycle and local UV/joint tubes qualify");
                }
            }
            // Actual source top/bottom rings: a smooth carrier/seam is NOT
            // enough when its geometric normal is parallel to height.
            for parameter in [1., 3.] {
                assert!(!source_height_transverse(&glued, [[0., 4.], [parameter, parameter]],
                    functional, &mut work, &cancel)?);
            }
            let vertex = domain.coedges()[0].vertices[0];
            assert_eq!(links.vertex_regular(&definition, &FaceHeightQuery { face: 0, glued: &glued, functional }, vertex,
                allowance, &mut work, &AtomicBool::new(true)).unwrap_err().code, GeometryErrorCode::Cancelled);
            Ok(())
        }).unwrap();
        assert!(constant > 0);
    }

    #[test]
    fn transport_supports_are_owned_metadata_not_isolated_events() {
        let cancel = AtomicBool::new(false);
        let definition = parity_definition("native-parity-blind-pocket.step");
        let mut work = GeometryWorkBudget::new(5_000_000);
        let prepared_source = super::super::material_source::prepare_solid(
            &definition,
            MAX_SLAB_SCRATCH_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let source = prepared_source.source_view().unwrap();
        let policy = AnalysisPolicy {
            maximum_height_uncertainty_mm: 1.0e-7,
            maximum_shape_error_mm: 0.1,
        };
        let covers = material_transport_height_cover(
            source,
            HeightFunctional::from_frame(frame()),
            [-1.0, 9.0],
            MAX_SLAB_SCRATCH_BYTES - prepared_source.retained_bytes(),
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(covers.len() <= MAX_EVENT_BRACKETS);
        assert!(
            covers.iter().any(|cover| {
                cover.base_interval[1].max(cover.corrected_interval[1])
                    - cover.base_interval[0].min(cover.corrected_interval[0])
                    > policy.maximum_height_uncertainty_mm
            }),
            "a full transport support must never be tiled into fabricated critical heights"
        );
        let retained =
            covers.capacity() * std::mem::size_of::<super::super::SourceTransportCover>();
        let material = analyze_height_domain_with_scratch_limit(
            source,
            frame(),
            [5.9, 6.1],
            policy,
            MAX_SLAB_SCRATCH_BYTES - prepared_source.retained_bytes() - retained,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(material.event_brackets_mm.is_empty());
        assert!(!material.capture.transport_support.is_empty());
        assert!(material.capture.transport_support_complete);
        assert!(material.requires_closed_material_capture([5.9, 6.1]));
    }

    #[test]
    fn actual_original_torus_height_zero_has_no_seam_or_vertex_critical_alias() {
        let definition = parity_definition("native-parity-nurbs-torus.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(30_000_000);
        let census = analyze_height_domain(
            &definition,
            frame(),
            [-0.01, 0.01],
            AnalysisPolicy {
                maximum_height_uncertainty_mm: 0.001,
                maximum_shape_error_mm: 0.1,
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(
            census.event_brackets_mm.is_empty() && census.proved_event_brackets_mm.is_empty(),
            "an original regular torus level is not critical merely because source chart cuts meet it; candidates={:?}, proved={:?}, consumed_work={}",
            census.event_brackets_mm,
            census.proved_event_brackets_mm,
            work.consumed(),
        );
        assert_eq!(
            census.requires_closed_material_capture([-0.01, 0.01]),
            !material_transport_zero(&definition, &mut work, &cancel).unwrap(),
            "an empty critical-event list alone never certifies a transported material band",
        );
    }

    #[test]
    fn shared_native_source_incidence_rejects_changed_association_and_keeps_failed_work() {
        let mut definition = parity_definition("native-parity-nurbs-torus.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let before = work.consumed();
        let mut incidence =
            SourceEdgeIncidence::new(&definition, MAX_SLAB_SCRATCH_BYTES, &mut work, &cancel)
                .unwrap();
        incidence
            .prepare_vertices(MAX_SLAB_SCRATCH_BYTES, &mut work, &cancel)
            .unwrap();
        for &vertex in definition.brep().vertex_list() {
            assert!(
                incidence
                    .vertex_link_is_cycle(&definition, vertex, &mut work, &cancel)
                    .unwrap()
            );
        }
        assert!(work.consumed() > before && incidence.owned_bytes() < MAX_SLAB_SCRATCH_BYTES);
        assert_eq!(
            SourceEdgeIncidence::new(&definition, 0, &mut work, &cancel)
                .err()
                .unwrap()
                .code,
            GeometryErrorCode::ResourceLimit
        );
        let before = work.consumed();
        let loop_id = definition
            .brep()
            .face(definition.face_ids()[0])
            .unwrap()
            .loops[0];
        let edge = &mut definition.brep.loop_mut(loop_id).unwrap().edges[0];
        edge.forward = !edge.forward;
        assert_eq!(
            SourceEdgeIncidence::new(&definition, MAX_SLAB_SCRATCH_BYTES, &mut work, &cancel)
                .err()
                .unwrap()
                .code,
            GeometryErrorCode::SingularGeometry
        );
        assert!(work.consumed() > before);
        assert_eq!(
            SourceEdgeIncidence::new(
                &definition,
                MAX_SLAB_SCRATCH_BYTES,
                &mut work,
                &AtomicBool::new(true)
            )
            .err()
            .unwrap()
            .code,
            GeometryErrorCode::Cancelled
        );
    }

    #[test]
    fn actual_original_rational_circle_passes_whole_coedge_height_census() {
        let definition = parity_definition("native-parity-nurbs-torus.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(30_000_000);
        let functional = HeightFunctional::from_frame(frame());
        let mut observed = false;
        let mut covered = Vec::new();
        for &edge_id in definition.brep().edge_list() {
            let edge = definition.brep().edge(edge_id).unwrap();
            let curve = definition.edge_curve(edge_id).unwrap();
            let curve_range = (edge.interval.min(), edge.interval.max());
            let varying = (|| -> Result<bool, GeometryError> {
                if constant_native_curve_height(curve, functional, &mut work, &cancel)?.is_some() {
                    return Ok(false);
                }
                let prepared = crate::parametric::prepare_curve_enclosure(
                    curve,
                    curve_range,
                    MAX_SLAB_SCRATCH_BYTES,
                    &mut work,
                    &cancel,
                )?;
                assert!(
                    !prepared.is_smooth_on([curve_range.0, curve_range.1], &mut work, &cancel)?,
                    "geometric circle/G1 does not erase an actual C0 parameter-speed jump"
                );
                let mut events = Vec::new();
                let mut proved = Vec::new();
                isolate_curve_events(
                    &OriginalCurveSpan {
                        curve: &prepared,
                        range: curve_range,
                    },
                    &HeightEventQuery {
                        functional,
                        heights: [-2.1, 2.1],
                        uncertainty: 0.01,
                    },
                    &mut events,
                    &mut proved,
                    &mut work,
                    &cancel,
                )?;
                covered.extend(events);
                Ok(true)
            })()
            .unwrap();
            if varying {
                observed = true;
                // Both original meridian halves retain their own finite
                // source intervals; inventory the actual union, not a
                // fictitious full-circle extension of the first half.
            }
        }
        assert!(
            observed,
            "actual original STEP meridian, not a synthetic Definition"
        );
        for height in [-2.0, 2.0] {
            assert!(
                covered
                    .iter()
                    .any(|event| event[0] <= height && height <= event[1])
            );
        }
    }
    #[test]
    fn native_height_events_include_curved_interior_critical_values() {
        let cancel = AtomicBool::new(false);
        let definition = parity_definition("native-parity-ring-torus.step");
        let mut work = GeometryWorkBudget::new(20_000_000);
        let result = analyze_height_domain(
            &definition,
            frame(),
            [-2.1, 2.1],
            AnalysisPolicy {
                maximum_height_uncertainty_mm: 0.01,
                maximum_shape_error_mm: 0.1,
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        for critical in [-2.0, 2.0] {
            assert!(
                result
                    .event_brackets_mm
                    .iter()
                    .any(|event| event[0] <= critical && event[1] >= critical)
            );
        }
        assert!(result.invariance.is_none());
    }
    #[test]
    fn rounded_housing_height_census_retains_caps_and_pocket_floor_with_frozen_work() {
        let cancel = AtomicBool::new(false);
        let definition = crate::from_step_bytes_with_cancel(
            include_bytes!(
                "../../../../fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step"
            ),
            &cancel,
        )
        .expect("unchanged independently exported housing");
        let mut work = GeometryWorkBudget::new(20_000_000);
        let result = analyze_height_domain(
            &definition,
            frame(),
            [-0.1, 10.1],
            AnalysisPolicy {
                maximum_height_uncertainty_mm: 1e-7,
                maximum_shape_error_mm: 0.1,
            },
            &mut work,
            &cancel,
        )
        .expect("critical circles must not exhaust work along constant-height generators");
        for height in [0.0, 4.13, 10.0] {
            assert!(
                result.event_brackets_mm.iter().any(|event| {
                    event[0] <= height && height <= event[1] && event[1] - event[0] <= 1e-7
                }),
                "missing actual cap or blind-pocket floor at {height}"
            );
        }
    }
    #[test]
    fn rounded_housing_compile_policy_does_not_schedule_near_pole_search_cell_heights() {
        let cancel = AtomicBool::new(false);
        let definition = crate::from_step_bytes_with_cancel(
            include_bytes!(
                "../../../../fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step"
            ),
            &cancel,
        )
        .unwrap();
        let mut work = GeometryWorkBudget::new(20_000_000);
        let result = analyze_height_domain(
            &definition,
            frame(),
            [-0.1, 10.1],
            AnalysisPolicy {
                maximum_height_uncertainty_mm: 1e-6,
                maximum_shape_error_mm: 0.1,
            },
            &mut work,
            &cancel,
        )
        .expect("the actual Compile uncertainty must not preserve loose pole strips");
        for height in [2.0, 8.0] {
            let mut retained = false;
            for event in &result.event_brackets_mm {
                if event[0] <= height + 1e-4 && event[1] >= height - 1e-4 {
                    retained = true;
                    assert!(
                        event[0] <= height && height <= event[1],
                        "the original pole/vertex event, not a displaced search height: {event:?}"
                    );
                    assert!(
                        event[1] - event[0] < 1e-12,
                        "a long chart strip is not the isolated pole height: {event:?}"
                    );
                }
            }
            assert!(
                retained,
                "the original source pole/vertex at {height} remains"
            );
        }
    }
    #[test]
    fn actual_bore_interior_uses_realized_regular_flow_under_unchanged_shape_policy() {
        let definition = parity_definition("native-parity-blind-pocket.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(70_000_000);
        let source = super::super::material_source::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let slab = analyze_slab(
            &source,
            SlabRequest {
                frame: frame(),
                height_interval_mm: [5.9, 6.1],
                sample_height_mm: 6.0,
                native_shape_error_mm: 0.05,
            },
            crate::MAX_PREPARED_SECTION_BYTES - source.retained_bytes(),
            &mut work,
            &cancel,
        )
        .expect("actual interior transport must use its realized flow, not a full vertical return");
        assert!(
            slab.closed_shape_error_mm()
                .is_some_and(|bound| bound <= 0.05)
        );
        assert!(slab.critical.as_ref().is_some_and(|proof| {
            proof
                .regular_flow_speed_mm_per_height()
                .is_some_and(|speed| speed.is_finite())
        }));
        if !source
            .source_view()
            .unwrap()
            .material_transport_is_proved_zero(&mut work, &cancel)
            .unwrap()
        {
            assert!(
                slab.invariance.is_none(),
                "a G-aware regular flow is not exact material invariance"
            );
        }
    }

    #[test]
    fn source_vertical_flow_covers_bores_generic_extrusions_and_exact_tensor_columns() {
        let cancel = AtomicBool::new(false);
        for name in [
            "native-parity-bezier-extrusion-bore.step",
            "native-parity-rotated-bezier-bore.step",
            "native-parity-nurbs-cylinder.step",
            "native-parity-nurbs-box.step",
        ] {
            let definition = parity_definition(name);
            let mut work = GeometryWorkBudget::new(30_000_000);
            let owner = super::super::material_source::prepare_solid(
                &definition,
                MAX_SLAB_SCRATCH_BYTES,
                &mut work,
                &cancel,
            )
            .unwrap();
            let source = owner.source_view().unwrap();
            let remaining = MAX_SLAB_SCRATCH_BYTES - owner.retained_bytes();
            let zero = source
                .material_transport_is_proved_zero(&mut work, &cancel)
                .unwrap();
            let certificate = narrow_invariance_with_scratch_limit(
                source,
                frame(),
                [-1.0, 9.0],
                remaining,
                &mut work,
                &cancel,
            )
            .unwrap();
            assert_eq!(
                certificate.is_some(),
                zero,
                "{name}: raw flow requires a zero-transport theorem"
            );
            if let Some(certificate) = certificate {
                assert_eq!(certificate.height_interval_mm, [0.0, 8.0], "{name}");
            }
            let inner = narrow_invariance_with_scratch_limit(
                source,
                frame(),
                [2.0, 6.0],
                remaining,
                &mut work,
                &cancel,
            )
            .unwrap();
            assert_eq!(inner.is_some(), zero, "{name}");
            if let Some(inner) = inner {
                assert_eq!(inner.height_interval_mm, [2.0, 6.0], "{name}");
            }
        }
    }
    #[test]
    fn exact_native_product_retains_flow_without_erasing_curved_sample_error() {
        let definition = parity_definition("native-parity-nurbs-cylinder.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(70_000_000);
        let source = super::super::material_source::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let slab = analyze_slab(
            &source,
            SlabRequest {
                frame: frame(),
                height_interval_mm: [2.0, 6.0],
                sample_height_mm: 3.3,
                native_shape_error_mm: 0.05,
            },
            crate::MAX_PREPARED_SECTION_BYTES - source.retained_bytes(),
            &mut work,
            &cancel,
        )
        .unwrap();
        let zero = source
            .source_view()
            .unwrap()
            .material_transport_is_proved_zero(&mut work, &cancel)
            .unwrap();
        assert_eq!(slab.invariance.is_some(), zero);
        let proof = slab
            .critical
            .as_ref()
            .expect("the exact original product is a real continuation");
        if zero {
            assert_eq!(proof.regular_flow_speed_mm_per_height(), Some(0.0));
        }
        assert_eq!(proof.material_component_count(), 1);
        assert_eq!(proof.tunnel_cycle_count(), 0);
        assert!(
            !proof.has_native_convex_section_topology_proof(),
            "one outer source-M loop plus a product is not a whole-source curvature proof"
        );
        assert!(
            slab.closed_shape_error_mm()
                .is_some_and(|bound| bound > 0.0 && bound <= 0.05),
            "unchanged original circle is not its finite polygon"
        );
        let closed = slab
            .occupied
            .as_ref()
            .expect("actual all-height source outer envelope");
        assert!(closed.loops.iter().all(|boundary| !boundary.is_hole));
        for axis in 0..2 {
            let lower = closed
                .loops
                .iter()
                .flat_map(|boundary| boundary.points_mm.iter())
                .map(|point| point[axis])
                .fold(f64::INFINITY, f64::min);
            let upper = closed
                .loops
                .iter()
                .flat_map(|boundary| boundary.points_mm.iter())
                .map(|point| point[axis])
                .fold(f64::NEG_INFINITY, f64::max);
            assert!(lower <= -5.0 && upper >= 5.0);
        }
    }
    #[test]
    fn actual_product_proof_honors_remaining_bytes_and_cumulative_cancellation() {
        let definition = parity_definition("native-parity-nurbs-cylinder.step");
        let cancel = AtomicBool::new(false);
        let mut preparation_work = GeometryWorkBudget::new(100_000_000);
        let source = super::super::material_source::prepare_solid(
            &definition,
            MAX_SLAB_SCRATCH_BYTES,
            &mut preparation_work,
            &cancel,
        )
        .unwrap();
        let mut work = GeometryWorkBudget::new(1000);
        let before = work.consumed();
        let error = source_vertical_boundary_flow(
            source.source_view().unwrap(),
            frame(),
            [2.0, 6.0],
            0,
            &mut work,
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert_eq!(error.code, GeometryErrorCode::ResourceLimit);
        assert!(work.consumed() > before);
        let error = source_vertical_boundary_flow(
            source.source_view().unwrap(),
            frame(),
            [2.0, 6.0],
            0,
            &mut work,
            &AtomicBool::new(true),
        )
        .unwrap_err();
        assert_eq!(error.code, GeometryErrorCode::Cancelled);
    }
    #[test]
    fn broad_source_height_query_retains_only_the_proved_column_interval() {
        let definition = parity_definition("native-parity-nurbs-cylinder.step");
        let mut work = GeometryWorkBudget::new(30_000_000);
        let result = analyze_height_domain(
            &definition,
            frame(),
            [-1.0, 9.0],
            AnalysisPolicy {
                maximum_height_uncertainty_mm: 0.01,
                maximum_shape_error_mm: 0.1,
            },
            &mut work,
            &AtomicBool::new(false),
        )
        .unwrap();
        let zero =
            material_transport_zero(&definition, &mut work, &AtomicBool::new(false)).unwrap();
        assert_eq!(result.invariance.is_some(), zero);
        assert_eq!(result.requires_closed_material_capture([2.0, 6.0]), !zero);
        if let Some(invariance) = result.invariance.as_ref() {
            assert_eq!(invariance.height_interval_mm, [0.0, 8.0]);
        }
        assert!(
            result
                .proved_event_brackets_mm
                .iter()
                .any(|bracket| bracket[0] <= 0.0 && bracket[1] >= 0.0)
        );
        assert!(
            result
                .proved_event_brackets_mm
                .iter()
                .any(|bracket| bracket[0] <= 8.0 && bracket[1] >= 8.0)
        );
    }
    #[test]
    fn vertical_flow_does_not_snap_a_revolution_or_torus_into_exact_reuse() {
        for name in [
            "native-parity-revolution.step",
            "native-parity-nurbs-torus.step",
        ] {
            let definition = parity_definition(name);
            let mut work = GeometryWorkBudget::new(30_000_000);
            assert!(
                narrow_invariance(
                    &definition,
                    frame(),
                    [1.0, 2.0],
                    &mut work,
                    &AtomicBool::new(false)
                )
                .unwrap()
                .is_none()
            );
        }
    }
    #[test]
    fn original_blind_floor_is_closed_stratum_not_the_ring_core_metric() {
        let cancel = AtomicBool::new(false);
        for name in [
            "native-parity-blind-pocket.step",
            "native-parity-blind-pocket-nurbs-floor.step",
        ] {
            let definition = parity_definition(name);
            let mut work = GeometryWorkBudget::new(90_000_000);
            let source = super::super::material_source::prepare_solid(
                &definition,
                crate::MAX_PREPARED_SECTION_BYTES,
                &mut work,
                &cancel,
            )
            .unwrap();
            let above = analyze_slab(
                &source,
                SlabRequest {
                    frame: frame(),
                    height_interval_mm: [4.0, 4.2],
                    sample_height_mm: 4.1,
                    native_shape_error_mm: 0.05,
                },
                crate::MAX_PREPARED_SECTION_BYTES - source.retained_bytes(),
                &mut work,
                &cancel,
            )
            .unwrap();
            assert_eq!(above.section.loops.len(), 2, "{name}");
            assert_eq!(
                above
                    .section
                    .loops
                    .iter()
                    .filter(|boundary| boundary.is_hole)
                    .count(),
                1
            );
            assert!(above.material_core_shape_error_mm() <= 0.05);
            assert!(
                above.closed_shape_error_mm().is_none(),
                "the full floor disk is not within this ring's small core error"
            );
            assert!(
                above.invariance.is_none(),
                "a source-closed reuse token cannot erase the outgoing floor"
            );
            assert_eq!(above.critical_material_component_count(), Some(1));
            assert_eq!(above.critical_bounded_void_count(), Some(0));
            assert_eq!(above.critical_tunnel_cycle_count(), Some(1));
            assert_eq!(above.endpoint_strata.len(), 1);
            let complex = above
                .collar_complex
                .as_ref()
                .expect("endpoint departing disk retains a genuine native collar complex");
            assert!(
                complex
                    .event_faces()
                    .any(|(_, height, sign)| height == [4.0, 4.0] && sign > 0)
            );
            let floor = &above.endpoint_strata[0];
            assert_eq!(floor.endpoint, super::super::NativeOwnedEndpoint::Lower);
            assert_eq!(floor.height_mm, 4.0);
            assert_eq!(floor.outward_height_sign, 1);
            assert!(!floor.patch_cover.is_empty());
            assert_eq!(floor.face_id, definition.faces[floor.face_index].face_id);
            let (_, original_face) =
                super::super::source_incidence::native_face(&definition, floor.face_index).unwrap();
            let incidence = original_edge_uses(&definition, &mut work, &cancel).unwrap();
            for wire_index in 0..original_face.loops.len() {
                let wire = super::super::source_incidence::boundary_loop(
                    &definition,
                    floor.face_index,
                    wire_index,
                )
                .unwrap();
                for (coedge_index, edge) in wire.edges.iter().enumerate() {
                    let uses = &incidence[&edge.edge];
                    let second = uses.second.unwrap();
                    assert_ne!(uses.first.orientation, second.orientation);
                    assert!(
                        [uses.first, second]
                            .iter()
                            .any(|use_| use_.face == floor.face_index
                                && use_.wire == wire_index
                                && use_.index == coedge_index)
                    );
                    let native_face = definition.face_ids()[floor.face_index];
                    let original = definition.original_source().face(native_face).unwrap();
                    assert!(!original.wires.is_empty());
                    for &source_index in original.wires.iter().flat_map(|wire| &wire.coedges) {
                        let source = definition.original_source().coedge(source_index).unwrap();
                        assert_eq!(source.source_face, original.source_face);
                    }
                }
            }
            let occupied = above.occupied.as_ref().unwrap();
            assert!(!occupied.loops.is_empty());
            assert!(occupied.loops.iter().all(|boundary| !boundary.is_hole));
            for axis in 0..2 {
                let lower = occupied
                    .loops
                    .iter()
                    .flat_map(|boundary| boundary.points_mm.iter())
                    .map(|point| point[axis])
                    .fold(f64::INFINITY, f64::min);
                let upper = occupied
                    .loops
                    .iter()
                    .flat_map(|boundary| boundary.points_mm.iter())
                    .map(|point| point[axis])
                    .fold(f64::NEG_INFINITY, f64::max);
                assert!(
                    lower <= 10.0 && upper >= 10.0,
                    "closed C must retain the original floor center"
                );
            }
            drop(incidence);
            drop(above);
            let below = analyze_slab(
                &source,
                SlabRequest {
                    frame: frame(),
                    height_interval_mm: [3.8, 4.0],
                    sample_height_mm: 3.9,
                    native_shape_error_mm: 0.05,
                },
                crate::MAX_PREPARED_SECTION_BYTES - source.retained_bytes(),
                &mut work,
                &cancel,
            )
            .unwrap();
            assert_eq!(below.section.loops.len(), 1);
            assert!(
                below.endpoint_strata.is_empty(),
                "the incoming floor belongs to the below-core closure"
            );
            assert!(
                below
                    .closed_shape_error_mm()
                    .is_some_and(|bound| bound <= 0.05)
            );
            assert_eq!(below.critical_tunnel_cycle_count(), Some(0));
            drop(below);
            let narrow = analyze_slab(
                &source,
                SlabRequest {
                    frame: frame(),
                    height_interval_mm: [4.0, 4.02],
                    sample_height_mm: 4.01,
                    native_shape_error_mm: 0.05,
                },
                crate::MAX_PREPARED_SECTION_BYTES - source.retained_bytes(),
                &mut work,
                &cancel,
            )
            .unwrap();
            assert_eq!(narrow.endpoint_strata.len(), 1);
            assert!(
                narrow.closed_shape_error_mm().is_none(),
                "shrinking height cannot turn the closed disk into the ring core"
            );
            assert_eq!(narrow.request().sample_height_mm, 4.01);
        }
    }
    #[test]
    fn a_true_interior_floor_band_uses_native_collar_incidence_not_closed_hausdorff() {
        let cancel = AtomicBool::new(false);
        for name in [
            "native-parity-blind-pocket.step",
            "native-parity-blind-pocket-nurbs-floor.step",
        ] {
            let definition = parity_definition(name);
            let mut work = GeometryWorkBudget::new(50_000_000);
            let source = super::super::material_source::prepare_solid(
                &definition,
                crate::MAX_PREPARED_SECTION_BYTES,
                &mut work,
                &cancel,
            )
            .unwrap();
            let slab = analyze_slab(
                &source,
                SlabRequest {
                    frame: frame(),
                    height_interval_mm: [3.99, 4.01],
                    sample_height_mm: 4.005,
                    native_shape_error_mm: 0.05,
                },
                crate::MAX_PREPARED_SECTION_BYTES - source.retained_bytes(),
                &mut work,
                &cancel,
            )
            .unwrap();
            assert_eq!(slab.section.loops.len(), 2, "{name}");
            assert_eq!(slab.request().sample_height_mm, 4.005);
            assert!(slab.material_core_shape_error_mm() <= 0.05);
            assert!(slab.closed_shape_error_mm().is_none());
            assert!(slab.has_transition_material());
            assert!(
                slab.endpoint_strata.is_empty(),
                "the source event is strictly inside this band"
            );
            let complex = slab
                .collar_complex
                .as_ref()
                .expect("actual original native critical complex");
            assert_eq!(complex.definition_id(), definition.id());
            assert!(
                complex
                    .event_faces()
                    .any(|(_, height, sign)| height[0] <= 4.0 && height[1] >= 4.0 && sign > 0)
            );
            assert!(
                slab.occupied
                    .as_ref()
                    .is_some_and(|closed| !closed.loops.is_empty())
            );
            assert!(
                slab.critical_material_component_count().is_none(),
                "a transition complex is not a fictitious sample-count critical-slab certificate"
            );
        }
    }
    #[test]
    fn empty_native_box_and_pocket_terminal_cores_retain_real_transition_complexes() {
        let cancel = AtomicBool::new(false);
        for name in [
            "native-parity-nurbs-box.step",
            "native-parity-blind-pocket.step",
            "native-parity-blind-pocket-nurbs-floor.step",
        ] {
            let definition = parity_definition(name);
            let mut work = GeometryWorkBudget::new(70_000_000);
            let source = super::super::material_source::prepare_solid(
                &definition,
                crate::MAX_PREPARED_SECTION_BYTES,
                &mut work,
                &cancel,
            )
            .unwrap();
            let slab = analyze_slab(
                &source,
                SlabRequest {
                    frame: frame(),
                    height_interval_mm: [8.0, 8.02],
                    sample_height_mm: 8.01,
                    native_shape_error_mm: 0.05,
                },
                crate::MAX_PREPARED_SECTION_BYTES - source.retained_bytes(),
                &mut work,
                &cancel,
            )
            .unwrap();
            assert!(slab.section.loops.is_empty(), "{name}");
            assert_eq!(slab.material_core_shape_error_mm(), 0.0);
            assert!(
                slab.closed_shape_error_mm().is_none(),
                "nonempty endpoint C is not an empty core"
            );
            assert!(
                slab.invariance.is_none(),
                "CLOSED contact cannot become an empty reuse token"
            );
            assert_eq!(slab.endpoint_strata.len(), 1);
            assert_eq!(slab.endpoint_strata[0].height_mm, 8.0);
            assert_eq!(slab.endpoint_strata[0].outward_height_sign, 1);
            assert_eq!(slab.critical_material_component_count(), Some(0));
            let complex = slab
                .collar_complex
                .as_ref()
                .expect("directed terminal leaf needs genuine native complex");
            assert!(
                complex
                    .event_faces()
                    .any(|(_, height, sign)| height == [8.0, 8.0] && sign > 0)
            );
            assert!(slab.has_transition_material());
            assert!(
                slab.occupied
                    .as_ref()
                    .is_some_and(|closed| !closed.loops.is_empty())
            );
        }
    }
    #[test]
    fn reversed_height_uses_the_same_source_floor_as_an_upper_outgoing_stratum() {
        let cancel = AtomicBool::new(false);
        let definition = parity_definition("native-parity-blind-pocket-nurbs-floor.step");
        let mut work = GeometryWorkBudget::new(30_000_000);
        let source = super::super::material_source::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let reversed = PlaneFrameMm {
            y_axis: [0.0, -1.0, 0.0],
            z_axis: [0.0, 0.0, -1.0],
            ..frame()
        };
        let slab = analyze_slab(
            &source,
            SlabRequest {
                frame: reversed,
                height_interval_mm: [-4.2, -4.0],
                sample_height_mm: -4.1,
                native_shape_error_mm: 0.05,
            },
            crate::MAX_PREPARED_SECTION_BYTES - source.retained_bytes(),
            &mut work,
            &cancel,
        )
        .unwrap();
        assert_eq!(slab.section.loops.len(), 2);
        assert_eq!(slab.endpoint_strata.len(), 1);
        assert_eq!(
            slab.endpoint_strata[0].endpoint,
            super::super::NativeOwnedEndpoint::Upper
        );
        assert_eq!(slab.endpoint_strata[0].outward_height_sign, -1);
        assert_eq!(slab.endpoint_strata[0].height_mm, -4.0);
        assert!(slab.closed_shape_error_mm().is_none());
    }
    #[test]
    fn independent_original_narrow_pocket_preserves_its_exact_endpoint_floor() {
        let cancel = AtomicBool::new(false);
        let definition = crate::from_step_bytes_with_cancel(
            include_bytes!(
                "../../../../fixtures/geometry/independent-exporters/occt-narrow-pocket-mm.step"
            ),
            &cancel,
        )
        .unwrap();
        let mut work = GeometryWorkBudget::new(50_000_000);
        let source = super::super::material_source::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let slab = analyze_slab(
            &source,
            SlabRequest {
                frame: frame(),
                height_interval_mm: [1.07, 1.09],
                sample_height_mm: 1.08,
                native_shape_error_mm: 0.05,
            },
            crate::MAX_PREPARED_SECTION_BYTES - source.retained_bytes(),
            &mut work,
            &cancel,
        )
        .unwrap();
        assert_eq!(slab.section.loops.len(), 2);
        assert_eq!(slab.endpoint_strata.len(), 1);
        assert_eq!(slab.endpoint_strata[0].height_mm, 1.07);
        assert!(slab.closed_shape_error_mm().is_none());
        assert!(slab.material_core_shape_error_mm() <= 0.05);
    }
    #[test]
    fn height_analysis_honors_job_remaining_scratch_and_preserves_real_events() {
        let cancel = AtomicBool::new(false);
        let definition = parity_definition("native-parity-ring-torus.step");
        let policy = AnalysisPolicy {
            maximum_height_uncertainty_mm: 0.01,
            maximum_shape_error_mm: 0.1,
        };
        let mut work = GeometryWorkBudget::new(20_000_000);
        let error = analyze_height_domain_with_scratch_limit(
            super::super::material_source::prepare_solid(
                &definition,
                MAX_SLAB_SCRATCH_BYTES,
                &mut work,
                &cancel,
            )
            .unwrap()
            .source_view()
            .unwrap(),
            frame(),
            [-2.1, 2.1],
            policy,
            HEIGHT_EVENT_SCRATCH_BYTES + STACK_SCRATCH_BYTES - 1,
            &mut work,
            &cancel,
        )
        .unwrap_err();
        assert_eq!(error.code, GeometryErrorCode::ResourceLimit);
        let result = analyze_height_domain_with_scratch_limit(
            super::super::material_source::prepare_solid(
                &definition,
                MAX_SLAB_SCRATCH_BYTES,
                &mut work,
                &cancel,
            )
            .unwrap()
            .source_view()
            .unwrap(),
            frame(),
            [-2.1, 2.1],
            policy,
            8 * 1024 * 1024,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(result.invariance.is_none());
        for critical in [-2.0, 2.0] {
            assert!(
                result
                    .proved_event_brackets_mm
                    .iter()
                    .any(|event| event[0] <= critical && critical <= event[1])
            );
        }
        let error = analyze_height_domain_with_scratch_limit(
            super::super::material_source::prepare_solid(
                &definition,
                MAX_SLAB_SCRATCH_BYTES,
                &mut work,
                &cancel,
            )
            .unwrap()
            .source_view()
            .unwrap(),
            frame(),
            [-2.1, 2.1],
            policy,
            0,
            &mut work,
            &AtomicBool::new(true),
        )
        .unwrap_err();
        assert_eq!(error.code, GeometryErrorCode::Cancelled);
    }
    #[test]
    fn native_sample_height_band_covers_plane_tube_and_origin_relative_direction_drift() {
        let cancel = AtomicBool::new(false);
        let definition = parity_definition("native-parity-full-sphere.step");
        let mut work = GeometryWorkBudget::new(30_000_000);
        let mut section = crate::section::section_for_query(
            &definition,
            &crate::section::SectionQuery {
                plane: PlaneMm {
                    origin_mm: [0.0; 3],
                    normal: [0.0, 0.0, 1.0],
                },
                maximum_chord_error_mm: spiling_contracts::geometry::SECTION_SAMPLING_TOLERANCE_MM,
                maximum_plane_error_mm: spiling_contracts::geometry::SECTION_PLANE_TOLERANCE_MM,
            },
            &cancel,
            MAX_SECTION_BYTES,
            &mut work,
        )
        .unwrap();
        // Math regression on a real captured source: the perturbation's
        // functional is centered at this plane origin, not the source origin.
        section.plane = PlaneMm {
            origin_mm: [1000.0, 0.0, 0.0],
            normal: [1e-6, 0.0, 1.0],
        };
        section.certificate.maximum_plane_error_mm = 0.01;
        let band =
            native_sample_height_band(&definition, frame(), &section, &mut work, &cancel).unwrap();
        assert!(band.lo <= -0.011 && band.hi >= 0.011);
    }
    #[test]
    fn homogeneous_constancy_preserves_a_nonrepresentable_actual_height_interval() {
        use solidcore::nurbs::{b_spline_surface::BSplineSurface, math::CPoint, srf_basic};
        let knots = vec![0.0, 0.0, 1.0, 1.0];
        let controls = vec![
            vec![
                CPoint::new(0.0, 0.0, 1.0, 3.0),
                CPoint::new(0.0, 6.0, 2.0, 6.0),
            ],
            vec![
                CPoint::new(6.0, 0.0, 2.0, 6.0),
                CPoint::new(12.0, 12.0, 4.0, 12.0),
            ],
        ];
        let raw = srf_basic::Surface::new(controls, 1, 1, knots.clone(), knots).unwrap();
        let surface = Surface::new(SurfaceKind::BSpline(BSplineSurface::from_nurb(raw, true)));
        let curve = native_test_curve(
            &[[0.0, 0.0, 1.0, 3.0], [6.0, 0.0, 2.0, 6.0]],
            1,
            &[0.0, 0.0, 1.0, 1.0],
        );
        let mut work = GeometryWorkBudget::new(10_000);
        let cancel = AtomicBool::new(false);
        let functional = HeightFunctional::from_frame(frame());
        let h = constant_spline_height(&surface, functional, &mut work, &cancel)
            .unwrap()
            .unwrap();
        assert!(h.lo < h.hi && h.lo <= 1.0 / 3.0 && h.hi >= 1.0 / 3.0);
        let edge_h = constant_native_curve_height(&curve, functional, &mut work, &cancel)
            .unwrap()
            .unwrap();
        assert!(edge_h.lo < edge_h.hi);
        let scaled = HeightFunctional {
            direction: [
                Interval::point(0.0),
                Interval::point(0.0),
                Interval::point(3.0),
            ],
            offset: Interval::point(0.1),
        };
        let shifted = constant_spline_height(&surface, scaled, &mut work, &cancel)
            .unwrap()
            .unwrap();
        assert!(shifted.lo < shifted.hi && shifted.lo <= 1.1 && shifted.hi >= 1.1);
    }
    #[test]
    fn actual_source_floor_plus_nonrepresentable_translation_stays_a_band() {
        let definition = parity_definition("native-parity-blind-pocket-nurbs-floor.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let mut floor = None;
        for index in 0..definition.faces.len() {
            let h = constant_source_face_height(
                &definition,
                index,
                HeightFunctional::from_frame(frame()),
                &mut work,
                &cancel,
            )
            .unwrap();
            if h.is_some_and(|h| h.lo == 4.0 && h.hi == 4.0) {
                floor = Some(index);
                break;
            }
        }
        let world = HeightFunctional {
            direction: [
                Interval::point(0.0),
                Interval::point(0.0),
                Interval::point(1.0),
            ],
            offset: Interval::point(0.1),
        };
        let height = constant_source_face_height(
            &definition,
            floor.expect("original source floor"),
            world,
            &mut work,
            &cancel,
        )
        .unwrap()
        .unwrap();
        assert!(
            height.lo < height.hi,
            "4+0.1 is a true constant, not a binary64 singleton"
        );
        assert!(height.lo <= 4.1 && height.hi >= 4.1);
    }
    #[test]
    fn original_coefficient_identity_keeps_large_origin_relative_height_exact() {
        let curve = solidcraft_step::geom::Curve::Line {
            origin: [3.0, -4.0, 1.0e12],
            direction: [1.0, 0.0, 0.0],
            magnitude: 17.0,
        };
        let functional = HeightFunctional {
            direction: [0.0, 0.0, 1.0].map(Interval::point),
            offset: Interval::point(-1.0e12),
        };
        let mut work = GeometryWorkBudget::new(100);
        let cancel = AtomicBool::new(false);
        let height = constant_original_curve_height(&curve, functional, 0, &mut work, &cancel)
            .unwrap()
            .unwrap();
        assert_eq!([height.lo, height.hi], [0.0, 0.0]);
        assert_eq!(
            constant_original_curve_height(
                &curve,
                functional,
                0,
                &mut work,
                &AtomicBool::new(true),
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::Cancelled
        );
    }
    #[test]
    fn selected_cylindrical_ruling_is_constant_but_a_small_angular_drift_is_not() {
        use solidcraft_step::geom::{
            Associated, Curve, Pcurve, Placement, PreferredRep, Surface, SurfaceCurveKind,
        };
        use std::sync::Arc;
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let angle = 0.37_f64;
        let carrier = Arc::new(Surface::Cylinder {
            position: Placement {
                origin: [2.0, 0.0, 5.0],
                axis: [0.0, 1.0, 0.0],
                ref_direction: [1.0, 0.0, 0.0],
            },
            radius: 2.0,
        });
        let functional = HeightFunctional {
            direction: [0.0, 0.0, 1.0].map(Interval::point),
            offset: Interval::point(0.0),
        };
        for angular_drift in [0.0, 1.0e-8] {
            let source = Curve::SurfaceCurve {
                kind: SurfaceCurveKind::Surface,
                curve_3d: Arc::new(Curve::Line {
                    origin: [0.0, 0.0, 123.0],
                    direction: [0.0, 0.0, 1.0],
                    magnitude: 1.0,
                }),
                associated: vec![Associated::Pcurve(Pcurve {
                    surface: carrier.clone(),
                    curve: Arc::new(Curve::Line {
                        origin: [angle, 2.0, 0.0],
                        direction: [angular_drift, 1.0, 0.0],
                        magnitude: 14.0,
                    }),
                })],
                master: PreferredRep::PcurveS1,
            };
            let prepared = crate::kernel::source_operand::PreparedOriginalCurve::new(
                &source,
                [0.0, 1.0],
                MAX_SLAB_SCRATCH_BYTES,
                &mut work,
                &cancel,
            )
            .unwrap();
            let height =
                constant_native_curve_height(&prepared, functional, &mut work, &cancel).unwrap();
            if angular_drift == 0.0 {
                let height = height.unwrap();
                assert!(height.contains(5.0 - 2.0 * angle.sin()));
                assert!(height.lo > 4.0 && height.hi < 5.0);
            } else {
                assert!(height.is_none());
            }
        }
    }

    #[test]
    fn piecewise_horizontal_height_jump_is_not_a_constant_curve() {
        use solidcraft_step::geom::{BSplineCurve, Curve, CurveForm, KnotSpec, Logical};
        let source = Curve::BSpline(Box::new(BSplineCurve {
            degree: 1,
            points: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 0.0, 1.0],
                [2.0, 0.0, 1.0],
            ],
            weights: None,
            knots: vec![0.0, 0.5, 1.0],
            multiplicities: vec![2, 2, 2],
            form: CurveForm::PolylineForm,
            closed: Logical::False,
            self_intersect: Logical::False,
            knot_spec: KnotSpec::Unspecified,
        }));
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let prepared = crate::kernel::source_operand::PreparedOriginalCurve::new(
            &source,
            [0.0, 1.0],
            MAX_SLAB_SCRATCH_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let height = constant_native_curve_height(
            &prepared,
            HeightFunctional {
                direction: [0.0, 0.0, 1.0].map(Interval::point),
                offset: Interval::point(0.0),
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(height.is_none());
    }

    #[test]
    fn source_constancy_follows_selected_pcurve_not_unselected_spatial_line() {
        use solidcraft_step::geom::{
            Associated, Curve, Pcurve, Placement, PreferredRep, Surface, SurfaceCurveKind,
        };
        use std::sync::Arc;
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let plane = Arc::new(Surface::Plane {
            position: Placement {
                origin: [0.0, 0.0, 1.0e12 + 4.0],
                axis: [0.0, 1.0, 0.0],
                ref_direction: [1.0, 0.0, 0.0],
            },
        });
        let functional = HeightFunctional {
            direction: [0.0, 0.0, 1.0].map(Interval::point),
            offset: Interval::point(-1.0e12),
        };
        for (chart_direction, spatial_direction, preferred_height, spatial_height) in [
            ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0], Some(4.0), None),
            ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], None, Some(123.0)),
        ] {
            let chart = Arc::new(Curve::Line {
                origin: [0.0; 3],
                direction: chart_direction,
                magnitude: 1.0,
            });
            let spatial = Arc::new(Curve::Line {
                origin: [0.0, 0.0, 1.0e12 + 123.0],
                direction: spatial_direction,
                magnitude: 1.0,
            });
            for (master, expected) in [
                (PreferredRep::PcurveS1, preferred_height),
                (PreferredRep::Curve3d, spatial_height),
            ] {
                let source = Curve::SurfaceCurve {
                    kind: SurfaceCurveKind::Surface,
                    curve_3d: spatial.clone(),
                    associated: vec![Associated::Pcurve(Pcurve {
                        surface: plane.clone(),
                        curve: chart.clone(),
                    })],
                    master,
                };
                let prepared = crate::kernel::source_operand::PreparedOriginalCurve::new(
                    &source,
                    [0.0, 1.0],
                    MAX_SLAB_SCRATCH_BYTES,
                    &mut work,
                    &cancel,
                )
                .unwrap();
                let actual =
                    constant_native_curve_height(&prepared, functional, &mut work, &cancel)
                        .unwrap()
                        .map(|height| [height.lo, height.hi]);
                assert_eq!(actual, expected.map(|height| [height, height]));
                if expected.is_none() {
                    assert!(
                        constant_original_curve_height(&source, functional, 0, &mut work, &cancel,)
                            .unwrap()
                            .is_none()
                    );
                }
            }
        }
    }

    #[test]
    fn closed_original_endpoint_uses_exact_projection_not_rounded_raw_parameter() {
        let curve = solidcraft_step::geom::Curve::Line {
            origin: [0.0, 0.0, 1.0e12],
            direction: [0.0, 0.0, 1.0],
            magnitude: 0.1,
        };
        let vertices = [[0.0, 0.0, 1.0e12], [0.0, 0.0, 1.0e12 + 1.0]];
        let functional = HeightFunctional {
            direction: [0.0, 0.0, 1.0].map(Interval::point),
            offset: Interval::point(-1.0e12),
        };
        let mut work = GeometryWorkBudget::new(100_000);
        let cancel = AtomicBool::new(false);
        let raw = crate::kernel::source_operand::PreparedOriginalCurve::new(
            &curve,
            [0.0, 10.0],
            MAX_SLAB_SCRATCH_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(
            !exact_affine_curve_height_at(&raw, 10.0, functional, 1.0, &mut work, &cancel,)
                .unwrap(),
            "binary64 0.1 times 10 is not the exact scalar one"
        );
        let selected = super::super::source_restriction::SourceLineRestriction::new(
            &curve, &vertices, true, 1.0e-6, &mut work, &cancel,
        )
        .unwrap()
        .unwrap();
        let height =
            original_endpoint_height(&selected, 1.0, vertices[1], functional, &mut work, &cancel)
                .unwrap();
        assert_eq!([height.lo, height.hi], [1.0, 1.0]);
    }
    #[test]
    fn original_rational_endpoint_identity_does_not_snap_an_interior_parameter() {
        use solidcraft_step::geom::{BSplineCurve, Curve, CurveForm, KnotSpec, Logical};
        let curve = Curve::BSpline(Box::new(BSplineCurve {
            degree: 1,
            points: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 1.0]],
            weights: Some(vec![2.0, 3.0]),
            knots: vec![0.0, 1.0],
            multiplicities: vec![2, 2],
            form: CurveForm::PolylineForm,
            closed: Logical::False,
            self_intersect: Logical::False,
            knot_spec: KnotSpec::Unspecified,
        }));
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let prepared = crate::kernel::source_operand::PreparedOriginalCurve::new(
            &curve,
            [0.0, 1.0],
            MAX_SLAB_SCRATCH_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let functional = HeightFunctional::from_frame(frame());
        let end = original_endpoint_height(
            &prepared,
            1.0,
            [1.0, 0.0, 1.0],
            functional,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert_eq!([end.lo, end.hi], [1.0, 1.0]);
        let interior = original_endpoint_height(
            &prepared,
            0.5,
            [1.0, 0.0, 1.0],
            functional,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(interior.lo <= 0.6 && interior.hi >= 1.0 && interior.lo < interior.hi);
    }
    #[test]
    fn native_affine_trim_coefficients_retain_exact_constant_height_identity() {
        let curve = native_test_curve(
            &[[3.2, -4.0, 5.0, 1.0], [3.8, -4.0, 5.0, 1.0]],
            1,
            &[0.2, 0.2, 0.8, 0.8],
        );
        let mut work = GeometryWorkBudget::new(100);
        let result = constant_native_curve_height(
            &curve,
            HeightFunctional::from_frame(frame()),
            &mut work,
            &AtomicBool::new(false),
        )
        .unwrap()
        .unwrap();
        assert_eq!([result.lo, result.hi], [5.0, 5.0]);
    }
    #[test]
    fn closed_support_excluded_empty_slab_has_exact_reuse_but_contact_does_not() {
        let cancel = AtomicBool::new(false);
        let definition = parity_definition("native-parity-full-sphere.step");
        let mut work = GeometryWorkBudget::new(50_000_000);
        let prepared = super::super::material_source::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        for range in [[-7.0, -6.0], [6.0, 7.0]] {
            let result = analyze_slab(
                &prepared,
                SlabRequest {
                    frame: frame(),
                    height_interval_mm: range,
                    sample_height_mm: range[0] * 0.5 + range[1] * 0.5,
                    native_shape_error_mm: 0.1,
                },
                crate::MAX_PREPARED_SECTION_BYTES - prepared.retained_bytes(),
                &mut work,
                &cancel,
            )
            .unwrap();
            assert!(result.section.loops.is_empty());
            assert!(result.invariance.is_some());
            assert_eq!(result.boundary_motion_bound_mm, 0.0);
        }
        let contact = analyze_slab(
            &prepared,
            SlabRequest {
                frame: frame(),
                height_interval_mm: [5.0, 5.1],
                sample_height_mm: 5.05,
                native_shape_error_mm: 0.1,
            },
            crate::MAX_PREPARED_SECTION_BYTES - prepared.retained_bytes(),
            &mut work,
            &cancel,
        );
        assert!(
            contact.is_err(),
            "a nonempty CLOSED endpoint contact is not empty-slab reuse"
        );
    }
    #[test]
    fn slab_crossing_native_critical_height_never_returns_a_sample_only_certificate() {
        let cancel = AtomicBool::new(false);
        let definition = parity_definition("native-parity-ring-torus.step");
        let mut work = GeometryWorkBudget::new(20_000_000);
        let prepared = super::super::material_source::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let result = analyze_slab(
            &prepared,
            SlabRequest {
                frame: frame(),
                height_interval_mm: [1.9, 2.1],
                sample_height_mm: 1.99,
                native_shape_error_mm: 0.5,
            },
            crate::MAX_PREPARED_SECTION_BYTES - prepared.retained_bytes(),
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(result.boundary_motion_bound_mm <= 0.5);
        assert!(result.critical.is_some());
        assert!(result.event_neighborhoods().iter().any(|event| {
            let bracket = event.height_bracket_mm();
            bracket[0] <= 2.0 && bracket[1] >= 2.0
        }));
        assert_eq!(result.request().sample_height_mm, 1.99);
    }
    #[test]
    fn sphere_terminal_cap_uses_the_actual_endpoint_biased_polygon_prism() {
        let cancel = AtomicBool::new(false);
        let definition = parity_definition("native-parity-full-sphere.step");
        let mut work = GeometryWorkBudget::new(30_000_000);
        let prepared = super::super::material_source::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let request = SlabRequest {
            frame: frame(),
            height_interval_mm: [4.999, 5.001],
            sample_height_mm: 4.999001,
            native_shape_error_mm: 0.02,
        };
        let result = analyze_slab(
            &prepared,
            request,
            crate::MAX_PREPARED_SECTION_BYTES - prepared.retained_bytes(),
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(result.boundary_motion_bound_mm <= 0.02);
        assert!(result.critical.is_some());
        assert!(
            result
                .critical
                .as_ref()
                .unwrap()
                .has_native_convex_section_topology_proof(),
            "the complete original affine sphere is a proved convex source"
        );
        assert_eq!(result.request().sample_height_mm, request.sample_height_mm);
        assert_eq!(result.section.loops.len(), 1);
        assert!(
            result
                .occupied
                .as_ref()
                .is_some_and(|occupied| !occupied.is_native_intersection())
        );
    }
    #[test]
    fn height_range_and_shape_policy_are_not_silently_repaired() {
        assert!(validate(frame(), [1.0, 1.0], 0.01).is_err());
        assert!(validate(frame(), [0.0, 1.0], f64::NAN).is_err());
    }
    #[test]
    fn distinct_uncertain_critical_brackets_are_not_proximity_merged() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(10);
        let mut events = Vec::new();
        add_event(
            &mut events,
            Interval { lo: 0.1, hi: 0.11 },
            [0.0, 1.0],
            0.02,
            &mut work,
            &cancel,
        )
        .unwrap();
        add_event(
            &mut events,
            Interval {
                lo: 0.105,
                hi: 0.115,
            },
            [0.0, 1.0],
            0.02,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert_eq!(events.len(), 2);
    }
    #[test]
    fn occupied_envelope_expands_material_and_does_not_preserve_an_unproved_hole() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100);
        let section = NativeSection {
            plane: PlaneMm {
                origin_mm: [0.0; 3],
                normal: [0.0, 0.0, 1.0],
            },
            frame: frame(),
            loops: vec![
                SectionLoopMm {
                    is_hole: false,
                    points_mm: vec![
                        [0.0, 0.0, 0.0],
                        [2.0, 0.0, 0.0],
                        [2.0, 2.0, 0.0],
                        [0.0, 2.0, 0.0],
                        [0.0, 0.0, 0.0],
                    ],
                },
                SectionLoopMm {
                    is_hole: true,
                    points_mm: vec![
                        [0.5, 0.5, 0.0],
                        [0.5, 1.5, 0.0],
                        [1.5, 1.5, 0.0],
                        [1.5, 0.5, 0.0],
                        [0.5, 0.5, 0.0],
                    ],
                },
            ],
            sampling_tolerance_mm: 0.001,
            boolean_tolerance_mm: 0.001,
            loop_source_faces: Vec::new(),
            loop_spans: Vec::new(),
            certificate: crate::NativeSectionCertificate {
                definition_id: spiling_contracts::geometry::DefinitionId::from_source_sha256(
                    &[0; 32],
                ),
                authority: crate::SectionAuthority::NativeIntersection,
                maximum_chord_error_mm: 0.0,
                maximum_plane_error_mm: 0.0,
            },
        };
        let occupied =
            occupied_envelope(&section, 0.25, MAX_SLAB_SCRATCH_BYTES, &mut work, &cancel).unwrap();
        assert_eq!(occupied.loops.len(), 1);
        assert!(!occupied.loops[0].is_hole);
        assert!(occupied.loops[0].points_mm[0][0] < -0.25);
        assert!(occupied.loops[0].points_mm[2][0] > 2.25);
    }
    #[test]
    fn interval_distance_encloses_endpoint_to_segment_minimum() {
        let distance =
            point_segment_distance_squared([0.5, 2.0, 0.0], [0.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
        assert!(distance.lo <= 4.0 && distance.hi >= 4.0);
    }
    #[test]
    fn occupied_growth_is_cancellable_and_charged() {
        let cancel = AtomicBool::new(true);
        let mut work = GeometryWorkBudget::new(100);
        let mut events = Vec::new();
        assert_eq!(
            add_event(
                &mut events,
                Interval::point(0.5),
                [0.0, 1.0],
                0.01,
                &mut work,
                &cancel
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::Cancelled
        );
    }
}
