// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Literal native charts, not a reconstruction of the imported material.
//!
//! Chart admission proves coefficient-level C0 joins and embedded, disjoint UV
//! circuits. C1 is an independent property of each original coedge restriction;
//! corners never acquire tangent authority by subdivision. Winding queries use
//! enclosures of the original coefficients, not a sampled trim polygon.
//!
//! Material admission has additional obligations: original-source parameter
//! correspondence, equality of opposed spatial traces and shared endpoints,
//! filled-face embedding, complete vertex links and nonincident spatial
//! separation. Native topology, tolerance/gap fields and interval overlap do not
//! prove those identities. Original operands are retained separately from native
//! generated charts; source admission must qualify their complete selected
//! restrictions and every correction joint before exposing material queries.

use super::Definition;
use super::source_operand::PreparedOriginalSurface;
use crate::analysis::source_restriction::census::SourceVertexRecord;
use crate::exact_coefficient::ExactCoefficient;
use crate::parametric::{self, CurveEvaluator as _, Interval, SurfaceEvaluator as _};
use crate::{GeometryError, GeometryWorkBudget, check_cancel};
use solidcore::curve::{curve::Curve, curve::p2::CurveKind};
use solidcore::nurbs::b_spline_surface_n_lib::p2::OrientType;
use solidcore::topology::edgeuse::p1 as edgeuse;
use solidcore::topology::{EdgeId, EdgeuseId, EdgeuseOwner, FaceId, LoopuseStart, VertexId};
use spiling_contracts::geometry::GeometryErrorCode;
use std::{
    mem::{size_of, size_of_val},
    sync::{Arc, atomic::AtomicBool},
};

const MAX_DEPTH: u8 = 32;
// Includes fixed subdivision stacks, bounded exact-expansion temporaries and the
// native coefficient evaluator. No unaccounted growable query scratch is used.
const SCRATCH_BYTES: usize = 128 * 1024
    + parametric::curve_prepared::CURVE_EVALUATION_SCRATCH_BYTES
    + parametric::nurbs_eval::SURFACE_EVALUATION_SCRATCH_BYTES;
const CLASSIFICATION_SCRATCH_BYTES: usize =
    SCRATCH_BYTES + 2 * size_of::<parametric::ChartCurveEnclosure>();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrimClassification {
    Inside,
    Outside,
    Boundary,
}

/// Analytic pole name only; this is not a source-joint qualification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SphericalPole {
    North,
    South,
}

/// Borrow the actual selected coefficient operator with its own chart gauge.
/// An original-radian provider is never relabelled as a native-degree surface.
#[derive(Clone, Copy)]
pub(crate) enum PreparedTrimSurface<'p, 'd> {
    Native(&'p solidcore::surface::surface::Surface),
    Original(&'p PreparedOriginalSurface<'d>),
}

impl parametric::SurfaceEvaluator for PreparedTrimSurface<'_, '_> {
    fn active_domain(&self) -> [[f64; 2]; 2] {
        match self {
            Self::Native(surface) => parametric::SurfaceEvaluator::active_domain(*surface),
            Self::Original(surface) => surface.active_domain(),
        }
    }
    fn tensor(
        &self,
        uv: [[f64; 2]; 2],
        order: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<parametric::nurbs_eval::SurfaceTensor, GeometryError> {
        match self {
            Self::Native(surface) => {
                parametric::SurfaceEvaluator::tensor(*surface, uv, order, work, cancel)
            }
            Self::Original(surface) => {
                parametric::SurfaceEvaluator::tensor(*surface, uv, order, work, cancel)
            }
        }
    }
    fn continuous(
        &self,
        uv: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        match self {
            Self::Native(surface) => {
                parametric::SurfaceEvaluator::continuous(*surface, uv, work, cancel)
            }
            Self::Original(surface) => {
                parametric::SurfaceEvaluator::continuous(*surface, uv, work, cancel)
            }
        }
    }
    fn smooth(
        &self,
        uv: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        match self {
            Self::Native(surface) => {
                parametric::SurfaceEvaluator::smooth(*surface, uv, work, cancel)
            }
            Self::Original(surface) => {
                parametric::SurfaceEvaluator::smooth(*surface, uv, work, cancel)
            }
        }
    }
    fn parameter_axes(
        &self,
        uv: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[(&[f64], usize); 2], GeometryError> {
        match self {
            Self::Native(surface) => {
                parametric::SurfaceEvaluator::parameter_axes(*surface, uv, work, cancel)
            }
            Self::Original(surface) => {
                parametric::SurfaceEvaluator::parameter_axes(*surface, uv, work, cancel)
            }
        }
    }
    fn native_surface(&self) -> Option<&solidcore::surface::surface::Surface> {
        match self {
            Self::Native(surface) => Some(surface),
            Self::Original(_) => None,
        }
    }
    fn original_surface(&self) -> Option<&solidcraft_step::geom::Surface> {
        match self {
            Self::Native(_) => None,
            Self::Original(surface) => Some(surface.source()),
        }
    }
}

/// Actual native use ownership. This is not an original STEP source binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeCoedgeIdentity {
    pub(crate) face_id: FaceId,
    pub(crate) wire_index: usize,
    pub(crate) coedge_index: usize,
    pub(crate) edge_id: EdgeId,
    pub(crate) orientation: bool,
    pub(crate) edgeuse: EdgeuseId,
}

/// Immutable bindings to actual native edgeuses; no nearest-point parameter map.
pub(crate) struct NativeTrimCoedge<'d> {
    pub(crate) edgeuse: EdgeuseId,
    pub(crate) uv_owner: EdgeuseId,
    pub(crate) edge: EdgeId,
    pub(crate) vertices: [VertexId; 2],
    pub(crate) native_range: [f64; 2],
    pub(crate) uv_range: [f64; 2],
    pub(crate) forward: bool,
    pub(crate) c1: bool,
    curve: &'d Curve,
    loop_index: usize,
    face: FaceId,
    index: usize,
}

impl<'d> NativeTrimCoedge<'d> {
    pub(crate) fn curve(&self) -> &'d Curve {
        self.curve
    }
    pub(crate) fn identity(&self) -> NativeCoedgeIdentity {
        NativeCoedgeIdentity {
            face_id: self.face,
            wire_index: self.loop_index,
            coedge_index: self.index,
            edge_id: self.edge,
            orientation: self.forward,
            edgeuse: self.edgeuse,
        }
    }
    pub(crate) fn oriented_range(&self) -> [f64; 2] {
        if self.forward {
            self.uv_range
        } else {
            [self.uv_range[1], self.uv_range[0]]
        }
    }

    /// Native trim parameter, NOT an original STEP parameter correspondence.
    pub(crate) fn chart_jet(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<parametric::ChartCurveEnclosure, GeometryError> {
        work.charge(1, cancel)?;
        Interval::new(range[0], range[1])?;
        if range[0] < self.uv_range[0] || range[1] > self.uv_range[1] {
            return Err(unavailable(
                "native trim query exceeds its complete original UV restriction",
            ));
        }
        parametric::curve2d_piecewise_jet(self.curve, range, work, cancel)
    }

    /// Independent strict tangent qualification on the requested native chart
    /// restriction. Piecewise first bounds must not be mistaken for C1.
    pub(crate) fn chart_c1_jet(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<parametric::ChartCurveEnclosure>, GeometryError> {
        self.chart_jet(range, work, cancel)?;
        parametric::curve2d_c1_jet(self.curve, range, work, cancel)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoundaryCurve {
    Coedge(usize),
    Joint(usize),
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum BoundaryEndpoint {
    ExactJoin(usize),
    Joint(usize, usize),
}
#[derive(Clone, Copy)]
struct TrimSpan {
    boundary: BoundaryCurve,
    range: [f64; 2],
    // True only at the original, topologically identified coedge endpoints.
    ends: [bool; 2],
}

/// One proved literal chart. This does not establish a spatial material face.
pub(crate) struct PreparedFaceTrimDomain<'d> {
    definition: &'d Definition,
    face: FaceId,
    face_index: usize,
    material_orientation: bool,
    domain: [[f64; 2]; 2],
    coedges: Vec<NativeTrimCoedge<'d>>,
    loops: Vec<[usize; 2]>,
    spans: Vec<TrimSpan>,
    carrier_c1: bool,
    original_surface: Option<PreparedOriginalSurface<'d>>,
    original_orientation: Option<bool>,
    original_coedges: Vec<PreparedOriginalCoedge<'d>>,
    original_joints: Vec<PreparedTrimJoint>,
    joint_after: Vec<Option<usize>>,
    original_margin_uv: Option<[f64; 2]>,
    original_chart_error_mm: Option<f64>,
}

/// One borrowed definition, with no duplicate BREP and no synthetic source law.
pub(crate) struct PreparedNativeTrimSource<'d> {
    definition: &'d Definition,
    faces: Vec<PreparedFaceTrimDomain<'d>>,
    allowance: usize,
    spatial_operators: Vec<SourceSpatialCacheEntry<'d>>,
    source_vertices: Vec<SourceVertexRecord>,
    source_inventory: Option<OriginalCorrespondenceInventory>,
}

impl<'d> PreparedNativeTrimSource<'d> {
    /// Raw native-chart admission, explicitly without spatial edge witnesses or
    /// source/material identity. Every original face and every loop is included.
    pub(crate) fn prepare_charts(
        definition: &'d Definition,
        allowance: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        check_cancel(cancel)?;
        let count = definition.face_ids().len();
        if count == 0 {
            return Err(unavailable("native chart source has no faces"));
        }
        let base = size_of::<Self>()
            .checked_add(checked_bytes::<PreparedFaceTrimDomain<'_>>(count)?)
            .ok_or_else(|| resource("native face inventory byte count overflow"))?;
        memory(base, allowance)?;
        let mut faces = Vec::new();
        faces
            .try_reserve_exact(count)
            .map_err(|_| resource("native face inventory allocation failed"))?;
        let mut retained =
            size_of::<Self>() + faces.capacity() * size_of::<PreparedFaceTrimDomain<'_>>();
        memory(retained, allowance)?;
        for &face in definition.face_ids() {
            work.charge(1, cancel)?;
            let domain = PreparedFaceTrimDomain::prepare(
                definition,
                face,
                allowance.saturating_sub(retained),
                work,
                cancel,
            )?;
            retained = retained
                .checked_add(domain.owned_bytes())
                .ok_or_else(|| resource("native trim retained byte count overflow"))?;
            memory(retained, allowance)?;
            faces.push(domain);
        }
        Ok(Self {
            definition,
            faces,
            allowance,
            spatial_operators: Vec::new(),
            source_vertices: Vec::new(),
            source_inventory: None,
        })
    }

    pub(crate) fn definition(&self) -> &'d Definition {
        self.definition
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            + self.faces.capacity() * size_of::<PreparedFaceTrimDomain<'_>>()
            + self
                .faces
                .iter()
                .map(PreparedFaceTrimDomain::owned_bytes)
                .sum::<usize>()
            + self.spatial_operators.capacity() * size_of::<SourceSpatialCacheEntry<'_>>()
            + self.source_vertices.capacity() * size_of::<SourceVertexRecord>()
            + self
                .spatial_operators
                .iter()
                .map(|entry| entry.operator.retained_bytes() + 2 * size_of::<usize>())
                .sum::<usize>()
    }
    pub(crate) fn face_domain(&self, ordinal: usize) -> Option<&PreparedFaceTrimDomain<'d>> {
        self.faces.get(ordinal)
    }

    /// Captured original vertex, never the healed/native BREP vertex point.
    pub(crate) fn source_vertex_point(
        &self,
        vertex: VertexId,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[f64; 3], GeometryError> {
        work.charge(
            usize::BITS.saturating_sub(self.source_vertices.len().leading_zeros()) as u64 + 1,
            cancel,
        )?;
        self.source_vertices.binary_search_by_key(&vertex,|record|record.native)
            .map(|index|self.source_vertices[index].point)
            .map_err(|_|unavailable("native vertex has no complete, source-identity-consistent authored vertex star"))
    }

    /// Read-only validation of an actual completed source/coedge receipt. This
    /// admits retained realization operands, not a global filled-material or
    /// nonincident F/G/cone embedding certificate.
    pub(crate) fn require_source_correspondence(
        &self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        work.charge(1, cancel)?;
        let receipt = self.source_inventory.as_ref().ok_or_else(|| {
            unavailable(
                "native-only charts have no completed original source/coedge/domain correspondence",
            )
        })?;
        if receipt.faces != self.faces.len()
            || receipt.faces != self.definition.face_ids().len()
            || receipt.source_uses != self.definition.original_source().coedges.len()
            || receipt.vertices != self.source_vertices.len()
        {
            return Err(unavailable(
                "immutable original correspondence inventory differs from its owning definition",
            ));
        }
        Ok(())
    }
    pub(crate) fn certify_zero_material_transport(
        &self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.require_source_correspondence(work, cancel)?;
        for face in &self.faces {
            for coedge in &face.original_coedges {
                work.charge(1, cancel)?;
                let receipt = &coedge.correspondence;
                if receipt.provenance() != OriginalCorrespondenceProvenance::ExactCoefficients
                    || receipt.position_error_mm() != 0.0
                    || receipt.relative_derivative_error_bound() != 0.0
                    || !coedge.curve.spatial.operator.exact_source_endpoints()
                {
                    return Ok(false);
                }
            }
            for joint in &face.original_joints {
                work.charge(1, cancel)?;
                if !joint.zero_displacement {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
}

impl<'d> PreparedFaceTrimDomain<'d> {
    /// Independent face admission preserves positive literal chart domains even
    /// when another face, or the source/material correspondence, is unsupported.
    pub(crate) fn prepare(
        definition: &'d Definition,
        face_id: FaceId,
        allowance: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        work.charge(definition.face_ids().len() as u64 + 1, cancel)?;
        memory(size_of::<Self>(), allowance)?;
        let face_index = definition
            .face_ordinal(face_id)
            .ok_or_else(|| unavailable("native chart face is not owned by this definition"))?;
        let brep = definition.brep();
        let face = brep.face(face_id).map_err(native_error)?;
        let surface = definition
            .face_surface(face_id)
            .ok_or_else(|| unavailable("native chart face has no coefficient carrier"))?;
        let natural = surface.natural_uv_domain();
        let domain = [
            [natural.u_min(), natural.u_max()],
            [natural.v_min(), natural.v_max()],
        ];
        for axis in domain {
            Interval::new(axis[0], axis[1])?;
            if axis[0] >= axis[1] {
                return Err(unavailable(
                    "native carrier chart has no finite two-dimensional domain",
                ));
            }
        }
        let carrier_c1 = parametric::surface_parameter_span_smooth(surface, domain, work, cancel)?;
        if !carrier_c1
            && !parametric::surface_parameter_span_continuous(surface, domain, work, cancel)?
        {
            return Err(unavailable(format!(
                "native face {face_id:?} carrier lacks literal whole-chart C0 qualification"
            )));
        }
        let fu_id = face
            .faceuse
            .ok_or_else(|| unavailable("native chart face has no faceuse"))?;
        let fu = brep.faceuse(fu_id).map_err(native_error)?;
        if fu.face != Some(face_id)
            || fu.orientation != OrientType::Same
            || fu.loopuses.len() != face.loops.len()
            || face.loops.is_empty()
        {
            return Err(unavailable(
                "native chart face/faceuse loop ownership is incomplete",
            ));
        }
        let shell = brep.shell(face.shell).map_err(native_error)?;
        let solidcore::topology::shell::ShellMembers::Faceuses(shell_uses) = &shell.members else {
            return Err(unavailable(
                "native chart face has no oriented shell faceuse inventory",
            ));
        };
        let mut material_orientation = None;
        for &use_id in shell_uses {
            work.charge(1, cancel)?;
            let use_ = brep.faceuse(use_id).map_err(native_error)?;
            if use_.face == Some(face_id) {
                let orientation = match use_.orientation {
                    OrientType::Same => true,
                    OrientType::Opposite => false,
                    _ => return Err(unavailable("native shell faceuse has unknown orientation")),
                };
                if material_orientation.replace(orientation).is_some() {
                    return Err(unavailable("native shell repeats a material faceuse"));
                }
            }
        }
        let material_orientation = material_orientation
            .ok_or_else(|| unavailable("native chart face is absent from its material shell"))?;
        let mut count = 0usize;
        for &loop_id in &face.loops {
            work.charge(1, cancel)?;
            let record = brep.loop_(loop_id).map_err(native_error)?;
            if record.face != face_id || record.edges.is_empty() {
                return Err(unavailable(
                    "native pole/empty loop requires a separate quotient-chart theorem",
                ));
            }
            count = count
                .checked_add(record.edges.len())
                .ok_or_else(|| resource("native coedge count overflow"))?;
        }
        let bytes = size_of::<Self>()
            .checked_add(checked_bytes::<NativeTrimCoedge<'_>>(count)?)
            .and_then(|n| n.checked_add(face.loops.len().checked_mul(size_of::<[usize; 2]>())?))
            .ok_or_else(|| resource("native chart byte count overflow"))?;
        memory(bytes, allowance)?;
        let mut result = Self {
            definition,
            face: face_id,
            face_index,
            material_orientation,
            domain,
            coedges: Vec::new(),
            loops: Vec::new(),
            spans: Vec::new(),
            carrier_c1,
            original_surface: None,
            original_orientation: None,
            original_coedges: Vec::new(),
            original_joints: Vec::new(),
            original_margin_uv: None,
            original_chart_error_mm: None,
            joint_after: Vec::new(),
        };
        result
            .coedges
            .try_reserve_exact(count)
            .map_err(|_| resource("native coedge allocation failed"))?;
        result
            .loops
            .try_reserve_exact(face.loops.len())
            .map_err(|_| resource("native loop allocation failed"))?;
        memory(size_of::<Self>() + result.owned_bytes(), allowance)?;
        for (loop_index, (&loop_id, &lu_id)) in face.loops.iter().zip(&fu.loopuses).enumerate() {
            work.charge(1, cancel)?;
            let record = brep.loop_(loop_id).map_err(native_error)?;
            let lu = brep.loopuse(lu_id).map_err(native_error)?;
            if lu.loop_ != Some(loop_id) || lu.faceuse != Some(fu_id) {
                return Err(unavailable(
                    "native chart loopuse does not own its original loop",
                ));
            }
            let first = match lu.start {
                LoopuseStart::Edgeuse(Some(id)) => id,
                _ => {
                    return Err(unavailable(
                        "native chart loop has no bounded edgeuse circuit",
                    ));
                }
            };
            let start = result.coedges.len();
            let mut current = first;
            for (index, expected) in record.edges.iter().enumerate() {
                work.charge(1, cancel)?;
                let eu = brep.edgeuse(current).map_err(native_error)?;
                let forward = match eu.orientation {
                    OrientType::Same => true,
                    OrientType::Opposite => false,
                    _ => return Err(unavailable("native chart edgeuse has unknown orientation")),
                };
                if eu.owner != EdgeuseOwner::Loopuse(Some(lu_id))
                    || eu.edge != Some(expected.edge)
                    || forward != expected.forward
                {
                    return Err(unavailable(
                        "native chart edgeuse circuit disagrees with the ordered original loop",
                    ));
                }
                let edge = brep.edge(expected.edge).map_err(native_error)?;
                let native_range = [edge.interval.min(), edge.interval.max()];
                Interval::new(native_range[0], native_range[1])?;
                if native_range[0] >= native_range[1] || edge.curve.is_none() {
                    return Err(unavailable(
                        "native chart coedge has no nondegenerate original spatial range",
                    ));
                }
                let (curve, owner) = edgeuse::uv_trim_curve(brep, current).map_err(native_error)?;
                let curve = curve.ok_or_else(|| {
                    unavailable(format!(
                        "native edgeuse {current:?} has no literal UV coefficient curve"
                    ))
                })?;
                let uv_owner = owner
                    .ok_or_else(|| unavailable("native UV curve has no exact edgeuse owner"))?;
                if edgeuse::face(brep, uv_owner).map_err(native_error)? != Some(face_id) {
                    return Err(unavailable(
                        "native UV curve owner belongs to another carrier chart",
                    ));
                }
                let uv = curve.natural_interval();
                let uv_range = [uv.min(), uv.max()];
                Interval::new(uv_range[0], uv_range[1])?;
                if uv_range[0] >= uv_range[1] {
                    return Err(unavailable("native chart trim has a degenerate UV range"));
                }
                let c1 = parametric::curve_parameter_span_smooth(curve, uv_range, work, cancel)?;
                if !c1
                    && !parametric::curve_parameter_span_continuous(curve, uv_range, work, cancel)?
                {
                    return Err(unavailable(format!(
                        "native edgeuse {current:?} has no exact C0 coefficient qualification across its original knots"
                    )));
                }
                if !parametric::curve2d_image_within_box(curve, uv_range, domain, work, cancel)? {
                    return Err(unavailable(format!(
                        "native edgeuse {current:?} leaves its literal carrier chart; periodic/pole chart identification is unproved"
                    )));
                }
                result.coedges.push(NativeTrimCoedge {
                    edgeuse: current,
                    uv_owner,
                    edge: expected.edge,
                    vertices: edge.vertices,
                    native_range,
                    uv_range,
                    forward,
                    c1,
                    curve,
                    loop_index,
                    face: face_id,
                    index,
                });
                let next = eu
                    .ccw
                    .ok_or_else(|| unavailable("native trim circuit is open"))?;
                if brep.edgeuse(next).map_err(native_error)?.cw != Some(current)
                    || (next == first) != (index + 1 == record.edges.len())
                {
                    return Err(unavailable(
                        "native trim circuit repeats early or lacks reciprocal ordered closure",
                    ));
                }
                current = next;
            }
            let end = result.coedges.len();
            for index in start..end {
                work.charge(1, cancel)?;
                let next = if index + 1 == end { start } else { index + 1 };
                let a = &result.coedges[index];
                let b = &result.coedges[next];
                let a_end = if a.forward { 1 } else { 0 };
                let b_start = if b.forward { 0 } else { 1 };
                if a.vertices[a_end] != b.vertices[b_start] {
                    return Err(unavailable(
                        "native chart circuit has inconsistent shared endpoint IDs",
                    ));
                }
                let p = exact_endpoint(a.curve, a_end, work, cancel)?;
                let q = exact_endpoint(b.curve, b_start, work, cancel)?;
                if !same_endpoint(&p, &q, work, cancel)? {
                    return Err(unavailable(format!(
                        "native chart C0 identity unproved at edgeuses {:?}/{:?}: original endpoint coefficients differ or exact arithmetic is unavailable; a nominal edge gap is not closure",
                        a.edgeuse, b.edgeuse
                    )));
                }
            }
            result.loops.push([start, end]);
        }
        // Each retained piece is injective by a strict monotone coordinate.
        // Every pair is then separated, except its genuine common endpoint.
        for coedge in 0..result.coedges.len() {
            let curve = result.coedges[coedge].curve;
            let range = result.coedges[coedge].uv_range;
            for part in parametric::curve2d_parameter_spans(curve, range, work, cancel)? {
                if part[0] == part[1] {
                    continue;
                }
                let mut stack = [([0.0; 2], 0u8); MAX_DEPTH as usize + 1];
                stack[0] = (part, 0);
                let mut live = 1;
                while live != 0 {
                    live -= 1;
                    let (part, depth) = stack[live];
                    work.charge(1, cancel)?;
                    let jet = parametric::curve2d_piecewise_jet(curve, part, work, cancel)?;
                    if jet.derivative.iter().any(|d| d.lo > 0.0 || d.hi < 0.0) {
                        let span = TrimSpan {
                            boundary: BoundaryCurve::Coedge(coedge),
                            range: part,
                            ends: [part[0] == range[0], part[1] == range[1]],
                        };
                        reserve_span(&mut result, allowance)?;
                        result.spans.push(span);
                    } else {
                        let middle = split(part, depth, "native trim injectivity/regularity")?;
                        stack[live] = ([middle, part[1]], depth + 1);
                        stack[live + 1] = ([part[0], middle], depth + 1);
                        live += 2;
                    }
                }
            }
        }
        for i in 0..result.spans.len() {
            for j in i + 1..result.spans.len() {
                result.separate_spans(result.spans[i], result.spans[j], work, cancel)?;
            }
        }
        Ok(result)
    }

    pub(crate) fn uv_domain(&self) -> [[f64; 2]; 2] {
        self.domain
    }
    pub(crate) fn face(&self) -> FaceId {
        self.face
    }
    pub(crate) fn face_index(&self) -> usize {
        self.face_index
    }
    /// Material sense in the selected carrier's own UV gauge.
    pub(crate) fn orientation(&self) -> bool {
        self.original_orientation
            .unwrap_or(self.material_orientation)
    }
    /// Native combinatorial shell sense, independent of the selected source gauge.
    pub(crate) fn native_orientation(&self) -> bool {
        self.material_orientation
    }
    pub(crate) fn definition(&self) -> &'d Definition {
        self.definition
    }
    pub(crate) fn coedges(&self) -> &[NativeTrimCoedge<'d>] {
        &self.coedges
    }
    /// Source correction joints only. An empty result is a proved zero ordinary
    /// completion, never a native-only stand-in for missing source operands.
    pub(crate) fn original_joints(&self) -> Result<&[PreparedTrimJoint], GeometryError> {
        if self.original_surface.is_none() {
            return Err(unavailable(
                "original joint inventory is unavailable in a native-only chart",
            ));
        }
        Ok(&self.original_joints)
    }
    pub(crate) fn carrier_is_c1(&self) -> bool {
        self.carrier_c1
    }
    pub(crate) fn surface_evaluator(&self) -> &dyn parametric::SurfaceEvaluator {
        match self.original_surface.as_ref() {
            Some(original) => original,
            None => self
                .definition
                .face_surface(self.face)
                .expect("prepared immutable native carrier"),
        }
    }

    pub(crate) fn surface_operand(&self) -> PreparedTrimSurface<'_, 'd> {
        match self.original_surface.as_ref() {
            Some(original) => PreparedTrimSurface::Original(original),
            None => PreparedTrimSurface::Native(
                self.definition
                    .face_surface(self.face)
                    .expect("prepared immutable native carrier"),
            ),
        }
    }

    /// Position-only coefficient-sheet bounds for exclusion overcovers. This
    /// does not grant selected-domain membership, continuity or material.
    pub(crate) fn original_carrier_position_bounds(
        &self,
        uv: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[Interval; 3], GeometryError> {
        self.original_surface
            .as_ref()
            .ok_or_else(|| {
                unavailable("source carrier evaluation requires captured original coefficients")
            })?
            .carrier_position_bounds(uv, work, cancel)
    }

    /// Original coefficient-sheet bounds only. The trim and source-material
    /// domains remain unchanged and require their independent qualifications.
    pub(crate) fn original_carrier_jet(
        &self,
        uv: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<parametric::SurfaceEnclosure, GeometryError> {
        self.original_surface
            .as_ref()
            .ok_or_else(|| {
                unavailable("source carrier evaluation requires captured original coefficients")
            })?
            .carrier_jet(uv, work, cancel)
    }

    pub(crate) fn original_carrier_piecewise_first(
        &self,
        uv: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<parametric::SurfaceFirstEnclosure, GeometryError> {
        self.original_surface
            .as_ref()
            .ok_or_else(|| {
                unavailable("source carrier evaluation requires captured original coefficients")
            })?
            .carrier_piecewise_first(uv, work, cancel)
    }

    /// All actual native knot restrictions, borrowed from the admitted bounded
    /// inventory. Their parameters are UV-curve parameters, not spatial edge t.
    pub(crate) fn boundary_parameter_intervals(
        &self,
        coedge: usize,
    ) -> Result<impl Iterator<Item = [f64; 2]> + '_, GeometryError> {
        self.coedges
            .get(coedge)
            .ok_or_else(|| unavailable("native trim coedge ordinal is absent"))?;
        Ok(self
            .spans
            .iter()
            .filter(move |span| span.boundary == BoundaryCurve::Coedge(coedge))
            .map(|span| span.range))
    }

    pub(crate) fn boundary_piecewise_enclosure(
        &self,
        coedge: usize,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<parametric::ChartCurveEnclosure, GeometryError> {
        let curve = self.coedge_operand(coedge)?;
        let active = self.boundary_restriction(BoundaryCurve::Coedge(coedge)).0;
        Interval::new(range[0], range[1])?;
        if range[0] < active[0] || range[1] > active[1] {
            return Err(unavailable(
                "selected boundary query leaves its complete actual coefficient restriction",
            ));
        }
        if self.original_surface.is_some() && range == curve.active_range() {
            work.charge(1, cancel)?;
            return Ok(self.original_coedges[coedge].full_uv_jet);
        }
        parametric::curve2d_piecewise_jet(&curve, range, work, cancel)
    }

    pub(crate) fn boundary_position_enclosure(
        &self,
        coedge: usize,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[[f64; 2]; 2], GeometryError> {
        Ok(self
            .boundary_piecewise_enclosure(coedge, range, work, cancel)?
            .position
            .map(|v| [v.lo, v.hi]))
    }

    pub(crate) fn boundary_first_enclosure(
        &self,
        coedge: usize,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<parametric::ChartCurveEnclosure, GeometryError> {
        let curve = self.coedge_operand(coedge)?;
        if !curve.smooth(range, work, cancel)? {
            return Err(unavailable(
                "selected boundary closed restriction lacks independent C1 qualification",
            ));
        }
        parametric::curve2d_piecewise_jet(&curve, range, work, cancel)
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        size_of::<Self>() + self.owned_bytes()
    }
    fn owned_bytes(&self) -> usize {
        self.coedges.capacity() * size_of::<NativeTrimCoedge<'_>>()
            + self.loops.capacity() * size_of::<[usize; 2]>()
            + self.spans.capacity() * size_of::<TrimSpan>()
            + self.original_coedges.capacity() * size_of::<PreparedOriginalCoedge<'_>>()
            + self
                .original_coedges
                .iter()
                .map(PreparedOriginalCoedge::owned_bytes)
                .sum::<usize>()
            + self.original_joints.capacity() * size_of::<PreparedTrimJoint>()
            + self.joint_after.capacity() * size_of::<Option<usize>>()
            + self.original_surface.as_ref().map_or(0, |surface| {
                surface
                    .retained_bytes()
                    .saturating_sub(size_of::<PreparedOriginalSurface<'_>>())
            })
    }

    /// `allowance` excludes the already borrowed domain. Classification has
    /// fixed bounded scratch only; do not charge retained metadata twice.
    pub(crate) fn classify_with_byte_limit(
        &self,
        uv_box: [[f64; 2]; 2],
        allowance: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<TrimClassification, GeometryError> {
        check_cancel(cancel)?;
        if allowance < CLASSIFICATION_SCRATCH_BYTES {
            return Err(resource(
                "native trim winding scratch exceeds the query allowance",
            ));
        }
        self.classify(uv_box, work, cancel)
    }

    /// Whole-box winding qualification. Boundary means intersecting OR not
    /// resolved within the fixed subdivision limit; it never means Inside.
    pub(crate) fn classify(
        &self,
        uv_box: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<TrimClassification, GeometryError> {
        work.charge(1, cancel)?;
        for axis in uv_box {
            Interval::new(axis[0], axis[1])?;
        }
        if (0..2).any(|a| uv_box[a][1] < self.domain[a][0] || uv_box[a][0] > self.domain[a][1]) {
            return Ok(TrimClassification::Outside);
        }
        if (0..2).any(|a| uv_box[a][0] < self.domain[a][0] || uv_box[a][1] > self.domain[a][1]) {
            return Ok(TrimClassification::Boundary);
        }
        for joint in &self.original_joints {
            work.charge(1, cancel)?;
            if (0..2).all(|axis| {
                uv_box[axis][0] <= joint.enclosure[axis].hi
                    && uv_box[axis][1] >= joint.enclosure[axis].lo
            }) {
                return Ok(TrimClassification::Boundary);
            }
        }
        let point = uv_box.map(|a| a[0] * 0.5 + a[1] * 0.5);
        // A shear merely changes the ray; it does not change the chart or its
        // orientation. Retry only ray degeneracy, never geometry admission.
        for shear in [0.0, 1.0, -1.0, 2.0, -2.0] {
            let mut outer = false;
            let mut hole = false;
            let mut resolved = true;
            for (loop_index, &[start, end]) in self.loops.iter().enumerate() {
                let mut winding = 0i64;
                for coedge in start..end {
                    match self.boundary_winding(
                        BoundaryCurve::Coedge(coedge),
                        uv_box,
                        point,
                        shear,
                        work,
                        cancel,
                    )? {
                        Some(value) => {
                            winding = winding
                                .checked_add(value)
                                .ok_or_else(|| resource("native trim winding count overflow"))?
                        }
                        None => {
                            resolved = false;
                            break;
                        }
                    }
                }
                if resolved {
                    for (index, joint) in self.original_joints.iter().enumerate() {
                        work.charge(1, cancel)?;
                        if joint.incoming.wire_index != loop_index {
                            continue;
                        }
                        match self.boundary_winding(
                            BoundaryCurve::Joint(index),
                            uv_box,
                            point,
                            shear,
                            work,
                            cancel,
                        )? {
                            Some(value) => {
                                winding = winding.checked_add(value).ok_or_else(|| {
                                    resource("source joint winding count overflow")
                                })?
                            }
                            None => {
                                resolved = false;
                                break;
                            }
                        }
                    }
                }
                if !resolved {
                    break;
                }
                if loop_index == 0 {
                    outer = winding != 0;
                } else {
                    hole |= winding != 0;
                }
            }
            if resolved {
                return Ok(if outer && !hole {
                    TrimClassification::Inside
                } else {
                    TrimClassification::Outside
                });
            }
        }
        Ok(TrimClassification::Boundary)
    }

    fn boundary_winding(
        &self,
        boundary: BoundaryCurve,
        uv_box: [[f64; 2]; 2],
        point: [f64; 2],
        shear: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<i64>, GeometryError> {
        let before = work.consumed();
        let mut active = ([0.0; 2], 0_u8);
        let mut last_split = None;
        let outcome = (|| {
            // Winding depends on the complete oriented path, not its calibrated
            // native/spatial speed. Never pass these parameters to physical G or a
            // correspondence consumer.
            let curve = match boundary {
                BoundaryCurve::Coedge(index) => {
                    self.coedge_geometric_operand(index, work, cancel)?
                }
                BoundaryCurve::Joint(_) => self.boundary_operand(boundary)?,
            };
            let (mut range, forward) = self.boundary_restriction(boundary);
            let geometric = matches!(curve, PreparedTrimCurve::GeometricPcurve(_));
            if geometric {
                range = curve.active_range();
            }
            let mut total = 0i64;
            let mut stack = [([0.0; 2], 0u8); MAX_DEPTH as usize + 1];
            stack[0] = (range, 0);
            let mut live = 1;
            while live != 0 {
                live -= 1;
                let (part, depth) = stack[live];
                active = (part, depth);
                work.charge(1, cancel)?;
                let whole = match boundary {
                    BoundaryCurve::Coedge(index) if !geometric => {
                        self.boundary_piecewise_enclosure(index, part, work, cancel)?
                    }
                    _ => parametric::curve2d_piecewise_jet(&curve, part, work, cancel)?,
                };
                let mut image = whole.position;
                if curve.native_curve().is_some()
                    && !(0..2).any(|a| image[a].hi < uv_box[a][0] || image[a].lo > uv_box[a][1])
                    && parametric::curve_parameter_span_continuous(&curve, part, work, cancel)?
                {
                    // Original operators are tightened by the shared source jet.
                    // Preserve the native fallback: both bounds enclose the same
                    // C0 piecewise curve, including its finite derivative jumps.
                    let middle = part[0] * 0.5 + part[1] * 0.5;
                    let midpoint =
                        parametric::curve2d_piecewise_jet(&curve, [middle; 2], work, cancel)?
                            .position;
                    let displacement =
                        Interval::new(part[0], part[1])?.sub(Interval::point(middle));
                    for axis in 0..2 {
                        image[axis] = image[axis].intersect(
                            midpoint[axis].add(displacement.mul(whole.derivative[axis])),
                        )?;
                    }
                }
                let disjoint =
                    (0..2).any(|a| image[a].hi < uv_box[a][0] || image[a].lo > uv_box[a][1]);
                if disjoint {
                    let a = parametric::curve2d_piecewise_jet(&curve, [part[0]; 2], work, cancel)?
                        .position;
                    let b = parametric::curve2d_piecewise_jet(&curve, [part[1]; 2], work, cancel)?
                        .position;
                    if let Some(delta) = chord_winding(a, b, point, shear) {
                        total = total
                            .checked_add(delta)
                            .ok_or_else(|| resource("native trim winding count overflow"))?;
                        continue;
                    }
                }
                last_split = Some((part, depth, image, disjoint));
                if depth == MAX_DEPTH {
                    return Ok(None);
                }
                let middle = part[0] * 0.5 + part[1] * 0.5;
                if middle <= part[0] || middle >= part[1] {
                    return Ok(None);
                }
                stack[live] = ([middle, part[1]], depth + 1);
                stack[live + 1] = ([part[0], middle], depth + 1);
                live += 2;
            }
            Ok(Some(if forward { total } else { -total }))
        })();
        outcome.map_err(|error| work.annotate_error(error, before, format_args!(
            "original trim winding boundary={boundary:?} shear={shear} active={active:?} last_split={last_split:?} query={uv_box:?}",
        )))
    }

    fn opposite_endpoint_germs(
        a: &parametric::ChartCurveEnclosure,
        sa: f64,
        b: &parametric::ChartCurveEnclosure,
        sb: f64,
    ) -> bool {
        [[1.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, -1.0]]
            .iter()
            .any(|normal| {
                let projected = |jet: &parametric::ChartCurveEnclosure, sign| {
                    jet.derivative[0]
                        .mul(Interval::point(normal[0]))
                        .add(jet.derivative[1].mul(Interval::point(normal[1])))
                        .mul(Interval::point(sign))
                };
                let x = projected(a, sa);
                let y = projected(b, sb);
                (x.lo > 0.0 && y.hi < 0.0) || (x.hi < 0.0 && y.lo > 0.0)
            })
    }

    fn parent_vertex_separates(
        &self,
        a: TrimSpan,
        b: TrimSpan,
        current_a: &parametric::ChartCurveEnclosure,
        current_b: &parametric::ChartCurveEnclosure,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        if a.boundary == b.boundary {
            return Ok(false);
        }
        let ar = self.boundary_restriction(a.boundary).0;
        let br = self.boundary_restriction(b.boundary).0;
        let ac = self.boundary_operand(a.boundary)?;
        let bc = self.boundary_operand(b.boundary)?;
        for ae in 0..2 {
            for be in 0..2 {
                if self.boundary_endpoint(a.boundary, ae) != self.boundary_endpoint(b.boundary, be)
                {
                    continue;
                }
                // The exact C0 anchor belongs to the complete boundary, not
                // necessarily this knot/cut leaf. Certify every derivative
                // between the leaf and anchor before applying monotonicity.
                let ah = [a.range[0].min(ar[ae]), a.range[1].max(ar[ae])];
                let bh = [b.range[0].min(br[be]), b.range[1].max(br[be])];
                let a_extension;
                let aj = if ah == a.range {
                    current_a
                } else {
                    a_extension = parametric::curve2d_piecewise_jet(&ac, ah, work, cancel)?;
                    &a_extension
                };
                let b_extension;
                let bj = if bh == b.range {
                    current_b
                } else {
                    b_extension = parametric::curve2d_piecewise_jet(&bc, bh, work, cancel)?;
                    &b_extension
                };
                if Self::opposite_endpoint_germs(
                    aj,
                    if ae == 0 { 1.0 } else { -1.0 },
                    bj,
                    if be == 0 { 1.0 } else { -1.0 },
                ) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn owned_joint_overlap(
        &self,
        a: BoundaryCurve,
        b: BoundaryCurve,
        overlap: [Interval; 2],
    ) -> bool {
        for ae in 0..2 {
            let BoundaryEndpoint::Joint(joint, _) = self.boundary_endpoint(a, ae) else {
                continue;
            };
            for be in 0..2 {
                if !matches!(self.boundary_endpoint(b, be), BoundaryEndpoint::Joint(other, _) if other == joint)
                {
                    continue;
                }
                let enclosure = self.original_joints[joint].enclosure;
                if (0..2).all(|axis| {
                    enclosure[axis].lo <= overlap[axis].lo && overlap[axis].hi <= enclosure[axis].hi
                }) {
                    return true;
                }
            }
        }
        false
    }

    fn owned_joint_germs_separate(
        &self,
        a: TrimSpan,
        b: TrimSpan,
        aj: &parametric::ChartCurveEnclosure,
        bj: &parametric::ChartCurveEnclosure,
        mut overlap: [Interval; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let shared = (0..2).any(|ae| {
            let BoundaryEndpoint::Joint(joint, _) = self.boundary_endpoint(a.boundary, ae) else {
                return false;
            };
            (0..2).any(|be| {
                matches!(self.boundary_endpoint(b.boundary, be), BoundaryEndpoint::Joint(other, _) if other == joint)
            })
        });
        if !shared {
            return Ok(false);
        }
        let ac = self.boundary_operand(a.boundary)?;
        let bc = self.boundary_operand(b.boundary)?;
        for axis in 0..2 {
            if aj.derivative[axis].contains_zero() || bj.derivative[axis].contains_zero() {
                continue;
            }
            // Any actual common point lies in `overlap`. On a complete C0
            // piece, its parameter must therefore satisfy this mean-value
            // contraction. Retain the full endpoint uncertainty; no equality
            // or material ownership follows from a successful contraction.
            for (curve, range, derivative) in [
                (&ac, a.range, aj.derivative[axis]),
                (&bc, b.range, bj.derivative[axis]),
            ] {
                let middle = range[0] * 0.5 + range[1] * 0.5;
                let center = curve.tensor([middle; 2], 0, work, cancel)?[0][axis];
                let parameter =
                    Interval::point(middle).add(overlap[axis].sub(center).div(derivative)?);
                let Some(parameter) = Interval::new(range[0], range[1])?.intersection(parameter)
                else {
                    return Ok(true);
                };
                let position = curve.tensor([parameter.lo, parameter.hi], 0, work, cancel)?[0];
                for coordinate in 0..2 {
                    let Some(intersection) = overlap[coordinate].intersection(position[coordinate])
                    else {
                        return Ok(true);
                    };
                    overlap[coordinate] = intersection;
                }
            }
            if self.owned_joint_overlap(a.boundary, b.boundary, overlap) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn separate_spans(
        &self,
        a: TrimSpan,
        b: TrimSpan,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        let mut stack = [(a, b, 0u8); MAX_DEPTH as usize + 1];
        let mut live = 1;
        while live != 0 {
            live -= 1;
            let (a, b, depth) = stack[live];
            work.charge(1, cancel)?;
            let ac = self.boundary_operand(a.boundary)?;
            let bc = self.boundary_operand(b.boundary)?;
            let mut aj = parametric::curve2d_piecewise_jet(&ac, a.range, work, cancel)?;
            let mut bj = parametric::curve2d_piecewise_jet(&bc, b.range, work, cancel)?;
            // A joint's complete uncertainty cell is excluded from positive
            // classification. Keep foreign boundaries out of that whole cell,
            // not merely out of one affine connector realization.
            if let BoundaryCurve::Joint(joint) = a.boundary {
                aj.position = self.original_joints[joint].enclosure;
            }
            if let BoundaryCurve::Joint(joint) = b.boundary {
                bj.position = self.original_joints[joint].enclosure;
            }
            if (0..2).any(|i| {
                aj.position[i].hi < bj.position[i].lo || bj.position[i].hi < aj.position[i].lo
            }) {
                continue;
            }
            let overlap = [
                aj.position[0].intersect(bj.position[0])?,
                aj.position[1].intersect(bj.position[1])?,
            ];
            // Only the actual incident source uses may remain unresolved,
            // wholly inside their own already-masked joint cell. This neither
            // welds endpoints nor supplies the later material vertex receipt.
            if self.owned_joint_overlap(a.boundary, b.boundary, overlap)
                || self.owned_joint_germs_separate(a, b, &aj, &bj, overlap, work, cancel)?
            {
                continue;
            }
            if let Some([ae, be]) = self.common_endpoint(a, b) {
                // A linear functional is strictly increasing away from the
                // common endpoint on one arc and decreasing on the other.
                // Exact C0 was proved before this test; no endpoint tube/weld.
                let sa = if ae == 0 { 1.0 } else { -1.0 };
                let sb = if be == 0 { 1.0 } else { -1.0 };
                let separated = Self::opposite_endpoint_germs(&aj, sa, &bj, sb);
                if separated {
                    continue;
                }
            }
            if self.parent_vertex_separates(a, b, &aj, &bj, work, cancel)? {
                continue;
            }
            if depth == MAX_DEPTH {
                return Err(unavailable(format!(
                    "original chart pair separation unproved at bounded subdivision depth: source_face={} a={:?} parameter={:?} position={:?} b={:?} parameter={:?} position={:?}",
                    self.face_index,
                    a.boundary,
                    a.range,
                    aj.position,
                    b.boundary,
                    b.range,
                    bj.position,
                )));
            }
            let aw = a.range[1] - a.range[0];
            let bw = b.range[1] - b.range[0];
            if aw >= bw {
                let middle = split(
                    a.range,
                    depth,
                    "native chart nonincident/incident arc separation",
                )?;
                let (left, right) = split_span(a, middle);
                stack[live] = (left, b, depth + 1);
                stack[live + 1] = (right, b, depth + 1);
            } else {
                let middle = split(
                    b.range,
                    depth,
                    "native chart nonincident/incident arc separation",
                )?;
                let (left, right) = split_span(b, middle);
                stack[live] = (a, left, depth + 1);
                stack[live + 1] = (a, right, depth + 1);
            }
            live += 2;
        }
        Ok(())
    }

    fn common_endpoint(&self, a: TrimSpan, b: TrimSpan) -> Option<[usize; 2]> {
        if a.boundary == b.boundary {
            if a.range[1] == b.range[0] {
                return Some([1, 0]);
            }
            if b.range[1] == a.range[0] {
                return Some([0, 1]);
            }
        }
        for ae in 0..2 {
            for be in 0..2 {
                if a.ends[ae]
                    && b.ends[be]
                    && self.boundary_endpoint(a.boundary, ae)
                        == self.boundary_endpoint(b.boundary, be)
                {
                    return Some([ae, be]);
                }
            }
        }
        None
    }
}

// A closed curve leaf and its endpoint chord are homotopic in their common
// enclosing rectangle. Once that rectangle misses the WHOLE query box, replacing
// the leaf by this chord preserves winding at every point of the query box.
fn chord_winding(a: [Interval; 2], b: [Interval; 2], p: [f64; 2], shear: f64) -> Option<i64> {
    let relative = |q: [Interval; 2]| {
        [
            q[0].sub(Interval::point(p[0])),
            q[1].sub(Interval::point(p[1])),
        ]
    };
    let a = relative(a);
    let b = relative(b);
    let ay = a[1].add(a[0].mul(Interval::point(shear)));
    let by = b[1].add(b[0].mul(Interval::point(shear)));
    if (ay.lo > 0.0 && by.lo > 0.0) || (ay.hi < 0.0 && by.hi < 0.0) {
        return Some(0);
    }
    let orientation = a[0].mul(b[1]).sub(a[1].mul(b[0]));
    if ay.hi < 0.0 && by.lo > 0.0 {
        if orientation.lo > 0.0 {
            Some(1)
        } else if orientation.hi < 0.0 {
            Some(0)
        } else {
            None
        }
    } else if by.hi < 0.0 && ay.lo > 0.0 {
        if orientation.hi < 0.0 {
            Some(-1)
        } else if orientation.lo > 0.0 {
            Some(0)
        } else {
            None
        }
    } else {
        None
    }
}

struct HomogeneousEndpoint {
    xy: [ExactCoefficient; 2],
    weight: f64,
}

/// Endpoint expressions, not point evaluations with a guessed equality epsilon.
fn exact_endpoint(
    curve: &Curve,
    end: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<HomogeneousEndpoint, GeometryError> {
    work.charge(1, cancel)?;
    if let CurveKind::Line(line) = curve.kind() {
        let c = line.canonical_unit();
        let t = if line.is_bounded() {
            if (end == 0) != c.inside_out {
                c.domain.min()
            } else {
                c.domain.max()
            }
        } else {
            let domain = curve.natural_interval();
            let t = if end == 0 { domain.min() } else { domain.max() };
            if c.inside_out { -t } else { t }
        };
        let point = [c.point.x, c.point.y];
        let vector = [c.vector.x, c.vector.y];
        let mut xy = std::array::from_fn(|_| ExactCoefficient::point(0.0));
        for axis in 0..2 {
            for v in [point[axis], vector[axis], c.scale, t] {
                Interval::new(v, v)?;
            }
            let velocity = exact_product(
                &ExactCoefficient::point(vector[axis]),
                &ExactCoefficient::point(c.scale),
                work,
                cancel,
            )?;
            xy[axis] = exact_product(&velocity, &ExactCoefficient::point(t), work, cancel)?;
            xy[axis].push(point[axis]).ok_or_else(|| {
                unavailable("native line endpoint exceeds bounded exact arithmetic")
            })?;
        }
        return Ok(HomogeneousEndpoint { xy, weight: 1.0 });
    }
    let base = match curve.kind() {
        CurveKind::BSpline(b) => b,
        CurveKind::Ellipse(e) => e.base(),
        CurveKind::Hyperbola(h) => h.base(),
        CurveKind::Parabola(p) => p.base(),
        _ => {
            return Err(unavailable(
                "native chart endpoint kind has no exact coefficient identity operation",
            ));
        }
    };
    let n = base.gw_nurb().ok_or_else(|| {
        unavailable("native chart endpoint has no retained homogeneous coefficients")
    })?;
    let points = n.cpts();
    let knots = n.knots();
    let degree = n.degree();
    if points.is_empty() || degree >= points.len() || knots.len() != points.len() + degree + 1 {
        return Err(unavailable(
            "native chart endpoint has an invalid original coefficient layout",
        ));
    }
    let t = if end == 0 {
        knots[degree]
    } else {
        knots[points.len()]
    };
    let clamped = if end == 0 {
        knots[..=degree].iter().all(|&k| k == t)
    } else {
        knots[points.len()..].iter().all(|&k| k == t)
    };
    if !clamped {
        return Err(unavailable(
            "unclamped native chart endpoint needs an exact original basis-expression identity; first/last control points are not endpoint evidence",
        ));
    }
    let p = if end == 0 {
        points[0]
    } else {
        points[points.len() - 1]
    };
    let weight = if n.is_rational() { p.w } else { 1.0 };
    for v in [p.x, p.y, weight] {
        Interval::new(v, v)?;
    }
    if weight == 0.0 {
        return Err(unavailable(
            "native chart homogeneous endpoint has zero weight",
        ));
    }
    Ok(HomogeneousEndpoint {
        xy: [ExactCoefficient::point(p.x), ExactCoefficient::point(p.y)],
        weight,
    })
}
fn same_endpoint(
    a: &HomogeneousEndpoint,
    b: &HomogeneousEndpoint,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    for axis in 0..2 {
        let mut difference = exact_product(
            &a.xy[axis],
            &ExactCoefficient::point(b.weight),
            work,
            cancel,
        )?;
        let other = exact_product(
            &b.xy[axis],
            &ExactCoefficient::point(a.weight),
            work,
            cancel,
        )?;
        difference.subtract(&other).ok_or_else(|| {
            unavailable("native endpoint comparison exceeds bounded exact arithmetic")
        })?;
        if difference.sign() != 0 {
            return Ok(false);
        }
    }
    Ok(true)
}
fn exact_product(
    a: &ExactCoefficient,
    b: &ExactCoefficient,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<ExactCoefficient, GeometryError> {
    a.product(b, work, cancel)?.ok_or_else(|| {
        unavailable("native endpoint coefficient product exceeds exact exponent/expansion limits")
    })
}
fn split_span(span: TrimSpan, middle: f64) -> (TrimSpan, TrimSpan) {
    (
        TrimSpan {
            range: [span.range[0], middle],
            ends: [span.ends[0], false],
            ..span
        },
        TrimSpan {
            range: [middle, span.range[1]],
            ends: [false, span.ends[1]],
            ..span
        },
    )
}
fn split(range: [f64; 2], depth: u8, owner: &str) -> Result<f64, GeometryError> {
    if depth == MAX_DEPTH {
        return Err(unavailable(format!(
            "{owner} unproved at bounded subdivision depth"
        )));
    }
    let middle = range[0] * 0.5 + range[1] * 0.5;
    if middle <= range[0] || middle >= range[1] {
        return Err(unavailable(format!(
            "{owner} exhausted representable parameter subdivision"
        )));
    }
    Ok(middle)
}
fn reserve_span(
    domain: &mut PreparedFaceTrimDomain<'_>,
    allowance: usize,
) -> Result<(), GeometryError> {
    if domain.spans.len() == domain.spans.capacity() {
        let additional = domain.spans.capacity().max(1);
        let requested = size_of::<PreparedFaceTrimDomain<'_>>()
            + domain.owned_bytes()
            + checked_bytes::<TrimSpan>(additional)?;
        memory(requested, allowance)?;
        domain
            .spans
            .try_reserve_exact(additional)
            .map_err(|_| resource("native trim span allocation failed"))?;
        memory(domain.retained_bytes(), allowance)?;
    }
    Ok(())
}
fn checked_bytes<T>(count: usize) -> Result<usize, GeometryError> {
    count
        .checked_mul(size_of::<T>())
        .ok_or_else(|| resource("native trim byte count overflow"))
}
fn memory(retained: usize, allowance: usize) -> Result<(), GeometryError> {
    if retained
        .checked_add(SCRATCH_BYTES)
        .is_none_or(|n| n > allowance)
    {
        return Err(resource(
            "native trim retained and fixed scratch bytes exceed the live allocation allowance",
        ));
    }
    Ok(())
}
fn unavailable(message: impl AsRef<str>) -> GeometryError {
    GeometryError::new(
        GeometryErrorCode::ParameterCorrespondenceUnavailable,
        message,
    )
}
fn resource(message: impl AsRef<str>) -> GeometryError {
    GeometryError::new(GeometryErrorCode::ResourceLimit, message)
}
fn native_error(error: solidcore::SolidError) -> GeometryError {
    unavailable(format!("native trim topology: {error}"))
}

/// Coefficient straightness and whole-restriction nonzero tangent.
/// No endpoint fit, sampled collinearity or small residual can return true.
pub(crate) fn certified_curve2d_is_straight_regular(
    curve: &(impl parametric::CurveEvaluator + ?Sized),
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(1, cancel)?;
    let range = [range[0].min(range[1]), range[0].max(range[1])];
    Interval::new(range[0], range[1])?;
    if range[0] == range[1] {
        return Ok(false);
    }
    if !parametric::curve_parameter_span_smooth(curve, range, work, cancel)? {
        return Ok(false);
    }
    if let Some(affine) = curve.exact_affine_coefficients(work, cancel)? {
        if affine.denominator.sign() == 0 {
            return Err(unavailable(
                "exact affine curve has a zero homogeneous denominator",
            ));
        }
        return Ok(affine.derivative[0].sign() != 0 || affine.derivative[1].sign() != 0);
    }
    if curve.native_curve().is_none() && curve.original_curve().is_none() {
        // A derived analytic chart need not have rational exact-affine
        // coefficients (its constant angle can be an isolated root). An
        // identically zero whole-range second jet still proves straightness;
        // smoothness above and a strict first jet prove regularity.
        let jet = parametric::curve2d_piecewise_jet(curve, range, work, cancel)?;
        if jet.second_derivative == [Interval::point(0.0); 2]
            && jet.derivative.iter().any(|d| d.lo > 0.0 || d.hi < 0.0)
        {
            return Ok(true);
        }
    }
    if let Some(native) = curve.native_curve() {
        match native.kind() {
            CurveKind::Line(_) => {}
            CurveKind::BSpline(base) => {
                let n = base.gw_nurb().ok_or_else(|| {
                    unavailable("native straightness lacks original coefficients")
                })?;
                let mut first = None;
                let mut second = None;
                let knots = n.knots();
                let degree = n.degree();
                for index in degree..n.cpts().len() {
                    work.charge(1, cancel)?;
                    if knots[index] == knots[index + 1]
                        || knots[index] >= range[1]
                        || knots[index + 1] <= range[0]
                    {
                        continue;
                    }
                    for p in &n.cpts()[index - degree..=index] {
                        work.charge(1, cancel)?;
                        let p = [p.x, p.y, if n.is_rational() { p.w } else { 1.0 }];
                        for v in p {
                            Interval::new(v, v)?;
                        }
                        if p[2] == 0.0 {
                            return Ok(false);
                        }
                        let Some(a) = first else {
                            first = Some(p);
                            continue;
                        };
                        let Some(b) = second else {
                            let point = |q: [f64; 3]| HomogeneousEndpoint {
                                xy: [ExactCoefficient::point(q[0]), ExactCoefficient::point(q[1])],
                                weight: q[2],
                            };
                            if !same_endpoint(&point(a), &point(p), work, cancel)? {
                                second = Some(p);
                            }
                            continue;
                        };
                        if !homogeneous_collinear(a, b, p, work, cancel)? {
                            return Ok(false);
                        }
                    }
                }
                if second.is_none() {
                    return Ok(false);
                }
            }
            _ => return Ok(false),
        }
    } else {
        let Some(mut original) = curve.original_curve() else {
            return Ok(false);
        };
        for _ in 0..16 {
            match original {
                solidcraft_step::geom::Curve::Trimmed { basis, .. } => original = basis,
                _ => break,
            }
        }
        match original {
            // Literal coefficient straightness is invariant under a monotone
            // parameter change, even when opaque endpoint roots prevent exact
            // affine coefficients in the query gauge. The complete strict
            // first-jet proof below is still required; this proves no endpoint
            // coordinate identity or literal port-span inclusion.
            solidcraft_step::geom::Curve::Line { .. } => {}
            solidcraft_step::geom::Curve::BSpline(spline) => {
                let Some(first) = spline.points.first() else {
                    return Ok(false);
                };
                let Some(second) = spline
                    .points
                    .iter()
                    .find(|point| point[0] != first[0] || point[1] != first[1])
                else {
                    return Ok(false);
                };
                work.charge(spline.points.len() as u64, cancel)?;
                for point in &spline.points {
                    if !homogeneous_collinear(
                        [first[0], first[1], 1.0],
                        [second[0], second[1], 1.0],
                        [point[0], point[1], 1.0],
                        work,
                        cancel,
                    )? {
                        return Ok(false);
                    }
                }
            }
            _ => return Ok(false),
        }
    }
    // A straight coefficient image can still stop or reverse. Prove a nonzero
    // coordinate derivative on every closed leaf, retaining original C1 above.
    let mut stack = [([0.0; 2], 0u8); MAX_DEPTH as usize + 1];
    stack[0] = (range, 0);
    let mut live = 1;
    while live != 0 {
        live -= 1;
        let (part, depth) = stack[live];
        work.charge(1, cancel)?;
        let jet = parametric::curve2d_piecewise_jet(curve, part, work, cancel)?;
        if jet.derivative.iter().any(|d| d.lo > 0.0 || d.hi < 0.0) {
            continue;
        }
        if depth == MAX_DEPTH {
            return Ok(false);
        }
        let middle = part[0] * 0.5 + part[1] * 0.5;
        if middle <= part[0] || middle >= part[1] {
            return Ok(false);
        }
        stack[live] = ([middle, part[1]], depth + 1);
        stack[live + 1] = ([part[0], middle], depth + 1);
        live += 2;
    }
    Ok(true)
}

fn homogeneous_collinear(
    a: [f64; 3],
    b: [f64; 3],
    c: [f64; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let mut determinant = ExactCoefficient::point(0.0);
    for axis in 0..3 {
        let i = (axis + 1) % 3;
        let j = (axis + 2) % 3;
        let mut minor = exact_product(
            &ExactCoefficient::point(b[i]),
            &ExactCoefficient::point(c[j]),
            work,
            cancel,
        )?;
        let other = exact_product(
            &ExactCoefficient::point(b[j]),
            &ExactCoefficient::point(c[i]),
            work,
            cancel,
        )?;
        minor
            .subtract(&other)
            .ok_or_else(|| unavailable("native straightness exceeds exact expansion capacity"))?;
        let term = exact_product(&ExactCoefficient::point(a[axis]), &minor, work, cancel)?;
        for value in &term.terms[..term.len] {
            determinant.push(*value).ok_or_else(|| {
                unavailable("native straightness exceeds exact expansion capacity")
            })?;
        }
    }
    Ok(determinant.sign() == 0)
}

/// Native incidence and original genealogy bound by a completed forward proof.
/// Native interval numbers are never used as original source parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OriginalCoedgeGenealogy {
    pub(crate) source_index: usize,
    pub(crate) source_edge: Option<u64>,
    pub(crate) source_coedge: Option<u64>,
    pub(crate) native_orientation: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct OriginalCoedgeBinding {
    pub(crate) face: FaceId,
    pub(crate) edge: EdgeId,
    pub(crate) edgeuse: EdgeuseId,
    pub(crate) endpoints: [VertexId; 2],
    pub(crate) native_interval: [f64; 2],
    pub(crate) owner: [usize; 3],
    pub(crate) source_face: Option<u64>,
    pub(crate) source_face_use: Option<u64>,
    pub(crate) source: OriginalCoedgeGenealogy,
    /// Increasing source spatial parameter follows native vertices[0] -> [1].
    pub(crate) spatial_forward: bool,
    /// Increasing selected spatial parameter is material boundary traversal.
    pub(crate) material_forward: bool,
    /// Captured ORIGINAL source vertices, in canonical native endpoint order.
    pub(crate) source_vertex_points_mm: [[f64; 3]; 2],
    pub(crate) source_vertex_ids: [spiling_contracts::geometry::StepEntityId; 2],
}
impl OriginalCoedgeBinding {
    pub(crate) fn spatial_endpoints(&self) -> [VertexId; 2] {
        if self.spatial_forward {
            self.endpoints
        } else {
            [self.endpoints[1], self.endpoints[0]]
        }
    }
    pub(crate) fn native_endpoint_index(&self, spatial_high: bool) -> usize {
        usize::from(spatial_high ^ !self.spatial_forward)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OriginalCorrespondenceProvenance {
    ExactCoefficients,
    Forward,
    CompletedPoleGraph,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct OriginalSpanInventory {
    pub(crate) source_knot_boundaries: usize,
    pub(crate) source_knot_bands: usize,
    pub(crate) uv_knot_boundaries: usize,
    pub(crate) uv_knot_bands: usize,
}

/// Sealed whole-original trace receipt. Construction evaluates the actual
/// selected source operators; candidate IDs/ranges cannot construct this type.
pub(crate) struct OriginalCoedgeCorrespondence {
    binding: OriginalCoedgeBinding,
    spatial_range: [f64; 2],
    uv_ends: [f64; 2],
    position_error_mm: f64,
    relative_derivative_error_bound: f64,
    inventory: OriginalSpanInventory,
    provenance: OriginalCorrespondenceProvenance,
    margin_uv: [f64; 2],
}
impl OriginalCoedgeCorrespondence {
    pub(crate) fn binding(&self) -> Option<&OriginalCoedgeBinding> {
        Some(&self.binding)
    }
    pub(crate) fn spatial_range(&self) -> [f64; 2] {
        self.spatial_range
    }
    pub(crate) fn uv_range(&self) -> [f64; 2] {
        [
            self.uv_ends[0].min(self.uv_ends[1]),
            self.uv_ends[0].max(self.uv_ends[1]),
        ]
    }
    pub(crate) fn source_index(&self) -> usize {
        self.binding.source.source_index
    }
    pub(crate) fn provenance(&self) -> OriginalCorrespondenceProvenance {
        self.provenance
    }
    pub(crate) fn position_error_mm(&self) -> f64 {
        self.position_error_mm
    }
    pub(crate) fn relative_derivative_error_bound(&self) -> f64 {
        self.relative_derivative_error_bound
    }
    // Query arithmetic is enclosed in jets. No artificial UV uncertainty is
    // added to the literal selected PCurve's coefficient expression.
    pub(crate) fn margin_uv(&self) -> [f64; 2] {
        self.margin_uv
    }
    pub(crate) fn span_inventory(&self) -> OriginalSpanInventory {
        self.inventory
    }
    pub(crate) fn uv_endpoint(&self, spatial_high: bool) -> f64 {
        self.uv_ends[usize::from(spatial_high)]
    }
    pub(crate) fn uv_endpoints(&self) -> [f64; 2] {
        self.uv_ends
    }
    pub(crate) fn spatial_endpoint(&self, native_back: bool) -> f64 {
        self.spatial_range[usize::from(native_back == self.binding.spatial_forward)]
    }
    pub(crate) fn uv_parameter_derivative(&self) -> Interval {
        // Positive spatial length and nonzero UV length were proved before the
        // sealed receipt was constructed.
        Interval::point(self.uv_ends[1])
            .sub(Interval::point(self.uv_ends[0]))
            .div(Interval::point(self.spatial_range[1]).sub(Interval::point(self.spatial_range[0])))
            .expect("qualified source affine parameter denominator")
    }
    pub(crate) fn uv_parameter_image(&self, range: [f64; 2]) -> Result<Interval, GeometryError> {
        Interval::new(range[0], range[1])?;
        if range[0] < self.spatial_range[0] || range[1] > self.spatial_range[1] {
            return Err(unavailable(
                "original source parameter query leaves its qualified spatial restriction",
            ));
        }
        let image = Interval::point(self.uv_ends[0]).add(
            Interval::new(range[0], range[1])?
                .sub(Interval::point(self.spatial_range[0]))
                .mul(self.uv_parameter_derivative()),
        );
        let domain = self.uv_range();
        // The exact affine expression is a convex combination of its literal
        // endpoint parameters on this restriction; intersection is proved.
        Interval::new(image.lo.max(domain[0]), image.hi.min(domain[1]))
    }
    pub(crate) fn affine_spatial_parameter_preimage(
        &self,
        range: [f64; 2],
    ) -> Result<Option<Interval>, GeometryError> {
        let image = Interval::new(range[0], range[1])?
            .sub(Interval::point(self.uv_ends[0]))
            .div(self.uv_parameter_derivative())?
            .add(Interval::point(self.spatial_range[0]));
        Ok(Some(image))
    }
    pub(crate) fn same_affine_spatial_uv_map(&self, other: &Self) -> bool {
        self.spatial_range == other.spatial_range
            && self.uv_ends == other.uv_ends
            && self.binding.source.source_edge == other.binding.source.source_edge
    }
}

fn lift_original_chart(
    surface: &dyn parametric::SurfaceEvaluator,
    chart: &dyn parametric::CurveEvaluator,
    spatial_range: [f64; 2],
    uv_ends: [f64; 2],
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(parametric::CurveEnclosure, bool), GeometryError> {
    let slope = Interval::point(uv_ends[1])
        .sub(Interval::point(uv_ends[0]))
        .div(Interval::point(spatial_range[1]).sub(Interval::point(spatial_range[0])))?;
    let image = Interval::point(uv_ends[0]).add(
        Interval::new(range[0], range[1])?
            .sub(Interval::point(spatial_range[0]))
            .mul(slope),
    );
    let chart_range = [
        image.lo.max(uv_ends[0].min(uv_ends[1])),
        image.hi.min(uv_ends[0].max(uv_ends[1])),
    ];
    let gamma = parametric::curve2d_piecewise_jet(chart, chart_range, work, cancel)?;
    let uv = gamma.position.map(|v| [v.lo, v.hi]);
    if !chart.continuous(chart_range, work, cancel)? || !surface.continuous(uv, work, cancel)? {
        return Err(unavailable(
            "original source lift has no whole-restriction C0 qualification",
        ));
    }
    let f = parametric::surface_piecewise_enclosure(surface, uv, work, cancel)?;
    let d = gamma.derivative.map(|v| v.mul(slope));
    let dd = gamma.second_derivative.map(|v| v.mul(slope.square()));
    let derivative = std::array::from_fn(|axis| f.du[axis].mul(d[0]).add(f.dv[axis].mul(d[1])));
    let second_derivative = std::array::from_fn(|axis| {
        f.duu[axis]
            .mul(d[0].square())
            .add(Interval::point(2.0).mul(f.duv[axis]).mul(d[0]).mul(d[1]))
            .add(f.dvv[axis].mul(d[1].square()))
            .add(f.du[axis].mul(dd[0]))
            .add(f.dv[axis].mul(dd[1]))
    });
    let smooth = chart.smooth(chart_range, work, cancel)? && surface.smooth(uv, work, cancel)?;
    Ok((
        parametric::CurveEnclosure {
            position: f.position,
            derivative,
            second_derivative,
        },
        smooth,
    ))
}

/// Qualify a proposed restriction without changing any original coefficient.
/// A declined candidate is not a certificate and cannot select a source branch.
fn qualify_original_correspondence(
    binding: OriginalCoedgeBinding,
    spatial: &dyn parametric::CurveEvaluator,
    surface: &dyn parametric::SurfaceEvaluator,
    chart: &dyn parametric::CurveEvaluator,
    spatial_range: [f64; 2],
    uv_ends: [f64; 2],
    maximum_error_mm: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<OriginalCoedgeCorrespondence>, GeometryError> {
    work.charge(1, cancel)?;
    Interval::new(spatial_range[0], spatial_range[1])?;
    for value in uv_ends {
        Interval::new(value, value)?;
    }
    if spatial_range[0] >= spatial_range[1]
        || uv_ends[0] == uv_ends[1]
        || !maximum_error_mm.is_finite()
        || maximum_error_mm < 0.0
    {
        return Err(unavailable(
            "original correspondence has an invalid proposed parameter restriction or allowance",
        ));
    }
    if !spatial.continuous(spatial_range, work, cancel)? {
        return Ok(None);
    }
    let mut stack = [([0.0; 2], 0u8); MAX_DEPTH as usize + 1];
    stack[0] = (spatial_range, 0);
    let mut live = 1;
    let mut maximum_position = 0.0_f64;
    let mut maximum_relative = 0.0_f64;
    let norm = |v: [Interval; 3]| {
        v.into_iter()
            .fold(Interval::point(0.0), |s, v| s.add(v.square()))
            .sqrt()
    };
    while live != 0 {
        live -= 1;
        let (part, depth) = stack[live];
        work.charge(1, cancel)?;
        let c = parametric::curve_enclosure(spatial, (part[0], part[1]), work, cancel)?;
        let (b, smooth) =
            lift_original_chart(surface, chart, spatial_range, uv_ends, part, work, cancel)?;
        let middle = part[0] * 0.5 + part[1] * 0.5;
        let c0 = parametric::curve_enclosure(spatial, (middle, middle), work, cancel)?;
        let (b0, _) = lift_original_chart(
            surface,
            chart,
            spatial_range,
            uv_ends,
            [middle; 2],
            work,
            cancel,
        )?;
        let delta = Interval::new(part[0], part[1])?.sub(Interval::point(middle));
        let first: [Interval; 3] = std::array::from_fn(|i| b.derivative[i].sub(c.derivative[i]));
        let residual = if smooth && spatial.smooth(part, work, cancel)? {
            std::array::from_fn(|i| {
                b0.position[i]
                    .sub(c0.position[i])
                    .add(b0.derivative[i].sub(c0.derivative[i]).mul(delta))
                    .add(
                        b.second_derivative[i]
                            .sub(c.second_derivative[i])
                            .mul(delta.square())
                            .mul(Interval::point(0.5)),
                    )
            })
        } else {
            // C0 plus complete one-sided first bounds is Lipschitz authority,
            // not C1/Taylor authority.
            std::array::from_fn(|i| b0.position[i].sub(c0.position[i]).add(first[i].mul(delta)))
        };
        if norm(std::array::from_fn(|i| b0.position[i].sub(c0.position[i])))?.lo > maximum_error_mm
        {
            return Ok(None);
        }
        let position = norm(residual)?.hi;
        let speed = norm(c.derivative)?.lo;
        let relative = if speed > 0.0 {
            Interval::point(norm(first)?.hi)
                .div(Interval::point(speed))?
                .hi
        } else {
            f64::INFINITY
        };
        if position <= maximum_error_mm && relative < 0.25 {
            maximum_position = maximum_position.max(position);
            maximum_relative = maximum_relative.max(relative);
            continue;
        }
        if depth == MAX_DEPTH || middle <= part[0] || middle >= part[1] {
            return Ok(None);
        }
        stack[live] = ([middle, part[1]], depth + 1);
        stack[live + 1] = ([part[0], middle], depth + 1);
        live += 2;
    }
    let source_pieces = spatial
        .parameter_spans(spatial_range, work, cancel)?
        .count();
    let uv_range = [uv_ends[0].min(uv_ends[1]), uv_ends[0].max(uv_ends[1])];
    let uv_pieces = chart.parameter_spans(uv_range, work, cancel)?.count();
    Ok(Some(OriginalCoedgeCorrespondence {
        binding,
        spatial_range,
        uv_ends,
        position_error_mm: maximum_position,
        relative_derivative_error_bound: maximum_relative,
        provenance: OriginalCorrespondenceProvenance::Forward,
        margin_uv: [0.0; 2],
        inventory: OriginalSpanInventory {
            source_knot_boundaries: source_pieces.saturating_sub(1),
            source_knot_bands: 0,
            uv_knot_boundaries: uv_pieces.saturating_sub(1),
            uv_knot_bands: 0,
        },
    }))
}

/// The unchanged original affine PCurve restricted by projecting the captured
/// original endpoint expressions onto its actual plane lift. No rounded
/// parameter quotient, native UV endpoint, or synthetic line is retained.
struct SourcePlaneLineProjection<'d> {
    surface: &'d solidcraft_step::geom::Surface,
    curve: &'d solidcraft_step::geom::Curve,
    vertices: &'d [[f64; 3]; 2],
    parameter_origin: Interval,
    parameter_slope: Interval,
    exact_endpoints: bool,
    source_error_mm: f64,
}
impl<'d> SourcePlaneLineProjection<'d> {
    fn new(
        surface: &'d solidcraft_step::geom::Surface,
        curve: &'d solidcraft_step::geom::Curve,
        vertices: &'d [[f64; 3]; 2],
        maximum_error_mm: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        let Some((lift_origin, lift_direction, _, _)) =
            original_plane_line_coefficients(surface, curve, work, cancel)?
        else {
            return Ok(None);
        };
        let denominator = exact_dot(&lift_direction, &lift_direction, work, cancel)?;
        let denominator_bound = exact_enclosure(&denominator);
        if denominator_bound.lo <= 0.0 {
            return Err(unavailable(
                "original plane/LINE lift has no strictly nonzero coefficient direction",
            ));
        }
        let mut exact_endpoints = true;
        let mut numerators: [ExactCoefficient; 2] =
            std::array::from_fn(|_| ExactCoefficient::point(0.0));
        for end in 0..2 {
            let delta = std::array::from_fn(|i| {
                let mut value = ExactCoefficient::point(vertices[end][i]);
                value.subtract(&lift_origin[i]).map(|_| value)
            });
            let [Some(x), Some(y), Some(z)] = delta else {
                return Err(unavailable(
                    "original plane/LINE endpoint expression exceeds exact expansion capacity",
                ));
            };
            let delta = [x, y, z];
            for i in 0..3 {
                let j = (i + 1) % 3;
                let mut wedge = exact_product(&delta[i], &lift_direction[j], work, cancel)?;
                let other = exact_product(&delta[j], &lift_direction[i], work, cancel)?;
                wedge.subtract(&other).ok_or_else(|| {
                    unavailable("original endpoint collinearity exceeds exact expansion capacity")
                })?;
                exact_endpoints &= wedge.sign() == 0;
            }
            numerators[end] = exact_dot(&delta, &lift_direction, work, cancel)?;
        }
        let parameter_origin = exact_enclosure(&numerators[0]).div(denominator_bound)?;
        let mut numerator_delta = ExactCoefficient::point(0.0);
        for i in 0..3 {
            let delta =
                ExactCoefficient::difference(vertices[1][i], vertices[0][i]).ok_or_else(|| {
                    unavailable("original endpoint difference exceeds exact expansion capacity")
                })?;
            let term = exact_product(&delta, &lift_direction[i], work, cancel)?;
            exact_accumulate(&mut numerator_delta, &term)?;
        }
        let parameter_slope = exact_enclosure(&numerator_delta).div(denominator_bound)?;
        if parameter_slope.contains_zero() {
            return Err(unavailable(
                "original PCurve LINE endpoint projections do not prove a directed nonzero restriction",
            ));
        }
        let mut source_error_mm = 0.0_f64;
        if !exact_endpoints {
            let origin = lift_origin.each_ref().map(exact_enclosure);
            let direction = lift_direction.each_ref().map(exact_enclosure);
            for end in 0..2 {
                let t = exact_enclosure(&numerators[end]).div(denominator_bound)?;
                let squared = (0..3).fold(Interval::point(0.0), |sum, i| {
                    sum.add(
                        origin[i]
                            .add(direction[i].mul(t))
                            .sub(Interval::point(vertices[end][i]))
                            .square(),
                    )
                });
                source_error_mm = source_error_mm.max(squared.sqrt()?.hi);
            }
        }
        if !maximum_error_mm.is_finite()
            || maximum_error_mm < 0.0
            || source_error_mm > maximum_error_mm
        {
            return Err(unavailable(
                "whole original affine PCurve lift endpoint residual exceeds its captured source allowance",
            ));
        }
        Ok(Some(Self {
            surface,
            curve,
            vertices,
            parameter_origin,
            parameter_slope,
            exact_endpoints,
            source_error_mm,
        }))
    }

    fn exact_affine(
        &self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<parametric::ExactAffineCurve, GeometryError> {
        let (lift_origin, lift_direction, chart_origin, chart_direction) =
            original_plane_line_coefficients(self.surface, self.curve, work, cancel)?
                .ok_or_else(|| unavailable("sealed original plane/LINE operands changed"))?;
        let denominator = exact_dot(&lift_direction, &lift_direction, work, cancel)?;
        let mut delta0 = std::array::from_fn(|i| ExactCoefficient::point(self.vertices[0][i]));
        for i in 0..3 {
            delta0[i].subtract(&lift_origin[i]).ok_or_else(|| {
                unavailable("source affine endpoint expression exceeds exact capacity")
            })?;
        }
        let numerator0 = exact_dot(&delta0, &lift_direction, work, cancel)?;
        let delta = std::array::from_fn(|i| {
            ExactCoefficient::difference(self.vertices[1][i], self.vertices[0][i])
        });
        let [Some(x), Some(y), Some(z)] = delta else {
            return Err(unavailable(
                "source affine endpoint difference exceeds exact capacity",
            ));
        };
        let numerator_slope = exact_dot(&[x, y, z], &lift_direction, work, cancel)?;
        let mut position = std::array::from_fn(|_| ExactCoefficient::point(0.0));
        let mut derivative = std::array::from_fn(|_| ExactCoefficient::point(0.0));
        for i in 0..2 {
            position[i] = exact_product(&chart_origin[i], &denominator, work, cancel)?;
            let term = exact_product(&chart_direction[i], &numerator0, work, cancel)?;
            exact_accumulate(&mut position[i], &term)?;
            derivative[i] = exact_product(&chart_direction[i], &numerator_slope, work, cancel)?;
        }
        Ok(parametric::ExactAffineCurve {
            position,
            derivative,
            denominator,
        })
    }
}
impl parametric::CurveEvaluator for SourcePlaneLineProjection<'_> {
    fn active_range(&self) -> [f64; 2] {
        [0.0, 1.0]
    }
    fn exact_affine_coefficients(
        &self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<parametric::ExactAffineCurve>, GeometryError> {
        self.exact_affine(work, cancel).map(Some)
    }
    fn source_parameter_map(&self) -> Option<[Interval; 2]> {
        Some([self.parameter_origin, self.parameter_slope])
    }
    fn tensor(
        &self,
        range: [f64; 2],
        order: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[[Interval; 3]; 4], GeometryError> {
        work.charge(1, cancel)?;
        let s = Interval::new(range[0], range[1])?;
        if range[0] < 0.0 || range[1] > 1.0 {
            return Err(unavailable(
                "source PCurve projection query exceeds its captured endpoint restriction",
            ));
        }
        let solidcraft_step::geom::Curve::Line {
            origin,
            direction,
            magnitude,
        } = self.curve
        else {
            return Err(unavailable(
                "sealed original PCurve LINE declaration changed",
            ));
        };
        let t = self.parameter_origin.add(self.parameter_slope.mul(s));
        let mut result = [[Interval::point(0.0); 3]; 4];
        for i in 0..2 {
            let velocity = Interval::point(direction[i]).mul(Interval::point(*magnitude));
            result[0][i] = Interval::point(origin[i]).add(velocity.mul(t));
            if order > 0 {
                result[1][i] = velocity.mul(self.parameter_slope);
            }
        }
        Ok(result)
    }
    fn continuous(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.tensor(range, 0, work, cancel)?;
        Ok(true)
    }
    fn smooth(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.tensor(range, 0, work, cancel)?;
        Ok(true)
    }
    fn parameter_spans(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<parametric::AxisSpans<'_>, GeometryError> {
        self.tensor(range, 0, work, cancel)?;
        parametric::AxisSpans::single(range)
    }
    fn original_curve(&self) -> Option<&solidcraft_step::geom::Curve> {
        Some(self.curve)
    }
}
/// The global inverse of the actual original plane, applied to the SAME
/// selected spatial expression. This is derived source evidence, not a
/// declared PCurve and not an inverse of a generated native chart.
struct SourcePlanePullback<'d> {
    surface: &'d solidcraft_step::geom::Surface,
    spatial: Arc<crate::analysis::source_restriction::SourceSpatialRestriction<'d>>,
    origin: [f64; 3],
    inverse: [[Interval; 3]; 2],
}
fn original_plane_placement<'a>(
    mut surface: &'a solidcraft_step::geom::Surface,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<&'a solidcraft_step::geom::Placement>, GeometryError> {
    for _ in 0..MAX_DEPTH {
        work.charge(1, cancel)?;
        match surface {
            solidcraft_step::geom::Surface::RectangularTrimmed { basis, .. } => surface = basis,
            solidcraft_step::geom::Surface::Plane { position } => return Ok(Some(position)),
            _ => return Ok(None),
        }
    }
    Err(unavailable(
        "original plane wrapper depth exceeds the bounded source policy",
    ))
}
fn original_plane_basis(
    position: &solidcraft_step::geom::Placement,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[[ExactCoefficient; 3]; 2], GeometryError> {
    for value in position
        .origin
        .into_iter()
        .chain(position.axis)
        .chain(position.ref_direction)
    {
        Interval::new(value, value)?;
    }
    let x = position.ref_direction.map(ExactCoefficient::point);
    let mut y = std::array::from_fn(|_| ExactCoefficient::point(0.0));
    for axis in 0..3 {
        let j = (axis + 1) % 3;
        let k = (axis + 2) % 3;
        y[axis] = exact_product(
            &ExactCoefficient::point(position.axis[j]),
            &x[k],
            work,
            cancel,
        )?;
        y[axis]
            .subtract(&exact_product(
                &ExactCoefficient::point(position.axis[k]),
                &x[j],
                work,
                cancel,
            )?)
            .ok_or_else(|| unavailable("original plane basis exceeds exact expansion capacity"))?;
    }
    Ok([x, y])
}
fn original_plane_inverse_coefficients(
    position: &solidcraft_step::geom::Placement,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<([[ExactCoefficient; 3]; 2], ExactCoefficient), GeometryError> {
    let [x, y] = original_plane_basis(position, work, cancel)?;
    let xx = exact_dot(&x, &x, work, cancel)?;
    let xy = exact_dot(&x, &y, work, cancel)?;
    let yy = exact_dot(&y, &y, work, cancel)?;
    let mut determinant = exact_product(&xx, &yy, work, cancel)?;
    determinant
        .subtract(&exact_product(&xy, &xy, work, cancel)?)
        .ok_or_else(|| unavailable("original plane Gram determinant exceeds exact capacity"))?;
    if determinant.sign() <= 0 || exact_enclosure(&determinant).lo <= 0.0 {
        return Err(unavailable(
            "original plane has no proved nondegenerate global Gram inverse",
        ));
    }
    let mut rows = std::array::from_fn(|_| std::array::from_fn(|_| ExactCoefficient::point(0.0)));
    for axis in 0..3 {
        rows[0][axis] = exact_product(&yy, &x[axis], work, cancel)?;
        rows[0][axis]
            .subtract(&exact_product(&xy, &y[axis], work, cancel)?)
            .ok_or_else(|| unavailable("original plane inverse exceeds exact capacity"))?;
        rows[1][axis] = exact_product(&xx, &y[axis], work, cancel)?;
        rows[1][axis]
            .subtract(&exact_product(&xy, &x[axis], work, cancel)?)
            .ok_or_else(|| unavailable("original plane inverse exceeds exact capacity"))?;
    }
    Ok((rows, determinant))
}
impl<'d> SourcePlanePullback<'d> {
    fn new(
        surface: &'d solidcraft_step::geom::Surface,
        spatial: &Arc<crate::analysis::source_restriction::SourceSpatialRestriction<'d>>,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        let Some(position) = original_plane_placement(surface, work, cancel)? else {
            return Ok(None);
        };
        let (rows, determinant) = original_plane_inverse_coefficients(position, work, cancel)?;
        let mut inverse = [[Interval::point(0.0); 3]; 2];
        for axis in 0..2 {
            for coordinate in 0..3 {
                inverse[axis][coordinate] =
                    exact_ratio(&rows[axis][coordinate], &determinant, work, cancel)?;
            }
        }
        Ok(Some(Self {
            surface,
            spatial: Arc::clone(spatial),
            origin: position.origin,
            inverse,
        }))
    }
    fn exact_affine(
        &self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<parametric::ExactAffineCurve>, GeometryError> {
        let Some(source) = self.spatial.exact_affine_coefficients(work, cancel)? else {
            return Ok(None);
        };
        let position = original_plane_placement(self.surface, work, cancel)?
            .ok_or_else(|| unavailable("sealed original plane inverse changed carrier"))?;
        let (rows, determinant) = original_plane_inverse_coefficients(position, work, cancel)?;
        let mut delta = std::array::from_fn(|_| ExactCoefficient::point(0.0));
        for axis in 0..3 {
            exact_accumulate(&mut delta[axis], &source.position[axis])?;
            delta[axis]
                .subtract(&exact_product(
                    &ExactCoefficient::point(position.origin[axis]),
                    &source.denominator,
                    work,
                    cancel,
                )?)
                .ok_or_else(|| {
                    unavailable("original plane projected position exceeds exact capacity")
                })?;
        }
        let mut projected = std::array::from_fn(|_| ExactCoefficient::point(0.0));
        let mut derivative = std::array::from_fn(|_| ExactCoefficient::point(0.0));
        for axis in 0..2 {
            projected[axis] = exact_dot(&rows[axis], &delta, work, cancel)?;
            derivative[axis] = exact_dot(&rows[axis], &source.derivative, work, cancel)?;
        }
        Ok(Some(parametric::ExactAffineCurve {
            position: projected,
            derivative,
            denominator: exact_product(&determinant, &source.denominator, work, cancel)?,
        }))
    }
    fn exact_lift_identity(
        &self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let Some(chart) = self.exact_affine(work, cancel)? else {
            return Ok(false);
        };
        let Some(source) = self.spatial.exact_affine_coefficients(work, cancel)? else {
            return Ok(false);
        };
        let position = original_plane_placement(self.surface, work, cancel)?
            .ok_or_else(|| unavailable("sealed original plane inverse changed carrier"))?;
        let [x, y] = original_plane_basis(position, work, cancel)?;
        let mut lifted = std::array::from_fn(|_| ExactCoefficient::point(0.0));
        let mut derivative = std::array::from_fn(|_| ExactCoefficient::point(0.0));
        for axis in 0..3 {
            lifted[axis] = exact_product(
                &ExactCoefficient::point(position.origin[axis]),
                &chart.denominator,
                work,
                cancel,
            )?;
            for (basis, uv) in [(&x, 0), (&y, 1)] {
                exact_accumulate(
                    &mut lifted[axis],
                    &exact_product(&basis[axis], &chart.position[uv], work, cancel)?,
                )?;
                exact_accumulate(
                    &mut derivative[axis],
                    &exact_product(&basis[axis], &chart.derivative[uv], work, cancel)?,
                )?;
            }
        }
        // Equality at BOTH endpoints proves equality of these complete affine
        // coefficient expressions; these are not sampled approximate positions.
        affine_endpoint_identity(
            &parametric::ExactAffineCurve {
                position: lifted,
                derivative,
                denominator: chart.denominator,
            },
            [0.0, 1.0],
            &source,
            [0.0, 1.0],
            work,
            cancel,
        )
    }
}
impl parametric::CurveEvaluator for SourcePlanePullback<'_> {
    fn active_range(&self) -> [f64; 2] {
        self.spatial.active_range()
    }
    fn exact_affine_coefficients(
        &self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<parametric::ExactAffineCurve>, GeometryError> {
        self.exact_affine(work, cancel)
    }
    fn tensor(
        &self,
        range: [f64; 2],
        order: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[[Interval; 3]; 4], GeometryError> {
        work.charge(1, cancel)?;
        if order > 3 {
            return Err(unavailable(
                "original plane pullback supports curve jets through order three",
            ));
        }
        let source = self.spatial.tensor(range, order, work, cancel)?;
        let mut result = [[Interval::point(0.0); 3]; 4];
        for derivative in 0..=order {
            for axis in 0..2 {
                result[derivative][axis] = (0..3).fold(Interval::point(0.0), |sum, coordinate| {
                    let value = if derivative == 0 {
                        source[0][coordinate].sub(Interval::point(self.origin[coordinate]))
                    } else {
                        source[derivative][coordinate]
                    };
                    sum.add(self.inverse[axis][coordinate].mul(value))
                });
            }
        }
        Ok(result)
    }
    fn continuous(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.spatial.continuous(range, work, cancel)
    }
    fn smooth(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.spatial.smooth(range, work, cancel)
    }
    fn parameter_spans(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<parametric::AxisSpans<'_>, GeometryError> {
        self.spatial.parameter_spans(range, work, cancel)
    }
}

/// A narrowly qualified inverse of an original cylinder on axial LINEs and
/// coaxial CIRCLEs. The law is affine in the actual selected source parameter,
/// not in a native convenience interval. Angular roots remain intervals; this
/// operator deliberately supplies no exact-affine coefficient receipt.
struct SourceCylinderPullback<'d> {
    spatial: Arc<crate::analysis::source_restriction::SourceSpatialRestriction<'d>>,
    origin: [Interval; 2],
    velocity: [Interval; 2],
    exact_lift: bool,
}
fn original_cylinder<'a>(
    mut surface: &'a solidcraft_step::geom::Surface,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<(&'a solidcraft_step::geom::Placement, f64)>, GeometryError> {
    for _ in 0..MAX_DEPTH {
        work.charge(1, cancel)?;
        match surface {
            solidcraft_step::geom::Surface::RectangularTrimmed { basis, .. } => surface = basis,
            solidcraft_step::geom::Surface::Cylinder { position, radius } => {
                Interval::new(*radius, *radius)?;
                if *radius <= 0.0 {
                    return Err(unavailable(
                        "original cylindrical pullback needs positive radius",
                    ));
                }
                return Ok(Some((position, *radius)));
            }
            _ => return Ok(None),
        }
    }
    Err(unavailable(
        "original cylinder wrapper depth exceeds source policy",
    ))
}
fn original_cylinder_curve<'a>(
    mut curve: &'a solidcraft_step::geom::Curve,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<&'a solidcraft_step::geom::Curve, GeometryError> {
    for _ in 0..MAX_DEPTH {
        work.charge(1, cancel)?;
        match curve {
            solidcraft_step::geom::Curve::Trimmed { basis, .. } => curve = basis,
            solidcraft_step::geom::Curve::SurfaceCurve { curve_3d, .. } => {
                if super::source_operand::preferred_source_pcurve(curve)?.is_some() {
                    return Err(unavailable(
                        "cylindrical pullback cannot replace a declared PCurve master",
                    ));
                }
                curve = curve_3d;
            }
            _ => return Ok(curve),
        }
    }
    Err(unavailable(
        "original cylindrical curve wrapper depth exceeds source policy",
    ))
}
fn exact_cross(
    a: &[ExactCoefficient; 3],
    b: &[ExactCoefficient; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[ExactCoefficient; 3], GeometryError> {
    let mut result = std::array::from_fn(|_| ExactCoefficient::point(0.0));
    for axis in 0..3 {
        let j = (axis + 1) % 3;
        let k = (axis + 2) % 3;
        result[axis] = exact_product(&a[j], &b[k], work, cancel)?;
        result[axis]
            .subtract(&exact_product(&a[k], &b[j], work, cancel)?)
            .ok_or_else(|| unavailable("original cylinder cofactor exceeds exact capacity"))?;
    }
    Ok(result)
}
struct OriginalCylinderFrame {
    rows: [[ExactCoefficient; 3]; 3],
    denominator: ExactCoefficient,
}
impl OriginalCylinderFrame {
    fn new(
        position: &solidcraft_step::geom::Placement,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        let [x, y] = original_plane_basis(position, work, cancel)?;
        let z = position.axis.map(ExactCoefficient::point);
        let rows = [
            exact_cross(&y, &z, work, cancel)?,
            exact_cross(&z, &x, work, cancel)?,
            exact_cross(&x, &y, work, cancel)?,
        ];
        let denominator = exact_dot(&x, &rows[0], work, cancel)?;
        if denominator.sign() == 0 || exact_enclosure(&denominator).contains_zero() {
            return Err(unavailable(
                "original cylinder has no nonsingular coefficient frame",
            ));
        }
        Ok(Self { rows, denominator })
    }
    fn local(
        &self,
        vector: &[ExactCoefficient; 3],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[ExactCoefficient; 3], GeometryError> {
        Ok([
            exact_dot(&self.rows[0], vector, work, cancel)?,
            exact_dot(&self.rows[1], vector, work, cancel)?,
            exact_dot(&self.rows[2], vector, work, cancel)?,
        ])
    }
    fn displacement(
        &self,
        point: [f64; 3],
        origin: [f64; 3],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[ExactCoefficient; 3], GeometryError> {
        let mut delta = point.map(ExactCoefficient::point);
        for axis in 0..3 {
            Interval::new(point[axis], point[axis])?;
            delta[axis]
                .subtract(&ExactCoefficient::point(origin[axis]))
                .ok_or_else(|| {
                    unavailable("original cylinder displacement exceeds exact capacity")
                })?;
        }
        self.local(&delta, work, cancel)
    }
    fn circle_height(
        &self,
        position: &solidcraft_step::geom::Placement,
        radius: f64,
        curve: &solidcraft_step::geom::Curve,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Interval>, GeometryError> {
        let solidcraft_step::geom::Curve::Circle {
            position: circle,
            radius: r,
        } = original_cylinder_curve(curve, work, cancel)?
        else {
            return Ok(None);
        };
        // Literal frame identity, never an epsilon comparison of converted
        // normalized axes. The exact cofactors prove the center displacement.
        if circle.axis != position.axis
            || circle.ref_direction != position.ref_direction
            || *r != radius
        {
            return Ok(None);
        }
        let local = self.displacement(circle.origin, position.origin, work, cancel)?;
        if local[0].sign() != 0 || local[1].sign() != 0 {
            return Ok(None);
        }
        Ok(Some(exact_ratio(
            &local[2],
            &self.denominator,
            work,
            cancel,
        )?))
    }
}
impl<'d> SourceCylinderPullback<'d> {
    fn new(
        definition: &'d Definition,
        surface: &'d solidcraft_step::geom::Surface,
        selected: &SelectedSourceUse<'d>,
        wire: &SelectedSourceWire<'d>,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        let Some((position, radius)) = original_cylinder(surface, work, cancel)? else {
            return Ok(None);
        };
        let frame = OriginalCylinderFrame::new(position, work, cancel)?;
        let source = definition.original_source().coedge(selected.source_index)?;
        let zero = Interval::point(0.0);
        if let Some(height) =
            frame.circle_height(position, radius, &source.spatial_curve, work, cancel)?
        {
            return Ok(Some(Self {
                spatial: Arc::clone(&selected.spatial.operator),
                origin: [zero, height],
                velocity: [Interval::point(1.0), zero],
                // Same literal radial coefficients and exact axial center
                // displacement give C(t)=F(t,height) on the COMPLETE selected
                // source restriction. Vertex/edge endpoint gaps are separate.
                exact_lift: true,
            }));
        }
        let solidcraft_step::geom::Curve::Line {
            origin,
            direction,
            magnitude,
        } = original_cylinder_curve(&source.spatial_curve, work, cancel)?
        else {
            return Err(unavailable(
                "undeclared original cylindrical UV supports only same-frame coaxial circles and axial lines",
            ));
        };
        Interval::new(*magnitude, *magnitude)?;
        for value in direction {
            Interval::new(*value, *value)?;
        }
        let velocity = frame.local(&direction.map(ExactCoefficient::point), work, cancel)?;
        if velocity[0].sign() != 0
            || velocity[1].sign() != 0
            || velocity[2].sign() == 0
            || *magnitude == 0.0
        {
            return Err(unavailable(
                "original cylindrical LINE is not coefficient-proved axial",
            ));
        }
        let local = frame.displacement(*origin, position.origin, work, cancel)?;
        let x = exact_ratio(&local[0], &frame.denominator, work, cancel)?;
        let y = exact_ratio(&local[1], &frame.denominator, work, cancel)?;
        let principal =
            super::source_operand::original_angular_parameter(y, x, 1.0e-12, work, cancel)?;
        let tau = Interval {
            lo: parametric::down(std::f64::consts::TAU),
            hi: parametric::up(std::f64::consts::TAU),
        };
        let mut angle = None;
        // A circle endpoint at the SAME original topology vertex owns the
        // covering branch. Proximity chooses no equality: the independently
        // isolated LINE angle is lifted, then the whole forward proof below
        // measures its actual physical residual.
        for other in &wire.uses {
            work.charge(1, cancel)?;
            let rim = definition.original_source().coedge(other.source_index)?;
            if frame
                .circle_height(position, radius, &rim.spatial_curve, work, cancel)?
                .is_none()
            {
                continue;
            }
            for end in 0..2 {
                if !source.vertex_ids.contains(&rim.vertex_ids[end]) {
                    continue;
                }
                let anchor = other
                    .spatial
                    .operator
                    .source_parameter_image([end as f64; 2], work, cancel)?
                    .ok_or_else(|| {
                        unavailable("original cylinder rim has no actual source parameter image")
                    })?;
                let turns = ((anchor.mid() - principal.mid()) / std::f64::consts::TAU).round();
                if !turns.is_finite() || turns.abs() > i32::MAX as f64 {
                    return Err(unavailable(
                        "original cylindrical branch lift exceeds bounded integer policy",
                    ));
                }
                let lifted = principal.add(tau.mul(Interval::point(turns)));
                let difference = lifted.sub(anchor);
                if difference.lo <= -std::f64::consts::PI || difference.hi >= std::f64::consts::PI {
                    return Err(unavailable(
                        "original cylindrical endpoint does not isolate one owned angular branch",
                    ));
                }
                if angle.is_some_and(|old: Interval| old != lifted) {
                    return Err(unavailable(
                        "original cylinder rims demand incompatible angular lifts",
                    ));
                }
                angle = Some(lifted);
            }
        }
        let angle = angle.ok_or_else(|| unavailable(
            "original axial LINE has no same-face coaxial rim endpoint owning its angular branch",
        ))?;
        Ok(Some(Self {
            spatial: Arc::clone(&selected.spatial.operator),
            origin: [
                angle,
                exact_ratio(&local[2], &frame.denominator, work, cancel)?,
            ],
            velocity: [
                zero,
                exact_ratio(
                    &exact_product(
                        &velocity[2],
                        &ExactCoefficient::point(*magnitude),
                        work,
                        cancel,
                    )?,
                    &frame.denominator,
                    work,
                    cancel,
                )?,
            ],
            // Isolated angular roots enclose an inverse, not an algebraic
            // identity. The complete forward/relative-tangent proof retains
            // the finite physical residual of this LINE law.
            exact_lift: false,
        }))
    }
}
impl parametric::CurveEvaluator for SourceCylinderPullback<'_> {
    fn active_range(&self) -> [f64; 2] {
        self.spatial.active_range()
    }
    fn tensor(
        &self,
        range: [f64; 2],
        order: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[[Interval; 3]; 4], GeometryError> {
        work.charge(1, cancel)?;
        if order > 3 {
            return Err(unavailable(
                "original cylindrical pullback curve order exceeds three",
            ));
        }
        // Evaluation also enforces every literal wrapper/domain restriction.
        self.spatial.tensor(range, 0, work, cancel)?;
        let parameter = self
            .spatial
            .source_parameter_image(range, work, cancel)?
            .ok_or_else(|| {
                unavailable("original cylindrical curve lost its source parameter image")
            })?;
        let derivative = if order > 0 {
            self.spatial.parameter_derivative(work, cancel)?
        } else {
            Interval::point(0.0)
        };
        let mut result = [[Interval::point(0.0); 3]; 4];
        for axis in 0..2 {
            result[0][axis] = self.origin[axis].add(self.velocity[axis].mul(parameter));
            if order > 0 {
                result[1][axis] = self.velocity[axis].mul(derivative);
            }
        }
        Ok(result)
    }
    fn continuous(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.spatial.continuous(range, work, cancel)
    }
    fn smooth(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.spatial.smooth(range, work, cancel)
    }
    fn parameter_spans(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<parametric::AxisSpans<'_>, GeometryError> {
        self.spatial.parameter_spans(range, work, cancel)
    }
}

fn exact_accumulate(
    target: &mut ExactCoefficient,
    value: &ExactCoefficient,
) -> Result<(), GeometryError> {
    for &term in &value.terms[..value.len] {
        target.push(term).ok_or_else(|| {
            unavailable("original coefficient expression exceeds bounded exact expansion capacity")
        })?;
    }
    Ok(())
}
fn exact_enclosure(value: &ExactCoefficient) -> Interval {
    value.terms[..value.len]
        .iter()
        .fold(Interval::point(0.0), |sum, &term| {
            sum.add(Interval::point(term))
        })
}
fn exact_dot(
    a: &[ExactCoefficient; 3],
    b: &[ExactCoefficient; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<ExactCoefficient, GeometryError> {
    let mut value = ExactCoefficient::point(0.0);
    for i in 0..3 {
        let term = exact_product(&a[i], &b[i], work, cancel)?;
        exact_accumulate(&mut value, &term)?;
    }
    Ok(value)
}
fn original_plane_line_coefficients(
    surface: &solidcraft_step::geom::Surface,
    curve: &solidcraft_step::geom::Curve,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<
    Option<(
        [ExactCoefficient; 3],
        [ExactCoefficient; 3],
        [ExactCoefficient; 2],
        [ExactCoefficient; 2],
    )>,
    GeometryError,
> {
    work.charge(1, cancel)?;
    let solidcraft_step::geom::Surface::Plane { position } = surface else {
        return Ok(None);
    };
    let solidcraft_step::geom::Curve::Line {
        origin,
        direction,
        magnitude,
    } = curve
    else {
        return Ok(None);
    };
    for v in position
        .origin
        .into_iter()
        .chain(position.axis)
        .chain(position.ref_direction)
        .chain(*origin)
        .chain(*direction)
        .chain([*magnitude])
    {
        Interval::new(v, v)?;
    }
    let mut y: [ExactCoefficient; 3] = std::array::from_fn(|_| ExactCoefficient::point(0.0));
    for i in 0..3 {
        let j = (i + 1) % 3;
        let k = (i + 2) % 3;
        y[i] = exact_product(
            &ExactCoefficient::point(position.axis[j]),
            &ExactCoefficient::point(position.ref_direction[k]),
            work,
            cancel,
        )?;
        let term = exact_product(
            &ExactCoefficient::point(position.axis[k]),
            &ExactCoefficient::point(position.ref_direction[j]),
            work,
            cancel,
        )?;
        y[i].subtract(&term).ok_or_else(|| {
            unavailable("original plane basis cross product exceeds exact capacity")
        })?;
    }
    let chart_origin = [origin[0], origin[1]].map(ExactCoefficient::point);
    let chart_direction = [
        exact_product(
            &ExactCoefficient::point(direction[0]),
            &ExactCoefficient::point(*magnitude),
            work,
            cancel,
        )?,
        exact_product(
            &ExactCoefficient::point(direction[1]),
            &ExactCoefficient::point(*magnitude),
            work,
            cancel,
        )?,
    ];
    let mut lift_origin = position.origin.map(ExactCoefficient::point);
    let mut lift_direction = std::array::from_fn(|_| ExactCoefficient::point(0.0));
    for i in 0..3 {
        let a = exact_product(
            &ExactCoefficient::point(position.ref_direction[i]),
            &chart_origin[0],
            work,
            cancel,
        )?;
        let b = exact_product(&y[i], &chart_origin[1], work, cancel)?;
        exact_accumulate(&mut lift_origin[i], &a)?;
        exact_accumulate(&mut lift_origin[i], &b)?;
        lift_direction[i] = exact_product(
            &ExactCoefficient::point(position.ref_direction[i]),
            &chart_direction[0],
            work,
            cancel,
        )?;
        let term = exact_product(&y[i], &chart_direction[1], work, cancel)?;
        exact_accumulate(&mut lift_direction[i], &term)?;
    }
    Ok(Some((
        lift_origin,
        lift_direction,
        chart_origin,
        chart_direction,
    )))
}

/// A validated affine completion between actual selected source endpoints.
/// Endpoint intervals enclose the fixed original endpoint expressions; they are
/// not midpoint control points and are never relabelled as a literal PCurve.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SourceEndpointConnector {
    endpoints: [[Interval; 2]; 2],
    collapsed: bool,
}
impl parametric::CurveEvaluator for SourceEndpointConnector {
    fn active_range(&self) -> [f64; 2] {
        [0.0, 1.0]
    }
    fn tensor(
        &self,
        range: [f64; 2],
        order: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[[Interval; 3]; 4], GeometryError> {
        work.charge(1, cancel)?;
        let t = Interval::new(range[0], range[1])?;
        if range[0] < 0.0 || range[1] > 1.0 {
            return Err(unavailable(
                "source joint connector query leaves its complete endpoint restriction",
            ));
        }
        let mut result = [[Interval::point(0.0); 3]; 4];
        for axis in 0..2 {
            if self.collapsed {
                // This intersection is licensed only by a completed exact
                // endpoint-expression equality proof in the joint constructor.
                result[0][axis] = self.endpoints[0][axis].intersect(self.endpoints[1][axis])?;
            } else {
                let delta = self.endpoints[1][axis].sub(self.endpoints[0][axis]);
                result[0][axis] = self.endpoints[0][axis].add(delta.mul(t));
                if order > 0 {
                    result[1][axis] = delta;
                }
            }
        }
        Ok(result)
    }
    fn continuous(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.tensor(range, 0, work, cancel)?;
        Ok(true)
    }
    fn smooth(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.tensor(range, 0, work, cancel)?;
        Ok(true)
    }
    fn parameter_spans(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<parametric::AxisSpans<'_>, GeometryError> {
        self.tensor(range, 0, work, cancel)?;
        parametric::AxisSpans::single(range)
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum TrimJointKind {
    EndpointUncertainty,
    SphericalPole {
        polar_axis: usize,
        pole: SphericalPole,
        polar_coordinate: Interval,
        world_enclosure: [Interval; 3],
        vertex_residual: [Interval; 3],
        connector: SourceEndpointConnector,
    },
}

/// Complete correction-joint inventory. Exact ordinary joins may be absent
/// only after the constructor proves their full completion is zero.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PreparedTrimJoint {
    pub(crate) vertex_id: VertexId,
    pub(crate) source_vertex_id: spiling_contracts::geometry::StepEntityId,
    pub(crate) vertex_point: [f64; 3],
    pub(crate) incoming: NativeCoedgeIdentity,
    pub(crate) outgoing: NativeCoedgeIdentity,
    pub(crate) enclosure: [Interval; 2],
    pub(crate) endpoint_uv: [[[f64; 2]; 2]; 2],
    pub(crate) kind: TrimJointKind,
    pub(crate) connector: SourceEndpointConnector,
    physical_error_mm: f64,
    zero_displacement: bool,
    endpoint_bindings: [SourceJointEndpointBinding; 2],
}

#[derive(Clone, Copy)]
struct ParameterSample {
    source: f64,
    target: f64,
    slope: f64,
}
#[derive(Clone, Copy)]
struct ParameterSegment {
    endpoints: [ParameterSample; 2],
}
impl ParameterSegment {
    fn jet(&self, range: [f64; 2]) -> Result<[Interval; 4], GeometryError> {
        let [a, b] = self.endpoints;
        let length = Interval::point(b.source).sub(Interval::point(a.source));
        let v = Interval::new(range[0], range[1])?
            .sub(Interval::point(a.source))
            .div(length)?
            .intersect(Interval::new(0.0, 1.0)?)?;
        let one = Interval::point(1.0).sub(v);
        let d0 = Interval::point(a.slope);
        let d2 = Interval::point(b.slope);
        let d1 = Interval::point(b.target)
            .sub(Interval::point(a.target))
            .div(length)?
            .mul(Interval::point(3.0))
            .sub(d0)
            .sub(d2);
        let controls = [
            Interval::point(a.target),
            Interval::point(a.target).add(length.mul(d0).div(Interval::point(3.0))?),
            Interval::point(b.target).sub(length.mul(d2).div(Interval::point(3.0))?),
            Interval::point(b.target),
        ];
        let value = controls[0]
            .mul(one.square().mul(one))
            .add(controls[1].mul(Interval::point(3.0).mul(one.square()).mul(v)))
            .add(controls[2].mul(Interval::point(3.0).mul(one).mul(v.square())))
            .add(controls[3].mul(v.square().mul(v)))
            .intersect(Interval::new(
                a.target.min(b.target),
                a.target.max(b.target),
            )?)?;
        let first = d0
            .mul(one.square())
            .add(d1.mul(Interval::point(2.0).mul(v).mul(one)))
            .add(d2.mul(v.square()))
            .intersect(d0.hull(d1).hull(d2))?;
        let second0 = d1.sub(d0).mul(Interval::point(2.0)).div(length)?;
        let second1 = d2.sub(d1).mul(Interval::point(2.0)).div(length)?;
        let second = second0
            .mul(one)
            .add(second1.mul(v))
            .intersect(second0.hull(second1))?;
        let third = d2
            .sub(d1.mul(Interval::point(2.0)))
            .add(d0)
            .mul(Interval::point(2.0))
            .div(length.square())?;
        Ok([value, first, second, third])
    }
    fn monotone(&self, forward: bool) -> Result<bool, GeometryError> {
        let [a, b] = self.endpoints;
        let middle = Interval::point(b.target)
            .sub(Interval::point(a.target))
            .div(Interval::point(b.source).sub(Interval::point(a.source)))?
            .mul(Interval::point(3.0))
            .sub(Interval::point(a.slope))
            .sub(Interval::point(b.slope));
        Ok(if forward {
            a.slope > 0.0 && b.slope > 0.0 && middle.lo > 0.0
        } else {
            a.slope < 0.0 && b.slope < 0.0 && middle.hi < 0.0
        })
    }
    fn directed_position_monotone(
        &self,
        source_range: [f64; 2],
        forward: bool,
    ) -> Result<bool, GeometryError> {
        let [a, b] = self.endpoints;
        let middle = Interval::point(b.target)
            .sub(Interval::point(a.target))
            .div(Interval::point(b.source).sub(Interval::point(a.source)))?
            .mul(Interval::point(3.0))
            .sub(Interval::point(a.slope))
            .sub(Interval::point(b.slope));
        let first_stationary = a.source == source_range[0] && a.slope == 0.0;
        let last_stationary = b.source == source_range[1] && b.slope == 0.0;
        // The middle Bernstein derivative coefficient is STRICT. Therefore
        // the derivative is strict on the complete open segment even when
        // its actual outer endpoint derivative is zero. Internal segment
        // junctions remain strictly directed; no flat interval is admitted.
        Ok(if forward {
            (a.slope > 0.0 || first_stationary)
                && (b.slope > 0.0 || last_stationary)
                && middle.lo > 0.0
        } else {
            (a.slope < 0.0 || first_stationary)
                && (b.slope < 0.0 || last_stationary)
                && middle.hi < 0.0
        })
    }
}

/// Strong original-source receipt: only the relative-tangent constructor can
/// expose this wrapper. Native positional proofs discard their private map.
pub(crate) struct SourceCurveCalibration(CurveCalibrationMap);
impl SourceCurveCalibration {
    pub(crate) fn retained_bytes(&self) -> usize {
        self.0.retained_bytes()
    }
    pub(crate) fn source_range(&self) -> [f64; 2] {
        self.0.source_range()
    }
    pub(crate) fn target_range(&self) -> [f64; 2] {
        self.0.target_range()
    }
    pub(crate) fn maximum_position_error_mm(&self) -> f64 {
        self.0.maximum_position_error_mm()
    }
    pub(crate) fn relative_derivative_error_bound(&self) -> f64 {
        self.0
            .relative_derivative_error_bound
            .expect("strong source constructor requires a relative-tangent receipt")
    }
    pub(crate) fn parameter_cuts(&self) -> &[f64] {
        self.0.parameter_cuts()
    }
    pub(crate) fn jet(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[Interval; 4], GeometryError> {
        self.0.jet(range, work, cancel)
    }
}

/// Retained coefficient map shared by two DISTINCT proof constructors.
/// No optional relative receipt means precisely no tangent theorem.
struct CurveCalibrationMap {
    segments: Vec<ParameterSegment>,
    cuts: Vec<f64>,
    source_range: [f64; 2],
    target_range: [f64; 2],
    affine: bool,
    maximum_position_error_mm: f64,
    relative_derivative_error_bound: Option<f64>,
}
impl CurveCalibrationMap {
    pub(crate) fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            + self.segments.capacity() * size_of::<ParameterSegment>()
            + self.cuts.capacity() * size_of::<f64>()
    }
    pub(crate) fn source_range(&self) -> [f64; 2] {
        self.source_range
    }
    pub(crate) fn target_range(&self) -> [f64; 2] {
        self.target_range
    }
    pub(crate) fn maximum_position_error_mm(&self) -> f64 {
        self.maximum_position_error_mm
    }
    pub(crate) fn parameter_cuts(&self) -> &[f64] {
        if self.affine {
            &self.source_range
        } else {
            &self.cuts
        }
    }
    pub(crate) fn jet(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[Interval; 4], GeometryError> {
        Interval::new(range[0], range[1])?;
        if range[0] < self.source_range[0] || range[1] > self.source_range[1] {
            return Err(unavailable(
                "source/native coefficient map query leaves its qualified complete source range",
            ));
        }
        if self.affine {
            work.charge(1, cancel)?;
            let slope = Interval::point(self.target_range[1])
                .sub(Interval::point(self.target_range[0]))
                .div(
                    Interval::point(self.source_range[1])
                        .sub(Interval::point(self.source_range[0])),
                )?;
            let mut value = Interval::point(self.target_range[0]).add(
                Interval::new(range[0], range[1])?
                    .sub(Interval::point(self.source_range[0]))
                    .mul(slope),
            );
            value = value.intersect(Interval::new(
                self.target_range[0].min(self.target_range[1]),
                self.target_range[0].max(self.target_range[1]),
            )?)?;
            if range[0] == range[1] {
                if range[0] == self.source_range[0] {
                    value = Interval::point(self.target_range[0]);
                }
                if range[0] == self.source_range[1] {
                    value = Interval::point(self.target_range[1]);
                }
            }
            return Ok([value, slope, Interval::point(0.0), Interval::point(0.0)]);
        }
        work.charge(
            (usize::BITS - self.segments.len().leading_zeros() + 1) as u64,
            cancel,
        )?;
        let first = self
            .segments
            .partition_point(|segment| segment.endpoints[1].source < range[0]);
        let mut result: Option<[Interval; 4]> = None;
        for segment in &self.segments[first..] {
            work.charge(1, cancel)?;
            let [a, b] = segment.endpoints;
            if a.source > range[1] {
                break;
            }
            let image = segment.jet([range[0].max(a.source), range[1].min(b.source)])?;
            result = Some(match result {
                None => image,
                Some(old) => std::array::from_fn(|i| old[i].hull(image[i])),
            });
        }
        result.ok_or_else(|| unavailable("source/native coefficient map has no covering segment"))
    }
}

fn compose_curve_jet(
    target: &dyn parametric::CurveEvaluator,
    map: [Interval; 4],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[[Interval; 3]; 4], GeometryError> {
    let raw = target.tensor([map[0].lo, map[0].hi], 3, work, cancel)?;
    let mut result = raw;
    for axis in 0..3 {
        result[1][axis] = raw[1][axis].mul(map[1]);
        result[2][axis] = raw[2][axis]
            .mul(map[1].square())
            .add(raw[1][axis].mul(map[2]));
        result[3][axis] = raw[3][axis]
            .mul(map[1].square().mul(map[1]))
            .add(
                raw[2][axis]
                    .mul(map[1])
                    .mul(map[2])
                    .mul(Interval::point(3.0)),
            )
            .add(raw[1][axis].mul(map[3]));
    }
    Ok(result)
}
fn parameter_sample(
    reference: &dyn parametric::CurveEvaluator,
    target: &dyn parametric::CurveEvaluator,
    source: f64,
    parameter: f64,
    secant: f64,
    allow_stationary: bool,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<ParameterSample, GeometryError> {
    let a = reference.tensor([source; 2], 1, work, cancel)?;
    let b = target.tensor([parameter; 2], 1, work, cancel)?;
    let middle = |v: Interval| v.lo * 0.5 + v.hi * 0.5;
    let numerator = (0..3)
        .map(|axis| middle(a[1][axis]) * middle(b[1][axis]))
        .sum::<f64>();
    let denominator = (0..3).map(|axis| middle(b[1][axis]).powi(2)).sum::<f64>();
    let proposed = numerator / denominator;
    let slope = if allow_stationary && a[1] == [Interval::point(0.0); 3] {
        0.0
    } else if proposed.is_finite() && proposed * secant > 0.0 {
        proposed
    } else {
        secant
    };
    Ok(ParameterSample {
        source,
        target: parameter,
        slope,
    })
}
fn propose_parameter(
    reference: &dyn parametric::CurveEvaluator,
    target: &dyn parametric::CurveEvaluator,
    source: f64,
    initial: f64,
    domain: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<f64, GeometryError> {
    let point = reference.tensor([source; 2], 0, work, cancel)?[0].map(|v| v.lo * 0.5 + v.hi * 0.5);
    let mut parameter = initial;
    for _ in 0..24 {
        work.charge(1, cancel)?;
        let jet = target.tensor([parameter; 2], 2, work, cancel)?;
        let p = jet[0].map(|v| v.lo * 0.5 + v.hi * 0.5);
        let d = jet[1].map(|v| v.lo * 0.5 + v.hi * 0.5);
        let dd = jet[2].map(|v| v.lo * 0.5 + v.hi * 0.5);
        let g = (0..3).map(|i| (p[i] - point[i]) * d[i]).sum::<f64>();
        let dg = (0..3)
            .map(|i| d[i] * d[i] + (p[i] - point[i]) * dd[i])
            .sum::<f64>();
        if !dg.is_finite() || dg <= 0.0 {
            break;
        }
        let next = (parameter - g / dg).clamp(domain[0].next_up(), domain[1].next_down());
        if !next.is_finite() || next == parameter {
            break;
        }
        parameter = next;
    }
    Ok(parameter)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CurveCalibrationTheorem {
    SourceRelativeTangent,
    NativeDirectedPosition,
}
impl CurveCalibrationTheorem {
    fn initial_relative_receipt(self) -> Option<f64> {
        match self {
            Self::SourceRelativeTangent => Some(0.0),
            Self::NativeDirectedPosition => None,
        }
    }
    fn monotone(
        self,
        segment: &ParameterSegment,
        source_range: [f64; 2],
        forward: bool,
    ) -> Result<bool, GeometryError> {
        match self {
            Self::SourceRelativeTangent => segment.monotone(forward),
            Self::NativeDirectedPosition => {
                segment.directed_position_monotone(source_range, forward)
            }
        }
    }
}

/// Calibrate two actual coefficient operators, without equating their gauges.
/// The result proves a complete directed target restriction and a strict
/// relative-tangent tube; it is not an exact-C0 or source-closure receipt.
pub(crate) fn calibrate_source_curves(
    reference: &dyn parametric::CurveEvaluator,
    target: &dyn parametric::CurveEvaluator,
    target_range: [f64; 2],
    maximum_error_mm: f64,
    maximum_depth: u8,
    allowance: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SourceCurveCalibration, GeometryError> {
    let map = calibrate_curves(
        reference,
        target,
        target_range,
        maximum_error_mm,
        maximum_depth,
        allowance,
        CurveCalibrationTheorem::SourceRelativeTangent,
        work,
        cancel,
    )?;
    if map.relative_derivative_error_bound.is_none() {
        return Err(unavailable(
            "source calibration lacks its required relative-tangent receipt",
        ));
    }
    Ok(SourceCurveCalibration(map))
}

/// Prove a complete directed native branch's positional tube, allowing actual
/// stationary OUTER parameter endpoints. The map is strictly monotone on its
/// interior and covers both literal target endpoints exactly. No source
/// relative-tangent receipt or parameter-map authority escapes this consumer.
pub(crate) fn prove_native_curve_position_correspondence(
    reference: &dyn parametric::CurveEvaluator,
    target: &dyn parametric::CurveEvaluator,
    target_range: [f64; 2],
    maximum_error_mm: f64,
    maximum_depth: u8,
    allowance: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    calibrate_curves(
        reference,
        target,
        target_range,
        maximum_error_mm,
        maximum_depth,
        allowance,
        CurveCalibrationTheorem::NativeDirectedPosition,
        work,
        cancel,
    )?;
    Ok(())
}

fn calibrate_curves(
    reference: &dyn parametric::CurveEvaluator,
    target: &dyn parametric::CurveEvaluator,
    target_range: [f64; 2],
    maximum_error_mm: f64,
    maximum_depth: u8,
    allowance: usize,
    theorem: CurveCalibrationTheorem,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<CurveCalibrationMap, GeometryError> {
    work.charge(1, cancel)?;
    let maximum_depth = maximum_depth.min(MAX_DEPTH);
    memory(size_of::<CurveCalibrationMap>(), allowance)?;
    let source_range = reference.active_range();
    let target_domain = target.active_range();
    Interval::new(source_range[0], source_range[1])?;
    let sorted_target = [
        target_range[0].min(target_range[1]),
        target_range[0].max(target_range[1]),
    ];
    if source_range[0] >= source_range[1]
        || sorted_target[0] >= sorted_target[1]
        || sorted_target[0] < target_domain[0]
        || sorted_target[1] > target_domain[1]
        || !maximum_error_mm.is_finite()
        || maximum_error_mm < 0.0
    {
        return Err(unavailable(
            "source/native calibration has invalid actual operator ranges or physical allowance",
        ));
    }
    if !reference.continuous(source_range, work, cancel)?
        || !target.continuous(sorted_target, work, cancel)?
    {
        return Err(unavailable(
            "source/native calibration lacks complete literal C0 coefficient continuity",
        ));
    }
    if let (Some(a), Some(b)) = (
        reference.exact_affine_coefficients(work, cancel)?,
        target.exact_affine_coefficients(work, cancel)?,
    ) {
        if a.denominator.sign() != 0
            && a.derivative.iter().any(|v| v.sign() != 0)
            && affine_endpoint_identity(&a, source_range, &b, target_range, work, cancel)?
        {
            return Ok(CurveCalibrationMap {
                segments: Vec::new(),
                cuts: Vec::new(),
                source_range,
                target_range,
                affine: true,
                maximum_position_error_mm: 0.0,
                relative_derivative_error_bound: theorem.initial_relative_receipt(),
            });
        }
    }
    if copied_source_spline_identity(reference, target, source_range, target_range, work, cancel)? {
        return Ok(CurveCalibrationMap {
            segments: Vec::new(),
            cuts: Vec::new(),
            source_range,
            target_range,
            affine: true,
            maximum_position_error_mm: 0.0,
            relative_derivative_error_bound: theorem.initial_relative_receipt(),
        });
    }
    let secant = (target_range[1] - target_range[0]) / (source_range[1] - source_range[0]);
    let first = parameter_sample(
        reference,
        target,
        source_range[0],
        target_range[0],
        secant,
        theorem == CurveCalibrationTheorem::NativeDirectedPosition,
        work,
        cancel,
    )?;
    let last = parameter_sample(
        reference,
        target,
        source_range[1],
        target_range[1],
        secant,
        theorem == CurveCalibrationTheorem::NativeDirectedPosition,
        work,
        cancel,
    )?;
    let root_segment = ParameterSegment {
        endpoints: [first, last],
    };
    let mut stack = [(root_segment, 0u8); MAX_DEPTH as usize + 1];
    let mut live = 1;
    let mut result = CurveCalibrationMap {
        segments: Vec::new(),
        cuts: Vec::new(),
        source_range,
        target_range,
        affine: false,
        maximum_position_error_mm: 0.0,
        relative_derivative_error_bound: theorem.initial_relative_receipt(),
    };
    while live > 0 {
        live -= 1;
        let (segment, depth) = stack[live];
        work.charge(1, cancel)?;
        let [a, b] = segment.endpoints;
        let range = [a.source, b.source];
        let middle = a.source * 0.5 + b.source * 0.5;
        let mut admitted = false;
        let monotone =
            theorem.monotone(&segment, source_range, target_range[1] > target_range[0])?;
        if monotone {
            let map = segment.jet(range)?;
            let at_middle = segment.jet([middle; 2])?;
            let c = reference.tensor(range, 2, work, cancel)?;
            let n = compose_curve_jet(target, map, work, cancel)?;
            let c0 = reference.tensor([middle; 2], 1, work, cancel)?;
            let n0 = compose_curve_jet(target, at_middle, work, cancel)?;
            let delta = Interval::new(range[0], range[1])?.sub(Interval::point(middle));
            let difference: [Interval; 3] = std::array::from_fn(|i| n[1][i].sub(c[1][i]));
            let smooth = reference.smooth(range, work, cancel)?
                && target.smooth([map[0].lo, map[0].hi], work, cancel)?;
            let residual = std::array::from_fn(|i| {
                let value = n0[0][i].sub(c0[0][i]);
                if smooth {
                    value.add(n0[1][i].sub(c0[1][i]).mul(delta)).add(
                        n[2][i]
                            .sub(c[2][i])
                            .mul(delta.square())
                            .mul(Interval::point(0.5)),
                    )
                } else {
                    value.add(difference[i].mul(delta))
                }
            });
            let error = norm(residual)?.hi;
            let relative = match theorem {
                CurveCalibrationTheorem::SourceRelativeTangent => {
                    let speed = norm(c[1])?.lo;
                    Some(if speed > 0.0 {
                        Interval::point(norm(difference)?.hi)
                            .div(Interval::point(speed))?
                            .hi
                    } else {
                        f64::INFINITY
                    })
                }
                CurveCalibrationTheorem::NativeDirectedPosition => None,
            };
            let tangent_admitted = match relative {
                Some(bound) => bound < 0.25,
                None => theorem == CurveCalibrationTheorem::NativeDirectedPosition,
            };
            if error <= maximum_error_mm && tangent_admitted {
                let required = result.segments.len() + 1;
                if required > result.segments.capacity() {
                    let capacity = required
                        .max(result.segments.capacity().saturating_mul(2))
                        .max(8);
                    memory(
                        result.retained_bytes()
                            + capacity * size_of::<ParameterSegment>()
                            + (capacity + 1) * size_of::<f64>(),
                        allowance,
                    )?;
                    result
                        .segments
                        .try_reserve_exact(capacity - result.segments.len())
                        .map_err(|_| {
                            resource("source/native calibration segment allocation failed")
                        })?;
                }
                result.maximum_position_error_mm = result.maximum_position_error_mm.max(error);
                if let (Some(maximum), Some(bound)) =
                    (&mut result.relative_derivative_error_bound, relative)
                {
                    *maximum = maximum.max(bound);
                }
                result.segments.push(segment);
                admitted = true;
            }
        }
        if admitted {
            continue;
        }
        if depth == maximum_depth || middle <= a.source || middle >= b.source {
            return Err(unavailable(format!(
                "whole source/native coefficient calibration unresolved at source range {:?}, target endpoints {:?}; endpoint/parameter-germ or one-sided knot correspondence is unproved",
                range,
                [a.target, b.target]
            )));
        }
        let domain = [a.target.min(b.target), a.target.max(b.target)];
        if domain[0].next_up() >= domain[1].next_down() {
            return Err(unavailable(
                "source/native parameter subdivision cannot isolate another distinct literal target coordinate",
            ));
        }
        let seed = if monotone {
            segment.jet([middle; 2])?[0].mid()
        } else {
            a.target * 0.5 + b.target * 0.5
        };
        let parameter = propose_parameter(reference, target, middle, seed, domain, work, cancel)?;
        let shared = parameter_sample(
            reference, target, middle, parameter, secant, false, work, cancel,
        )?;
        stack[live] = (
            ParameterSegment {
                endpoints: [shared, b],
            },
            depth + 1,
        );
        stack[live + 1] = (
            ParameterSegment {
                endpoints: [a, shared],
            },
            depth + 1,
        );
        live += 2;
    }
    memory(
        result.retained_bytes() + (result.segments.len() + 1) * size_of::<f64>(),
        allowance,
    )?;
    result
        .cuts
        .try_reserve_exact(result.segments.len() + 1)
        .map_err(|_| resource("source/native map fence allocation failed"))?;
    result.cuts.push(source_range[0]);
    for segment in &result.segments {
        work.charge(1, cancel)?;
        result.cuts.push(segment.endpoints[1].source);
    }
    Ok(result)
}

struct SourceSpatialCacheEntry<'d> {
    source: &'d super::source::OriginalCoedgeSource,
    operator: Arc<PreparedSharedSpatialCurve<'d>>,
}
struct PreparedSharedSpatialCurve<'d> {
    operator: Arc<crate::analysis::source_restriction::SourceSpatialRestriction<'d>>,
    native_map: SourceCurveCalibration,
    edge: EdgeId,
    spatial_forward: bool,
}
impl PreparedSharedSpatialCurve<'_> {
    fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            + self.operator.retained_bytes()
            + 2 * size_of::<usize>()
            + self.native_map.retained_bytes()
            - size_of::<SourceCurveCalibration>()
    }
}
enum OriginalUvCurve<'d> {
    PlaneLine(SourcePlaneLineProjection<'d>),
    DerivedPlane(SourcePlanePullback<'d>),
    DerivedCylinder(SourceCylinderPullback<'d>),
    Pcurve(crate::analysis::source_restriction::pcurve::SourcePcurveRestriction<'d>),
}
pub(crate) struct PreparedOriginalUvCurve<'d> {
    curve: OriginalUvCurve<'d>,
    spatial: Arc<PreparedSharedSpatialCurve<'d>>,
}
struct PreparedOriginalCoedge<'d> {
    curve: PreparedOriginalUvCurve<'d>,
    correspondence: OriginalCoedgeCorrespondence,
    full_uv_image: [Interval; 2],
    // Raw whole-restriction jet of this same immutable selected operator.
    // The selected envelope above retains its separate domain semantics.
    full_uv_jet: parametric::ChartCurveEnclosure,
    source_pcurve: Option<&'d super::source::OriginalPcurveSource>,
}
impl PreparedOriginalCoedge<'_> {
    fn owned_bytes(&self) -> usize {
        self.curve.owned_bytes()
    }
}
impl PreparedOriginalUvCurve<'_> {
    fn evaluator(&self) -> &dyn parametric::CurveEvaluator {
        match &self.curve {
            OriginalUvCurve::PlaneLine(curve) => curve,
            OriginalUvCurve::DerivedPlane(curve) => curve,
            OriginalUvCurve::DerivedCylinder(curve) => curve,
            OriginalUvCurve::Pcurve(curve) => curve,
        }
    }
    fn owned_bytes(&self) -> usize {
        match &self.curve {
            OriginalUvCurve::PlaneLine(_)
            | OriginalUvCurve::DerivedPlane(_)
            | OriginalUvCurve::DerivedCylinder(_) => 0,
            OriginalUvCurve::Pcurve(curve) => curve.retained_bytes() - size_of_val(curve),
        }
    }
}
impl parametric::CurveEvaluator for PreparedOriginalUvCurve<'_> {
    fn active_range(&self) -> [f64; 2] {
        self.evaluator().active_range()
    }
    fn exact_affine_coefficients(
        &self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<parametric::ExactAffineCurve>, GeometryError> {
        self.evaluator().exact_affine_coefficients(work, cancel)
    }
    fn source_parameter_map(&self) -> Option<[Interval; 2]> {
        self.evaluator().source_parameter_map()
    }
    fn source_parameter_image(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Interval>, GeometryError> {
        self.evaluator().source_parameter_image(range, work, cancel)
    }
    fn tensor(
        &self,
        range: [f64; 2],
        order: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[[Interval; 3]; 4], GeometryError> {
        let active = self.active_range();
        Interval::new(range[0], range[1])?;
        if order > 3 || range[0] < active[0] || range[1] > active[1] {
            return Err(unavailable(
                "selected original UV curve query leaves its actual coefficient restriction/order",
            ));
        }
        if let Some(affine) = self.exact_affine_coefficients(work, cancel)? {
            return exact_affine_tensor(&affine, range, order, work, cancel);
        }
        self.evaluator().tensor(range, order, work, cancel)
    }
    fn continuous(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.evaluator().continuous(range, work, cancel)
    }
    fn smooth(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.evaluator().smooth(range, work, cancel)
    }
    fn parameter_spans(
        &self,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<parametric::AxisSpans<'_>, GeometryError> {
        self.evaluator().parameter_spans(range, work, cancel)
    }
    fn original_curve(&self) -> Option<&solidcraft_step::geom::Curve> {
        self.evaluator().original_curve()
    }
}

#[derive(Clone, Copy)]
pub(crate) enum PreparedTrimCurve<'p, 'd> {
    Native(&'p Curve),
    Original(&'p PreparedOriginalUvCurve<'d>),
    /// Complete-path geometry only; its interior parameter is NOT native gauge.
    GeometricPcurve(crate::analysis::source_restriction::pcurve::SourcePcurveGeometry<'p, 'd>),
    Completion(&'p SourceEndpointConnector),
}
impl PreparedTrimCurve<'_, '_> {
    fn evaluator(&self) -> &dyn parametric::CurveEvaluator {
        match self {
            Self::Native(curve) => *curve,
            Self::Original(curve) => *curve,
            Self::GeometricPcurve(curve) => curve,
            Self::Completion(curve) => *curve,
        }
    }
}
impl parametric::CurveEvaluator for PreparedTrimCurve<'_, '_> {
    fn active_range(&self) -> [f64; 2] {
        self.evaluator().active_range()
    }
    fn exact_affine_coefficients(
        &self,
        w: &mut GeometryWorkBudget,
        c: &AtomicBool,
    ) -> Result<Option<parametric::ExactAffineCurve>, GeometryError> {
        self.evaluator().exact_affine_coefficients(w, c)
    }
    fn source_parameter_map(&self) -> Option<[Interval; 2]> {
        self.evaluator().source_parameter_map()
    }
    fn source_parameter_image(
        &self,
        r: [f64; 2],
        w: &mut GeometryWorkBudget,
        c: &AtomicBool,
    ) -> Result<Option<Interval>, GeometryError> {
        self.evaluator().source_parameter_image(r, w, c)
    }
    fn tensor(
        &self,
        r: [f64; 2],
        o: usize,
        w: &mut GeometryWorkBudget,
        c: &AtomicBool,
    ) -> Result<[[Interval; 3]; 4], GeometryError> {
        self.evaluator().tensor(r, o, w, c)
    }
    fn continuous(
        &self,
        r: [f64; 2],
        w: &mut GeometryWorkBudget,
        c: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.evaluator().continuous(r, w, c)
    }
    fn smooth(
        &self,
        r: [f64; 2],
        w: &mut GeometryWorkBudget,
        c: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.evaluator().smooth(r, w, c)
    }
    fn parameter_spans(
        &self,
        r: [f64; 2],
        w: &mut GeometryWorkBudget,
        c: &AtomicBool,
    ) -> Result<parametric::AxisSpans<'_>, GeometryError> {
        self.evaluator().parameter_spans(r, w, c)
    }
    fn native_curve(&self) -> Option<&Curve> {
        self.evaluator().native_curve()
    }
    fn original_curve(&self) -> Option<&solidcraft_step::geom::Curve> {
        self.evaluator().original_curve()
    }
}

impl<'d> PreparedFaceTrimDomain<'d> {
    pub(crate) fn coedge_operand(
        &self,
        index: usize,
    ) -> Result<PreparedTrimCurve<'_, 'd>, GeometryError> {
        if self.original_surface.is_some() {
            return self
                .original_coedges
                .get(index)
                .map(|coedge| PreparedTrimCurve::Original(&coedge.curve))
                .ok_or_else(|| unavailable("selected original coedge ordinal is absent"));
        }
        self.coedges
            .get(index)
            .map(|coedge| PreparedTrimCurve::Native(coedge.curve))
            .ok_or_else(|| unavailable("native coedge ordinal is absent"))
    }
    /// Same complete oriented boundary image and endpoint expressions, with a
    /// potentially different interior gauge. Only parameterization-invariant
    /// path operations may use this; physical ribbons, cuts and correspondence
    /// queries must keep `coedge_operand`.
    pub(crate) fn coedge_geometric_operand(
        &self,
        index: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<PreparedTrimCurve<'_, 'd>, GeometryError> {
        work.charge(1, cancel)?;
        if self.original_surface.is_some() {
            let coedge = self
                .original_coedges
                .get(index)
                .ok_or_else(|| unavailable("geometric original coedge ordinal is absent"))?;
            if let OriginalUvCurve::Pcurve(curve) = &coedge.curve.curve
                && let Some(geometry) = curve.geometric_path(work, cancel)?
            {
                return Ok(PreparedTrimCurve::GeometricPcurve(geometry));
            }
        }
        self.coedge_operand(index)
    }
    pub(crate) fn original_coedge_correspondence(
        &self,
        index: usize,
    ) -> Result<&OriginalCoedgeCorrespondence, GeometryError> {
        self.original_coedges
            .get(index)
            .map(|coedge| &coedge.correspondence)
            .ok_or_else(|| unavailable("selected original coedge correspondence is unavailable"))
    }
    pub(crate) fn original_spatial_restriction(
        &self,
        index: usize,
    ) -> Result<&crate::analysis::source_restriction::SourceSpatialRestriction<'d>, GeometryError>
    {
        self.original_coedges
            .get(index)
            .map(|coedge| coedge.curve.spatial.operator.as_ref())
            .ok_or_else(|| unavailable("actual original spatial restriction is unavailable"))
    }
    pub(crate) fn with_original_spatial_curve<R>(
        &self,
        index: usize,
        allowance: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
        callback: impl FnOnce(
            &dyn parametric::CurveEvaluator,
            &mut GeometryWorkBudget,
            &AtomicBool,
        ) -> Result<R, GeometryError>,
    ) -> Result<R, GeometryError> {
        work.charge(1, cancel)?;
        memory(
            super::source_operand::SOURCE_OPERAND_SCRATCH_BYTES,
            allowance,
        )?;
        callback(self.original_spatial_restriction(index)?, work, cancel)
    }
    pub(crate) fn original_coedge_uv_piecewise_jet(
        &self,
        index: usize,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<parametric::ChartCurveEnclosure, GeometryError> {
        let receipt = self.original_coedge_correspondence(index)?;
        let parameter = receipt.uv_parameter_image(range)?;
        let curve = self.coedge_operand(index)?;
        if !curve.continuous([parameter.lo, parameter.hi], work, cancel)? {
            return Err(unavailable(
                "original UV query lacks literal C0 coefficient continuity",
            ));
        }
        let mut raw =
            parametric::curve2d_piecewise_jet(&curve, [parameter.lo, parameter.hi], work, cancel)?;
        let full = self.original_coedges[index].full_uv_image;
        for axis in 0..2 {
            raw.position[axis] = raw.position[axis].intersect(full[axis])?;
        }
        let scale = receipt.uv_parameter_derivative();
        Ok(parametric::ChartCurveEnclosure {
            position: raw.position,
            derivative: raw.derivative.map(|v| v.mul(scale)),
            second_derivative: raw.second_derivative.map(|v| v.mul(scale.square())),
        })
    }
    pub(crate) fn original_coedge_uv_c1_jet(
        &self,
        index: usize,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<parametric::ChartCurveEnclosure>, GeometryError> {
        let parameter = self
            .original_coedge_correspondence(index)?
            .uv_parameter_image(range)?;
        if !self
            .coedge_operand(index)?
            .smooth([parameter.lo, parameter.hi], work, cancel)?
        {
            return Ok(None);
        }
        self.original_coedge_uv_piecewise_jet(index, range, work, cancel)
            .map(Some)
    }
    pub(crate) fn original_coedge_uv_jet(
        &self,
        index: usize,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<parametric::ChartCurveEnclosure, GeometryError> {
        self.original_coedge_uv_c1_jet(index, range, work, cancel)?
            .ok_or_else(|| {
                unavailable(
                    "selected original UV restriction lacks independent closed C1 qualification",
                )
            })
    }
    pub(crate) fn original_coedge_jets(
        &self,
        index: usize,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(parametric::CurveEnclosure, parametric::ChartCurveEnclosure), GeometryError> {
        let spatial = parametric::curve_enclosure(
            self.original_spatial_restriction(index)?,
            (range[0], range[1]),
            work,
            cancel,
        )?;
        let chart = self.original_coedge_uv_piecewise_jet(index, range, work, cancel)?;
        Ok((spatial, chart))
    }
    pub(crate) fn same_original_spatial_trace(
        &self,
        index: usize,
        other: &PreparedFaceTrimDomain<'_>,
        other_index: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        work.charge(1, cancel)?;
        if !std::ptr::eq(self.definition, other.definition) {
            return Ok(false);
        }
        self.original_spatial_restriction(index)?.same_operator(
            other.original_spatial_restriction(other_index)?,
            work,
            cancel,
        )
    }
    pub(crate) fn material_realization(&self) -> super::OriginalMaterialRealization<'d> {
        super::OriginalMaterialRealization::new(self.definition)
    }
}

#[derive(Clone, Copy, Debug)]
struct SourceJointEndpointBinding {
    identity: NativeCoedgeIdentity,
    source_index: usize,
    uv_parameter: f64,
    spatial_high: bool,
    source_vertex: spiling_contracts::geometry::StepEntityId,
}
impl PreparedTrimJoint {
    /// Exact ownership and expression association, not numerical endpoint
    /// equality. Slot zero is the material incoming endpoint, slot one outgoing.
    pub(crate) fn connector_endpoint_is_bound(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        endpoint: usize,
        coedge_index: usize,
        spatial_high: bool,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        work.charge(1, cancel)?;
        let Some(key) = self.endpoint_bindings.get(endpoint) else {
            return Ok(false);
        };
        let mut owned = false;
        for saved in domain.original_joints()? {
            work.charge(1, cancel)?;
            if std::ptr::eq(self, saved) {
                owned = true;
                break;
            }
        }
        if !owned {
            return Ok(false);
        }
        let Some(native) = domain.coedges.get(coedge_index) else {
            return Ok(false);
        };
        let receipt = domain.original_coedge_correspondence(coedge_index)?;
        let binding = &receipt.binding;
        let canonical = binding.native_endpoint_index(spatial_high);
        let expected_identity = if endpoint == 0 {
            self.incoming
        } else {
            self.outgoing
        };
        let expected_high = if endpoint == 0 {
            binding.material_forward
        } else {
            !binding.material_forward
        };
        Ok(key.identity == native.identity()
            && key.identity == expected_identity
            && key.source_index == receipt.source_index()
            && key.spatial_high == spatial_high
            && spatial_high == expected_high
            && key.uv_parameter == receipt.uv_endpoint(spatial_high)
            && key.source_vertex == binding.source_vertex_ids[canonical]
            && self.source_vertex_id == key.source_vertex
            && self.vertex_id == binding.endpoints[canonical]
            && self.vertex_point == binding.source_vertex_points_mm[canonical])
    }
}
impl PreparedFaceTrimDomain<'_> {
    pub(crate) fn original_uv_margin(&self) -> Result<[f64; 2], GeometryError> {
        self.original_margin_uv.ok_or_else(|| {
            unavailable("selected original UV policy is unavailable in a native-only chart")
        })
    }
    /// Measured selected-original chart error; never the admissible ceiling.
    pub(crate) fn source_chart_error_mm(&self) -> f64 {
        self.original_chart_error_mm
            .expect("source-only metadata requires an admitted original domain")
    }
}

fn exact_ratio(
    numerator: &ExactCoefficient,
    denominator: &ExactCoefficient,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Interval, GeometryError> {
    let enclosure = exact_enclosure(numerator).div(exact_enclosure(denominator))?;
    let proposal = enclosure.mid();
    if proposal.is_finite() {
        let product = exact_product(
            &ExactCoefficient::point(proposal),
            denominator,
            work,
            cancel,
        )?;
        let mut difference = ExactCoefficient::point(0.0);
        exact_accumulate(&mut difference, numerator)?;
        difference.subtract(&product).ok_or_else(|| {
            unavailable("source rational identity exceeds exact arithmetic capacity")
        })?;
        if difference.sign() == 0 {
            return Ok(Interval::point(proposal));
        }
    }
    Ok(enclosure)
}
fn exact_affine_tensor(
    affine: &parametric::ExactAffineCurve,
    range: [f64; 2],
    order: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[[Interval; 3]; 4], GeometryError> {
    Interval::new(range[0], range[1])?;
    let mut result = [[Interval::point(0.0); 3]; 4];
    for axis in 0..3 {
        let mut endpoints = [Interval::point(0.0); 2];
        for (index, parameter) in range.into_iter().enumerate() {
            let term = exact_product(
                &affine.derivative[axis],
                &ExactCoefficient::point(parameter),
                work,
                cancel,
            )?;
            let mut value = ExactCoefficient::point(0.0);
            exact_accumulate(&mut value, &affine.position[axis])?;
            exact_accumulate(&mut value, &term)?;
            endpoints[index] = exact_ratio(&value, &affine.denominator, work, cancel)?;
        }
        result[0][axis] = endpoints[0].hull(endpoints[1]);
        if order > 0 {
            result[1][axis] =
                exact_ratio(&affine.derivative[axis], &affine.denominator, work, cancel)?;
        }
    }
    Ok(result)
}

fn affine_endpoint_identity(
    a: &parametric::ExactAffineCurve,
    a_range: [f64; 2],
    b: &parametric::ExactAffineCurve,
    b_range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    if a.denominator.sign() == 0 || b.denominator.sign() == 0 {
        return Ok(false);
    }
    for endpoint in 0..2 {
        for axis in 0..3 {
            let mut av = ExactCoefficient::point(0.0);
            let mut bv = ExactCoefficient::point(0.0);
            exact_accumulate(&mut av, &a.position[axis])?;
            exact_accumulate(&mut bv, &b.position[axis])?;
            exact_accumulate(
                &mut av,
                &exact_product(
                    &a.derivative[axis],
                    &ExactCoefficient::point(a_range[endpoint]),
                    work,
                    cancel,
                )?,
            )?;
            exact_accumulate(
                &mut bv,
                &exact_product(
                    &b.derivative[axis],
                    &ExactCoefficient::point(b_range[endpoint]),
                    work,
                    cancel,
                )?,
            )?;
            let mut difference = exact_product(&av, &b.denominator, work, cancel)?;
            difference
                .subtract(&exact_product(&bv, &a.denominator, work, cancel)?)
                .ok_or_else(|| {
                    unavailable("whole affine source association exceeds exact arithmetic capacity")
                })?;
            if difference.sign() != 0 {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn same_source_curve(
    a: &solidcraft_step::geom::Curve,
    b: &solidcraft_step::geom::Curve,
    depth: u8,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    use solidcraft_step::geom::{Associated, Curve as C};
    work.charge(1, cancel)?;
    if std::ptr::eq(a, b) {
        return Ok(true);
    }
    if depth == MAX_DEPTH {
        return Err(resource(
            "original coefficient identity wrapper depth exceeds the bounded inventory",
        ));
    }
    Ok(match (a, b) {
        (C::Line { .. }, C::Line { .. })
        | (C::Circle { .. }, C::Circle { .. })
        | (C::Ellipse { .. }, C::Ellipse { .. })
        | (C::Hyperbola { .. }, C::Hyperbola { .. })
        | (C::Parabola { .. }, C::Parabola { .. }) => a == b,
        (C::BSpline(x), C::BSpline(y)) => {
            work.charge(
                (x.points.len()
                    + y.points.len()
                    + x.knots.len()
                    + y.knots.len()
                    + x.multiplicities.len()
                    + y.multiplicities.len()
                    + x.weights.as_ref().map_or(0, Vec::len)
                    + y.weights.as_ref().map_or(0, Vec::len)) as u64,
                cancel,
            )?;
            x == y
        }
        (C::Polyline(x), C::Polyline(y)) => {
            work.charge((x.len() + y.len()) as u64, cancel)?;
            x == y
        }
        (
            C::Trimmed {
                basis: x,
                trim_1: x0,
                trim_2: x1,
                sense: xs,
                preference: xp,
            },
            C::Trimmed {
                basis: y,
                trim_1: y0,
                trim_2: y1,
                sense: ys,
                preference: yp,
            },
        ) => {
            work.charge((x0.len() + x1.len() + y0.len() + y1.len()) as u64, cancel)?;
            xs == ys
                && xp == yp
                && x0 == y0
                && x1 == y1
                && same_source_curve(x, y, depth + 1, work, cancel)?
        }
        (
            C::SurfaceCurve {
                kind: xk,
                curve_3d: x,
                associated: xa,
                master: xm,
            },
            C::SurfaceCurve {
                kind: yk,
                curve_3d: y,
                associated: ya,
                master: ym,
            },
        ) => {
            if xk != yk
                || xm != ym
                || xa.len() != ya.len()
                || !same_source_curve(x, y, depth + 1, work, cancel)?
            {
                return Ok(false);
            }
            for (x, y) in xa.iter().zip(ya) {
                work.charge(1, cancel)?;
                let equal = match (x, y) {
                    (Associated::Pcurve(x), Associated::Pcurve(y)) => {
                        same_source_surface(&x.surface, &y.surface, depth + 1, work, cancel)?
                            && same_source_curve(&x.curve, &y.curve, depth + 1, work, cancel)?
                    }
                    (Associated::Surface(x), Associated::Surface(y)) => {
                        same_source_surface(x, y, depth + 1, work, cancel)?
                    }
                    _ => false,
                };
                if !equal {
                    return Ok(false);
                }
            }
            true
        }
        (
            C::OffsetCurve3d {
                basis: x,
                distance: xd,
                self_intersect: xi,
                ref_direction: xr,
            },
            C::OffsetCurve3d {
                basis: y,
                distance: yd,
                self_intersect: yi,
                ref_direction: yr,
            },
        ) => xd == yd && xi == yi && xr == yr && same_source_curve(x, y, depth + 1, work, cancel)?,
        _ => false,
    })
}
fn same_source_surface(
    a: &solidcraft_step::geom::Surface,
    b: &solidcraft_step::geom::Surface,
    depth: u8,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    use solidcraft_step::geom::Surface as S;
    work.charge(1, cancel)?;
    if std::ptr::eq(a, b) {
        return Ok(true);
    }
    if depth == MAX_DEPTH {
        return Err(resource(
            "original carrier identity wrapper depth exceeds the bounded inventory",
        ));
    }
    Ok(match (a, b) {
        (S::Plane { .. }, S::Plane { .. })
        | (S::Cylinder { .. }, S::Cylinder { .. })
        | (S::Cone { .. }, S::Cone { .. })
        | (S::Sphere { .. }, S::Sphere { .. })
        | (S::Torus { .. }, S::Torus { .. })
        | (S::DegenerateTorus { .. }, S::DegenerateTorus { .. }) => a == b,
        (S::BSpline(x), S::BSpline(y)) => {
            for surface in [x, y] {
                for row in &surface.points {
                    work.charge(row.len() as u64 + 1, cancel)?;
                }
                if let Some(weights) = &surface.weights {
                    for row in weights {
                        work.charge(row.len() as u64 + 1, cancel)?;
                    }
                }
                work.charge(
                    (surface.u_knots.len()
                        + surface.v_knots.len()
                        + surface.u_multiplicities.len()
                        + surface.v_multiplicities.len()) as u64,
                    cancel,
                )?;
            }
            x == y
        }
        (
            S::Revolution {
                profile: x,
                axis: xa,
            },
            S::Revolution {
                profile: y,
                axis: ya,
            },
        ) => xa == ya && same_source_curve(x, y, depth + 1, work, cancel)?,
        (
            S::Extrusion {
                profile: x,
                direction: xd,
                magnitude: xm,
            },
            S::Extrusion {
                profile: y,
                direction: yd,
                magnitude: ym,
            },
        ) => xd == yd && xm == ym && same_source_curve(x, y, depth + 1, work, cancel)?,
        (
            S::Offset {
                basis: x,
                distance: xd,
                self_intersect: xi,
            },
            S::Offset {
                basis: y,
                distance: yd,
                self_intersect: yi,
            },
        ) => xd == yd && xi == yi && same_source_surface(x, y, depth + 1, work, cancel)?,
        (
            S::RectangularTrimmed {
                basis: x,
                u1: x0,
                u2: x1,
                v1: x2,
                v2: x3,
                u_sense: xu,
                v_sense: xv,
            },
            S::RectangularTrimmed {
                basis: y,
                u1: y0,
                u2: y1,
                v1: y2,
                v2: y3,
                u_sense: yu,
                v_sense: yv,
            },
        ) => {
            x0 == y0
                && x1 == y1
                && x2 == y2
                && x3 == y3
                && xu == yu
                && xv == yv
                && same_source_surface(x, y, depth + 1, work, cancel)?
        }
        _ => false,
    })
}

/// Exact copied-basis lane. The normalized source operator's *actual affine
/// endpoint recipe* is transported unchanged; a nonrepresentable knot preimage
/// is never replaced by a Hermite node or a rounded cover fence.
fn copied_source_spline_identity(
    reference: &dyn parametric::CurveEvaluator,
    target: &dyn parametric::CurveEvaluator,
    source_range: [f64; 2],
    target_range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    use solidcraft_step::geom::Curve as C;
    let Some(mut original) = reference.original_curve() else {
        return Ok(false);
    };
    for depth in 0..MAX_DEPTH {
        work.charge(1, cancel)?;
        match original {
            C::Trimmed { basis, .. } => original = basis,
            C::SurfaceCurve { curve_3d, .. } => {
                if super::source_operand::preferred_source_pcurve(original)?.is_some() {
                    return Ok(false);
                }
                original = curve_3d;
            }
            _ => break,
        }
        if depth + 1 == MAX_DEPTH {
            return Err(resource("copied original spline wrapper depth exceeded"));
        }
    }
    let C::BSpline(source) = original else {
        return Ok(false);
    };
    let Some(native) = parametric::native_curve_nurbs(target)? else {
        return Ok(false);
    };
    if reference.source_parameter_map().is_none()
        || source.degree != native.degree()
        || source.points.len() != native.cpts().len()
    {
        return Ok(false);
    }
    for endpoint in 0..2 {
        let Some(image) =
            reference.source_parameter_image([source_range[endpoint]; 2], work, cancel)?
        else {
            return Ok(false);
        };
        if image.lo != image.hi || image.lo != target_range[endpoint] {
            return Ok(false);
        }
    }
    let mut index = 0usize;
    for (&knot, &multiplicity) in source.knots.iter().zip(&source.multiplicities) {
        work.charge(multiplicity as u64 + 1, cancel)?;
        for _ in 0..multiplicity {
            if native.knots().get(index) != Some(&knot) {
                return Ok(false);
            }
            index = index
                .checked_add(1)
                .ok_or_else(|| resource("copied spline knot inventory overflow"))?;
        }
    }
    if index != native.knots().len() || source.knots.len() != source.multiplicities.len() {
        return Ok(false);
    }
    for (index, (point, coefficient)) in source.points.iter().zip(native.cpts()).enumerate() {
        work.charge(1, cancel)?;
        let weight = source
            .weights
            .as_ref()
            .map_or(1.0, |weights| weights[index]);
        let native_weight = if native.is_rational() {
            coefficient.w
        } else {
            1.0
        };
        if weight <= 0.0 || weight != native_weight {
            return Ok(false);
        }
        let coordinates = [coefficient.x, coefficient.y, coefficient.z];
        for axis in 0..3 {
            Interval::new(point[axis], point[axis])?;
            Interval::new(coordinates[axis], coordinates[axis])?;
            let mut difference = exact_product(
                &ExactCoefficient::point(point[axis]),
                &ExactCoefficient::point(weight),
                work,
                cancel,
            )?;
            difference.push(-coordinates[axis]).ok_or_else(|| {
                unavailable(
                    "copied source spline coefficient identity exceeds exact arithmetic capacity",
                )
            })?;
            if difference.sign() != 0 {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

type SourceSpatialLookup =
    std::collections::HashMap<(spiling_contracts::geometry::StepEntityId, EdgeId, bool), usize>;
fn source_lookup_bytes(capacity: usize) -> Result<usize, GeometryError> {
    capacity
        .checked_add(16)
        .and_then(|n| n.checked_mul(2))
        .and_then(|n| {
            n.checked_mul(
                size_of::<(
                    (spiling_contracts::geometry::StepEntityId, EdgeId, bool),
                    usize,
                )>() + 2 * size_of::<usize>(),
            )
        })
        .ok_or_else(|| resource("source mapping lookup byte count overflow"))
}
struct SelectedSourceUse<'d> {
    source_index: usize,
    spatial: Arc<PreparedSharedSpatialCurve<'d>>,
    material_forward: bool,
}
struct SelectedSourceWire<'d> {
    source_wire: usize,
    uses: Vec<SelectedSourceUse<'d>>,
}
struct NativeFaceSkeleton<'d> {
    definition: &'d Definition,
    face: FaceId,
    face_index: usize,
    material_orientation: bool,
    coedges: Vec<NativeTrimCoedge<'d>>,
    loops: Vec<[usize; 2]>,
}
impl<'d> NativeFaceSkeleton<'d> {
    fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            + self.coedges.capacity() * size_of::<NativeTrimCoedge<'_>>()
            + self.loops.capacity() * size_of::<[usize; 2]>()
    }
    /// Native incidence skeleton only. Generated UV C0 is not substituted for
    /// original chart admission, and no native guide receives source authority.
    fn new(
        definition: &'d Definition,
        face: FaceId,
        incidence: &crate::analysis::source_incidence::SourceEdgeIncidence<'d>,
        allowance: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        let face_index = definition
            .face_ordinal(face)
            .ok_or_else(|| unavailable("source face has no native arena ordinal"))?;
        let record = definition.brep().face(face).map_err(native_error)?;
        let material_orientation = crate::analysis::source_incidence::face_orientation(
            definition, face_index, work, cancel,
        )?;
        let mut count = 0usize;
        for &wire in &record.loops {
            work.charge(1, cancel)?;
            let wire = definition.brep().loop_(wire).map_err(native_error)?;
            if wire.face != face || wire.edges.is_empty() {
                return Err(unavailable(format!(
                    "native face {face:?} has a vertex/empty loop: original vertex-loop chart/quotient ownership must be constructed, not an invented coedge or omitted loop"
                )));
            }
            count = count
                .checked_add(wire.edges.len())
                .ok_or_else(|| resource("source coedge count overflow"))?;
        }
        memory(
            size_of::<Self>()
                + checked_bytes::<NativeTrimCoedge<'_>>(count)?
                + checked_bytes::<[usize; 2]>(record.loops.len())?,
            allowance,
        )?;
        let mut domain = Self {
            definition,
            face,
            face_index,
            material_orientation,
            coedges: Vec::new(),
            loops: Vec::new(),
        };
        domain
            .coedges
            .try_reserve_exact(count)
            .map_err(|_| resource("source native-use allocation failed"))?;
        domain
            .loops
            .try_reserve_exact(record.loops.len())
            .map_err(|_| resource("source wire allocation failed"))?;
        for (wire_index, &wire) in record.loops.iter().enumerate() {
            let wire = definition.brep().loop_(wire).map_err(native_error)?;
            let start = domain.coedges.len();
            for (index, entry) in wire.edges.iter().enumerate() {
                work.charge(1, cancel)?;
                let pair = incidence.get(entry.edge).ok_or_else(|| {
                    unavailable("native source edge is absent from complete opposed incidence")
                })?;
                let use_ = std::iter::once(pair.first)
                    .chain(pair.second)
                    .find(|use_| {
                        use_.face == face_index && use_.wire == wire_index && use_.index == index
                    })
                    .ok_or_else(|| {
                        unavailable(
                            "native ordered source use is absent from complete physical incidence",
                        )
                    })?;
                let edge = definition.brep().edge(entry.edge).map_err(native_error)?;
                let native_range = [edge.interval.min(), edge.interval.max()];
                Interval::new(native_range[0], native_range[1])?;
                if native_range[0] >= native_range[1] || edge.curve.is_none() {
                    return Err(unavailable(
                        "native source edge lacks a complete nondegenerate coefficient restriction",
                    ));
                }
                let (curve, owner) = edgeuse::uv_trim_curve(definition.brep(), use_.edgeuse)
                    .map_err(native_error)?;
                let curve=curve.ok_or_else(||unavailable(format!("native edgeuse {:?} has no UV coefficient guide; original operands are not replaced by a fictitious guide",use_.edgeuse)))?;
                let owner =
                    owner.ok_or_else(|| unavailable("native UV guide has no arena owner"))?;
                if edgeuse::face(definition.brep(), owner).map_err(native_error)? != Some(face) {
                    return Err(unavailable(
                        "native UV guide belongs to a different carrier face",
                    ));
                }
                let interval = curve.natural_interval();
                let uv_range = [interval.min(), interval.max()];
                Interval::new(uv_range[0], uv_range[1])?;
                if uv_range[0] >= uv_range[1] {
                    return Err(unavailable(
                        "native source-use UV guide has no complete bounded restriction",
                    ));
                }
                domain.coedges.push(NativeTrimCoedge {
                    edgeuse: use_.edgeuse,
                    uv_owner: owner,
                    edge: entry.edge,
                    vertices: edge.vertices,
                    native_range,
                    uv_range,
                    forward: entry.forward,
                    c1: false,
                    curve,
                    loop_index: wire_index,
                    face,
                    index,
                });
            }
            domain.loops.push([start, domain.coedges.len()]);
        }
        memory(domain.retained_bytes(), allowance)?;
        Ok(domain)
    }
}
impl<'d> PreparedNativeTrimSource<'d> {
    fn source_spatial_use(
        &mut self,
        lookup: &mut SourceSpatialLookup,
        source: &'d super::source::OriginalCoedgeSource,
        native: &NativeTrimCoedge<'d>,
        native_orientation: bool,
        material_forward: bool,
        ceiling: f64,
        extra_live: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Arc<PreparedSharedSpatialCurve<'d>>, GeometryError> {
        work.charge(1, cancel)?;
        let spatial_forward = material_forward == (native.forward == native_orientation);
        let key = (source.edge_curve_id, native.edge, spatial_forward);
        if let Some(&index) = lookup.get(&key) {
            let saved = &self.spatial_operators[index];
            if saved.source.curve_id != source.curve_id
                || saved.source.vertex_ids != source.vertex_ids
                || saved.source.vertex_points_mm != source.vertex_points_mm
                || saved.source.same_sense != source.same_sense
                || !same_source_curve(
                    &saved.source.spatial_curve,
                    &source.spatial_curve,
                    0,
                    work,
                    cancel,
                )?
            {
                return Err(unavailable(
                    "opposed uses of one original EDGE_CURVE disagree in literal geometry, selectors, master, endpoint identities or sense",
                ));
            }
            return Ok(Arc::clone(&saved.operator));
        }
        let native_curve = self.definition.edge_curve(native.edge).ok_or_else(|| {
            unavailable("native source edge has no actual spatial coefficient curve")
        })?;
        let target_range = if spatial_forward {
            native.native_range
        } else {
            [native.native_range[1], native.native_range[0]]
        };
        // Necessary endpoint condition only. This rejects impossible candidates;
        // it does not promote vertex proximity into a source binding.
        let twice = Interval::point(ceiling).mul(Interval::point(2.0)).hi;
        for endpoint in 0..2 {
            let point = native_curve.tensor([target_range[endpoint]; 2], 0, work, cancel)?[0];
            let gap = norm(std::array::from_fn(|axis| {
                point[axis].sub(Interval::point(source.vertex_points_mm[endpoint][axis]))
            }))?;
            if gap.lo > twice {
                return Err(unavailable(
                    "native endpoint germ cannot lie in the original endpoint plus complete source/native trace tubes",
                ));
            }
        }
        let lookup_bytes = source_lookup_bytes(lookup.capacity())?;
        let available = self
            .allowance
            .checked_sub(self.retained_bytes())
            .and_then(|n| n.checked_sub(extra_live))
            .and_then(|n| n.checked_sub(lookup_bytes))
            .ok_or_else(|| {
                resource("source spatial preparation exceeds the shared owner allowance")
            })?;
        let overhead = size_of::<PreparedSharedSpatialCurve<'_>>() + 4 * size_of::<usize>();
        let operand_allowance = available
            .checked_sub(overhead)
            .ok_or_else(|| resource("source spatial operator header exceeds allowance"))?;
        let operator = crate::analysis::source_restriction::SourceSpatialRestriction::new(
            source,
            ceiling,
            operand_allowance,
            work,
            cancel,
        )?;
        let map_allowance = available
            .checked_sub(operator.retained_bytes())
            .and_then(|n| n.checked_sub(overhead))
            .and_then(|n| n.checked_add(size_of::<SourceCurveCalibration>()))
            .ok_or_else(|| resource("native source-association proof exceeds allowance"))?;
        let native_map = calibrate_source_curves(
            &operator,
            native_curve,
            target_range,
            ceiling,
            MAX_DEPTH,
            map_allowance,
            work,
            cancel,
        )?;
        let shared = PreparedSharedSpatialCurve {
            operator: Arc::new(operator),
            native_map,
            edge: native.edge,
            spatial_forward,
        };
        let payload = shared.retained_bytes() + 2 * size_of::<usize>();
        memory(
            self.retained_bytes()
                + extra_live
                + lookup_bytes
                + payload
                + size_of::<SourceSpatialCacheEntry<'_>>(),
            self.allowance,
        )?;
        if self.spatial_operators.len() == self.spatial_operators.capacity() {
            let additional = self.spatial_operators.capacity().max(1);
            memory(
                self.retained_bytes()
                    + extra_live
                    + lookup_bytes
                    + payload
                    + (self.spatial_operators.capacity() + additional)
                        * size_of::<SourceSpatialCacheEntry<'_>>(),
                self.allowance,
            )?;
            self.spatial_operators
                .try_reserve_exact(additional)
                .map_err(|_| resource("source spatial cache allocation failed"))?;
        }
        if lookup.len() == lookup.capacity() {
            let additional = lookup.capacity().max(1);
            memory(
                self.retained_bytes()
                    + extra_live
                    + payload
                    + lookup_bytes
                    + source_lookup_bytes((lookup.len() + additional) * 2 + 1)?,
                self.allowance,
            )?;
            lookup
                .try_reserve(additional)
                .map_err(|_| resource("source spatial lookup allocation failed"))?;
        }
        memory(
            self.retained_bytes() + extra_live + payload + source_lookup_bytes(lookup.capacity())?,
            self.allowance,
        )?;
        let operator = Arc::new(shared);
        let index = self.spatial_operators.len();
        self.spatial_operators.push(SourceSpatialCacheEntry {
            source,
            operator: Arc::clone(&operator),
        });
        lookup.insert(key, index);
        Ok(operator)
    }
    fn select_source_wire(
        &mut self,
        lookup: &mut SourceSpatialLookup,
        domain: &NativeFaceSkeleton<'d>,
        wire_index: usize,
        already_selected: &[SelectedSourceWire<'d>],
        ceiling: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<SelectedSourceWire<'d>, GeometryError> {
        let original = self.definition.original_source().face(domain.face)?;
        let [start, end] = domain.loops[wire_index];
        let count = end - start;
        let mut selected: Option<SelectedSourceWire<'d>> = None;
        let mut first_failure: Option<GeometryError> = None;
        for (source_wire, wire) in original.wires.iter().enumerate() {
            work.charge(1, cancel)?;
            if wire.vertex_loop.is_some() {
                return Err(unavailable(
                    "original VERTEX_LOOP requires its real native vertex binding and quotient-chart completion",
                ));
            }
            if wire.coedges.len() != count
                || (wire.declared_outer && wire_index != 0)
                || already_selected
                    .iter()
                    .any(|selected| selected.source_wire == source_wire)
            {
                continue;
            }
            let source_loop_material = wire.forward == original.face_use_forward;
            let same_order = source_loop_material == domain.material_orientation;
            for offset in 0..count {
                work.charge(1, cancel)?;
                let mut uses = Vec::new();
                let retained_choices = already_selected
                    .iter()
                    .map(|wire| wire.uses.capacity() * size_of::<SelectedSourceUse<'_>>())
                    .sum::<usize>()
                    + selected.as_ref().map_or(0, |wire| {
                        wire.uses.capacity() * size_of::<SelectedSourceUse<'_>>()
                    });
                let extra = domain.retained_bytes()
                    + retained_choices
                    + checked_bytes::<SelectedSourceUse<'_>>(count)?;
                memory(
                    self.retained_bytes() + extra + source_lookup_bytes(lookup.capacity())?,
                    self.allowance,
                )?;
                uses.try_reserve_exact(count)
                    .map_err(|_| resource("original whole-wire association allocation failed"))?;
                let mut admissible = true;
                for local in 0..count {
                    let position = if same_order {
                        (offset + local) % count
                    } else {
                        (offset + count - local) % count
                    };
                    let source_index = wire.coedges[position];
                    let source = self.definition.original_source().coedge(source_index)?;
                    if source.source_face != original.source_face
                        || source.bound_id != wire.bound_id
                        || source.loop_id != wire.loop_id
                    {
                        return Err(unavailable(
                            "original coedge genealogy does not belong to its complete declared source wire",
                        ));
                    }
                    let material_forward = source.forward == source_loop_material;
                    match self.source_spatial_use(
                        lookup,
                        source,
                        &domain.coedges[start + local],
                        domain.material_orientation,
                        material_forward,
                        ceiling,
                        extra,
                        work,
                        cancel,
                    ) {
                        Ok(spatial) => uses.push(SelectedSourceUse {
                            source_index,
                            spatial,
                            material_forward,
                        }),
                        Err(error)
                            if matches!(
                                error.code,
                                GeometryErrorCode::Cancelled | GeometryErrorCode::ResourceLimit
                            ) =>
                        {
                            return Err(error);
                        }
                        Err(error) => {
                            if first_failure.is_none() {
                                first_failure = Some(error);
                            }
                            admissible = false;
                            break;
                        }
                    }
                }
                if admissible {
                    if selected.is_some() {
                        return Err(unavailable(format!(
                            "native face {:?}, wire {wire_index} has multiple whole original wire/rotation assignments; coefficient tubes and topology do not establish unique original ownership",
                            domain.face
                        )));
                    }
                    selected = Some(SelectedSourceWire { source_wire, uses });
                }
            }
        }
        selected.ok_or_else(||unavailable(format!("native face {:?}, wire {wire_index} has no complete original directed-wire assignment: {}",
            domain.face,first_failure.map_or_else(||"original wire cardinality, outer/hole or ownership differs".to_owned(),|error|error.message))))
    }
}

impl<'d> PreparedFaceTrimDomain<'d> {
    fn boundary_operand(
        &self,
        boundary: BoundaryCurve,
    ) -> Result<PreparedTrimCurve<'_, 'd>, GeometryError> {
        match boundary {
            BoundaryCurve::Coedge(index) => self.coedge_operand(index),
            BoundaryCurve::Joint(index) => self
                .original_joints
                .get(index)
                .map(|joint| PreparedTrimCurve::Completion(&joint.connector))
                .ok_or_else(|| unavailable("source completion boundary ordinal is absent")),
        }
    }
    fn boundary_restriction(&self, boundary: BoundaryCurve) -> ([f64; 2], bool) {
        match boundary {
            BoundaryCurve::Coedge(index) => {
                let native = &self.coedges[index];
                match self.original_coedges.get(index) {
                    Some(original) => {
                        let receipt = &original.correspondence;
                        let spatial_raw = native.forward == receipt.binding.spatial_forward;
                        (
                            receipt.uv_range(),
                            spatial_raw == (receipt.uv_ends[1] > receipt.uv_ends[0]),
                        )
                    }
                    None => (native.uv_range, native.forward),
                }
            }
            BoundaryCurve::Joint(_) => ([0.0, 1.0], self.material_orientation),
        }
    }
    fn boundary_endpoint(&self, boundary: BoundaryCurve, end: usize) -> BoundaryEndpoint {
        let BoundaryCurve::Coedge(index) = boundary else {
            let BoundaryCurve::Joint(index) = boundary else {
                unreachable!()
            };
            return BoundaryEndpoint::Joint(index, end);
        };
        let native = &self.coedges[index];
        let [start, finish] = self.loops[native.loop_index];
        let raw_end = end == usize::from(self.boundary_restriction(boundary).1);
        let previous = if index == start {
            finish - 1
        } else {
            index - 1
        };
        let join = if raw_end { index } else { previous };
        match self.joint_after.get(join).copied().flatten() {
            Some(joint) if !self.original_joints[joint].connector.collapsed => {
                BoundaryEndpoint::Joint(joint, usize::from(raw_end != self.material_orientation))
            }
            _ => BoundaryEndpoint::ExactJoin(join),
        }
    }
}

struct OriginalCorrespondenceInventory {
    faces: usize,
    source_uses: usize,
    vertices: usize,
}
fn norm(value: [Interval; 3]) -> Result<Interval, GeometryError> {
    parametric::IntervalVec3 {
        x: value[0],
        y: value[1],
        z: value[2],
    }
    .norm()
}
impl PreparedFaceTrimDomain<'_> {
    /// Declaration provenance of the actual retained basis, not an inverse or
    /// endpoint-equality theorem.
    pub(crate) fn original_coedge_is_declared(&self, index: usize) -> Result<bool, GeometryError> {
        self.original_coedges
            .get(index)
            .map(|coedge| coedge.source_pcurve.is_some())
            .ok_or_else(|| unavailable("selected original coedge declaration scope is unavailable"))
    }
    /// True only for a retained selected expression with a proved
    /// nondegenerate original-plane inverse or an owned analytic cylindrical
    /// branch. Declaration or a native UV proposal cannot establish this.
    pub(crate) fn original_coedge_has_regular_inverse(
        &self,
        index: usize,
    ) -> Result<bool, GeometryError> {
        self.original_coedges
            .get(index)
            .map(|coedge| {
                matches!(
                    &coedge.curve.curve,
                    OriginalUvCurve::DerivedPlane(_) | OriginalUvCurve::DerivedCylinder(_)
                )
            })
            .ok_or_else(|| unavailable("selected original inverse scope is unavailable"))
    }
}

impl PreparedFaceTrimDomain<'_> {
    /// Complete coefficient-piece coverage in the actual selected spatial query
    /// gauge. A band is coverage/ordering data, never a feasible knot incidence.
    pub(crate) fn visit_original_coedge_span_boundaries(
        &self,
        index: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
        callback: &mut impl FnMut(Interval) -> Result<(), GeometryError>,
    ) -> Result<(), GeometryError> {
        let original = self
            .original_coedges
            .get(index)
            .ok_or_else(|| unavailable("original span owner is unavailable"))?;
        let range = original.correspondence.spatial_range();
        work.charge(2, cancel)?;
        callback(Interval::point(range[0]))?;
        callback(Interval::point(range[1]))?;
        for &band in original.curve.spatial.operator.knot_bands() {
            work.charge(1, cancel)?;
            callback(band)?;
        }
        if let OriginalUvCurve::Pcurve(curve) = &original.curve.curve {
            for &band in curve.knot_bands() {
                work.charge(1, cancel)?;
                callback(band)?;
            }
            if let Some(map) = curve.calibration() {
                for &cut in map.parameter_cuts() {
                    work.charge(1, cancel)?;
                    callback(Interval::point(cut))?;
                }
            }
        }
        Ok(())
    }
}

struct OriginalCoedgeProposal<'d> {
    curve: PreparedOriginalUvCurve<'d>,
    binding: OriginalCoedgeBinding,
    image: [Interval; 2],
    pcurve: Option<&'d super::source::OriginalPcurveSource>,
}
fn source_binding(
    native: &NativeTrimCoedge<'_>,
    face_ordinal: usize,
    face: &super::source::OriginalFaceSource,
    selected: &SelectedSourceUse<'_>,
    source: &super::source::OriginalCoedgeSource,
) -> OriginalCoedgeBinding {
    let forward = selected.spatial.spatial_forward;
    OriginalCoedgeBinding {
        face: native.face,
        edge: native.edge,
        edgeuse: native.edgeuse,
        endpoints: native.vertices,
        native_interval: native.native_range,
        owner: [face_ordinal, native.loop_index, native.index],
        source_face: Some(face.face_entity.0),
        source_face_use: Some(face.source_face.0),
        source: OriginalCoedgeGenealogy {
            source_index: selected.source_index,
            source_edge: Some(source.edge_curve_id.0),
            source_coedge: Some(source.oriented_edge_id.0),
            native_orientation: native.forward,
        },
        spatial_forward: forward,
        material_forward: selected.material_forward,
        source_vertex_points_mm: if forward {
            source.vertex_points_mm
        } else {
            [source.vertex_points_mm[1], source.vertex_points_mm[0]]
        },
        source_vertex_ids: if forward {
            source.vertex_ids
        } else {
            [source.vertex_ids[1], source.vertex_ids[0]]
        },
    }
}
fn source_uv_image(
    curve: &PreparedOriginalUvCurve<'_>,
    natural: Option<[[f64; 2]; 2]>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[Interval; 2], GeometryError> {
    let range = curve.active_range();
    let p = curve.tensor(range, 0, work, cancel)?[0];
    let mut image = [p[0], p[1]];
    let Some(natural) = natural else {
        return Ok(image);
    };
    if parametric::curve2d_image_within_box(curve, range, natural, work, cancel)? {
        for axis in 0..2 {
            image[axis] =
                image[axis].intersect(Interval::new(natural[axis][0], natural[axis][1])?)?;
        }
        return Ok(image);
    }
    if let Some(mut raw) = curve.original_curve() {
        for _ in 0..MAX_DEPTH {
            work.charge(1, cancel)?;
            match raw {
                solidcraft_step::geom::Curve::Trimmed { basis, .. } => raw = basis,
                solidcraft_step::geom::Curve::Line {
                    origin, direction, ..
                } => {
                    for axis in 0..2 {
                        if direction[axis] == 0.0 {
                            if origin[axis] < natural[axis][0] || origin[axis] > natural[axis][1] {
                                return Err(unavailable(
                                    "literal original PCurve constant coordinate leaves its carrier's original knot domain",
                                ));
                            }
                            image[axis] = Interval::point(origin[axis]);
                        }
                    }
                    break;
                }
                solidcraft_step::geom::Curve::BSpline(spline) => {
                    work.charge(spline.points.len() as u64 + 1, cancel)?;
                    let contained = spline.points.iter().enumerate().all(|(i, p)| {
                        spline
                            .weights
                            .as_ref()
                            .is_none_or(|w| w.get(i).is_some_and(|&v| v > 0.0))
                            && (0..2).all(|axis| {
                                p[axis] >= natural[axis][0] && p[axis] <= natural[axis][1]
                            })
                    });
                    if contained {
                        for axis in 0..2 {
                            image[axis] = image[axis]
                                .intersect(Interval::new(natural[axis][0], natural[axis][1])?)?;
                        }
                    }
                    break;
                }
                _ => break,
            }
        }
    }
    if (0..2).any(|axis| image[axis].lo < natural[axis][0] || image[axis].hi > natural[axis][1]) {
        return Err(unavailable(
            "complete actual original PCurve image cannot be proved inside the original carrier knot domain; an interval overbound is not licensed chart extrapolation",
        ));
    }
    Ok(image)
}
fn source_span_inventory(
    curve: &PreparedOriginalUvCurve<'_>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<OriginalSpanInventory, GeometryError> {
    let spatial = &curve.spatial.operator;
    work.charge(spatial.knot_bands().len() as u64 + 1, cancel)?;
    let mut inventory = OriginalSpanInventory {
        source_knot_boundaries: spatial.original_knots().len(),
        source_knot_bands: spatial
            .knot_bands()
            .iter()
            .filter(|band| band.lo < band.hi)
            .count(),
        uv_knot_boundaries: 0,
        uv_knot_bands: 0,
    };
    if let OriginalUvCurve::Pcurve(pcurve) = &curve.curve {
        work.charge(pcurve.knot_bands().len() as u64 + 1, cancel)?;
        inventory.uv_knot_boundaries = pcurve.original_knots().len()
            + pcurve
                .calibration()
                .map_or(0, |map| map.parameter_cuts().len());
        inventory.uv_knot_bands = pcurve
            .knot_bands()
            .iter()
            .filter(|band| band.lo < band.hi)
            .count();
    }
    Ok(inventory)
}
impl<'d> PreparedNativeTrimSource<'d> {
    /// Prepare actual original chart/coedge operators and a complete source-use
    /// census. Global filled-material and assembled F/G/V embedding authority is
    /// deliberately not a postcondition of this operand owner.
    pub(crate) fn prepare_originals(
        definition: &'d Definition,
        allowance: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        work.charge(1, cancel)?;
        let ceiling = definition.provenance().uncertainty_mm.ok_or_else(|| {
            unavailable(
                "original spatial realization lacks the captured physical uncertainty operand",
            )
        })?;
        if !ceiling.is_finite() || ceiling < 0.0 {
            return Err(unavailable(
                "captured original realization uncertainty is not finite/nonnegative",
            ));
        }
        let count = definition.face_ids().len();
        if count == 0 {
            return Err(unavailable(
                "original source has no native face arena owners",
            ));
        }
        memory(
            size_of::<Self>() + checked_bytes::<PreparedFaceTrimDomain<'_>>(count)?,
            allowance,
        )?;
        let mut incidence = crate::analysis::source_incidence::SourceEdgeIncidence::new(
            definition,
            allowance.saturating_sub(size_of::<Self>()),
            work,
            cancel,
        )?;
        incidence.prepare_vertices(allowance.saturating_sub(size_of::<Self>()), work, cancel)?;
        for vertex in incidence
            .vertices()
            .ok_or_else(|| unavailable("complete native source vertex census is absent"))?
        {
            work.charge(1, cancel)?;
            if !incidence.vertex_link_is_cycle(definition, vertex, work, cancel)? {
                return Err(unavailable(format!(
                    "native source vertex {vertex:?} has no complete cyclic endpoint-germ link"
                )));
            }
        }
        let live_allowance = allowance
            .checked_sub(incidence.owned_bytes())
            .ok_or_else(|| {
                resource("native incidence consumes the original preparation allowance")
            })?;
        let mut prepared = Self {
            definition,
            faces: Vec::new(),
            allowance: live_allowance,
            spatial_operators: Vec::new(),
            source_vertices: Vec::new(),
            source_inventory: None,
        };
        prepared
            .faces
            .try_reserve_exact(count)
            .map_err(|_| resource("original face inventory allocation failed"))?;
        memory(prepared.retained_bytes(), live_allowance)?;
        let mut lookup = SourceSpatialLookup::new();
        for &face in definition.face_ids() {
            work.charge(1, cancel)?;
            let remaining = live_allowance
                .checked_sub(prepared.retained_bytes())
                .and_then(|n| n.checked_sub(source_lookup_bytes(lookup.capacity()).ok()?))
                .ok_or_else(|| resource("original face preparation exceeds allowance"))?;
            let domain =
                NativeFaceSkeleton::new(definition, face, &incidence, remaining, work, cancel)
                    .and_then(|skeleton| {
                        prepared.prepare_source_face(skeleton, &mut lookup, ceiling, work, cancel)
                    })?;
            memory(
                prepared.retained_bytes()
                    + domain.owned_bytes()
                    + source_lookup_bytes(lookup.capacity())?,
                live_allowance,
            )?;
            prepared.faces.push(domain);
        }
        drop(lookup);
        prepared
            .spatial_operators
            .retain(|entry| Arc::strong_count(&entry.operator) > 1);
        let uses = prepared.faces.iter().map(|face| face.coedges.len()).sum();
        let census_allowance = live_allowance
            .checked_sub(prepared.retained_bytes())
            .ok_or_else(|| resource("source binding census exceeds allowance"))?;
        let mut census = crate::analysis::source_restriction::census::SourceBindingCensus::new(
            definition,
            uses,
            census_allowance,
            work,
            cancel,
        )?;
        for face in &prepared.faces {
            for coedge in &face.original_coedges {
                census.insert(&coedge.correspondence.binding, work, cancel)?;
            }
        }
        prepared.source_vertices = census.finish(&incidence, work, cancel)?;
        memory(prepared.retained_bytes(), live_allowance)?;
        prepared.allowance = allowance;
        prepared.source_inventory = Some(OriginalCorrespondenceInventory {
            faces: count,
            source_uses: uses,
            vertices: prepared.source_vertices.len(),
        });
        prepared.require_source_correspondence(work, cancel)?;
        Ok(prepared)
    }
    fn prepare_source_face(
        &mut self,
        skeleton: NativeFaceSkeleton<'d>,
        lookup: &mut SourceSpatialLookup,
        ceiling: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<PreparedFaceTrimDomain<'d>, GeometryError> {
        use crate::analysis::source_restriction::{
            carrier::SourceCarrierAssociation, pcurve::SourcePcurveRestriction,
        };
        let original = self.definition.original_source().face(skeleton.face)?;
        if original.wires.len() != skeleton.loops.len() {
            return Err(unavailable(
                "native/source face has no complete wire bijection, including every declared collapsed loop",
            ));
        }
        let native_surface = self
            .definition
            .face_surface(skeleton.face)
            .ok_or_else(|| unavailable("native source face has no actual coefficient carrier"))?;
        let carrier =
            SourceCarrierAssociation::new(&original.carrier, native_surface, work, cancel)?;
        let mut wires = Vec::new();
        memory(
            self.retained_bytes()
                + skeleton.retained_bytes()
                + checked_bytes::<SelectedSourceWire<'_>>(skeleton.loops.len())?
                + source_lookup_bytes(lookup.capacity())?,
            self.allowance,
        )?;
        wires
            .try_reserve_exact(skeleton.loops.len())
            .map_err(|_| resource("original wire correspondence inventory allocation failed"))?;
        for wire in 0..skeleton.loops.len() {
            wires.push(
                self.select_source_wire(lookup, &skeleton, wire, &wires, ceiling, work, cancel)?,
            );
        }
        let wire_bytes = wires.capacity() * size_of::<SelectedSourceWire<'_>>()
            + wires
                .iter()
                .map(|wire| wire.uses.capacity() * size_of::<SelectedSourceUse<'_>>())
                .sum::<usize>();
        let mut proposals: Vec<Vec<OriginalCoedgeProposal<'d>>> = Vec::new();
        memory(
            self.retained_bytes()
                + skeleton.retained_bytes()
                + wire_bytes
                + checked_bytes::<Vec<OriginalCoedgeProposal<'_>>>(skeleton.coedges.len())?
                + source_lookup_bytes(lookup.capacity())?,
            self.allowance,
        )?;
        proposals
            .try_reserve_exact(skeleton.coedges.len())
            .map_err(|_| resource("original PCurve proposal inventory allocation failed"))?;
        let mut proposal_bytes =
            proposals.capacity() * size_of::<Vec<OriginalCoedgeProposal<'_>>>();
        let mut full: Option<[Interval; 2]> = None;
        for (wire_index, wire) in wires.iter().enumerate() {
            let [start, _] = skeleton.loops[wire_index];
            for (local, selected) in wire.uses.iter().enumerate() {
                let source = self
                    .definition
                    .original_source()
                    .coedge(selected.source_index)?;
                let native = &skeleton.coedges[start + local];
                let binding =
                    source_binding(native, skeleton.face_index, original, selected, source);
                let mut choices = Vec::new();
                let mut failure = None;
                work.charge(source.pcurves.len() as u64 + 1, cancel)?;
                let undeclared = !source
                    .pcurves
                    .iter()
                    .any(|pcurve| pcurve.carrier_id == original.carrier_id);
                let derive_plane = undeclared
                    && original_plane_placement(&original.carrier, work, cancel)?.is_some();
                let derive_cylinder =
                    undeclared && original_cylinder(&original.carrier, work, cancel)?.is_some();
                let choice_capacity = source
                    .pcurves
                    .len()
                    .checked_add(usize::from(derive_plane || derive_cylinder))
                    .ok_or_else(|| resource("original PCurve choice count overflow"))?;
                memory(
                    self.retained_bytes()
                        + skeleton.retained_bytes()
                        + wire_bytes
                        + proposal_bytes
                        + checked_bytes::<OriginalCoedgeProposal<'_>>(choice_capacity)?
                        + source_lookup_bytes(lookup.capacity())?,
                    self.allowance,
                )?;
                choices.try_reserve_exact(choice_capacity).map_err(|_| {
                    resource("literal original PCurve association allocation failed")
                })?;
                proposal_bytes += choices.capacity() * size_of::<OriginalCoedgeProposal<'_>>();
                for pcurve in &source.pcurves {
                    work.charge(1, cancel)?;
                    if pcurve.carrier_id != original.carrier_id {
                        continue;
                    }
                    if !same_source_surface(
                        &pcurve.operands.surface,
                        &original.carrier,
                        0,
                        work,
                        cancel,
                    )? {
                        return Err(unavailable(
                            "original PCurve carrier entity and literal carrier coefficients disagree",
                        ));
                    }
                    let remaining = self
                        .allowance
                        .checked_sub(
                            self.retained_bytes()
                                + skeleton.retained_bytes()
                                + wire_bytes
                                + proposal_bytes
                                + source_lookup_bytes(lookup.capacity())?,
                        )
                        .ok_or_else(|| resource("original PCurve operand exceeds allowance"))?;
                    let operator = (|| {
                        if let Some(line) = SourcePlaneLineProjection::new(
                            &original.carrier,
                            &pcurve.operands.curve,
                            &source.vertex_points_mm,
                            ceiling,
                            work,
                            cancel,
                        )? {
                            return Ok(OriginalUvCurve::PlaneLine(line));
                        }
                        SourcePcurveRestriction::new(
                            original,
                            source,
                            pcurve,
                            &selected.spatial.operator,
                            ceiling,
                            remaining,
                            work,
                            cancel,
                        )
                        .map(OriginalUvCurve::Pcurve)
                    })();
                    let curve = match operator {
                        Ok(curve) => PreparedOriginalUvCurve {
                            curve,
                            spatial: Arc::clone(&selected.spatial),
                        },
                        Err(error)
                            if matches!(
                                error.code,
                                GeometryErrorCode::Cancelled | GeometryErrorCode::ResourceLimit
                            ) =>
                        {
                            return Err(error);
                        }
                        Err(error) => {
                            if failure.is_none() {
                                failure = Some(error);
                            }
                            continue;
                        }
                    };
                    let image =
                        source_uv_image(&curve, carrier.original_natural_domain(), work, cancel)?;
                    proposal_bytes = proposal_bytes
                        .checked_add(curve.owned_bytes())
                        .ok_or_else(|| resource("original PCurve byte count overflow"))?;
                    memory(
                        self.retained_bytes()
                            + skeleton.retained_bytes()
                            + wire_bytes
                            + proposal_bytes
                            + source_lookup_bytes(lookup.capacity())?,
                        self.allowance,
                    )?;
                    full = Some(match full {
                        None => image,
                        Some(old) => std::array::from_fn(|axis| old[axis].hull(image[axis])),
                    });
                    choices.push(OriginalCoedgeProposal {
                        curve,
                        binding,
                        image,
                        pcurve: Some(pcurve),
                    });
                }
                if derive_plane || derive_cylinder {
                    let operator = if derive_plane {
                        OriginalUvCurve::DerivedPlane(
                            SourcePlanePullback::new(
                                &original.carrier,
                                &selected.spatial.operator,
                                work,
                                cancel,
                            )?
                            .ok_or_else(|| {
                                unavailable(
                                    "original plane inverse changed carrier during preparation",
                                )
                            })?,
                        )
                    } else {
                        OriginalUvCurve::DerivedCylinder(
                            SourceCylinderPullback::new(
                                self.definition,
                                &original.carrier,
                                selected,
                                wire,
                                work,
                                cancel,
                            )?
                            .ok_or_else(|| {
                                unavailable(
                                    "original cylinder inverse changed carrier during preparation",
                                )
                            })?,
                        )
                    };
                    let curve = PreparedOriginalUvCurve {
                        curve: operator,
                        spatial: Arc::clone(&selected.spatial),
                    };
                    let image =
                        source_uv_image(&curve, carrier.original_natural_domain(), work, cancel)?;
                    full = Some(match full {
                        None => image,
                        Some(old) => std::array::from_fn(|axis| old[axis].hull(image[axis])),
                    });
                    choices.push(OriginalCoedgeProposal {
                        curve,
                        binding,
                        image,
                        pcurve: None,
                    });
                }
                if choices.is_empty() {
                    return Err(unavailable(format!("native face {:?}, wire {wire_index}, coedge {local}, original EDGE_CURVE {:?}: {}",
                        skeleton.face,source.edge_curve_id,failure.map_or_else(||
                            "no actual declared source PCurve on this carrier; a derived pullback needs its own complete original inverse/branch qualification, not generated UV promotion".to_owned(),|error|error.message))));
                }
                proposals.push(choices);
            }
        }
        let full = full.ok_or_else(|| {
            unavailable("original face has no bounded source chart boundary operands")
        })?;
        let full = augment_literal_pole_domain(&original.carrier, full, &proposals, work, cancel)?;
        let active = full.map(|axis| [axis.lo, axis.hi]);
        let remaining = self
            .allowance
            .checked_sub(
                self.retained_bytes()
                    + skeleton.retained_bytes()
                    + wire_bytes
                    + proposal_bytes
                    + source_lookup_bytes(lookup.capacity())?,
            )
            .ok_or_else(|| resource("original carrier inventory exceeds allowance"))?;
        let surface =
            PreparedOriginalSurface::new(&original.carrier, active, remaining, work, cancel)?;
        if !surface.continuous(active, work, cancel)? {
            return Err(unavailable(
                "complete selected original carrier domain lacks coefficient-level C0",
            ));
        }
        let carrier_c1 = surface.smooth(active, work, cancel)?;
        let carrier_error = carrier.position_error_mm(active, work, cancel)?;
        if !carrier_error.is_finite() || carrier_error > ceiling {
            return Err(unavailable(
                "whole original/native carrier coefficient correspondence exceeds captured source allowance",
            ));
        }
        let mut candidates = Vec::new();
        let mut measured = carrier_error;
        let total_choices = proposals.iter().map(Vec::len).sum::<usize>();
        memory(
            self.retained_bytes()
                + skeleton.retained_bytes()
                + wire_bytes
                + proposal_bytes
                + surface.retained_bytes()
                + checked_bytes::<Vec<Option<PreparedOriginalCoedge<'_>>>>(proposals.len())?
                + checked_bytes::<Option<PreparedOriginalCoedge<'_>>>(total_choices)?
                + size_of::<parametric::ChartCurveEnclosure>()
                + source_lookup_bytes(lookup.capacity())?,
            self.allowance,
        )?;
        candidates
            .try_reserve_exact(proposals.len())
            .map_err(|_| resource("qualified original PCurve list allocation failed"))?;
        for group in proposals {
            let mut admitted = Vec::new();
            admitted
                .try_reserve_exact(group.len())
                .map_err(|_| resource("qualified original PCurve choice allocation failed"))?;
            for proposal in group {
                let inventory = source_span_inventory(&proposal.curve, work, cancel)?;
                let receipt = match &proposal.curve.curve {
                    OriginalUvCurve::Pcurve(pcurve) => Some(OriginalCoedgeCorrespondence {
                        binding: proposal.binding,
                        spatial_range: [0.0, 1.0],
                        uv_ends: [0.0, 1.0],
                        position_error_mm: pcurve.source_error_mm(),
                        relative_derivative_error_bound: pcurve.relative_derivative_error_bound(),
                        inventory,
                        margin_uv: [0.0; 2],
                        provenance: if pcurve.shared_master() {
                            OriginalCorrespondenceProvenance::ExactCoefficients
                        } else {
                            OriginalCorrespondenceProvenance::Forward
                        },
                    }),
                    OriginalUvCurve::DerivedPlane(curve)
                        if curve.exact_lift_identity(work, cancel)? =>
                    {
                        Some(OriginalCoedgeCorrespondence {
                            binding: proposal.binding,
                            spatial_range: [0.0, 1.0],
                            uv_ends: [0.0, 1.0],
                            position_error_mm: 0.0,
                            relative_derivative_error_bound: 0.0,
                            inventory,
                            provenance: OriginalCorrespondenceProvenance::ExactCoefficients,
                            margin_uv: [0.0; 2],
                        })
                    }
                    OriginalUvCurve::DerivedCylinder(curve) if curve.exact_lift => {
                        Some(OriginalCoedgeCorrespondence {
                            binding: proposal.binding,
                            spatial_range: [0.0, 1.0],
                            uv_ends: [0.0, 1.0],
                            position_error_mm: 0.0,
                            relative_derivative_error_bound: 0.0,
                            inventory,
                            provenance: OriginalCorrespondenceProvenance::ExactCoefficients,
                            margin_uv: [0.0; 2],
                        })
                    }
                    OriginalUvCurve::PlaneLine(line)
                        if line.exact_endpoints
                            && proposal.curve.spatial.operator.exact_source_endpoints()
                            && proposal
                                .curve
                                .spatial
                                .operator
                                .exact_affine_coefficients(work, cancel)?
                                .is_some() =>
                    {
                        Some(OriginalCoedgeCorrespondence {
                            binding: proposal.binding,
                            spatial_range: [0.0, 1.0],
                            uv_ends: [0.0, 1.0],
                            position_error_mm: 0.0,
                            relative_derivative_error_bound: 0.0,
                            inventory,
                            provenance: OriginalCorrespondenceProvenance::ExactCoefficients,
                            margin_uv: [0.0; 2],
                        })
                    }
                    OriginalUvCurve::PlaneLine(_)
                    | OriginalUvCurve::DerivedPlane(_)
                    | OriginalUvCurve::DerivedCylinder(_) => qualify_original_correspondence(
                        proposal.binding,
                        proposal.curve.spatial.operator.as_ref(),
                        &surface,
                        &proposal.curve,
                        [0.0, 1.0],
                        [0.0, 1.0],
                        ceiling,
                        work,
                        cancel,
                    )?,
                };
                if let Some(mut receipt) = receipt {
                    receipt.inventory = inventory;
                    measured = measured
                        .max(receipt.position_error_mm())
                        .max(proposal.curve.spatial.operator.source_error_mm());
                    let full_uv_jet = parametric::curve2d_piecewise_jet(
                        &proposal.curve,
                        proposal.curve.active_range(),
                        work,
                        cancel,
                    )?;
                    admitted.push(Some(PreparedOriginalCoedge {
                        curve: proposal.curve,
                        correspondence: receipt,
                        full_uv_image: proposal.image,
                        full_uv_jet,
                        source_pcurve: proposal.pcurve,
                    }));
                }
            }
            if admitted.is_empty() {
                return Err(unavailable(
                    "no retained original PCurve candidate proves its complete source-forward and relative-derivative law",
                ));
            }
            candidates.push(admitted);
        }
        drop(wires);
        let budget = self
            .allowance
            .checked_sub(self.retained_bytes() + source_lookup_bytes(lookup.capacity())?)
            .ok_or_else(|| resource("source chart closure exceeds allowance"))?;
        finish_original_face(
            skeleton,
            surface,
            candidates,
            original.material_orientation(),
            carrier_c1,
            measured,
            ceiling,
            budget,
            work,
            cancel,
        )
    }
}

fn literal_source_pole(
    surface: &solidcraft_step::geom::Surface,
    point: [f64; 3],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<SphericalPole>, GeometryError> {
    let solidcraft_step::geom::Surface::Sphere { position, radius } = surface else {
        return Ok(None);
    };
    for (pole, sign) in [(SphericalPole::North, 1.0), (SphericalPole::South, -1.0)] {
        let mut equal = true;
        for axis in 0..3 {
            let mut value = exact_product(
                &ExactCoefficient::point(*radius),
                &ExactCoefficient::point(position.axis[axis]),
                work,
                cancel,
            )?;
            if sign < 0.0 {
                let mut negative = ExactCoefficient::point(0.0);
                negative.subtract(&value).ok_or_else(|| {
                    unavailable("literal source pole coefficient exceeds exact arithmetic capacity")
                })?;
                value = negative;
            }
            value
                .push(position.origin[axis])
                .and_then(|_| value.push(-point[axis]))
                .ok_or_else(|| {
                    unavailable(
                        "literal source pole point identity exceeds exact arithmetic capacity",
                    )
                })?;
            equal &= value.sign() == 0;
        }
        if equal {
            return Ok(Some(pole));
        }
    }
    Ok(None)
}
fn pole_coordinate(pole: SphericalPole) -> Interval {
    let pi = Interval {
        lo: parametric::down(std::f64::consts::PI),
        hi: parametric::up(std::f64::consts::PI),
    };
    let half = pi.mul(Interval::point(0.5));
    if pole == SphericalPole::North {
        half
    } else {
        half.neg()
    }
}
fn augment_literal_pole_domain(
    surface: &solidcraft_step::geom::Surface,
    mut bounds: [Interval; 2],
    proposals: &[Vec<OriginalCoedgeProposal<'_>>],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[Interval; 2], GeometryError> {
    for group in proposals {
        for proposal in group {
            for point in proposal.binding.source_vertex_points_mm {
                work.charge(1, cancel)?;
                if let Some(pole) = literal_source_pole(surface, point, work, cancel)? {
                    bounds[1] = bounds[1].hull(pole_coordinate(pole));
                }
            }
        }
    }
    Ok(bounds)
}
fn exact_uv_endpoint_equal(
    a: &PreparedOriginalCoedge<'_>,
    ta: f64,
    b: &PreparedOriginalCoedge<'_>,
    tb: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    if let (Some(a), Some(b)) = (
        a.curve.exact_affine_coefficients(work, cancel)?,
        b.curve.exact_affine_coefficients(work, cancel)?,
    ) {
        if affine_endpoint_identity(&a, [ta; 2], &b, [tb; 2], work, cancel)? {
            return Ok(true);
        }
    }
    if let (OriginalUvCurve::PlaneLine(a), OriginalUvCurve::PlaneLine(b)) =
        (&a.curve.curve, &b.curve.curve)
    {
        if a.exact_endpoints
            && b.exact_endpoints
            && std::ptr::eq(a.surface, b.surface)
            && a.vertices[usize::from(ta == 1.0)] == b.vertices[usize::from(tb == 1.0)]
        {
            // Both actual affine plane lifts equal the SAME captured point;
            // the already-qualified original plane Gram inverse is injective.
            return Ok(true);
        }
    }
    if std::ptr::eq(a, b) && ta != tb {
        if let OriginalUvCurve::Pcurve(curve) = &a.curve.curve {
            return curve.closed_uv_endpoint_identity(work, cancel);
        }
    }
    Ok(false)
}
fn make_original_joint(
    surface: &PreparedOriginalSurface<'_>,
    native: &[NativeTrimCoedge<'_>],
    previous: usize,
    next: usize,
    a: &PreparedOriginalCoedge<'_>,
    b: &PreparedOriginalCoedge<'_>,
    native_orientation: bool,
    ceiling: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<PreparedTrimJoint>, GeometryError> {
    let (incoming, outgoing, ai, bi) = if native_orientation {
        (a, b, previous, next)
    } else {
        (b, a, next, previous)
    };
    let ir = &incoming.correspondence;
    let or = &outgoing.correspondence;
    let ih = ir.binding.material_forward;
    let oh = !or.binding.material_forward;
    let ie = ir.binding.native_endpoint_index(ih);
    let oe = or.binding.native_endpoint_index(oh);
    if ir.binding.endpoints[ie] != or.binding.endpoints[oe]
        || ir.binding.source_vertex_ids[ie] != or.binding.source_vertex_ids[oe]
        || ir.binding.source_vertex_points_mm[ie] != or.binding.source_vertex_points_mm[oe]
    {
        return Err(unavailable(
            "original wire joint has no common exact authored vertex identity/coefficients and native endpoint germ",
        ));
    }
    let ti = ir.uv_endpoint(ih);
    let to = or.uv_endpoint(oh);
    let p = incoming.curve.tensor([ti; 2], 0, work, cancel)?[0];
    let q = outgoing.curve.tensor([to; 2], 0, work, cancel)?[0];
    let endpoints = [[p[0], p[1]], [q[0], q[1]]];
    // Equal singleton outward enclosures prove actual UV identity. Overlap
    // or a small interval does not; nonzero physical residuals stay below.
    let singleton_identity = (0..2).all(|axis| p[axis].is_point() && p[axis] == q[axis]);
    let collapsed =
        singleton_identity || exact_uv_endpoint_equal(incoming, ti, outgoing, to, work, cancel)?;
    let zero = collapsed
        && ir.provenance() == OriginalCorrespondenceProvenance::ExactCoefficients
        && or.provenance() == OriginalCorrespondenceProvenance::ExactCoefficients
        && incoming.curve.spatial.operator.exact_source_endpoints()
        && outgoing.curve.spatial.operator.exact_source_endpoints();
    if zero {
        return Ok(None);
    }
    let connector = SourceEndpointConnector {
        endpoints,
        collapsed,
    };
    let mut enclosure = std::array::from_fn(|axis| endpoints[0][axis].hull(endpoints[1][axis]));
    let vertex_point = ir.binding.source_vertex_points_mm[ie];
    let pole = literal_source_pole(surface.source(), vertex_point, work, cancel)?;
    if let Some(pole) = pole {
        enclosure[1] = enclosure[1].hull(pole_coordinate(pole));
    }
    let world = surface.position_bounds(enclosure.map(|axis| [axis.lo, axis.hi]), work, cancel)?;
    let residual = std::array::from_fn(|axis| world[axis].sub(Interval::point(vertex_point[axis])));
    let physical_error_mm = norm(residual)?.hi;
    if physical_error_mm > ceiling {
        return Err(unavailable(
            "whole original endpoint-expression connector/pole strip exceeds captured physical source uncertainty",
        ));
    }
    let kind = match pole {
        Some(pole) => TrimJointKind::SphericalPole {
            polar_axis: 1,
            pole,
            polar_coordinate: pole_coordinate(pole),
            world_enclosure: world,
            vertex_residual: residual,
            connector,
        },
        None => TrimJointKind::EndpointUncertainty,
    };
    let incoming_id = native[ai].identity();
    let outgoing_id = native[bi].identity();
    Ok(Some(PreparedTrimJoint {
        vertex_id: ir.binding.endpoints[ie],
        source_vertex_id: ir.binding.source_vertex_ids[ie],
        vertex_point,
        incoming: incoming_id,
        outgoing: outgoing_id,
        enclosure,
        endpoint_uv: endpoints.map(|point| point.map(|axis| [axis.lo, axis.hi])),
        kind,
        connector,
        physical_error_mm,
        // A small physical image is NOT literal zero displacement.
        zero_displacement: false,
        endpoint_bindings: [
            SourceJointEndpointBinding {
                identity: incoming_id,
                source_index: ir.source_index(),
                uv_parameter: ti,
                spatial_high: ih,
                source_vertex: ir.binding.source_vertex_ids[ie],
            },
            SourceJointEndpointBinding {
                identity: outgoing_id,
                source_index: or.source_index(),
                uv_parameter: to,
                spatial_high: oh,
                source_vertex: or.binding.source_vertex_ids[oe],
            },
        ],
    }))
}
fn candidate_bytes(candidates: &[Vec<Option<PreparedOriginalCoedge<'_>>>]) -> usize {
    candidates
        .iter()
        .map(|group| {
            group.capacity() * size_of::<Option<PreparedOriginalCoedge<'_>>>()
                + group
                    .iter()
                    .flatten()
                    .map(PreparedOriginalCoedge::owned_bytes)
                    .sum::<usize>()
        })
        .sum()
}
fn next_choices(
    choices: &mut [usize],
    candidates: &[Vec<Option<PreparedOriginalCoedge<'_>>>],
) -> bool {
    for index in (0..choices.len()).rev() {
        choices[index] += 1;
        if choices[index] < candidates[index].len() {
            return true;
        }
        choices[index] = 0;
    }
    false
}
fn finish_original_face<'d>(
    mut skeleton: NativeFaceSkeleton<'d>,
    surface: PreparedOriginalSurface<'d>,
    mut candidates: Vec<Vec<Option<PreparedOriginalCoedge<'d>>>>,
    orientation: bool,
    carrier_c1: bool,
    measured: f64,
    ceiling: f64,
    allowance: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<PreparedFaceTrimDomain<'d>, GeometryError> {
    let count = skeleton.coedges.len();
    memory(
        skeleton.retained_bytes()
            + surface.retained_bytes()
            + candidate_bytes(&candidates)
            + checked_bytes::<usize>(count)?
            + checked_bytes::<PreparedTrimJoint>(count)?
            + checked_bytes::<Option<usize>>(count)?
            + checked_bytes::<PreparedOriginalCoedge<'_>>(count)?,
        allowance,
    )?;
    let mut choices = Vec::new();
    choices
        .try_reserve_exact(count)
        .map_err(|_| resource("original chart association search allocation failed"))?;
    choices.resize(count, 0);
    let mut surface = Some(surface);
    let mut last_failure = None;
    loop {
        work.charge(1, cancel)?;
        let mut joints = Vec::new();
        let mut joint_after = Vec::new();
        joints
            .try_reserve_exact(count)
            .map_err(|_| resource("original joint inventory allocation failed"))?;
        joint_after
            .try_reserve_exact(count)
            .map_err(|_| resource("original joint incidence allocation failed"))?;
        joint_after.resize(count, None);
        let mut admissible = true;
        for &[start, end] in &skeleton.loops {
            for previous in start..end {
                let next = if previous + 1 == end {
                    start
                } else {
                    previous + 1
                };
                let a = candidates[previous][choices[previous]]
                    .as_ref()
                    .expect("retained source proposal");
                let b = candidates[next][choices[next]]
                    .as_ref()
                    .expect("retained source proposal");
                match make_original_joint(
                    surface.as_ref().expect("owned original carrier"),
                    &skeleton.coedges,
                    previous,
                    next,
                    a,
                    b,
                    skeleton.material_orientation,
                    ceiling,
                    work,
                    cancel,
                ) {
                    Ok(Some(joint)) => {
                        joint_after[previous] = Some(joints.len());
                        joints.push(joint);
                    }
                    Ok(None) => {}
                    Err(error)
                        if matches!(
                            error.code,
                            GeometryErrorCode::Cancelled | GeometryErrorCode::ResourceLimit
                        ) =>
                    {
                        return Err(error);
                    }
                    Err(error) => {
                        last_failure = Some(error);
                        admissible = false;
                        break;
                    }
                }
            }
            if !admissible {
                break;
            }
        }
        if admissible {
            let mut selected = Vec::new();
            selected
                .try_reserve_exact(count)
                .map_err(|_| resource("selected original coedge allocation failed"))?;
            let mut bbox: Option<[Interval; 2]> = None;
            for (index, &choice) in choices.iter().enumerate() {
                let coedge = candidates[index][choice]
                    .take()
                    .expect("selected retained source operator");
                bbox = Some(match bbox {
                    None => coedge.full_uv_image,
                    Some(old) => {
                        std::array::from_fn(|axis| old[axis].hull(coedge.full_uv_image[axis]))
                    }
                });
                selected.push(coedge);
            }
            let mut bbox =
                bbox.ok_or_else(|| unavailable("original face has no complete chart boundary"))?;
            let mut actual_error = measured;
            for joint in &joints {
                for axis in 0..2 {
                    bbox[axis] = bbox[axis].hull(joint.enclosure[axis]);
                }
                actual_error = actual_error.max(joint.physical_error_mm);
            }
            let mut domain = PreparedFaceTrimDomain {
                definition: skeleton.definition,
                face: skeleton.face,
                face_index: skeleton.face_index,
                material_orientation: skeleton.material_orientation,
                domain: bbox.map(|axis| [axis.lo, axis.hi]),
                coedges: std::mem::take(&mut skeleton.coedges),
                loops: std::mem::take(&mut skeleton.loops),
                spans: Vec::new(),
                carrier_c1,
                original_surface: surface.take(),
                original_orientation: Some(orientation),
                original_coedges: selected,
                original_joints: joints,
                joint_after,
                original_margin_uv: Some([0.0; 2]),
                original_chart_error_mm: Some(actual_error),
            };
            let remaining = allowance
                .checked_sub(candidate_bytes(&candidates) + choices.capacity() * size_of::<usize>())
                .ok_or_else(|| {
                    resource("retained source alternatives exhaust chart qualification allowance")
                })?;
            let qualification = domain.qualify_source_charts(remaining, work, cancel);
            match qualification {
                Ok(()) => {
                    let carrier = domain
                        .original_surface
                        .take()
                        .expect("selected original carrier");
                    domain.original_surface =
                        Some(carrier.restrict_active(domain.domain, work, cancel)?);
                    return Ok(domain);
                }
                Err(error)
                    if matches!(
                        error.code,
                        GeometryErrorCode::Cancelled | GeometryErrorCode::ResourceLimit
                    ) =>
                {
                    return Err(error);
                }
                Err(error) => {
                    last_failure = Some(error);
                    surface = domain.original_surface.take();
                    skeleton.coedges = std::mem::take(&mut domain.coedges);
                    skeleton.loops = std::mem::take(&mut domain.loops);
                    for (index, coedge) in domain.original_coedges.into_iter().enumerate() {
                        candidates[index][choices[index]] = Some(coedge);
                    }
                }
            }
        }
        if !next_choices(&mut choices, &candidates) {
            return Err(last_failure.unwrap_or_else(|| {
                unavailable("no complete original PCurve/joint/chart association qualifies")
            }));
        }
    }
}
impl PreparedFaceTrimDomain<'_> {
    fn qualify_source_charts(
        &mut self,
        allowance: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        if self.domain.iter().any(|axis| axis[0] >= axis[1]) {
            return Err(unavailable(
                "selected source boundaries do not establish a two-dimensional literal UV chart; periodic/vertex-loop quotient coverage is still required",
            ));
        }
        let total = self.coedges.len() + self.original_joints.len();
        for index in 0..total {
            let boundary = if index < self.coedges.len() {
                BoundaryCurve::Coedge(index)
            } else {
                BoundaryCurve::Joint(index - self.coedges.len())
            };
            if let BoundaryCurve::Joint(index) = boundary {
                if self.original_joints[index].connector.collapsed {
                    continue;
                }
                // Every member of this sealed endpoint family is affine
                // (possibly constant), but interval endpoint uncertainty need
                // not prove a nonzero derivative. Its owned cell is Boundary,
                // never Inside; pair separation still excludes foreign uses.
                reserve_span(self, allowance)?;
                self.spans.push(TrimSpan {
                    boundary,
                    range: [0.0, 1.0],
                    ends: [true; 2],
                });
                continue;
            }
            let (range, _) = self.boundary_restriction(boundary);
            if !self
                .boundary_operand(boundary)?
                .continuous(range, work, cancel)?
            {
                return Err(unavailable(
                    "selected original boundary coefficient operator lacks actual C0",
                ));
            }
            // Proof cuts are not topology. Qualify complete C0 pieces first;
            // the coefficient evaluator still retains every original knot.
            let mut stack = [([0.0; 2], 0u8); MAX_DEPTH as usize + 1];
            stack[0] = (range, 0);
            let mut live = 1;
            while live > 0 {
                live -= 1;
                let (piece, depth) = stack[live];
                work.charge(1, cancel)?;
                let jet = parametric::curve2d_piecewise_jet(
                    &self.boundary_operand(boundary)?,
                    piece,
                    work,
                    cancel,
                )?;
                if jet.derivative.iter().any(|d| d.lo > 0.0 || d.hi < 0.0) {
                    reserve_span(self, allowance)?;
                    self.spans.push(TrimSpan {
                        boundary,
                        range: piece,
                        ends: [piece[0] == range[0], piece[1] == range[1]],
                    });
                } else {
                    let middle = split(
                        piece,
                        depth,
                        "original chart boundary injectivity/regularity",
                    ).map_err(|error| work.annotate_error(error, work.consumed(), format_args!(
                        "original chart source_face={} boundary={boundary:?} parameter={piece:?} derivative={:?}",
                        self.face_index, jet.derivative,
                    )))?;
                    // Prefer an actual cover fence near the midpoint. Narrow
                    // original spans need not wait for dyadic resolution.
                    let operand = self.boundary_operand(boundary)?;
                    let mut cover = None;
                    for part in operand.parameter_spans(piece, work, cancel)? {
                        work.charge(1, cancel)?;
                        if part[0] < part[1] && part[0] <= middle && middle <= part[1] {
                            cover = Some(part);
                            break;
                        }
                    }
                    let cover = cover.ok_or_else(|| {
                        unavailable("original coefficient span cover is incomplete")
                    })?;
                    let mut nearest = None;
                    for fence in cover {
                        if piece[0] < fence && fence < piece[1] {
                            let distance = (fence - middle).abs();
                            if nearest.is_none_or(|(best, _)| distance < best) {
                                nearest = Some((distance, fence));
                            }
                        }
                    }
                    let cut = nearest.map_or(middle, |(_, fence)| fence);
                    stack[live] = ([cut, piece[1]], depth + 1);
                    stack[live + 1] = ([piece[0], cut], depth + 1);
                    live += 2;
                }
            }
        }
        for i in 0..self.spans.len() {
            for j in i + 1..self.spans.len() {
                self.separate_spans(self.spans[i], self.spans[j], work, cancel)?;
            }
        }
        // Boundaries are embedded and disjoint outside owned uncertainty
        // cells; foreign loops are excluded from those complete cells too.
        // Winding of a complete endpoint expression box proves containment.
        for hole in 1..self.loops.len() {
            let [start, _] = self.loops[hole];
            let p = self.original_coedges[start]
                .curve
                .tensor([0.0; 2], 0, work, cancel)?[0];
            let query = [[p[0].lo, p[0].hi], [p[1].lo, p[1].hi]];
            for other in 0..self.loops.len() {
                if other == hole {
                    continue;
                }
                let inside = self.loop_contains_box(other, query, work, cancel)?;
                if inside != (other == 0) {
                    return Err(unavailable(
                        "original trim hole nesting/exterior ownership is unproved or inconsistent",
                    ));
                }
            }
        }
        Ok(())
    }
    fn loop_contains_box(
        &self,
        wire: usize,
        query: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let point = query.map(|axis| axis[0] * 0.5 + axis[1] * 0.5);
        let [start, end] = self.loops[wire];
        for shear in [0.0, 1.0, -1.0, 2.0, -2.0] {
            let mut winding = 0i64;
            let mut resolved = true;
            for index in start..end {
                match self.boundary_winding(
                    BoundaryCurve::Coedge(index),
                    query,
                    point,
                    shear,
                    work,
                    cancel,
                )? {
                    Some(value) => {
                        winding = winding
                            .checked_add(value)
                            .ok_or_else(|| resource("source hole winding overflow"))?
                    }
                    None => {
                        resolved = false;
                        break;
                    }
                }
            }
            if resolved {
                for (index, joint) in self.original_joints.iter().enumerate() {
                    if joint.incoming.wire_index != wire {
                        continue;
                    }
                    match self.boundary_winding(
                        BoundaryCurve::Joint(index),
                        query,
                        point,
                        shear,
                        work,
                        cancel,
                    )? {
                        Some(value) => {
                            winding = winding.checked_add(value).ok_or_else(|| {
                                resource("source hole completion winding overflow")
                            })?
                        }
                        None => {
                            resolved = false;
                            break;
                        }
                    }
                }
            }
            if resolved {
                return Ok(winding != 0);
            }
        }
        Err(unavailable(
            "complete original hole-containment winding cannot exclude its boundary",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use TrimClassification::{Boundary, Inside, Outside};

    const HOUSING: &[u8] = include_bytes!(
        "../../../../fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step"
    );

    fn assert_original_face_regions(
        fixture: &[u8],
        source_face: spiling_contracts::geometry::StepEntityId,
        regions: &[([[f64; 2]; 2], TrimClassification)],
    ) {
        let definition = crate::from_step_bytes(fixture).unwrap();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(20_000_000);
        let bytes = 32 * 1024 * 1024;
        let mut incidence = crate::analysis::source_incidence::SourceEdgeIncidence::new(
            &definition,
            bytes,
            &mut work,
            &cancel,
        )
        .unwrap();
        incidence
            .prepare_vertices(bytes, &mut work, &cancel)
            .unwrap();
        let live = bytes - incidence.owned_bytes();
        let mut source = PreparedNativeTrimSource {
            definition: &definition,
            faces: Vec::new(),
            allowance: live,
            spatial_operators: Vec::new(),
            source_vertices: Vec::new(),
            source_inventory: None,
        };
        let face = definition
            .original_source()
            .faces
            .iter()
            .find(|face| face.source_face == source_face)
            .unwrap()
            .native_face;
        let skeleton = NativeFaceSkeleton::new(
            &definition,
            face,
            &incidence,
            live - source.retained_bytes(),
            &mut work,
            &cancel,
        )
        .unwrap();
        let domain = source
            .prepare_source_face(
                skeleton,
                &mut SourceSpatialLookup::new(),
                1.0e-7,
                &mut work,
                &cancel,
            )
            .unwrap();
        for (query, expected) in regions {
            assert_eq!(
                domain.classify(*query, &mut work, &cancel).unwrap(),
                *expected,
                "query={query:?}",
            );
        }
    }

    #[test]
    fn original_pole_face_classifies_across_internal_endpoint_cut_bands() {
        assert_original_face_regions(
            HOUSING,
            spiling_contracts::geometry::StepEntityId(461),
            &[
                ([[0.5, 0.6], [-0.6, -0.5]], Inside),
                ([[2.0, 2.1], [-0.6, -0.5]], Outside),
            ],
        );
    }

    #[test]
    fn original_periodic_cylinder_retains_a_simple_material_chart() {
        assert_original_face_regions(
            HOUSING,
            spiling_contracts::geometry::StepEntityId(1254),
            &[
                ([[0.5, 0.6], [2.0, 2.1]], Inside),
                ([[0.5, 0.6], [6.0, 6.1]], Outside),
            ],
        );
    }

    #[test]
    fn original_closed_disk_retains_material_inside_and_outside() {
        assert_original_face_regions(
            HOUSING,
            spiling_contracts::geometry::StepEntityId(1407),
            &[
                ([[0.5, 0.6], [0.5, 0.6]], Inside),
                ([[3.1, 3.2], [0.1, 0.2]], Outside),
            ],
        );
    }

    #[test]
    fn original_annulus_keeps_uncertain_joins_boundary_and_protects_its_void() {
        assert_original_face_regions(
            include_bytes!("../../../../fixtures/geometry/through-hole.step"),
            spiling_contracts::geometry::StepEntityId(18),
            &[
                ([[14.9, 15.1], [9.9, 10.1]], Inside),
                ([[9.9, 10.1], [9.9, 10.1]], Outside),
                ([[21.0, 22.0], [9.0, 11.0]], Outside),
                ([[13.0; 2], [10.0; 2]], Boundary),
            ],
        );
    }

    #[test]
    fn original_split_bore_cylinders_keep_their_owned_half_turn_and_height() {
        for face in [57, 64] {
            assert_original_face_regions(
                include_bytes!("../../../../fixtures/geometry/through-hole.step"),
                spiling_contracts::geometry::StepEntityId(face),
                &[
                    ([[0.5, 0.6], [3.0, 4.0]], Inside),
                    ([[0.5, 0.6], [8.1, 8.2]], Outside),
                    ([[-0.2, -0.1], [3.0, 4.0]], Outside),
                    ([[3.4, 3.5], [3.0, 4.0]], Outside),
                ],
            );
        }
    }
}
