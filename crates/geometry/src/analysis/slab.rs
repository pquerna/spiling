// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Stratified height exclusion and bounded native-section continuation. Search
//! cells are not geometry: every accepted bound comes from complete native
//! parameter intervals, and every returned sample is a native section.
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
    Interval, PreparedCurveEnclosure, down, surface_crease_interfaces, surface_enclosure,
    surface_parameter_boxes, up,
};
use crate::{Definition, GeometryError, NativeSection, PlaneFrameMm, PlaneMm, SectionLoopMm};
use monstertruck_modeling::{BoundedCurve, Curve, Point3, Surface};
use monstertruck_topology::{EdgeId, VertexId};
use spiling_contracts::geometry::GeometryErrorCode;
#[cfg(test)]
use spiling_contracts::geometry::MAX_SECTION_BYTES;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;

const MAX_DEPTH: u8 = 64;
const MAX_EVENT_BRACKETS: usize = 65_536;
const MAX_SLAB_SCRATCH_BYTES: usize = 32 * 1024 * 1024;
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
    let mut uv = domain.uv_bounds();
    if let Some(certified) = domain.certified_uv_domain() {
        for axis in 0..2 {
            uv[axis][0] = uv[axis][0].max(certified[axis][0]);
            uv[axis][1] = uv[axis][1].min(certified[axis][1]);
        }
    }
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
struct HeightSourceLinks {
    incidence: SourceEdgeIncidence,
    edges: HashMap<EdgeId<Curve>, SeamHeightLink>,
    vertices: HashMap<VertexId<Point3>, bool>,
}

struct FaceHeightQuery<'q, 'a, 's> {
    face: usize,
    glued: &'q GluedSourceFace<'a, 's>,
    functional: HeightFunctional,
}
struct EdgeHeightQuery<'q, 'a, 's> {
    source: &'q FaceHeightQuery<'q, 'a, 's>,
    edge: EdgeId<Curve>,
}
fn receipt_map_bytes<K, V>(map: &HashMap<K, V>) -> usize {
    map.capacity()
        .saturating_mul(std::mem::size_of::<(K, V)>() + 16)
        .saturating_mul(2)
}
impl HeightSourceLinks {
    fn new(
        definition: &Definition,
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
                std::mem::size_of::<Self>() - std::mem::size_of::<SourceEdgeIncidence>(),
            )
            .saturating_add(receipt_map_bytes(&self.vertices))
    }
    fn owned_bytes(&self) -> usize {
        self.fixed_bytes()
            .saturating_add(receipt_map_bytes(&self.edges))
    }
    fn edge_link(
        incidence: &SourceEdgeIncidence,
        cache: &mut HashMap<EdgeId<Curve>, SeamHeightLink>,
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
                        let range = domain.native().coedges()[occurrence].curve.range_tuple();
                        let parameter = if endpoint == 0 { range.0 } else { range.1 };
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
                .saturating_mul(std::mem::size_of::<(EdgeId<Curve>, SeamHeightLink)>() + 16)
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
        edge: EdgeId<Curve>,
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
        vertex: VertexId<Point3>,
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
                .native()
                .joints()
                .iter()
                .filter(|joint| joint.vertex_id == vertex)
            {
                work.charge(1, cancel)?;
                if !source_height_transverse(
                    glued,
                    joint.enclosure.map(|axis| [axis.lower, axis.upper]),
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
                .saturating_mul(std::mem::size_of::<(VertexId<Point3>, bool)>() + 16)
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
fn height_source_links<'a>(
    slot: &'a mut Option<HeightSourceLinks>,
    definition: &Definition,
    max_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<&'a mut HeightSourceLinks, GeometryError> {
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

fn edge_scratch(face: &monstertruck_modeling::Face) -> Result<usize, GeometryError> {
    face.absolute_boundaries()
        .iter()
        .try_fold(STACK_SCRATCH_BYTES, |sum, wire| {
            wire.len()
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
fn constant_spline_height(
    surface: &Surface,
    functional: HeightFunctional,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Interval>, GeometryError> {
    constant_surface_height_inner(surface, functional, 0, work, cancel)
}
fn constant_surface_height_inner(
    surface: &Surface,
    functional: HeightFunctional,
    depth: u8,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Interval>, GeometryError> {
    work.charge(1, cancel)?;
    if depth >= MAX_DEPTH {
        return Err(indeterminate(
            "native carrier height identity nesting exhausted",
        ));
    }
    match surface {
        Surface::Plane(plane) => {
            let u: [f64; 3] = plane.axis_u().into();
            let v: [f64; 3] = plane.axis_v().into();
            if !equal_coefficient_height(
                [0.0, 0.0, 0.0, 1.0],
                [u[0], u[1], u[2], 1.0],
                functional,
                work,
                cancel,
            )? || !equal_coefficient_height(
                [0.0, 0.0, 0.0, 1.0],
                [v[0], v[1], v[2], 1.0],
                functional,
                work,
                cancel,
            )? {
                return Ok(None);
            }
            Ok(Some(functional.vertex(plane.origin().into())))
        }
        Surface::BsplineSurface(spline) => {
            let Some(first) = spline.control_points().iter().flatten().next() else {
                return Ok(None);
            };
            let a: [f64; 3] = (*first).into();
            for point in spline.control_points().iter().flatten() {
                work.charge(1, cancel)?;
                let b: [f64; 3] = (*point).into();
                if !equal_coefficient_height(
                    [a[0], a[1], a[2], 1.0],
                    [b[0], b[1], b[2], 1.0],
                    functional,
                    work,
                    cancel,
                )? {
                    return Ok(None);
                }
            }
            Ok(Some(functional.vertex(a)))
        }
        Surface::NurbsSurface(spline) => {
            let mut controls = spline.control_points().iter().flatten();
            let Some(first) = controls.next() else {
                return Err(error(
                    GeometryErrorCode::InvalidGeometry,
                    "rational surface has no control coefficients",
                ));
            };
            if first.w == 0.0 {
                return Ok(None);
            }
            let candidate = tight_add(
                tight_div(
                    functional.dot([first.x, first.y, first.z].map(Interval::point)),
                    Interval::point(first.w),
                )?,
                functional.offset,
            );
            let positive = first.w > 0.0;
            for point in spline.control_points().iter().flatten() {
                work.charge(1, cancel)?;
                // Positive (or all-negative) homogeneous weights prove that
                // the partition-of-unity denominator never vanishes.
                if point.w == 0.0 || (point.w > 0.0) != positive {
                    return Ok(None);
                }
                if !equal_coefficient_height(
                    [first.x, first.y, first.z, first.w],
                    [point.x, point.y, point.z, point.w],
                    functional,
                    work,
                    cancel,
                )? {
                    return Ok(None);
                }
            }
            Ok(Some(candidate))
        }
        Surface::ExtrusionSurface(extrusion) => {
            let vector = extrusion.extruding_vector();
            if !equal_coefficient_height(
                [0.0, 0.0, 0.0, 1.0],
                [vector.x, vector.y, vector.z, 1.0],
                functional,
                work,
                cancel,
            )? {
                return Ok(None);
            }
            constant_native_curve_height_inner(
                extrusion.entity_curve(),
                functional,
                depth + 1,
                work,
                cancel,
            )
        }
        Surface::RevolutionSurface(revolution) => {
            let matrix = revolution.transform();
            if matrix.x.w != 0.0 || matrix.y.w != 0.0 || matrix.z.w != 0.0 || matrix.w.w != 1.0 {
                return Ok(None);
            }
            let local = HeightFunctional {
                direction: [matrix.x, matrix.y, matrix.z].map(|column| {
                    functional.dot([column.x, column.y, column.z].map(Interval::point))
                }),
                offset: tight_add(
                    functional.offset,
                    functional.dot([matrix.w.x, matrix.w.y, matrix.w.z].map(Interval::point)),
                ),
            };
            let axis = revolution.entity().axis();
            let axis = [axis.x, axis.y, axis.z];
            // Native axis-angle evaluation assumes a unit axis. A rounded
            // near-unit vector is not an exact rotational-height identity.
            if !super::critical::exact_unit_vector_identity(axis, work, cancel)? {
                return Ok(None);
            }
            for index in 0..3 {
                let i = (index + 1) % 3;
                let j = (index + 2) % 3;
                let mut perpendicular = [0.0, 0.0, 0.0, 1.0];
                perpendicular[i] = axis[j];
                perpendicular[j] = -axis[i];
                if !equal_coefficient_height(
                    [0.0, 0.0, 0.0, 1.0],
                    perpendicular,
                    local,
                    work,
                    cancel,
                )? {
                    return Ok(None);
                }
            }
            constant_native_curve_height_inner(
                revolution.entity().entity_curve(),
                local,
                depth + 1,
                work,
                cancel,
            )
        }
        _ => Ok(None),
    }
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
    let face = definition
        .solid
        .face_iter()
        .nth(face_index)
        .ok_or_else(|| indeterminate("source face constancy mapping is incomplete"))?;
    face.with_surface(|surface| constant_spline_height(surface, functional, work, cancel))
}
pub(super) fn constant_native_curve_height(
    curve: &monstertruck_modeling::Curve,
    functional: HeightFunctional,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Interval>, GeometryError> {
    constant_native_curve_height_inner(curve, functional, 0, work, cancel)
}
fn constant_native_curve_height_inner(
    curve: &monstertruck_modeling::Curve,
    functional: HeightFunctional,
    depth: u8,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<Interval>, GeometryError> {
    work.charge(1, cancel)?;
    if depth >= MAX_DEPTH {
        return Err(indeterminate(
            "native curve height identity nesting exhausted",
        ));
    }
    use monstertruck_modeling::Curve;
    match curve {
        Curve::Line(line) => {
            let a: [f64; 3] = line.0.into();
            let b: [f64; 3] = line.1.into();
            if !equal_coefficient_height(
                [a[0], a[1], a[2], 1.0],
                [b[0], b[1], b[2], 1.0],
                functional,
                work,
                cancel,
            )? {
                return Ok(None);
            }
            Ok(Some(functional.vertex(a)))
        }
        Curve::BsplineCurve(spline) => {
            let Some(first) = spline.control_points().first() else {
                return Ok(None);
            };
            let a: [f64; 3] = (*first).into();
            for point in spline.control_points() {
                work.charge(1, cancel)?;
                let b: [f64; 3] = (*point).into();
                if !equal_coefficient_height(
                    [a[0], a[1], a[2], 1.0],
                    [b[0], b[1], b[2], 1.0],
                    functional,
                    work,
                    cancel,
                )? {
                    return Ok(None);
                }
            }
            Ok(Some(functional.vertex(a)))
        }
        Curve::NurbsCurve(spline) => {
            let Some(first) = spline.control_points().first() else {
                return Err(error(
                    GeometryErrorCode::InvalidGeometry,
                    "rational curve has no control coefficients",
                ));
            };
            if first.w == 0.0 {
                return Ok(None);
            }
            let candidate = tight_add(
                tight_div(
                    functional.dot([first.x, first.y, first.z].map(Interval::point)),
                    Interval::point(first.w),
                )?,
                functional.offset,
            );
            for point in spline.control_points() {
                work.charge(1, cancel)?;
                if point.w == 0.0 || (point.w > 0.0) != (first.w > 0.0) {
                    return Ok(None);
                }
                if !equal_coefficient_height(
                    [first.x, first.y, first.z, first.w],
                    [point.x, point.y, point.z, point.w],
                    functional,
                    work,
                    cancel,
                )? {
                    return Ok(None);
                }
            }
            Ok(Some(candidate))
        }
        Curve::ParameterCurve(curve) => constant_surface_height_inner(
            curve.surface().as_ref(),
            functional,
            depth + 1,
            work,
            cancel,
        ),
        Curve::TrimmedCurve(curve) => {
            let matrix = curve.transform();
            if matrix.x.w != 0.0 || matrix.y.w != 0.0 || matrix.z.w != 0.0 || matrix.w.w != 1.0 {
                return Ok(None);
            }
            let direction = [matrix.x, matrix.y, matrix.z]
                .map(|column| functional.dot([column.x, column.y, column.z].map(Interval::point)));
            let offset = tight_add(
                functional.offset,
                functional.dot([matrix.w.x, matrix.w.y, matrix.w.z].map(Interval::point)),
            );
            constant_native_curve_height_inner(
                curve.entity().curve().as_ref(),
                HeightFunctional { direction, offset },
                depth + 1,
                work,
                cancel,
            )
        }
        _ => Ok(None),
    }
}
fn flat_native_surface_height(
    surface: &Surface,
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
fn flat_native_plane_height(surface: &Surface, frame: PlaneFrameMm) -> Option<Interval> {
    let Surface::Plane(plane) = surface else {
        return None;
    };
    let axis = (0..3).find(|&i| frame.z_axis[i].abs() == 1.0)?;
    if (0..3).any(|i| i != axis && frame.z_axis[i] != 0.0) {
        return None;
    }
    let u: [f64; 3] = plane.axis_u().into();
    let v: [f64; 3] = plane.axis_v().into();
    (u[axis] == 0.0 && v[axis] == 0.0).then(|| vertex_height(plane.origin().into(), frame))
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
    surface: &Surface,
    frame: PlaneFrameMm,
) -> Option<([Interval; 2], usize)> {
    let Surface::ToroidalSurface(torus) = surface else {
        return None;
    };
    if torus.entity().large_radius() <= torus.entity().small_radius() {
        return None;
    }
    let m = torus.transform();
    if m.x.w != 0.0 || m.y.w != 0.0 || m.z.w != 0.0 || m.w.w != 1.0 {
        return None;
    }
    let a = tight_dot([m.x.x, m.x.y, m.x.z], frame.z_axis);
    let b = tight_dot([m.y.x, m.y.y, m.y.z], frame.z_axis);
    if a.lo != 0.0 || a.hi != 0.0 || b.lo != 0.0 || b.hi != 0.0 {
        return None;
    }
    let c = tight_dot([m.z.x, m.z.y, m.z.z], frame.z_axis);
    if c.contains_zero() {
        return None;
    }
    let center: [f64; 3] = torus.entity().center().into();
    let offset = (0..3).fold(vertex_height([m.w.x, m.w.y, m.w.z], frame), |s, i| {
        let column = match i {
            0 => [m.x.x, m.x.y, m.x.z],
            1 => [m.y.x, m.y.y, m.y.z],
            _ => [m.z.x, m.z.y, m.z.z],
        };
        tight_add(
            s,
            tight_mul(tight_dot(column, frame.z_axis), Interval::point(center[i])),
        )
    });
    let amplitude = tight_mul(c, Interval::point(torus.entity().small_radius()));
    Some((
        [
            tight_add(offset, amplitude),
            tight_add(offset, amplitude.neg()),
        ],
        if torus.orientation() { 1 } else { 0 },
    ))
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

fn material_transport_height_padding(
    definition: &Definition,
    functional: HeightFunctional,
) -> Result<f64, GeometryError> {
    let allowance = definition.provenance().uncertainty_mm.ok_or_else(|| {
        error(
            GeometryErrorCode::ParameterCorrespondenceUnavailable,
            "nonzero or unresolved material transport has no declared source allowance",
        )
    })?;
    if !allowance.is_finite() || allowance < 0.0 {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "invalid original material source allowance",
        ));
    }
    Ok(Interval::point(allowance)
        .mul(norm(functional.direction)?)
        .hi)
}

/// Retain actual source-owned collar, edge-end and joint-star supports.
/// The provider evaluates the full unnormalized scalar covector; only the
/// affine height offset is applied here. No whole-face blanket or fabricated
/// root subdivisions replace these physical supports.
fn material_transport_height_cover(
    source: crate::section::OriginalTrimSource<'_, '_>,
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
    source: crate::section::OriginalTrimSource<'_, 'd>,
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

/// Complete conservative isolation of literal native critical-height strata.
/// Source-material completeness additionally retains the entire possible
/// transport support on HeightCapture. Such supports never become isolated
/// root brackets or authorize a raw regular slab through their neighborhoods.
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
    let remaining_bytes = remaining_scratch_with_limit(
        total_bytes.min(MAX_SLAB_SCRATCH_BYTES),
        definition.retained_query_proof_bytes(),
    )?;
    analyze_height_domain_with_scratch_limit(
        crate::section::OriginalTrimSource::standalone(definition),
        frame,
        height_interval_mm,
        policy,
        remaining_bytes,
        work,
        cancel,
    )
}
pub(crate) struct HeightDomainQuery<'s, 'd> {
    pub(crate) source: crate::section::OriginalTrimSource<'s, 'd>,
    pub(crate) frame: PlaneFrameMm,
    pub(crate) functional: HeightFunctional,
    pub(crate) height_interval_mm: [f64; 2],
    pub(crate) policy: AnalysisPolicy,
}

/// `remaining_bytes` excludes the already reserved pinned definition proof and
/// other coexisting buffers. It covers event arrays, trim/curve captures,
/// source incidence tables and subdivision stacks; never debit pinned twice.
pub(crate) fn analyze_height_domain_with_scratch_limit(
    source: crate::section::OriginalTrimSource<'_, '_>,
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
        HeightCensusScope::OriginalMaterial,
    )
}

/// Literal native-section regularity needs geometric candidate heights, not a
/// source-material realization. Its private capture explicitly lacks material
/// support completeness and cannot authorize material reuse.
pub(super) fn analyze_literal_native_height_domain(
    source: crate::section::OriginalTrimSource<'_, '_>,
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
        HeightCensusScope::LiteralNative,
    )
}
pub(crate) fn analyze_height_domain_enclosed(
    definition: &Definition,
    frame: PlaneFrameMm,
    functional: HeightFunctional,
    height_interval_mm: [f64; 2],
    policy: AnalysisPolicy,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<HeightAnalysis, GeometryError> {
    work.charge(1, cancel)?;
    let remaining_bytes = remaining_scratch_with_limit(
        MAX_SLAB_SCRATCH_BYTES,
        definition.retained_query_proof_bytes(),
    )?;
    analyze_height_domain_enclosed_with_scratch_limit(
        &HeightDomainQuery {
            source: crate::section::OriginalTrimSource::standalone(definition),
            frame,
            functional,
            height_interval_mm,
            policy,
        },
        remaining_bytes,
        work,
        cancel,
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
    analyze_height_functional(
        query,
        remaining_bytes,
        work,
        cancel,
        exact_nominal,
        HeightCensusScope::OriginalMaterial,
    )
}
#[derive(Clone, Copy)]
enum HeightCensusScope {
    LiteralNative,
    OriginalMaterial,
}
fn analyze_height_functional(
    query: &HeightDomainQuery<'_, '_>,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    nominal_fast_paths: bool,
    scope: HeightCensusScope,
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
        .checked_sub(definition.retained_query_proof_bytes())
        .and_then(|bytes| bytes.checked_sub(definition.id().allocated_bytes()));
    let material_scope = matches!(scope, HeightCensusScope::OriginalMaterial);
    let material_transport_zero =
        material_scope && definition.material_transport_is_proved_zero(work, cancel)?;
    // Keep source-only preparation alive across support, all corner attempts
    // and the native face census. Existing prepared sources lend their owner;
    // query-dependent roots and frame proofs are still evaluated below.
    let originals = if material_scope && !material_transport_zero {
        source.prepare_domains_if_missing(
            remaining_scratch_with_limit(
                scratch_limit,
                HEIGHT_EVENT_SCRATCH_BYTES + STACK_SCRATCH_BYTES,
            )?,
            work,
            cancel,
        )?
    } else {
        None
    };
    let source = source.borrowing_domains(originals.as_ref());
    let scratch_limit = remaining_scratch_with_limit(
        scratch_limit,
        originals.as_ref().map_or(0, |owner| owner.retained_bytes()),
    )?;
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
    let transport_support = if !material_scope || material_transport_zero {
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
    let raw_before = work.consumed();
    if let Some(round) = if nominal_fast_paths {
        raw_round_continuation(
            source,
            frame,
            remaining_scratch_with_limit(scratch_limit, HEIGHT_EVENT_SCRATCH_BYTES)?,
            work,
            cancel,
        )
        .map_err(|error| {
            work.annotate_error(
                error,
                raw_before,
                "raw height census round continuation after material transport",
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
                transport_support_complete: material_scope,
            },
        });
    }
    let mut source_links: Option<HeightSourceLinks> = None;
    for (index, face) in definition.solid.face_iter().enumerate() {
        let face_before = work.consumed();
        work.charge(1, cancel).map_err(|error| {
            work.annotate_error(
                error,
                face_before,
                format_args!("raw height census source_face={index} begin face"),
            )
        })?;
        let face_reserved = HEIGHT_EVENT_SCRATCH_BYTES.saturating_add(edge_scratch(face)?);
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
                        "raw height census source_face={index} prepare original trim domain"
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
                        let bounds = domain.uv_bounds()[phase_axis];
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
        let wires = face.absolute_boundaries();
        let mut constant_heights = HashMap::new();
        let references = definition.face_boundaries.get(index).ok_or_else(|| indeterminate("native trim correspondence is incomplete"))?;
        for (wire_index, coedge_index, reference) in references.wires.iter().enumerate().flat_map(|(wire_index, wire)| {
            wire.iter().enumerate().map(move |(coedge_index, reference)| (wire_index, coedge_index, reference))
        }) {
            stage = "source boundary witness";
            curve_owner = Some((wire_index, coedge_index));
            curve_window = None;
            work.charge(1, cancel)?;
            let witness = definition.boundary_curves.get(reference.witness).ok_or_else(|| indeterminate("native boundary witness is unavailable"))?;
            if nominal_fast_paths
                && let Some(h) = constant_boundary_height(witness, frame) { constant_heights.insert(reference.edge, h); }
            prove_circle_stationary_heights(witness, &HeightEventQuery { functional, heights: height_interval_mm, uncertainty: policy.maximum_height_uncertainty_mm }, &mut events, &mut proved_events, work, cancel)?;
        }
        for (wire_index, wire) in wires.iter().enumerate() {
            for (coedge_index, edge) in wire.iter().enumerate() {
                stage = "source coedge census";
                curve_owner = Some((wire_index, coedge_index));
                curve_window = Some(edge.with_curve(|curve| curve.range_tuple()));
                vertex_point = None;
                work.charge(1, cancel)?;
                let constant = match constant_heights.get(&edge.id()).copied() {
                    Some(h) => Some(h),
                    None => edge.with_curve(|curve| constant_native_curve_height(curve, functional, work, cancel))?,
                };
                if let Some(h) = constant {
                    let mut proved_physical_boundary = true;
                    if flat_height.is_none() && overlaps(h, height_interval_mm) {
                        let allowance = remaining_scratch_with_limit(scratch_limit,
                            face_reserved.saturating_add(domain.owned_bytes()))?;
                        let links = height_source_links(&mut source_links, definition, allowance, work, cancel)?;
                        if links.incidence.get(edge.id()).is_some_and(|uses| {
                            uses.second.is_some_and(|second| uses.first.face == second.face)
                        }) {
                            // An unresolved chart-seam differential is only a
                            // candidate, never an asserted physical critical.
                            proved_physical_boundary = false;
                        }
                        if links.whole_edge_regular(&FaceHeightQuery { face: index, glued: glued.as_ref().unwrap(), functional }, edge.id(), allowance, work, cancel)?
                        {
                            continue;
                        }
                    }
                    add_event(&mut events, h, height_interval_mm, policy.maximum_height_uncertainty_mm, work, cancel)?;
                    if proved_physical_boundary {
                        add_event(&mut proved_events, h, height_interval_mm, policy.maximum_height_uncertainty_mm, work, cancel)?;
                    }
                    continue;
                }
                for vertex in [edge.front(), edge.back()] {
                    stage = "source vertex census";
                    vertex_point = Some(vertex.point().into());
                    let height = functional.vertex(vertex.point().into());
                    if flat_height.is_none() && overlaps(height, height_interval_mm) {
                        let allowance = remaining_scratch_with_limit(scratch_limit,
                            face_reserved.saturating_add(domain.owned_bytes()))?;
                        if height_source_links(&mut source_links, definition, allowance, work, cancel)?
                            .vertex_regular(definition, &FaceHeightQuery { face: index, glued: glued.as_ref().unwrap(), functional }, vertex.id(), allowance, work, cancel)?
                        {
                            continue;
                        }
                    }
                    add_event(&mut events, height, height_interval_mm, policy.maximum_height_uncertainty_mm, work, cancel)?;
                }
                if flat_height.is_none() {
                    stage = "source coedge height isolation";
                    vertex_point = None;
                    edge.with_curve(|curve| -> Result<(), GeometryError> {
                        let occupied = face_reserved.saturating_add(domain.owned_bytes())
                            .saturating_add(source_links.as_ref().map_or(0, HeightSourceLinks::owned_bytes));
                        let prepared = definition.prepare_boundary_curve(edge.id(), curve, curve.range_tuple(), remaining_scratch_with_limit(scratch_limit, occupied)?, work, cancel)?;
                        isolate_curve_events(&OriginalCurveSpan { curve: &prepared, range: curve.range_tuple() }, &HeightEventQuery { functional, heights: height_interval_mm, uncertainty: policy.maximum_height_uncertainty_mm }, &mut events, &mut proved_events, work, cancel)
                    }).map_err(|error| height_face_context(error, index, "isolate original coedge heights"))?;
                }
            }
        }
        if (0..2).any(|i| torus_candidates[i] && !torus_proved[i]) {
            return Err(indeterminate("analytic critical-height value lacks a feasible native trim witness"));
        }
        Ok(())
        }).map_err(|error| work.annotate_error(error, face_before, format_args!(
            "raw height census stage={stage} source_face={index} wire_coedge={curve_owner:?} native_curve_window={curve_window:?} uv_window={surface_window:?} vertex_mm={vertex_point:?} transport_work_before={transport_before}"
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
            transport_support_complete: material_scope,
        },
    })
}

// These witnesses retain exact source analytic curves, not endpoint chords for
// general native edges. Only exact coefficient identities grant constancy.
fn constant_boundary_height(
    witness: &crate::kernel::BoundaryCurveMm,
    frame: PlaneFrameMm,
) -> Option<Interval> {
    use crate::kernel::BoundaryCurveMm;
    let functional = HeightFunctional::from_frame(frame);
    let zero = |value: Interval| value.lo == 0.0 && value.hi == 0.0;
    match witness {
        BoundaryCurveMm::Line { start_mm, end_mm } => {
            let start = functional.dot(start_mm.map(Interval::point));
            let end = functional.dot(end_mm.map(Interval::point));
            (start.lo == start.hi && end.lo == end.hi && start.lo == end.lo)
                .then(|| functional.vertex(*start_mm))
        }
        BoundaryCurveMm::CircleArc {
            center_mm,
            u_mm,
            v_mm,
            ..
        } => (zero(functional.dot(u_mm.map(Interval::point)))
            && zero(functional.dot(v_mm.map(Interval::point))))
        .then(|| functional.vertex(*center_mm)),
        BoundaryCurveMm::Native { .. } => None,
    }
}

struct HeightEventQuery {
    functional: HeightFunctional,
    heights: [f64; 2],
    uncertainty: f64,
}
struct OriginalCurveSpan<'q, 'c> {
    curve: &'q PreparedCurveEnclosure<'c>,
    range: (f64, f64),
}

fn prove_circle_stationary_heights(
    witness: &crate::kernel::BoundaryCurveMm,
    query: &HeightEventQuery,
    events: &mut Vec<[f64; 2]>,
    proved: &mut Vec<[f64; 2]>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    let HeightEventQuery {
        functional,
        heights,
        uncertainty,
    } = *query;
    let crate::kernel::BoundaryCurveMm::CircleArc {
        center_mm,
        u_mm,
        v_mm,
        start_angle,
        end_angle,
    } = witness
    else {
        return Ok(());
    };
    let cosine = functional.dot(u_mm.map(Interval::point));
    let sine = functional.dot(v_mm.map(Interval::point));
    let zero = |value: Interval| value.lo == 0.0 && value.hi == 0.0;
    let (coefficient, phases) = if zero(sine) && !zero(cosine) {
        (cosine, [0.0, 1.0])
    } else if zero(cosine) && !zero(sine) {
        (sine, [0.5, 1.5])
    } else {
        return Ok(());
    };
    let range = [start_angle.min(*end_angle), start_angle.max(*end_angle)];
    // This is only a bounded analytic proof shortcut. Longer native branches
    // still go through the prepared interval curve owner below.
    if !range[0].is_finite()
        || !range[1].is_finite()
        || range[0].abs().max(range[1].abs()) > 8.0 * std::f64::consts::TAU
    {
        return Ok(());
    }
    let pi = Interval {
        lo: down(std::f64::consts::PI),
        hi: up(std::f64::consts::PI),
    };
    let center = functional.height(center_mm.map(Interval::point));
    for (index, phase) in phases.into_iter().enumerate() {
        for turn in -9..=9 {
            work.charge(1, cancel)?;
            let root = tight_mul(pi, Interval::point(phase + 2.0 * f64::from(turn)));
            if root.lo < range[0] || root.hi > range[1] {
                continue;
            }
            let h = tight_add(
                center,
                tight_mul(
                    coefficient,
                    Interval::point(if index == 0 { 1.0 } else { -1.0 }),
                ),
            );
            add_event(events, h, heights, uncertainty, work, cancel)?;
            add_event(proved, h, heights, uncertainty, work, cancel)?;
        }
    }
    Ok(())
}

/// Necessary height cover of derivative roots in a separately qualified C1
/// original coedge span. Interval Newton contracts parameters, not the source
/// support; no existence or endpoint-incidence claim follows from this bound.
fn necessary_curve_critical_height(
    curve: &PreparedCurveEnclosure<'_>,
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
    let value = functional.dot(curve.jet((midpoint, midpoint), work, cancel)?.derivative);
    let newton = Interval::point(midpoint).sub(value.div(second)?);
    let root = (range.0.max(newton.lo), range.1.min(newton.hi));
    if root.0 > root.1 {
        return Ok(None);
    }
    let root_height = functional.height(curve.jet(root, work, cancel)?.position);
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
    let height = functional.height(curve.endpoint_position(knot, work, cancel)?);
    if !overlaps(height, heights) {
        return Ok(());
    }
    let (mut lower, mut upper) = neighbors;
    for depth in 0..=MAX_DEPTH {
        work.charge(1, cancel)?;
        let left = curve.jet((lower, knot), work, cancel)?;
        let right = curve.jet((knot, upper), work, cancel)?;
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
            let a = functional.height(curve.endpoint_position(lower, work, cancel)?);
            let b = functional.height(curve.endpoint_position(upper, work, cancel)?);
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
    if curve.is_smooth_on(original_range, work, cancel)? {
        for span in curve.parameter_intervals() {
            isolate_smooth_curve_span_events(
                &OriginalCurveSpan { curve, range: span },
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
        functional.height(curve.position_bounds(original_range, work, cancel)?),
        heights,
    ) {
        return Ok(());
    }
    // The complete explicit-knot visitor analytically qualifies every original
    // between-knot span. Do not re-map those exact closed intervals through
    // Native's wider recursive C1 query, which can re-cross a reflected endpoint.
    let mut lower = original_range.0;
    let mut pending = None;
    let complete =
        curve.visit_explicit_knots(original_range, work, cancel, &mut |next, work, cancel| {
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
        })?;
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
            let jet = curve.jet((lo, hi), work, cancel)?;
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
                let left = functional.dot(curve.jet(left_range, work, cancel)?.derivative);
                let right = functional.dot(curve.jet(right_range, work, cancel)?.derivative);
                if (left.hi < 0.0 && right.lo > 0.0) || (left.lo > 0.0 && right.hi < 0.0) {
                    let expanded_jet = curve.jet(expanded, work, cancel)?;
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
        error, before, format_args!("raw height census smooth original coedge native_parameter_bounds={span:?} curve_window={window:?}"),
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
) -> Result<
    HashMap<monstertruck_topology::EdgeId<monstertruck_modeling::Curve>, FlowEdgeUses>,
    GeometryError,
> {
    let mut edges: HashMap<_, FlowEdgeUses> = HashMap::new();
    for (face_index, face) in definition.solid.face_iter().enumerate() {
        for (wire_index, wire) in face.absolute_boundaries().iter().enumerate() {
            for (index, edge) in wire.iter().enumerate() {
                work.charge(1, cancel)?;
                let use_ = FlowCoedgeUse {
                    face: face_index,
                    wire: wire_index,
                    index,
                    orientation: edge.orientation() == face.orientation(),
                };
                if let Some(uses) = edges.get_mut(&edge.id()) {
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
                        .saturating_mul(std::mem::size_of::<(
                            monstertruck_topology::EdgeId<monstertruck_modeling::Curve>,
                            FlowEdgeUses,
                        )>());
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
                        edge.id(),
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
    source: crate::section::OriginalTrimSource<'_, '_>,
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
    for (face_index, face) in definition.solid.face_iter().enumerate() {
        work.charge(1, cancel)?;
        let Some(h) =
            face.with_surface(|surface| constant_spline_height(surface, functional, work, cancel))?
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
        let mut cover = Vec::new();
        let mut position: Option<[Interval; 3]> = None;
        let mut normal_sign = None;
        let oriented = face.with_surface(|surface| -> Result<bool, GeometryError> {
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
                let vertical = functional.dot(if face.orientation() {
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
        })?;
        if !oriented {
            continue;
        }
        let (Some(position), Some(normal_sign)) = (position, normal_sign) else {
            continue;
        };
        let mut conforming = true;
        for coedge in domain.native().coedges() {
            work.charge(1, cancel)?;
            let identity = &coedge.identity;
            let edge = &face.absolute_boundaries()[identity.wire_index][identity.coedge_index];
            let edge_height = edge.with_curve(|curve| {
                constant_native_curve_height(curve, functional, work, cancel)
            })?;
            if edge_height
                .is_none_or(|edge_height| edge_height.lo != h.lo || edge_height.hi != h.hi)
            {
                conforming = false;
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
            if definition
                .source_topology
                .faces
                .get(face_index)
                .and_then(|face| face.wires.get(identity.wire_index))
                .and_then(|wire| wire.coedges.get(identity.coedge_index))
                .is_none()
            {
                return Err(indeterminate(
                    "endpoint coedge original STEP lineage is unavailable",
                ));
            }
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
    let source = crate::section::OriginalTrimSource::standalone(definition);
    work.charge(1, cancel)?;
    narrow_invariance_with_scratch_limit(
        source,
        frame,
        range,
        remaining_scratch_with_limit(
            MAX_SLAB_SCRATCH_BYTES,
            definition.retained_query_proof_bytes(),
        )?,
        work,
        cancel,
    )
}
fn narrow_invariance_with_scratch_limit(
    source: crate::section::OriginalTrimSource<'_, '_>,
    frame: PlaneFrameMm,
    range: [f64; 2],
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<InvariantHeightDomain>, GeometryError> {
    if frame.z_axis != [0.0, 0.0, 1.0] && frame.z_axis != [0.0, 0.0, -1.0] {
        return Ok(None);
    }
    match native_vertical_boundary_flow(source, frame, range, remaining_bytes, work, cancel) {
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
/// `remaining_bytes` excludes pinned proof and all caller-retained buffers.
pub(super) fn native_vertical_boundary_flow(
    source: crate::section::OriginalTrimSource<'_, '_>,
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
    if !definition.material_transport_is_proved_zero(work, cancel)? {
        return Ok(None);
    }
    use monstertruck_modeling::Curve;
    use monstertruck_topology::EdgeId;
    let mut edges: HashMap<EdgeId<Curve>, FlowEdgeUses> = HashMap::new();
    let functional = HeightFunctional::from_frame(frame);
    let mut cap_levels = [None, None];
    let mut side_cells = 0u64;
    for (face_index, face) in definition.solid.face_iter().enumerate() {
        work.charge(1, cancel)?;
        let domain = source.face_domain(
            face_index,
            remaining_scratch_with_limit(scratch_limit, EDGE_BYTES + STACK_SCRATCH_BYTES)?,
            work,
            cancel,
        )?;
        for coedge in domain.native().coedges() {
            work.charge(1, cancel)?;
            let use_ = FlowCoedgeUse {
                face: face_index,
                wire: coedge.identity.wire_index,
                index: coedge.identity.coedge_index,
                orientation: coedge.identity.orientation,
            };
            if let Some(uses) = edges.get_mut(&coedge.identity.edge_id) {
                if uses.second.is_some() || uses.first.orientation == use_.orientation {
                    return Ok(None);
                }
                uses.second = Some(use_);
            } else {
                let bytes = edges
                    .len()
                    .checked_add(16)
                    .and_then(|count| {
                        count.checked_mul(std::mem::size_of::<(EdgeId<Curve>, FlowEdgeUses)>())
                    })
                    .and_then(|bytes| bytes.checked_mul(4));
                if bytes.is_none_or(|bytes| bytes > EDGE_BYTES) {
                    return Err(error(
                        GeometryErrorCode::ResourceLimit,
                        "native vertical-flow incidence capacity exhausted",
                    ));
                }
                edges.try_reserve(1).map_err(|_| {
                    error(
                        GeometryErrorCode::ResourceLimit,
                        "native vertical-flow incidence allocation refused",
                    )
                })?;
                edges.insert(
                    coedge.identity.edge_id,
                    FlowEdgeUses {
                        first: use_,
                        second: None,
                    },
                );
            }
        }
        let compatible = face.with_surface(|surface| -> Result<bool, GeometryError> {
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
        })?;
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
        work.charge(first.face as u64 + second.face as u64 + 2, cancel)?;
        let first_face = definition
            .solid
            .face_iter()
            .nth(first.face)
            .ok_or_else(|| indeterminate("native vertical-flow face mapping is incomplete"))?;
        let second_face = definition
            .solid
            .face_iter()
            .nth(second.face)
            .ok_or_else(|| indeterminate("native vertical-flow face mapping is incomplete"))?;
        // Artificial cuts cancel by original native edge identity and actual
        // coefficient identity, never by proximity or approximate normal fit.
        if first_face.orientation() == second_face.orientation() {
            let same = if first_face.id() == second_face.id() {
                first_face.with_surface(|surface| {
                    crate::parametric::same_surface_coefficients(surface, surface, work, cancel)
                })?
            } else if first.face < second.face {
                first_face.with_surface(|a| {
                    second_face.with_surface(|b| {
                        crate::parametric::same_surface_coefficients(a, b, work, cancel)
                    })
                })?
            } else {
                second_face.with_surface(|b| {
                    first_face.with_surface(|a| {
                        crate::parametric::same_surface_coefficients(a, b, work, cancel)
                    })
                })?
            };
            if same {
                continue;
            }
        }
        let edge = &first_face.absolute_boundaries()[first.wire][first.index];
        let compatible = edge.with_curve(|curve| -> Result<bool, GeometryError> {
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
            let prepared = definition.prepare_boundary_curve(
                edge.id(),
                curve,
                curve.range_tuple(),
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
        })?;
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
    let source = crate::section::OriginalTrimSource::standalone(definition);
    work.charge(1, cancel)?;
    if definition.solid.boundaries().len() != 1 {
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
    for (index, face) in definition.solid.face_iter().enumerate() {
        work.charge(1, cancel)?;
        let Some(height) =
            face.with_surface(|surface| flat_native_surface_height(surface, frame, work, cancel))?
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
        if caps[endpoint] || face.absolute_boundaries().len() != 1 {
            return Ok(None);
        }
        let domain = source.face_domain(
            index,
            remaining_scratch_with_limit(remaining_bytes, STACK_SCRATCH_BYTES)?,
            work,
            cancel,
        )?;
        let accepted = face.with_surface(|surface| -> Result<bool, GeometryError> {
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
                if !face.orientation() {
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
        })?;
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
fn round_carrier(surface: &Surface) -> Option<(RoundCarrier, bool)> {
    let (center, radius, major_radius, matrix, orientation) = match surface {
        Surface::SphericalSurface(s) => (
            s.entity().center(),
            s.entity().radius(),
            None,
            s.transform(),
            s.orientation(),
        ),
        Surface::ToroidalSurface(t) => (
            t.entity().center(),
            t.entity().small_radius(),
            Some(t.entity().large_radius()),
            t.transform(),
            t.orientation(),
        ),
        _ => return None,
    };
    if matrix.x.w != 0.0 || matrix.y.w != 0.0 || matrix.z.w != 0.0 || matrix.w.w != 1.0 {
        return None;
    }
    Some((
        RoundCarrier {
            center: center.into(),
            radius,
            major_radius,
            columns: [
                [matrix.x.x, matrix.x.y, matrix.x.z],
                [matrix.y.x, matrix.y.y, matrix.y.z],
                [matrix.z.x, matrix.z.y, matrix.z.z],
            ],
            translation: [matrix.w.x, matrix.w.y, matrix.w.z],
        },
        orientation,
    ))
}
/// The admitted closed oriented shell, exact co-carrier equality, native trim
/// feasibility and nonzero affine determinant establish a complete carrier,
/// not permission from a family label. A regular closed same-carrier patch
/// complex is open and closed in the connected carrier: opposite edge uses
/// continue across chart/face subdivisions rather than leaving physical trims.
fn raw_round_continuation<'a>(
    source: crate::section::OriginalTrimSource<'_, 'a>,
    frame: PlaneFrameMm,
    scratch_limit: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<RoundContinuation<'a>>, GeometryError> {
    let definition = source.definition();
    if definition.solid.boundaries().len() != 1 {
        return Ok(None);
    }
    let mut capture = None;
    let mut source_face_count = 0;
    for face in definition.solid.face_iter() {
        work.charge(1, cancel)?;
        let Some((carrier, orientation)) = face.with_surface(round_carrier) else {
            return Ok(None);
        };
        if carrier.radius <= 0.0 || carrier.major_radius.is_some_and(|r| r <= carrier.radius) {
            return Err(error(
                GeometryErrorCode::SingularGeometry,
                "round continuation requires a regular sphere or ring torus",
            ));
        }
        if orientation != face.orientation() {
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
fn broad_height(definition: &Definition, frame: PlaneFrameMm) -> Interval {
    (0..8).fold(
        Interval {
            lo: f64::INFINITY,
            hi: f64::NEG_INFINITY,
        },
        |range, mask| {
            let point = std::array::from_fn(|axis| {
                if mask & (1 << axis) == 0 {
                    definition.bounds.min[axis]
                } else {
                    definition.bounds.max[axis]
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
    source: crate::section::OriginalTrimSource<'s, 'd>,
    request: &'q SlabRequest,
    source_retained_bytes: usize,
}

/// A transition collar uses the explicitly scoped core C intersected with the
/// vertical sweep of the actual native filled sample S. This does NOT assert
/// a reverse distance from all CLOSED C to the current candidate prism.
fn recover_critical_slab(
    query: &SlabSourceQuery<'_, '_, '_>,
    section: NativeSection,
    capture: super::SlabCapture,
    endpoints: EndpointDecomposition,
    unresolved: GeometryError,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SlabAnalysis, GeometryError> {
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
    if (sample_inside || section.loops.is_empty()) && one_sided <= request.native_shape_error_mm {
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
            let occupied = Some(occupied_envelope_from_definition_bounds(
                definition,
                &section,
                remaining_scratch(reserved.saturating_add(complex.retained_bytes()))?,
                work,
                cancel,
            )?);
            return Ok(SlabAnalysis {
                section,
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
            section: &section,
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
            &section,
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
    Ok(SlabAnalysis {
        section,
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
    source: crate::section::OriginalTrimSource<'_, '_>,
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
    section: NativeSection,
    definition: &Definition,
    request: SlabRequest,
    source_retained_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SlabAnalysis, GeometryError> {
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
    let capture = super::SlabCapture::new(definition, request, &section, work, cancel)?;
    let occupied = Some(occupied_envelope(&section, 0.0, remaining, work, cancel)?);
    // Only a whole CLOSED-slab support exclusion authorizes exact empty reuse.
    // A regular empty sample or endpoint-only contact is not this witness.
    let invariance = Some(InvariantHeightDomain {
        definition_id: definition.id().clone(),
        material_realization: crate::kernel::MaterialRealizationIdentity::new(definition),
        frame: request.frame,
        height_interval_mm: request.height_interval_mm,
        pose: None,
    });
    Ok(SlabAnalysis {
        section,
        occupied,
        boundary_motion_bound_mm: 0.0,
        invariance,
        critical: None,
        endpoint_strata: Vec::new(),
        collar_complex: None,
        capture,
    })
}
pub(crate) fn analyze_slab(
    source: &crate::PreparedMaterialSource<'_>,
    request: SlabRequest,
    query_byte_limit: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SlabAnalysis, GeometryError> {
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
    let material_transport_zero = definition.material_transport_is_proved_zero(work, cancel)?;
    let transport_height_padding = if material_transport_zero {
        0.0
    } else {
        material_transport_height_padding(definition, HeightFunctional::from_frame(request.frame))?
    };
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
    let section = source.section_for_query(
        &crate::section::SectionQuery {
            plane,
            maximum_plane_error_mm,
        },
        cancel,
        section_bytes as u32,
        work,
    )?;
    let source = source.trim_source();
    work.charge(
        section
            .loops
            .iter()
            .map(|boundary| boundary.points_mm.len() as u64)
            .sum(),
        cancel,
    )?;
    let broad = broad_height(definition, request.frame).add(Interval {
        lo: -transport_height_padding,
        hi: transport_height_padding,
    });
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
    let resident_bytes = source_retained_bytes
        .saturating_add(section.retained_bytes())
        .saturating_add(definition.id().allocated_bytes());
    remaining_scratch(resident_bytes.saturating_add(STACK_SCRATCH_BYTES))?;
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
        raw_round_continuation(
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
    let capture = super::SlabCapture::new(definition, request, &section, work, cancel)?;
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
                &section,
                0.0,
                remaining_scratch(resident_bytes.saturating_add(endpoints.retained_bytes()))?,
                work,
                cancel,
            )?);
            return Ok(SlabAnalysis {
                section,
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
            &section,
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
                    section: &section,
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
        return Ok(SlabAnalysis {
            section,
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
        &section,
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
        &section,
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
                section: &section,
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
                    section: &section,
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
    Ok(SlabAnalysis {
        section,
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
    source: crate::section::OriginalTrimSource<'_, 'a>,
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
        let face = definition
            .solid
            .face_iter()
            .nth(proof.face)
            .ok_or_else(|| indeterminate("proved endpoint face mapping is incomplete"))?;
        let count = face
            .absolute_boundaries()
            .iter()
            .map(|wire| wire.len())
            .sum::<usize>();
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
        cap_faces.insert(face.id());
        for edge in face
            .absolute_boundaries()
            .iter()
            .flat_map(|wire| wire.iter())
        {
            work.charge(1, cancel)?;
            cap_edges.insert(edge.id());
            for vertex in [edge.front(), edge.back()] {
                let mask = cap_vertices.entry(vertex.id()).or_insert(0u8);
                *mask |= 1 << proof.endpoint;
            }
        }
        covered_face_cells += proof.covered_cells;
    }
    for (index, face) in definition.solid.face_iter().enumerate() {
        work.charge(1, cancel)?;
        if cap_faces.contains(&face.id()) {
            continue;
        }
        if face
            .with_surface(|surface| {
                constant_spline_height(surface, HeightFunctional::from_frame(frame), work, cancel)
            })?
            .is_some_and(|h| overlaps(h, range))
        {
            return Err(indeterminate(
                "slab contains an original horizontal critical face, not a regular continuation",
            ));
        }
        let reserved = resident_bytes
            .saturating_add(edge_scratch(face)?)
            .saturating_add(endpoints.retained_bytes())
            .saturating_add(
                (cap_faces.capacity() + cap_edges.capacity() + cap_vertices.capacity())
                    .saturating_mul(64),
            );
        let domain = source.face_domain(index, remaining_scratch(reserved)?, work, cancel)?;
        let bounded_uv = trim_parameter_box(&domain)?;
        face.with_surface(|surface| -> Result<(), GeometryError> {
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
            let wires = face.absolute_boundaries();
            let mut seen = HashSet::new();
            seen.try_reserve(wires.iter().map(|wire| wire.len()).sum())
                .map_err(|_| {
                    error(
                        GeometryErrorCode::ResourceLimit,
                        "core original edge identity allocation refused",
                    )
                })?;
            for wire in wires {
                for edge in wire {
                    work.charge(1, cancel)?;
                    if cap_edges.contains(&edge.id()) || !seen.insert(edge.id()) {
                        continue;
                    }
                    // A paired periodic use is not automatically a smooth seam.
                    // Prove its actual 3D tangent flow as for every physical edge.
                    for vertex in [edge.front(), edge.back()] {
                        let h = vertex_height(vertex.point().into(), frame);
                        let terminal_vertex = h.lo == h.hi
                            && cap_vertices.get(&vertex.id()).is_some_and(|mask| {
                                (mask & 1 != 0 && h.lo == range[0])
                                    || (mask & 2 != 0 && h.lo == range[1])
                            });
                        if overlaps(h, range) && !terminal_vertex {
                            return Err(indeterminate(
                                "slab contains a native trim corner or singular endpoint",
                            ));
                        }
                    }
                    edge.with_curve(|curve| -> Result<(), GeometryError> {
                        let prepared = definition.prepare_boundary_curve(
                            edge.id(),
                            curve,
                            curve.range_tuple(),
                            remaining_scratch(reserved.saturating_add(domain.owned_bytes()))?,
                            work,
                            cancel,
                        )?;
                        if !prepared.is_smooth_on(curve.range_tuple(), work, cancel)? {
                            if !overlaps(
                                height(
                                    prepared.position_bounds(curve.range_tuple(), work, cancel)?,
                                    frame,
                                ),
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
                        while let Some(((lo, hi), depth)) = curves
                            .pop()
                            .or_else(|| spans.next().map(|range| (range, 0u8)))
                        {
                            work.charge(1, cancel)?;
                            let jet = prepared.jet((lo, hi), work, cancel)?;
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
                            curves.push(((lo, mid), depth + 1));
                            curves.push(((mid, hi), depth + 1));
                        }
                        Ok(())
                    })?;
                }
            }
            Ok(())
        })?;
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
fn occupied_envelope(
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
        sampling_tolerance_mm: section.sampling_tolerance_mm,
        boolean_tolerance_mm: section.boolean_tolerance_mm,
        certificate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame() -> PlaneFrameMm {
        PlaneFrameMm {
            origin_mm: [0.0; 3],
            x_axis: [1.0, 0.0, 0.0],
            y_axis: [0.0, 1.0, 0.0],
            z_axis: [0.0, 0.0, 1.0],
        }
    }
    fn parity_definition(name: &str) -> Definition {
        let (_, solid, _) = crate::parity_fixtures::recipes()
            .into_iter()
            .find(|(candidate, _, _)| *candidate == name)
            .expect("native parity recipe");
        let source = crate::parity_fixtures::original(name, solid);
        crate::import_step(source.as_bytes(), &AtomicBool::new(false))
            .expect("native source-backed definition")
    }
    #[test]
    fn standalone_material_domains_borrow_one_definition_bound_owner() {
        let cancel = AtomicBool::new(false);
        let definition = crate::import_step(
            include_bytes!("../../../../fixtures/geometry/box-mm.step"),
            &cancel,
        )
        .unwrap();
        let other = crate::import_step(
            include_bytes!("../../../../fixtures/geometry/box-mm.step"),
            &cancel,
        )
        .unwrap();
        let standalone = crate::section::OriginalTrimSource::standalone(&definition);
        let mut preparation = GeometryWorkBudget::new(20_000_000);
        let originals = standalone
            .prepare_domains_if_missing(MAX_SLAB_SCRATCH_BYTES, &mut preparation, &cancel)
            .unwrap()
            .expect("standalone query owns its original domains");
        assert!(originals.retained_bytes() > 0);
        let source = standalone.borrowing_domains(Some(&originals));
        // Reborrowing consumes only binding work, not domain preparation or
        // another retained allocation, even with no additional metadata space.
        let mut work = GeometryWorkBudget::new(64);
        assert!(
            source
                .prepare_domains_if_missing(0, &mut work, &cancel)
                .unwrap()
                .is_none()
        );
        let first = source.face_domain(0, 0, &mut work, &cancel).unwrap();
        assert_eq!(first.owned_bytes(), 0);
        for _ in 0..10 {
            let again = source.face_domain(0, 0, &mut work, &cancel).unwrap();
            assert!(std::ptr::eq(&*first, &*again));
            assert_eq!(again.owned_bytes(), 0);
        }
        let foreign = crate::section::OriginalTrimSource::standalone(&other)
            .borrowing_domains(Some(&originals));
        let error = foreign.face_domain(0, 0, &mut work, &cancel).err().unwrap();
        assert_eq!(error.code, GeometryErrorCode::AnalysisIndeterminate);
        let error = standalone
            .prepare_domains_if_missing(0, &mut work, &cancel)
            .err()
            .unwrap();
        assert_eq!(error.code, GeometryErrorCode::ResourceLimit);
        assert!(
            error
                .message
                .contains("standalone SourceOriginals preparation")
        );
        assert!(error.message.contains("work_before="));
        assert!(error.message.contains("work_delta=0"));
        assert!(error.message.contains("work_remaining="));
        let error = standalone
            .prepare_domains_if_missing(MAX_SLAB_SCRATCH_BYTES, &mut work, &AtomicBool::new(true))
            .err()
            .unwrap();
        assert_eq!(error.code, GeometryErrorCode::Cancelled);
        assert!(error.message.contains("standalone SourceOriginals"));
        assert!(error.message.contains("work_consumed="));
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
            let source = crate::prepare_section_source(
                &definition,
                crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
                &cancel,
                &mut preparation_work,
            )
            .unwrap();
            // Isolate census cost from import/Boolean-source preparation.
            let mut work = GeometryWorkBudget::new(1_000_000);
            let result = analyze_height_domain_with_scratch_limit(
                source.trim_source(),
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
        let definition = crate::import_step(source.as_bytes(), &cancel).unwrap();
        assert!(definition.boundary_continuations.iter().flatten().count() > 0);
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
            crate::section::OriginalTrimSource::standalone(&definition),
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
        let definition = crate::import_step(
            include_bytes!("../../../../fixtures/geometry/box-mm.step"),
            &cancel,
        )
        .unwrap();
        let mut work = GeometryWorkBudget::new(5_000_000);
        let source = crate::prepare_material_source(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &cancel,
            &mut work,
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
            let prepared = crate::prepare_material_source(
                &definition,
                crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
                &cancel,
                &mut work,
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
            .expect("regular native slab with bounded shape motion");
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
        use monstertruck_modeling::{BsplineCurve, Curve, KnotVector, Point3};
        // Actual z=t² on the original closed [0,1] span. Its only derivative
        // root is the endpoint at height zero, not a micrometre search band.
        let original = Curve::BsplineCurve(BsplineCurve::new(
            KnotVector::bezier_knot(2),
            vec![
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(0.5, 0.0, 0.0),
                Point3::new(1.0, 0.0, 1.0),
            ],
        ));
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
        use monstertruck_modeling::{BsplineCurve, Curve, KnotVector, Point3};
        let original = Curve::BsplineCurve(BsplineCurve::new(
            KnotVector::from(vec![0.0, 0.0, 0.5, 1.0, 1.0]),
            vec![
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(0.5, 0.0, 1.0),
                Point3::new(1.0, 0.0, 0.0),
            ],
        ));
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
                .is_smooth_on((0.0, 1.0), &mut work, &cancel)
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
        use monstertruck_modeling::{
            BsplineCurve, Curve, KnotVector, Processor, TrimmedCurve, base::*,
        };
        let cancel = AtomicBool::new(false);
        for reflected in [false, true] {
            let original = Curve::BsplineCurve(BsplineCurve::new(
                KnotVector::from(vec![2., 2., 3., 7., 7.]),
                vec![
                    Point3::new(0., 0., 0.),
                    Point3::new(1., 0., 1.),
                    Point3::new(2., 0., 3.),
                ],
            ));
            let mut original =
                Processor::<_, Matrix4>::new(TrimmedCurve::new(Box::new(original), (2., 7.)));
            if reflected {
                original.invert();
            }
            let original = Curve::TrimmedCurve(original);
            let mut work = GeometryWorkBudget::new(1_000_000);
            let prepared = crate::parametric::prepare_curve_enclosure(
                &original,
                (2., 7.),
                MAX_SLAB_SCRATCH_BYTES,
                &mut work,
                &cancel,
            )
            .unwrap();
            assert!(!prepared.is_smooth_on((2., 7.), &mut work, &cancel).unwrap());
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
        use monstertruck_modeling::{
            BsplineCurve, Curve, KnotVector, Processor, TrimmedCurve, base::*,
        };
        let cancel = AtomicBool::new(false);
        for reflected in [false, true] {
            let original = Curve::BsplineCurve(BsplineCurve::new(
                KnotVector::from(vec![2., 2., 3., 7., 7.]),
                vec![
                    Point3::new(0., 0., 0.),
                    Point3::new(1., 0., 1.),
                    Point3::new(2., 0., 0.),
                ],
            ));
            let mut inner =
                Processor::<_, Matrix4>::new(TrimmedCurve::new(Box::new(original), (2., 7.)));
            if reflected {
                inner.invert();
            }
            let original = Curve::TrimmedCurve(Processor::with_transform(
                TrimmedCurve::new(Box::new(Curve::TrimmedCurve(inner)), (2.5, 6.25)),
                Matrix4::from_translation(Vector3::new(0., 0., 4.)),
            ));
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
                    .is_smooth_on((2.5, 6.25), &mut work, &cancel)
                    .unwrap()
            );
            let mut knots = Vec::new();
            assert!(
                prepared
                    .visit_explicit_knots((2.5, 6.25), &mut work, &cancel, &mut |knot, _, _| {
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
                        (2.5, 6.25),
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
                        (2.5, 6.25),
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
        use monstertruck_modeling::{
            BsplineCurve, Curve, KnotVector, Processor, TrimmedCurve, base::*,
        };
        let original = Curve::BsplineCurve(BsplineCurve::new(
            KnotVector::from(vec![0.1, 0.1, 0.2, 0.4, 0.4]),
            vec![
                Point3::new(0., 0., 0.),
                Point3::new(1., 0., 1.),
                Point3::new(2., 0., 0.),
            ],
        ));
        let mut reflected =
            Processor::<_, Matrix4>::new(TrimmedCurve::new(Box::new(original), (0.1, 0.4)));
        reflected.invert();
        let original = Curve::TrimmedCurve(reflected);
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let prepared = crate::parametric::prepare_curve_enclosure(
            &original,
            (0.1, 0.4),
            MAX_SLAB_SCRATCH_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let mut knots = Vec::new();
        assert!(
            !prepared
                .visit_explicit_knots((0.1, 0.4), &mut work, &cancel, &mut |knot, _, _| {
                    knots.push(knot);
                    Ok(())
                })
                .unwrap()
        );
        assert!(knots.is_empty());
        let mut events = Vec::new();
        let mut proved = Vec::new();
        let error = isolate_curve_events(
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
        .unwrap_err();
        assert_eq!(error.code, GeometryErrorCode::AnalysisIndeterminate);
        assert!(error.message.contains("internal C0 knot incidence"));
    }

    #[test]
    fn original_torus_self_seam_and_closed_vertex_links_are_height_transverse_at_zero() {
        let definition = parity_definition("native-parity-nurbs-torus.step");
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(30_000_000);
        let source = crate::section::OriginalTrimSource::standalone(&definition);
        let domain = source
            .face_domain(0, MAX_SLAB_SCRATCH_BYTES, &mut work, &cancel)
            .unwrap();
        let allowance = MAX_SLAB_SCRATCH_BYTES - domain.owned_bytes();
        let mut links = HeightSourceLinks::new(&definition, allowance, &mut work, &cancel).unwrap();
        let face = domain.native().face();
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
            for edge in face.absolute_boundaries().iter().flatten() {
                if edge.with_curve(|curve| constant_native_curve_height(curve, functional, &mut work, &cancel))?.is_some() {
                    assert!(links.whole_edge_regular(&FaceHeightQuery { face: 0, glued: &glued, functional }, edge.id(), allowance, &mut work, &cancel)?);
                    constant += 1;
                }
                for vertex in [edge.front(), edge.back()] {
                    assert!(links.vertex_regular(&definition, &FaceHeightQuery { face: 0, glued: &glued, functional }, vertex.id(),
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
            let vertex = face.absolute_boundaries()[0][0].front().id();
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
        let source = crate::section::OriginalTrimSource::standalone(&definition);
        let policy = AnalysisPolicy {
            maximum_height_uncertainty_mm: 1.0e-7,
            maximum_shape_error_mm: 0.1,
        };
        let covers = material_transport_height_cover(
            source,
            HeightFunctional::from_frame(frame()),
            [-1.0, 9.0],
            MAX_SLAB_SCRATCH_BYTES - definition.retained_query_proof_bytes(),
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
        let literal = analyze_literal_native_height_domain(
            source,
            frame(),
            [5.9, 6.1],
            policy,
            MAX_SLAB_SCRATCH_BYTES - definition.retained_query_proof_bytes() - retained,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(literal.event_brackets_mm.is_empty());
        assert!(literal.capture.transport_support.is_empty());
        assert!(!literal.capture.transport_support_complete);
        assert!(literal.requires_closed_material_capture([5.9, 6.1]));
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
            !definition
                .material_transport_is_proved_zero(&mut work, &cancel)
                .unwrap(),
            "an empty literal event list alone never certifies a transported material band",
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
        for edge in definition.solid.edge_iter() {
            for vertex in [edge.front(), edge.back()] {
                assert!(
                    incidence
                        .vertex_link_is_cycle(&definition, vertex.id(), &mut work, &cancel)
                        .unwrap()
                );
            }
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
        definition.face_boundaries[0].wires[0][0].orientation =
            !definition.face_boundaries[0].wires[0][0].orientation;
        assert_eq!(
            SourceEdgeIncidence::new(&definition, MAX_SLAB_SCRATCH_BYTES, &mut work, &cancel)
                .err()
                .unwrap()
                .code,
            GeometryErrorCode::ParameterCorrespondenceUnavailable
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
        for face in definition.solid.face_iter() {
            for edge in face
                .absolute_boundaries()
                .iter()
                .flat_map(|wire| wire.iter())
            {
                let varying = edge
                    .with_curve(|curve| -> Result<bool, GeometryError> {
                        if constant_native_curve_height(curve, functional, &mut work, &cancel)?
                            .is_some()
                        {
                            return Ok(false);
                        }
                        let prepared = definition.prepare_boundary_curve(
                            edge.id(),
                            curve,
                            curve.range_tuple(),
                            MAX_SLAB_SCRATCH_BYTES,
                            &mut work,
                            &cancel,
                        )?;
                        assert!(
                            !prepared.is_smooth_on(curve.range_tuple(), &mut work, &cancel)?,
                            "geometric circle/G1 does not erase an actual C0 parameter-speed jump"
                        );
                        let mut events = Vec::new();
                        let mut proved = Vec::new();
                        isolate_curve_events(
                            &OriginalCurveSpan {
                                curve: &prepared,
                                range: curve.range_tuple(),
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
                    })
                    .unwrap();
                if varying {
                    observed = true;
                    // Both original meridian halves retain their own finite
                    // source intervals; inventory the actual union, not a
                    // fictitious full-circle extension of the first half.
                }
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
        let definition = crate::import_step(
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
        let definition = crate::import_step(
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
        let source = crate::prepare_material_source(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &cancel,
            &mut work,
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
        if !definition
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
            let zero = definition
                .material_transport_is_proved_zero(&mut work, &cancel)
                .unwrap();
            let certificate =
                narrow_invariance(&definition, frame(), [-1.0, 9.0], &mut work, &cancel).unwrap();
            assert_eq!(
                certificate.is_some(),
                zero,
                "{name}: raw flow requires a zero-transport theorem"
            );
            if let Some(certificate) = certificate {
                assert_eq!(certificate.height_interval_mm, [0.0, 8.0], "{name}");
            }
            let inner =
                narrow_invariance(&definition, frame(), [2.0, 6.0], &mut work, &cancel).unwrap();
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
        let source = crate::prepare_material_source(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &cancel,
            &mut work,
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
        let zero = definition
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
        let mut work = GeometryWorkBudget::new(1000);
        let before = work.consumed();
        let error = native_vertical_boundary_flow(
            crate::section::OriginalTrimSource::standalone(&definition),
            frame(),
            [2.0, 6.0],
            0,
            &mut work,
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert_eq!(error.code, GeometryErrorCode::ResourceLimit);
        assert!(work.consumed() > before);
        let error = native_vertical_boundary_flow(
            crate::section::OriginalTrimSource::standalone(&definition),
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
        let zero = definition
            .material_transport_is_proved_zero(&mut work, &AtomicBool::new(false))
            .unwrap();
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
            let source = crate::prepare_material_source(
                &definition,
                crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
                &cancel,
                &mut work,
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
            let original_face = definition.solid.face_iter().nth(floor.face_index).unwrap();
            let incidence = original_edge_uses(&definition, &mut work, &cancel).unwrap();
            for (wire_index, wire) in original_face.absolute_boundaries().iter().enumerate() {
                for (coedge_index, edge) in wire.iter().enumerate() {
                    let uses = &incidence[&edge.id()];
                    let second = uses.second.unwrap();
                    assert_ne!(uses.first.orientation, second.orientation);
                    assert!(
                        [uses.first, second]
                            .iter()
                            .any(|use_| use_.face == floor.face_index
                                && use_.wire == wire_index
                                && use_.index == coedge_index)
                    );
                    let source = &definition.source_topology.faces[floor.face_index].wires
                        [wire_index]
                        .coedges[coedge_index];
                    assert!(source.source_edge.is_some() && source.source_coedge.is_some());
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
            let source = crate::prepare_material_source(
                &definition,
                crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
                &cancel,
                &mut work,
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
            let source = crate::prepare_material_source(
                &definition,
                crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
                &cancel,
                &mut work,
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
        let source = crate::prepare_material_source(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &cancel,
            &mut work,
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
        let definition = crate::import_step(
            include_bytes!(
                "../../../../fixtures/geometry/independent-exporters/occt-narrow-pocket-mm.step"
            ),
            &cancel,
        )
        .unwrap();
        let mut work = GeometryWorkBudget::new(50_000_000);
        let source = crate::prepare_material_source(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &cancel,
            &mut work,
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
            crate::section::OriginalTrimSource::standalone(&definition),
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
            crate::section::OriginalTrimSource::standalone(&definition),
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
            crate::section::OriginalTrimSource::standalone(&definition),
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
        let prepared = crate::prepare_section_source(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &cancel,
            &mut work,
        )
        .unwrap();
        let mut section = prepared
            .section_with_byte_limit(
                PlaneMm {
                    origin_mm: [0.0; 3],
                    normal: [0.0, 0.0, 1.0],
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
        use monstertruck_modeling::{
            BsplineCurve, BsplineSurface, Curve, KnotVector, NurbsCurve, NurbsSurface, Vector4,
        };
        let knots = KnotVector::bezier_knot(1);
        let controls = vec![
            vec![
                Vector4::new(0.0, 0.0, 1.0, 3.0),
                Vector4::new(0.0, 6.0, 2.0, 6.0),
            ],
            vec![
                Vector4::new(6.0, 0.0, 2.0, 6.0),
                Vector4::new(12.0, 12.0, 4.0, 12.0),
            ],
        ];
        let surface = Surface::NurbsSurface(NurbsSurface::new(BsplineSurface::new(
            (knots.clone(), knots.clone()),
            controls,
        )));
        let curve = Curve::NurbsCurve(NurbsCurve::new(BsplineCurve::new(
            knots,
            vec![
                Vector4::new(0.0, 0.0, 1.0, 3.0),
                Vector4::new(6.0, 0.0, 2.0, 6.0),
            ],
        )));
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
    fn affine_trimmed_original_curve_retains_exact_constant_height_identity() {
        use monstertruck_modeling::{
            Curve, Line, Matrix4, Point3, Processor, TrimmedCurve, Vector3,
        };
        let basis = Curve::Line(Line(Point3::new(0.0, 0.0, 2.0), Point3::new(1.0, 0.0, 2.0)));
        let curve = Curve::TrimmedCurve(Processor::with_transform(
            TrimmedCurve::new(Box::new(basis), (0.2, 0.8)),
            Matrix4::from_translation(Vector3::new(3.0, -4.0, 3.0)),
        ));
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
        let prepared = crate::prepare_material_source(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &cancel,
            &mut work,
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
        let prepared = crate::prepare_material_source(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &cancel,
            &mut work,
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
        let prepared = crate::prepare_material_source(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &cancel,
            &mut work,
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
