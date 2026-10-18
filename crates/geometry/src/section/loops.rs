// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{endpoint_genealogy, in_frame, kernel_error, resource_error};
use crate::parametric::{
    CurveSamplingEndpoint, CurveSamplingInterval, CurveSamplingSmoothRange, Interval,
    PreparedCurveEnclosure, SectionPlaneCoverage, SectionPlaneEndpointReceipt,
};
use crate::{
    Definition, GeometryError, GeometryWorkBudget, NativeSection, PlaneFrameMm, SectionLoopMm,
    check_cancel,
};
use monstertruck_modeling::{BoundedCurve, CoordinatePlane, Curve, Point3, Surface, Wire};
use spiling_contracts::geometry::{
    GeometryErrorCode, MAX_GEOMETRY_CHUNK_BYTES, MAX_LOOP_POINTS, MAX_SECTION_BYTES,
    SECTION_PLANE_TOLERANCE_MM, SECTION_SAMPLING_TOLERANCE_MM,
};
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    sync::atomic::AtomicBool,
};

/// Mirrors complete-loop SPLS packing without allocating an encoded copy.
/// Header and aligned offsets count against the same aggregate transfer cap.
pub(super) struct SectionBudget {
    committed_bytes: usize,
    chunk_loops: usize,
    chunk_points: usize,
    byte_limit: usize,
}

impl SectionBudget {
    pub(super) fn with_limit(bytes: u32) -> Result<Self, GeometryError> {
        if bytes > MAX_SECTION_BYTES {
            return Err(resource_error("section capacity exceeds frozen cap"));
        }
        Ok(Self {
            committed_bytes: 0,
            chunk_loops: 0,
            chunk_points: 0,
            byte_limit: bytes as usize,
        })
    }
    fn chunk_size(loops: usize, points: usize) -> usize {
        if loops == 0 {
            return 0;
        }
        let offsets = ((loops + 1) * 4 + 7) & !7;
        64 + offsets + points * 24
    }

    pub(super) fn check_loop(&self, points: usize) -> Result<(), GeometryError> {
        if points > MAX_LOOP_POINTS as usize {
            return Err(resource_error(
                "native section loop exceeds 40000-point cap",
            ));
        }
        let pending = Self::chunk_size(self.chunk_loops + 1, self.chunk_points + points);
        let bytes = if pending <= MAX_GEOMETRY_CHUNK_BYTES as usize {
            self.committed_bytes + pending
        } else {
            self.committed_bytes
                + Self::chunk_size(self.chunk_loops, self.chunk_points)
                + Self::chunk_size(1, points)
        };
        if bytes > self.byte_limit {
            return Err(resource_error("native section exceeds aggregate byte cap"));
        }
        Ok(())
    }

    pub(super) fn push_loop(&mut self, points: usize) -> Result<(), GeometryError> {
        self.check_loop(points)?;
        if Self::chunk_size(self.chunk_loops + 1, self.chunk_points + points)
            > MAX_GEOMETRY_CHUNK_BYTES as usize
        {
            self.committed_bytes += Self::chunk_size(self.chunk_loops, self.chunk_points);
            self.chunk_loops = 0;
            self.chunk_points = 0;
        }
        self.chunk_loops += 1;
        self.chunk_points += points;
        Ok(())
    }
}

impl Default for SectionBudget {
    fn default() -> Self {
        Self {
            committed_bytes: 0,
            chunk_loops: 0,
            chunk_points: 0,
            byte_limit: MAX_SECTION_BYTES as usize,
        }
    }
}

pub(super) struct CanonicalLoop {
    native: SectionLoopMm,
    bounds: [f64; 4],
    source_faces: Vec<usize>,
}

impl CanonicalLoop {
    pub(super) fn is_hole(&self) -> bool {
        self.native.is_hole
    }
    pub(super) fn point_count(&self) -> usize {
        self.native.points_mm.len()
    }
    pub(super) fn into_parts(self) -> (SectionLoopMm, Vec<usize>) {
        (self.native, self.source_faces)
    }
}

const MAX_SAMPLING_NODES: usize = 1_000_000;
const MAX_SAMPLING_DEPTH: u8 = 48;

type SamplingNode<'a, 'source> = (
    [CurveSamplingEndpoint<'a, 'source>; 2],
    Option<CurveSamplingSmoothRange<'a, 'source>>,
    u8,
);

/// Shared by every cap/wire/edge in one query, including rejected subdivisions.
pub(super) struct SamplingWork<'a> {
    nodes: usize,
    native: &'a mut GeometryWorkBudget,
    maximum_chord_error_mm: f64,
    maximum_plane_error_mm: f64,
    remaining_bytes: usize,
}

impl<'a> SamplingWork<'a> {
    pub(super) fn new(native: &'a mut GeometryWorkBudget, remaining_bytes: usize) -> Self {
        Self {
            nodes: 0,
            native,
            maximum_chord_error_mm: 0.0,
            maximum_plane_error_mm: 0.0,
            remaining_bytes,
        }
    }

    pub(super) fn error_bounds(&self) -> [f64; 2] {
        [self.maximum_chord_error_mm, self.maximum_plane_error_mm]
    }

    pub(super) fn remaining_bytes(&self) -> usize {
        self.remaining_bytes
    }

    fn reserve<T>(&mut self, values: &mut Vec<T>, capacity: usize) -> Result<(), GeometryError> {
        let previous = values.capacity();
        if capacity <= previous {
            return Ok(());
        }
        (capacity - previous)
            .checked_mul(std::mem::size_of::<T>())
            .filter(|bytes| *bytes <= self.remaining_bytes)
            .ok_or_else(|| resource_error("native section sampled scratch exhausted"))?;
        values
            .try_reserve_exact(capacity - values.len())
            .map_err(|_| resource_error("native section sampled allocation failed"))?;
        let actual_growth = (values.capacity() - previous)
            .checked_mul(std::mem::size_of::<T>())
            .ok_or_else(|| resource_error("native section sampled byte overflow"))?;
        self.remaining_bytes = self
            .remaining_bytes
            .checked_sub(actual_growth)
            .ok_or_else(|| resource_error("native section sampled scratch exhausted"))?;
        Ok(())
    }

    pub(super) fn reserve_loop(
        &mut self,
        loops: &mut Vec<CanonicalLoop>,
    ) -> Result<(), GeometryError> {
        if loops.len() == loops.capacity() {
            let capacity = if loops.capacity() == 0 {
                4
            } else {
                loops
                    .capacity()
                    .checked_mul(2)
                    .ok_or_else(|| resource_error("native section loop capacity overflow"))?
            };
            self.reserve(loops, capacity)?;
        }
        Ok(())
    }

    fn reserve_point(&mut self, points: &mut Vec<[f64; 3]>) -> Result<(), GeometryError> {
        if points.len() == points.capacity() {
            let capacity = if points.capacity() == 0 {
                4
            } else {
                points
                    .capacity()
                    .checked_mul(2)
                    .ok_or_else(|| resource_error("native section point capacity overflow"))?
            }
            .min(MAX_LOOP_POINTS as usize);
            if capacity <= points.len() {
                return Err(resource_error(
                    "native section loop exceeds 40000-point cap",
                ));
            }
            self.reserve(points, capacity)?;
        }
        Ok(())
    }

    fn push_source_face(
        &mut self,
        faces: &mut Vec<usize>,
        face: usize,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        self.native.charge(1, cancel)?;
        if faces.len() == faces.capacity() {
            let capacity = faces
                .capacity()
                .checked_mul(2)
                .map(|capacity| capacity.max(4))
                .ok_or_else(|| resource_error("native slice source capacity overflow"))?;
            self.reserve(faces, capacity)?;
        }
        faces.push(face);
        Ok(())
    }

    fn charge(&mut self, cancel: &AtomicBool) -> Result<(), GeometryError> {
        check_cancel(cancel)?;
        if self.nodes == MAX_SAMPLING_NODES {
            return Err(resource_error(
                "native section curve enclosure work exhausted",
            ));
        }
        self.native.charge(1, cancel)?;
        self.nodes += 1;
        Ok(())
    }
}

pub(super) const MAX_CURVE_CAPTURE_BYTES: usize = 32 * 1024 * 1024;
type CurveUse = (monstertruck_topology::EdgeId<Curve>, bool);

#[derive(Clone, Copy, PartialEq, Eq)]
struct SourceFaceUse {
    face_id: monstertruck_topology::FaceId<Surface>,
    shell_index: usize,
    face_index: usize,
}

#[derive(Clone, Copy)]
enum CapturedEndpoint {
    Unbound,
    Generated { peer: usize, point: Point3 },
    Coordinate(CoordinatePlane),
}

struct CapturedCurve {
    curve: Curve,
    endpoints: [CapturedEndpoint; 2],
}

/// Bounded oriented search guides and exact original source-face uses.
pub(super) struct CapturedCurves {
    curves: Vec<CapturedCurve>,
    uses: HashMap<CurveUse, usize>,
    source_faces: Vec<Vec<SourceFaceUse>>,
    endpoint_origins: Vec<endpoint_genealogy::SectionPlaneEndpointOrigin>,
    retained_bytes: usize,
    byte_limit: usize,
}

/// Bounded error-only expression ancestry, never a geometry dump or proof.
struct CurveDomainContext<'a>(&'a Curve);
impl std::fmt::Display for CurveDomainContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut curve = self.0;
        for depth in 0..=8 {
            let kind = match curve {
                Curve::Line(_) => "LINE",
                Curve::BsplineCurve(_) => "BSPLINE",
                Curve::NurbsCurve(_) => "NURBS",
                Curve::ParameterCurve(_) => "PARAMETER",
                Curve::IntersectionCurve(_) => "INTERSECTION",
                Curve::TrimmedCurve(_) => "TRIMMED",
            };
            write!(f, "{kind}{:?}", curve.range_tuple())?;
            match curve {
                Curve::TrimmedCurve(wrapper) if depth < 8 => {
                    write!(f, "(forward={})->", wrapper.orientation())?;
                    curve = wrapper.entity().curve();
                }
                Curve::IntersectionCurve(intersection) if depth < 8 => {
                    write!(f, "->leader=")?;
                    curve = intersection.leader();
                }
                Curve::TrimmedCurve(_) | Curve::IntersectionCurve(_) => {
                    return write!(f, "->depth-limit");
                }
                Curve::ParameterCurve(parameter) => {
                    return write!(
                        f,
                        "(uv={:?}{:?},carrier={:?})",
                        std::mem::discriminant(parameter.curve()),
                        parameter.curve().range_tuple(),
                        std::mem::discriminant(parameter.surface().as_ref())
                    );
                }
                _ => return Ok(()),
            }
        }
        Ok(())
    }
}

struct CarrierDomainContext<'a>(&'a Surface);
impl std::fmt::Display for CarrierDomainContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Surface::ExtrusionSurface(extrusion) = self.0 {
            write!(
                f,
                "EXTRUSION(profile={},direction={:?})",
                CurveDomainContext(extrusion.entity_curve()),
                extrusion.extruding_vector()
            )
        } else {
            write!(f, "{:?}", std::mem::discriminant(self.0))
        }
    }
}

impl CapturedCurves {
    pub(super) fn collect(
        definition: &Definition,
        cut: &monstertruck_solid::BooleanWithProvenance<Curve, Surface>,
        caps: &HashSet<monstertruck_topology::FaceId<Surface>>,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        let byte_limit = remaining_bytes.min(MAX_CURVE_CAPTURE_BYTES);
        let mut result = Self {
            curves: Vec::new(),
            uses: HashMap::new(),
            source_faces: Vec::new(),
            endpoint_origins: Vec::new(),
            retained_bytes: 0,
            byte_limit,
        };
        let mut child_bytes = 0_usize;
        for face in cut.solid.face_iter() {
            work.charge(1, cancel)?;
            if !caps.contains(&face.id()) {
                continue;
            }
            for wire in face.absolute_boundaries() {
                for edge in wire {
                    work.charge(1, cancel)?;
                    let key = (edge.id(), edge.orientation());
                    if result.uses.contains_key(&key) {
                        continue;
                    }
                    // Reserve only bounded index/inline storage first. Count
                    // recursive native buffers before cloning their contents.
                    let next = result
                        .curves
                        .len()
                        .checked_add(1)
                        .ok_or_else(|| resource_error("native curve capture count overflow"))?;
                    next.checked_next_power_of_two()
                        .and_then(|count| count.checked_mul(2))
                        .map(|count| count.max(8))
                        .and_then(|count| {
                            count.checked_mul(2 * (std::mem::size_of::<(CurveUse, usize)>() + 32))
                        })
                        .and_then(|bytes| {
                            next.checked_mul(
                                2 * (std::mem::size_of::<CapturedCurve>()
                                    + std::mem::size_of::<Vec<SourceFaceUse>>()),
                            )
                            .and_then(|inline| bytes.checked_add(inline))
                        })
                        .and_then(|bytes| bytes.checked_add(child_bytes))
                        .filter(|bytes| *bytes <= byte_limit)
                        .ok_or_else(|| {
                            resource_error("native curve index growth exceeds scratch")
                        })?;
                    result
                        .curves
                        .try_reserve_exact(1)
                        .map_err(|_| resource_error("native curve capture allocation failed"))?;
                    result
                        .source_faces
                        .try_reserve_exact(1)
                        .map_err(|_| resource_error("native source lineage allocation failed"))?;
                    result
                        .uses
                        .try_reserve(1)
                        .map_err(|_| resource_error("native curve index allocation failed"))?;
                    let metadata = result
                        .curves
                        .capacity()
                        .checked_mul(std::mem::size_of::<CapturedCurve>())
                        .and_then(|bytes| {
                            result
                                .uses
                                .capacity()
                                .checked_mul(2 * (std::mem::size_of::<(CurveUse, usize)>() + 32))
                                .and_then(|index| bytes.checked_add(index))
                        })
                        .and_then(|bytes| {
                            result
                                .source_faces
                                .capacity()
                                .checked_mul(std::mem::size_of::<Vec<SourceFaceUse>>())
                                .and_then(|origins| bytes.checked_add(origins))
                        })
                        .and_then(|bytes| bytes.checked_add(child_bytes))
                        .filter(|bytes| *bytes <= byte_limit)
                        .ok_or_else(|| resource_error("native curve capture scratch exhausted"))?;
                    let allowance = byte_limit - metadata;
                    let (curve, owned_bytes) =
                        crate::parametric::clone_oriented_curve_with_byte_limit(
                            edge, allowance, work, cancel,
                        )?;
                    let children = owned_bytes
                        .checked_sub(std::mem::size_of::<Curve>())
                        .ok_or_else(|| {
                            kernel_error("native curve clone accounting omitted inline data")
                        })?;
                    child_bytes = child_bytes
                        .checked_add(children)
                        .ok_or_else(|| resource_error("native curve capture byte overflow"))?;
                    result.retained_bytes = metadata
                        .checked_add(children)
                        .filter(|bytes| *bytes <= byte_limit)
                        .ok_or_else(|| resource_error("native curve capture scratch exhausted"))?;
                    result.uses.insert(key, result.curves.len());
                    result.curves.push(CapturedCurve {
                        curve,
                        endpoints: [CapturedEndpoint::Unbound; 2],
                    });
                    result.source_faces.push(Vec::new());
                }
                // Adjacent directed uses, not point proximity, identify a
                // generated cut junction. Both curves receive the same
                // coordinate proposal; native continuation must still prove
                // their common original root and complete joined coverage.
                for (before, after) in wire.iter().zip(wire.iter().cycle().skip(1)) {
                    work.charge(1, cancel)?;
                    if before.back().id() != after.front().id() {
                        return Err(kernel_error("native cap wire lost shared vertex identity"));
                    }
                    let before_index = *result
                        .uses
                        .get(&(before.id(), before.orientation()))
                        .ok_or_else(|| kernel_error("native junction lost its incoming curve"))?;
                    let after_index = *result
                        .uses
                        .get(&(after.id(), after.orientation()))
                        .ok_or_else(|| kernel_error("native junction lost its outgoing curve"))?;
                    let point = before.back().point();
                    result.curves[before_index].endpoints[1] = CapturedEndpoint::Generated {
                        peer: after_index,
                        point,
                    };
                    result.curves[after_index].endpoints[0] = CapturedEndpoint::Generated {
                        peer: before_index,
                        point,
                    };
                }
            }
        }
        // The opposite native coedge use binds a cap edge to its original
        // source face. Carrier similarity and nearest geometry cannot do so.
        for coedge in &cut.coedge_origins {
            work.charge(1, cancel)?;
            let indices = [true, false]
                .map(|orientation| result.uses.get(&(coedge.output_edge, orientation)).copied());
            if indices.iter().all(Option::is_none) {
                continue;
            }
            let origin = coedge.source_face();
            if origin.operand != monstertruck_solid::BooleanOperand::Source {
                continue;
            }
            let native_face = definition
                .solid
                .boundaries()
                .get(origin.shell_index)
                .and_then(|shell| shell.get(origin.face_index))
                .ok_or_else(|| kernel_error("native cap source-face ordinal is invalid"))?;
            if native_face.id() != origin.face_id {
                return Err(kernel_error("native cap source-face identity changed"));
            }
            let owner = SourceFaceUse {
                face_id: origin.face_id,
                shell_index: origin.shell_index,
                face_index: origin.face_index,
            };
            for index in indices.into_iter().flatten() {
                let owners = &mut result.source_faces[index];
                if owners.contains(&owner) {
                    continue;
                }
                owners
                    .len()
                    .checked_add(1)
                    .and_then(|count| count.checked_mul(2 * std::mem::size_of::<SourceFaceUse>()))
                    .and_then(|bytes| bytes.checked_add(result.retained_bytes))
                    .filter(|bytes| *bytes <= byte_limit)
                    .ok_or_else(|| resource_error("native source owner growth exceeds scratch"))?;
                let old_capacity = owners.capacity();
                owners
                    .try_reserve_exact(1)
                    .map_err(|_| resource_error("native source owner allocation failed"))?;
                let growth = (owners.capacity() - old_capacity)
                    .checked_mul(std::mem::size_of::<SourceFaceUse>())
                    .ok_or_else(|| resource_error("native source owner byte overflow"))?;
                result.retained_bytes = result
                    .retained_bytes
                    .checked_add(growth)
                    .filter(|bytes| *bytes <= byte_limit)
                    .ok_or_else(|| resource_error("native source owner scratch exhausted"))?;
                owners.push(owner);
            }
        }
        if result.source_faces.iter().any(Vec::is_empty) {
            return Err(GeometryError::new(
                GeometryErrorCode::AnalysisIndeterminate,
                "native cap edge has no paired original source-face use",
            ));
        }
        let (endpoint_origins, endpoint_bytes) = endpoint_genealogy::collect(
            definition,
            cut,
            &result.uses,
            byte_limit - result.retained_bytes,
            work,
            cancel,
        )?;
        result.retained_bytes = result
            .retained_bytes
            .checked_add(endpoint_bytes)
            .filter(|bytes| *bytes <= byte_limit)
            .ok_or_else(|| resource_error("native endpoint genealogy exceeds capture capacity"))?;
        for origin in &endpoint_origins {
            work.charge(1, cancel)?;
            let curve = result
                .curves
                .get_mut(origin.candidate_index)
                .ok_or_else(|| {
                    kernel_error("original endpoint genealogy lost its captured curve")
                })?;
            // Original trim closure retains its independent root census and
            // receipt, never the generated-junction coordinate proposal.
            curve.endpoints[usize::from(origin.endpoint_is_back)] = CapturedEndpoint::Unbound;
        }
        for index in 0..result.curves.len() {
            work.charge(1, cancel)?;
            let CapturedEndpoint::Generated { peer, point } = result.curves[index].endpoints[1]
            else {
                continue;
            };
            if !matches!(
                result.curves[peer].endpoints[0],
                CapturedEndpoint::Generated { peer: previous, .. } if previous == index
            ) {
                continue;
            }
            let coordinate =
                generated_endpoint_coordinate(&result.curves[index].curve, point, work, cancel)?
                    .map_or(CapturedEndpoint::Unbound, CapturedEndpoint::Coordinate);
            result.curves[index].endpoints[1] = coordinate;
            result.curves[peer].endpoints[0] = coordinate;
        }
        result.endpoint_origins = endpoint_origins;
        Ok(result)
    }

    pub(super) fn endpoint_origins(&self) -> &[endpoint_genealogy::SectionPlaneEndpointOrigin] {
        &self.endpoint_origins
    }

    pub(super) fn remaining_bytes(&self) -> usize {
        self.byte_limit - self.retained_bytes
    }

    pub(super) fn prepare(
        &self,
        definition: &Definition,
        plane: &monstertruck_modeling::Plane,
        roots: &crate::parametric::PreparedSectionPlaneRoots<'_>,
        root_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(Vec<PreparedCurveEnclosure<'static>>, usize), GeometryError> {
        let byte_limit = self
            .byte_limit
            .checked_sub(root_bytes)
            .ok_or_else(|| resource_error("native original roots exceed capture capacity"))?;
        self.curves
            .len()
            .checked_mul(std::mem::size_of::<PreparedCurveEnclosure<'_>>())
            .and_then(|bytes| bytes.checked_add(self.retained_bytes))
            .filter(|bytes| *bytes <= byte_limit)
            .ok_or_else(|| resource_error("native continuation index exceeds scratch"))?;
        let mut prepared = Vec::new();
        prepared
            .try_reserve_exact(self.curves.len())
            .map_err(|_| resource_error("native continuation index allocation failed"))?;
        let mut retained = prepared
            .capacity()
            .checked_mul(std::mem::size_of::<PreparedCurveEnclosure<'_>>())
            .and_then(|bytes| bytes.checked_add(self.retained_bytes))
            .filter(|bytes| *bytes <= byte_limit)
            .ok_or_else(|| resource_error("native continuation scratch exhausted"))?;
        let mut chart_proposals = crate::parametric::EndpointChartProposals::new(
            roots,
            self.endpoint_origins.len(),
            byte_limit - retained,
            work,
            cancel,
        )?;
        let candidate_byte_limit = byte_limit
            .checked_sub(chart_proposals.retained_bytes())
            .filter(|bytes| *bytes >= retained)
            .ok_or_else(|| resource_error("native endpoint proposals exceed capture capacity"))?;
        for (candidate_index, (capture, owners)) in
            self.curves.iter().zip(&self.source_faces).enumerate()
        {
            let curve = &capture.curve;
            let selected = owners
                .first()
                .ok_or_else(|| kernel_error("captured native edge has no source owner"))?;
            let source_face = definition
                .solid
                .boundaries()
                .get(selected.shell_index)
                .and_then(|shell| shell.get(selected.face_index))
                .filter(|face| face.id() == selected.face_id)
                .ok_or_else(|| kernel_error("captured native source-face identity is invalid"))?;
            // A chart seam can have several original source owners. Only
            // exact native coefficient identity licenses one root carrier
            // for all of them; retain every source identity regardless.
            for owner in owners.iter().skip(1) {
                let other = definition
                    .solid
                    .boundaries()
                    .get(owner.shell_index)
                    .and_then(|shell| shell.get(owner.face_index))
                    .filter(|face| face.id() == owner.face_id)
                    .ok_or_else(|| {
                        kernel_error("captured native source-face identity is invalid")
                    })?;
                // IDs identify the shared Surface mutex, including cloned or
                // inverted uses. Keep coefficient validation even for one
                // shared carrier. Distinct carriers lock in definition order,
                // independently of which owner supplies the root source.
                let same = if other.id() == source_face.id() {
                    source_face.with_surface(|source| {
                        crate::parametric::same_surface_coefficients(source, source, work, cancel)
                    })
                } else if (selected.shell_index, selected.face_index)
                    < (owner.shell_index, owner.face_index)
                {
                    source_face.with_surface(|source| {
                        other.with_surface(|other| {
                            crate::parametric::same_surface_coefficients(
                                source, other, work, cancel,
                            )
                        })
                    })
                } else {
                    other.with_surface(|other| {
                        source_face.with_surface(|source| {
                            crate::parametric::same_surface_coefficients(
                                source, other, work, cancel,
                            )
                        })
                    })
                }?;
                if !same {
                    return Err(GeometryError::new(
                        GeometryErrorCode::AnalysisIndeterminate,
                        "native cap source owners require unresolved carrier correspondence",
                    ));
                }
            }
            let mut original_ends = [None; 2];
            let mut ambiguous_ends = [false; 2];
            for origin in &self.endpoint_origins {
                work.charge(1, cancel)?;
                if origin.candidate_index == candidate_index
                    && origin.shell_index == selected.shell_index
                    && origin.face_index == selected.face_index
                    && origin.identity.face_id == selected.face_id
                {
                    let end = usize::from(origin.endpoint_is_back);
                    ambiguous_ends[end] |= original_ends[end].replace(origin).is_some();
                }
            }
            let anchor_end = usize::from(original_ends[0].is_none());
            if ambiguous_ends[anchor_end] {
                return Err(GeometryError::new(
                    GeometryErrorCode::AnalysisIndeterminate,
                    "native section endpoint anchor has ambiguous original source genealogy",
                ));
            }
            // Derive each consumed original cut root before locking the
            // candidate carrier. Midpoints only propose charts; native whole
            // source certificates and complete original coverage remain proof.
            let both = original_ends.iter().all(Option::is_some)
                && !ambiguous_ends.iter().any(|ambiguous| *ambiguous);
            let mut source_charts = [None; 2];
            for (end, origin) in original_ends.into_iter().enumerate() {
                if !both && end != anchor_end {
                    continue;
                }
                if let Some(origin) = origin {
                    source_charts[end] = Some(chart_proposals.lookup(origin, work, cancel)?);
                }
            }
            use monstertruck_modeling::ValidatedFacePlaneSourceAnchor;
            let source_anchor = match source_charts {
                [Some(front), Some(back)] => {
                    Some(ValidatedFacePlaneSourceAnchor::Both { front, back })
                }
                [Some(front), None] => Some(ValidatedFacePlaneSourceAnchor::Front(front)),
                [None, Some(back)] => Some(ValidatedFacePlaneSourceAnchor::Back(back)),
                [None, None] => None,
            };
            let proof = source_face
                .with_surface(|source| {
                    let result = crate::parametric::prepare_section_curve_enclosure(
                        curve,
                        monstertruck_modeling::ValidatedFacePlaneSource {
                            surface: source,
                            anchor: source_anchor,
                            endpoint_coordinate_planes: capture.endpoints.map(|endpoint| {
                                match endpoint {
                                    CapturedEndpoint::Coordinate(coordinate) => Some(coordinate),
                                    _ => None,
                                }
                            }),
                        },
                        plane,
                        curve.range_tuple(),
                        candidate_byte_limit - retained,
                        work,
                        cancel,
                    );
                    result.map_err(|mut error| {
                        let original_face = (selected.shell_index == 0)
                            .then(|| definition.faces.get(selected.face_index))
                            .flatten().map(|face| face.face_id.as_str());
                        error.message = format!(
                            "source={} source_face={original_face:?} guide={} carrier={} anchor={source_anchor:?} endpoint_planes={:?} section_plane_origin={:?}: {}",
                            definition.id().as_str(), CurveDomainContext(curve),
                            CarrierDomainContext(source), capture.endpoints.map(|endpoint| {
                                match endpoint {
                                    CapturedEndpoint::Coordinate(coordinate) => Some(coordinate),
                                    _ => None,
                                }
                            }), plane.origin(), error.message,
                        );
                        error
                    })
                })
                .map_err(|mut error| {
                    error.message = format!(
                        "original source shell {} face {} / section-root range {:?}: {}",
                        selected.shell_index,
                        selected.face_index,
                        curve.range_tuple(),
                        error.message,
                    );
                    error
                })?;
            retained = retained
                .checked_add(proof.retained_bytes())
                .filter(|bytes| *bytes <= candidate_byte_limit)
                .ok_or_else(|| resource_error("native continuation scratch exhausted"))?;
            prepared.push(proof);
        }
        drop(chart_proposals);
        Ok((prepared, byte_limit - retained))
    }
}

fn generated_endpoint_coordinate(
    curve: &Curve,
    point: Point3,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<CoordinatePlane>, GeometryError> {
    let guide: &Curve = match curve {
        Curve::IntersectionCurve(branch) => branch.leader(),
        _ => curve,
    };
    let end = curve.range_tuple().1;
    let tangent = crate::parametric::curve_enclosure(guide, (end, end), work, cancel)?
        .derivative
        .map(|range| 0.5 * range.lo + 0.5 * range.hi);
    if (0..3).any(|axis| !tangent[axis].is_finite() || !point[axis].is_finite()) {
        return Ok(None);
    }
    let mut axis = 0;
    for next in 1..3 {
        if tangent[next].abs() > tangent[axis].abs() {
            axis = next;
        }
    }
    if tangent[axis] == 0.0 {
        return Ok(None);
    }
    let mut normal = [0.0; 3];
    normal[axis] = 1.0;
    Ok(Some(CoordinatePlane {
        normal,
        offset: point[axis],
    }))
}

/// Samples complete original face wires, without intersection genealogy or a
/// section certificate. The caller must independently prove an injective
/// carrier projection before treating their winding as a filled projection.
pub(crate) fn sample_original_face(
    definition: &Definition,
    face: &monstertruck_modeling::Face,
    frame: &PlaneFrameMm,
    byte_limit: usize,
    native: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(Vec<SectionLoopMm>, [f64; 2]), GeometryError> {
    let mut work = SamplingWork::new(native, byte_limit);
    let mut budget = SectionBudget::with_limit(
        u32::try_from(byte_limit.min(MAX_SECTION_BYTES as usize))
            .map_err(|_| resource_error("original face sampling capacity overflow"))?,
    )?;
    let mut loops = Vec::new();
    for wire in face.absolute_boundaries() {
        work.charge(cancel)?;
        if !wire.is_closed() || wire.is_empty() {
            return Err(kernel_error(
                "original face has an open or empty trim cycle",
            ));
        }
        let reference: [f64; 3] = wire
            .iter()
            .next()
            .expect("nonempty original wire")
            .front()
            .point()
            .into();
        let mut points = Vec::new();
        let mut area = Interval::point(0.0);
        for edge in wire {
            work.charge(cancel)?;
            let previous = points.pop();
            let start = points.len();
            let part = edge.with_curve(|curve| {
                let prepared = definition.prepare_boundary_curve(
                    edge.id(),
                    curve,
                    curve.range_tuple(),
                    work.remaining_bytes,
                    work.native,
                    cancel,
                )?;
                let retained = prepared.retained_bytes();
                work.remaining_bytes = work
                    .remaining_bytes
                    .checked_sub(retained)
                    .ok_or_else(|| resource_error("original curve and sample exceed scratch"))?;
                let directed_front: [f64; 3] = edge.front().point().into();
                let directed_back: [f64; 3] = edge.back().point().into();
                let (front, back) = if edge.orientation() {
                    (directed_front, directed_back)
                } else {
                    (directed_back, directed_front)
                };
                let result = sample_curve(
                    &CurveSamplingQuery {
                        prepared: &prepared,
                        range: curve.range_tuple(),
                        endpoint_strata: &[],
                        front,
                        back,
                        frame,
                        maximum_chord_error_mm: SECTION_SAMPLING_TOLERANCE_MM,
                        maximum_plane_error_mm: SECTION_PLANE_TOLERANCE_MM,
                        reference_mm: reference,
                        budget: &budget,
                        new_curve: true,
                    },
                    &mut work,
                    &mut points,
                    cancel,
                );
                work.remaining_bytes += retained;
                result
            })?;
            let part = if edge.orientation() {
                Interval::new(part[0], part[1])?
            } else {
                work.native.charge((points.len() - start) as u64, cancel)?;
                points[start..].reverse();
                Interval::new(-part[1], -part[0])?
            };
            if previous.is_some_and(|previous| points.get(start) != Some(&previous)) {
                return Err(kernel_error(
                    "original trim uses disagree at their shared vertex",
                ));
            }
            area = area.add(part);
        }
        let boundary = canonicalize(points, frame, 1.0, [area.lo, area.hi], work.native, cancel)?;
        budget.push_loop(boundary.native.points_mm.len())?;
        let count = loops.len() + 1;
        work.reserve(&mut loops, count)?;
        loops.push(boundary.native);
    }
    if loops.is_empty() {
        return Err(kernel_error("original face has no complete trim cycles"));
    }
    Ok((loops, work.error_bounds()))
}

pub(super) struct WireSampling<'a, 'source> {
    pub(super) frame: &'a PlaneFrameMm,
    pub(super) maximum_chord_error_mm: f64,
    pub(super) maximum_plane_error_mm: f64,
    pub(super) face_sign: f64,
    pub(super) budget: &'a SectionBudget,
    pub(super) captures: &'a CapturedCurves,
    pub(super) prepared: &'a [PreparedCurveEnclosure<'source>],
    pub(super) coverage: &'a SectionPlaneCoverage,
}

pub(super) fn sample_wire(
    wire: &Wire,
    query: &WireSampling<'_, '_>,
    work: &mut SamplingWork<'_>,
    cancel: &AtomicBool,
) -> Result<CanonicalLoop, GeometryError> {
    let &WireSampling {
        frame,
        maximum_chord_error_mm,
        maximum_plane_error_mm,
        face_sign,
        budget,
        captures,
        prepared,
        coverage,
    } = query;
    check_cancel(cancel)?;
    if !wire.is_closed() || !face_sign.is_finite() || face_sign.abs() < 0.5 {
        return Err(kernel_error(
            "native section cap boundary is not a closed oriented wire",
        ));
    }
    let mut points = Vec::new();
    let reference_mm = wire
        .iter()
        .next()
        .ok_or_else(|| kernel_error("native section cap has an empty wire"))?
        .front()
        .point()
        .into();
    let mut native_area = [0.0, 0.0];
    let mut source_faces = Vec::new();
    for edge in wire {
        check_cancel(cancel)?;
        let index = *captures
            .uses
            .get(&(edge.id(), edge.orientation()))
            .ok_or_else(|| kernel_error("native cap edge has no retained curve capture"))?;
        let curve = &captures.curves[index].curve;
        let proof = prepared
            .get(index)
            .ok_or_else(|| kernel_error("native cap edge has no continuation proof"))?;
        let endpoint_strata = coverage.endpoint_strata(index);
        for receipt in endpoint_strata {
            work.charge(cancel)?;
            if receipt.origin.candidate_index != index
                || !captures.source_faces[index].iter().any(|owner| {
                    owner.face_id == receipt.origin.identity.face_id
                        && owner.shell_index == receipt.origin.shell_index
                        && owner.face_index == receipt.origin.face_index
                })
            {
                return Err(kernel_error(
                    "native endpoint receipt lost its original curve owner",
                ));
            }
        }
        let range = curve.range_tuple();
        let front: [f64; 3] = edge.front().point().into();
        let back: [f64; 3] = edge.back().point().into();
        let area = sample_curve(&CurveSamplingQuery { prepared: proof, range, endpoint_strata, front, back, frame, maximum_chord_error_mm, maximum_plane_error_mm, reference_mm, budget, new_curve: false }, work, &mut points, cancel)
        .map_err(|mut error| {
            error.message = format!(
                "native cap curve {index} / sample range {range:?}: {}; sampling nodes={}, scratch remaining={} B, geometry work consumed={}, remaining={}",
                error.message,
                work.nodes,
                work.remaining_bytes,
                work.native.consumed(),
                work.native.remaining(),
            );
            error
        })?;
        native_area[0] = (native_area[0] + area[0]).next_down();
        native_area[1] = (native_area[1] + area[1]).next_up();
        // Every owner was identity-checked and proved to share this actual
        // source-face/plane root during CapturedCurves::prepare.
        for owner in &captures.source_faces[index] {
            if owner.shell_index != 0 {
                return Err(GeometryError::new(
                    GeometryErrorCode::AnalysisIndeterminate,
                    "native slice source ordinals require the admitted single source shell",
                ));
            }
            work.push_source_face(&mut source_faces, owner.face_index, cancel)?;
        }
        check_cancel(cancel)?;
    }
    if source_faces.is_empty() {
        return Err(GeometryError::new(
            GeometryErrorCode::AnalysisIndeterminate,
            "native section wire has no proved original source-face owners",
        ));
    }
    let levels = usize::BITS - source_faces.len().leading_zeros();
    work.native.charge(
        (source_faces.len() as u64)
            .checked_mul(u64::from(levels))
            .ok_or_else(|| resource_error("native slice source ordering work overflow"))?,
        cancel,
    )?;
    source_faces.sort_unstable();
    source_faces.dedup();
    let mut boundary = canonicalize(points, frame, face_sign, native_area, work.native, cancel)?;
    boundary.source_faces = source_faces;
    Ok(boundary)
}

struct CurveSamplingQuery<'a, 'source> {
    prepared: &'a PreparedCurveEnclosure<'source>,
    range: (f64, f64),
    endpoint_strata: &'a [SectionPlaneEndpointReceipt],
    front: [f64; 3],
    back: [f64; 3],
    frame: &'a PlaneFrameMm,
    maximum_chord_error_mm: f64,
    maximum_plane_error_mm: f64,
    reference_mm: [f64; 3],
    budget: &'a SectionBudget,
    new_curve: bool,
}

fn sample_curve(
    query: &CurveSamplingQuery<'_, '_>,
    work: &mut SamplingWork<'_>,
    points: &mut Vec<[f64; 3]>,
    cancel: &AtomicBool,
) -> Result<[f64; 2], GeometryError> {
    let &CurveSamplingQuery {
        prepared,
        range,
        endpoint_strata,
        front,
        back,
        frame,
        maximum_chord_error_mm,
        maximum_plane_error_mm,
        reference_mm,
        budget,
        new_curve,
    } = query;
    check_cancel(cancel)?;
    if !range.0.is_finite() || !range.1.is_finite() || range.0 >= range.1 {
        return Err(kernel_error(
            "native cap curve has no ordered bounded parameter interval",
        ));
    }
    for endpoint in [front, back] {
        if plane_residual_upper(endpoint, frame)? > maximum_plane_error_mm {
            return Err(kernel_error(
                "native cap endpoint exceeds plane residual tolerance",
            ));
        }
    }
    if !new_curve && let Some(previous) = points.last() {
        // Native coedges share vertex identity, not proximity. Preserve those
        // exact points so closure never silently shifts a certified last chord.
        if *previous != front {
            return Err(kernel_error(
                "native section wire has unequal shared endpoints",
            ));
        }
    } else {
        budget.check_loop(1)?;
        work.reserve_point(points)?;
        points.push(front);
    }
    // Native partitions include every knot and continuation chart transition.
    // No second-derivative interpolation bound may cross an unresolved corner.
    let mut covered_until = range.0;
    let mut area = [0.0, 0.0];
    let mut endpoint_extension = [0.0_f64; 2];
    let mut endpoint_plane_error = 0.0_f64;
    for receipt in endpoint_strata {
        work.charge(cancel)?;
        let native = &receipt.native;
        let end = usize::from(native.endpoint_is_back());
        endpoint_extension[end] = endpoint_extension[end].max(native.maximum_extension_mm());
        endpoint_plane_error = endpoint_plane_error.max(native.maximum_plane_error_mm());
        if endpoint_extension[end] > maximum_chord_error_mm
            || endpoint_plane_error > maximum_plane_error_mm
        {
            return Err(GeometryError::new(
                GeometryErrorCode::AnalysisIndeterminate,
                "native original endpoint closure exceeds the requested section error allowance",
            ));
        }
        let correction = endpoint_area_error(receipt, frame, reference_mm)?;
        area[0] = (area[0] - correction).next_down();
        area[1] = (area[1] + correction).next_up();
    }
    // The fixed recursion stack and its live proof values share the existing
    // query allowance. Receipts never escape this exact prepared-curve sample.
    let scratch = std::mem::size_of::<
        [Option<SamplingNode<'_, '_>>; MAX_SAMPLING_DEPTH as usize + 1],
    >() + std::mem::size_of::<SamplingNode<'_, '_>>()
        + std::mem::size_of::<CurveSamplingInterval<'_, '_>>()
        + std::mem::size_of::<Option<CurveSamplingSmoothRange<'_, '_>>>()
        + 3 * std::mem::size_of::<CurveSamplingEndpoint<'_, '_>>()
        + 3 * std::mem::size_of::<crate::parametric::CurveEnclosure>()
        + std::mem::size_of::<[[crate::parametric::Interval; 3]; 2]>()
        + std::mem::size_of::<monstertruck_modeling::ValidatedCurveJet>();
    work.remaining_bytes = work.remaining_bytes.checked_sub(scratch).ok_or_else(|| {
        resource_error("native section sampling proof stack exceeds scratch capacity")
    })?;
    let result = (|| {
        let mut stack = [None; MAX_SAMPLING_DEPTH as usize + 1];
        let mut previous_end: Option<CurveSamplingEndpoint<'_, '_>> = None;
        for span in prepared.parameter_intervals() {
            check_cancel(cancel)?;
            if span.0 != covered_until || span.0 >= span.1 || span.1 > range.1 {
                return Err(kernel_error(
                    "native curve partitions do not cover the original edge",
                ));
            }
            covered_until = span.1;
            let start = match previous_end.filter(|endpoint| endpoint.parameter() == span.0) {
                Some(endpoint) => endpoint,
                None => prepared.sampling_endpoint(span.0, work.native, cancel)?,
            };
            let end = prepared.sampling_endpoint(span.1, work.native, cancel)?;
            previous_end = Some(end);
            stack[0] = Some(([start, end], None, 0_u8));
            let mut pending = 1;
            while pending != 0 {
                work.charge(cancel)?;
                pending -= 1;
                let (endpoints, inherited_smooth_range, depth) =
                    stack[pending].take().expect("pending native sampling span");
                let start = endpoints[0].parameter();
                let end = endpoints[1].parameter();
                let interval = prepared.sampling_interval(
                    &endpoints,
                    inherited_smooth_range,
                    frame,
                    maximum_plane_error_mm,
                    work.native,
                    cancel,
                )?;
                let enclosure = interval.enclosure();
                check_cancel(cancel)?;
                if !enclosure.chord_error_mm.is_finite()
                    || enclosure.chord_error_mm < 0.0
                    || !enclosure.plane_error_mm.is_finite()
                    || enclosure.plane_error_mm < 0.0
                    || enclosure
                        .start_mm
                        .iter()
                        .chain(&enclosure.end_mm)
                        .any(|v| !v.is_finite())
                {
                    return Err(kernel_error(
                        "native curve enclosure has an invalid error bound",
                    ));
                }
                let final_interval = end == range.1;
                let output_end = if final_interval {
                    back
                } else {
                    enclosure.end_mm
                };
                let output_start = *points.last().expect("native curve start retained");
                let start_shift = separation_upper(output_start, enclosure.start_mm);
                let end_shift = separation_upper(output_end, enclosure.end_mm);
                if start == range.0 && start_shift > SECTION_PLANE_TOLERANCE_MM
                    || final_interval && end_shift > SECTION_PLANE_TOLERANCE_MM
                {
                    return Err(kernel_error(
                        "native curve enclosure disagrees with edge endpoints",
                    ));
                }
                // Vertex displacement and the sealed original-root continuation
                // spend the same frozen chord allowance as the sampled interior.
                let front_extension = if start == range.0 {
                    endpoint_extension[0]
                } else {
                    0.0
                };
                let back_extension = if final_interval {
                    endpoint_extension[1]
                } else {
                    0.0
                };
                let chord_error = (enclosure.chord_error_mm
                    + start_shift.max(end_shift)
                    + front_extension.max(back_extension))
                .next_up();
                let plane_error = enclosure
                    .plane_error_mm
                    .max(endpoint_plane_error)
                    .max(plane_residual_upper(output_start, frame)?)
                    .max(plane_residual_upper(output_end, frame)?);
                if chord_error <= maximum_chord_error_mm && plane_error <= maximum_plane_error_mm {
                    budget.check_loop(points.len() + 1)?;
                    let part_area = interval.signed_area(
                        &endpoints,
                        frame,
                        reference_mm,
                        work.native,
                        cancel,
                    )?;
                    if part_area.iter().any(|value| !value.is_finite())
                        || part_area[0] > part_area[1]
                    {
                        return Err(kernel_error(
                            "native curve has an invalid signed-area enclosure",
                        ));
                    }
                    area[0] = (area[0] + part_area[0]).next_down();
                    area[1] = (area[1] + part_area[1]).next_up();
                    work.maximum_chord_error_mm = work.maximum_chord_error_mm.max(chord_error);
                    work.maximum_plane_error_mm = work.maximum_plane_error_mm.max(plane_error);
                    work.reserve_point(points)?;
                    points.push(output_end);
                    continue;
                }
                if depth == MAX_SAMPLING_DEPTH {
                    return Err(resource_error(
                        "native section curve enclosure depth exhausted",
                    ));
                }
                let middle = start * 0.5 + end * 0.5;
                if middle <= start || middle >= end {
                    return Err(GeometryError::new(
                        GeometryErrorCode::AnalysisIndeterminate,
                        "native section curve cannot resolve the requested interval error",
                    ));
                }
                let midpoint = prepared.sampling_endpoint(middle, work.native, cancel)?;
                let smooth_range = interval.smooth_range();
                stack[pending] = Some(([midpoint, endpoints[1]], smooth_range, depth + 1));
                stack[pending + 1] = Some(([endpoints[0], midpoint], smooth_range, depth + 1));
                pending += 2;
            }
        }
        if covered_until != range.1 {
            return Err(kernel_error(
                "native curve partitions omit an original edge interval",
            ));
        }
        Ok(area)
    })();
    work.remaining_bytes += scratch;
    result
}

/// Independently enclose the additional Green integral. The outward L1 bounds
/// dominate Euclidean norms; the actual frame covectors retain their rounded
/// area scale rather than assuming an exactly orthonormal floating frame.
fn endpoint_area_error(
    receipt: &SectionPlaneEndpointReceipt,
    frame: &PlaneFrameMm,
    reference_mm: [f64; 3],
) -> Result<f64, GeometryError> {
    let position = receipt.native.position_bounds();
    let radius = (0..3).fold(Interval::point(0.0), |sum, axis| {
        let component = Interval {
            lo: position[axis].lower,
            hi: position[axis].upper,
        }
        .sub(Interval::point(reference_mm[axis]));
        sum.add(Interval::point(component.abs_upper()))
    });
    let frame_area_scale = (0..3).fold(Interval::point(0.0), |sum, axis| {
        let a = (axis + 1) % 3;
        let b = (axis + 2) % 3;
        let component = Interval::point(frame.x_axis[a])
            .mul(Interval::point(frame.y_axis[b]))
            .sub(Interval::point(frame.x_axis[b]).mul(Interval::point(frame.y_axis[a])));
        sum.add(Interval::point(component.abs_upper()))
    });
    let correction = Interval::point(0.5)
        .mul(radius)
        .mul(Interval::point(receipt.native.maximum_arc_length_mm()))
        .mul(frame_area_scale)
        .hi;
    if !correction.is_finite() || correction < 0.0 {
        return Err(GeometryError::new(
            GeometryErrorCode::ConditioningFailure,
            "native endpoint continuation has a nonfinite Green-area bound",
        ));
    }
    Ok(correction)
}

fn canonicalize(
    points: Vec<[f64; 3]>,
    frame: &PlaneFrameMm,
    face_sign: f64,
    native_area: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<CanonicalLoop, GeometryError> {
    if !native_area.iter().all(|value| value.is_finite())
        || native_area[0] > native_area[1]
        || native_area[0] <= 0.0 && native_area[1] >= 0.0
    {
        return Err(GeometryError::new(
            GeometryErrorCode::AnalysisIndeterminate,
            format!(
                "native section wire orientation is unresolved within interval bounds [{:.17e}, {:.17e}] mm^2",
                native_area[0], native_area[1]
            ),
        ));
    }
    canonicalize_oriented(points, frame, face_sign, native_area[0] > 0.0, work, cancel)
}

fn canonicalize_oriented(
    mut points: Vec<[f64; 3]>,
    frame: &PlaneFrameMm,
    face_sign: f64,
    native_positive: bool,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<CanonicalLoop, GeometryError> {
    check_cancel(cancel)?;
    if points.len() < 4 || points.first() != points.last() {
        return Err(kernel_error("native section loop is not exactly closed"));
    }
    points.pop();
    let reference = points[0];
    let mut previous = [Interval::point(0.0); 2];
    let mut twice_area = Interval::point(0.0);
    let mut bounds = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for point in &points {
        work.charge(1, cancel)?;
        let local = in_frame(*point, frame)?;
        let offset: [Interval; 3] = std::array::from_fn(|axis| {
            Interval::point(point[axis]).sub(Interval::point(reference[axis]))
        });
        let current = [frame.x_axis, frame.y_axis].map(|axis| {
            (0..3).fold(Interval::point(0.0), |sum, index| {
                sum.add(offset[index].mul(Interval::point(axis[index])))
            })
        });
        twice_area = twice_area.add(previous[0].mul(current[1]).sub(previous[1].mul(current[0])));
        previous = current;
        bounds[0] = bounds[0].min(local[0]);
        bounds[1] = bounds[1].min(local[1]);
        bounds[2] = bounds[2].max(local[0]);
        bounds[3] = bounds[3].max(local[1]);
    }
    // The final edge returns to the translated origin and contributes zero.
    if !twice_area.lo.is_finite() || !twice_area.hi.is_finite() || twice_area.contains_zero() {
        return Err(GeometryError::new(
            GeometryErrorCode::AnalysisIndeterminate,
            "sampled native section boundary has unresolved polygon orientation",
        ));
    }
    let polygon_positive = twice_area.lo > 0.0;
    if native_positive != polygon_positive {
        return Err(GeometryError::new(
            GeometryErrorCode::AnalysisIndeterminate,
            "sampled section orientation disagrees with whole-native-curve orientation",
        ));
    }
    let is_hole = native_positive != (face_sign > 0.0);
    if polygon_positive == is_hole {
        points.reverse();
    }
    let mut least = 0;
    let mut least_coordinate = in_frame(points[0], frame)?;
    for (index, point) in points.iter().enumerate().skip(1) {
        work.charge(1, cancel)?;
        let coordinate = in_frame(*point, frame)?;
        if compare_coordinates(&coordinate, &least_coordinate) == Ordering::Less {
            least = index;
            least_coordinate = coordinate;
        }
    }
    points.rotate_left(least);
    // Closure has already been proven. Represent the repeated endpoint exactly
    // once, with the same f64 coordinate as the first canonical native point.
    points.push(points[0]);
    Ok(CanonicalLoop {
        native: SectionLoopMm {
            is_hole,
            points_mm: points,
        },
        bounds,
        source_faces: Vec::new(),
    })
}

/// Reorder a placed section after its caller proves positive source/world frame
/// orientation. Its existing native winding remains the authority; a rounded
/// world polygon with zero-containing or opposite signed area refuses instead
/// of silently reversing that winding. The caller discards the section on error.
pub(crate) fn canonicalize_section_in_place(
    section: &mut NativeSection,
    query_byte_limit: u32,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    work.charge(1, cancel)?;
    let native_lineage = matches!(
        section.certificate.authority,
        crate::SectionAuthority::NativeIntersection,
    );
    if native_lineage {
        if section.loop_source_faces.len() != section.loops.len()
            || section
                .loop_source_faces
                .iter()
                .any(|owners| owners.is_empty() || owners.windows(2).any(|pair| pair[0] >= pair[1]))
        {
            return Err(GeometryError::new(
                GeometryErrorCode::AnalysisIndeterminate,
                "native section loop source lineage is missing or inconsistent",
            ));
        }
    } else if !section.loop_source_faces.is_empty() {
        return Err(kernel_error(
            "derived slab envelope cannot carry native slice source lineage",
        ));
    }
    let remaining_bytes = (query_byte_limit as usize)
        .checked_sub(section.retained_bytes())
        .ok_or_else(|| resource_error("placed section exceeds query capacity"))?;
    section
        .loops
        .len()
        .checked_mul(std::mem::size_of::<CanonicalLoop>())
        .filter(|bytes| *bytes <= remaining_bytes)
        .ok_or_else(|| resource_error("placed section ordering exceeds query capacity"))?;
    let mut ordered = Vec::new();
    ordered
        .try_reserve_exact(section.loops.len())
        .map_err(|_| resource_error("placed section ordering allocation failed"))?;
    ordered
        .capacity()
        .checked_mul(std::mem::size_of::<CanonicalLoop>())
        .filter(|bytes| *bytes <= remaining_bytes)
        .ok_or_else(|| resource_error("placed section ordering exceeds query capacity"))?;
    for (index, boundary) in section.loops.iter_mut().enumerate() {
        work.charge(1, cancel)?;
        let native_positive = !boundary.is_hole;
        let points = std::mem::take(&mut boundary.points_mm);
        let mut canonical =
            canonicalize_oriented(points, &section.frame, 1.0, native_positive, work, cancel)?;
        if native_lineage {
            canonical.source_faces = std::mem::take(&mut section.loop_source_faces[index]);
        }
        ordered.push(canonical);
    }
    sort_loops(&mut ordered, &section.frame, work, cancel)?;
    for (index, (boundary, canonical)) in section.loops.iter_mut().zip(ordered).enumerate() {
        work.charge(1, cancel)?;
        let (native, source_faces) = canonical.into_parts();
        *boundary = native;
        if native_lineage {
            section.loop_source_faces[index] = source_faces;
        }
    }
    Ok(())
}

pub(super) fn sort_loops(
    boundaries: &mut [CanonicalLoop],
    frame: &PlaneFrameMm,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    let points = boundaries
        .iter()
        .try_fold(0_u64, |sum, boundary| {
            sum.checked_add(boundary.point_count() as u64)
                .and_then(|sum| sum.checked_add(boundary.source_faces.len() as u64))
        })
        .ok_or_else(|| resource_error("native section ordering work overflow"))?;
    let levels = usize::BITS - boundaries.len().max(1).leading_zeros();
    work.charge(
        points
            .checked_mul(u64::from(levels))
            .ok_or_else(|| resource_error("native section ordering work overflow"))?,
        cancel,
    )?;
    boundaries.sort_unstable_by(|a, b| compare_loops(a, b, frame));
    check_cancel(cancel)
}

pub(super) fn compare_loops(
    a: &CanonicalLoop,
    b: &CanonicalLoop,
    frame: &PlaneFrameMm,
) -> Ordering {
    a.native
        .is_hole
        .cmp(&b.native.is_hole)
        .then_with(|| compare_coordinates(&a.bounds, &b.bounds))
        .then_with(|| {
            for (a, b) in a.native.points_mm.iter().zip(&b.native.points_mm) {
                // Coordinates were validated during canonicalization; only
                // equal-bounds ties need this frame-sequence comparison.
                let a = in_frame(*a, frame).expect("validated native section point");
                let b = in_frame(*b, frame).expect("validated native section point");
                let order = compare_coordinates(&a, &b);
                if order != Ordering::Equal {
                    return order;
                }
            }
            a.native.points_mm.len().cmp(&b.native.points_mm.len())
        })
        .then_with(|| a.source_faces.cmp(&b.source_faces))
}

fn compare_coordinates<const N: usize>(a: &[f64; N], b: &[f64; N]) -> Ordering {
    for axis in 0..N {
        let order = a[axis].total_cmp(&b[axis]);
        if order != Ordering::Equal {
            return order;
        }
    }
    Ordering::Equal
}

fn plane_residual_upper(point: [f64; 3], frame: &PlaneFrameMm) -> Result<f64, GeometryError> {
    if point.iter().any(|coordinate| !coordinate.is_finite()) {
        return Err(kernel_error("native section point is nonfinite"));
    }
    let residual = (0..3).fold(Interval::point(0.0), |sum, axis| {
        sum.add(
            Interval::point(point[axis])
                .sub(Interval::point(frame.origin_mm[axis]))
                .mul(Interval::point(frame.z_axis[axis])),
        )
    });
    Ok(residual.abs_upper())
}

/// Outward L1 bound also bounds Euclidean displacement. Each correctly-rounded
/// subtraction and sum is widened separately, including subnormal results.
fn separation_upper(a: [f64; 3], b: [f64; 3]) -> f64 {
    let mut bound = 0.0;
    for axis in 0..3 {
        let difference = (a[axis] - b[axis]).abs().next_up();
        bound = (bound + difference).next_up();
    }
    bound
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PlaneMm;
    use spiling_contracts::geometry::GeometryErrorCode;

    #[test]
    fn canonical_loops_preserve_roles_and_reverse_to_declared_winding() {
        let frame = PlaneFrameMm::from_plane(PlaneMm {
            origin_mm: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
        })
        .unwrap();
        let points = vec![
            [1.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
        ];
        let mut work = GeometryWorkBudget::new(100);
        let cancel = AtomicBool::new(false);
        let mut outer = canonicalize(
            points.clone(),
            &frame,
            -1.0,
            [-1.0, -1.0],
            &mut work,
            &cancel,
        )
        .unwrap();
        let mut hole = canonicalize(points, &frame, 1.0, [-1.0, -1.0], &mut work, &cancel).unwrap();
        assert!(!outer.is_hole());
        assert!(hole.is_hole());
        assert_eq!(outer.native.points_mm[0], [0.0, 1.0, 0.0]);
        assert_eq!(
            outer.native.points_mm.first(),
            outer.native.points_mm.last()
        );
        assert_eq!(hole.native.points_mm.first(), hole.native.points_mm.last());
        assert_eq!(compare_loops(&outer, &hole, &frame), Ordering::Less);
        outer.source_faces = vec![7, 11];
        hole.source_faces = vec![3, 9];
        let mut boundaries = vec![hole, outer];
        sort_loops(&mut boundaries, &frame, &mut work, &cancel).unwrap();
        assert!(!boundaries[0].native.is_hole);
        assert_eq!(boundaries[0].source_faces, [7, 11]);
        assert!(boundaries[1].native.is_hole);
        assert_eq!(boundaries[1].source_faces, [3, 9]);
    }

    #[test]
    fn section_caps_are_enforced_without_truncating_loops() {
        let mut budget = SectionBudget::default();
        assert_eq!(
            budget
                .push_loop(MAX_LOOP_POINTS as usize + 1)
                .unwrap_err()
                .code,
            GeometryErrorCode::ResourceLimit
        );
        for _ in 0..17 {
            budget.push_loop(MAX_LOOP_POINTS as usize).unwrap();
        }
        assert_eq!(
            budget.push_loop(MAX_LOOP_POINTS as usize).unwrap_err().code,
            GeometryErrorCode::ResourceLimit
        );
    }

    #[test]
    fn native_open_or_zero_area_boundary_is_not_canonicalized_as_success() {
        let frame = PlaneFrameMm::from_plane(PlaneMm {
            origin_mm: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
        })
        .unwrap();
        for points in [
            vec![[0.0; 3], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
            vec![[0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0; 3]],
        ] {
            assert!(
                canonicalize(
                    points,
                    &frame,
                    1.0,
                    [0.0, 0.0],
                    &mut GeometryWorkBudget::new(100),
                    &AtomicBool::new(false),
                )
                .is_err()
            );
        }
    }

    #[test]
    fn native_area_uncertainty_or_orientation_conflict_never_invents_loop_role() {
        let frame = PlaneFrameMm::from_plane(PlaneMm {
            origin_mm: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
        })
        .unwrap();
        let points = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0],
        ];
        for native_area in [[-0.1, 0.1], [-1.0, -1.0]] {
            let error = canonicalize(
                points.clone(),
                &frame,
                1.0,
                native_area,
                &mut GeometryWorkBudget::new(100),
                &AtomicBool::new(false),
            )
            .err()
            .expect("native orientation must be proved and agree with output");
            assert_eq!(error.code, GeometryErrorCode::AnalysisIndeterminate);
        }
    }

    #[test]
    fn whole_interval_sampling_keeps_cubic_excursions_hidden_from_midpoint() {
        use monstertruck_modeling::{BsplineCurve, KnotVector, Point3};
        let curve = Curve::BsplineCurve(BsplineCurve::new(
            KnotVector::bezier_knot(3),
            vec![
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(1.0 / 3.0, 1.0, 0.0),
                Point3::new(2.0 / 3.0, -1.0, 0.0),
                Point3::new(1.0, 0.0, 0.0),
            ],
        ));
        let frame = PlaneFrameMm::from_plane(PlaneMm {
            origin_mm: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
        })
        .unwrap();
        let mut native = GeometryWorkBudget::new(1_000_000);
        let prepared = crate::parametric::prepare_curve_enclosure(
            &curve,
            (0.0, 1.0),
            MAX_CURVE_CAPTURE_BYTES,
            &mut native,
            &AtomicBool::new(false),
        )
        .unwrap();
        let mut points = Vec::new();
        sample_curve(
            &CurveSamplingQuery {
                prepared: &prepared,
                range: (0.0, 1.0),
                endpoint_strata: &[],
                front: [0.0; 3],
                back: [1.0, 0.0, 0.0],
                frame: &frame,
                maximum_plane_error_mm: SECTION_PLANE_TOLERANCE_MM,
                maximum_chord_error_mm: SECTION_SAMPLING_TOLERANCE_MM,
                reference_mm: [0.0; 3],
                budget: &SectionBudget::default(),
                new_curve: false,
            },
            &mut SamplingWork::new(&mut native, MAX_CURVE_CAPTURE_BYTES),
            &mut points,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(points.iter().any(|point| point[1] > 0.2));
        assert!(points.iter().any(|point| point[1] < -0.2));
        assert_eq!(points.first(), Some(&[0.0; 3]));
        assert_eq!(points.last(), Some(&[1.0, 0.0, 0.0]));
    }

    #[test]
    fn knot_corner_is_not_certified_by_zero_piecewise_second_derivatives() {
        use monstertruck_modeling::{BsplineCurve, KnotVector, Point3};
        let curve = Curve::BsplineCurve(BsplineCurve::new(
            KnotVector::from(vec![0.0, 0.0, 0.5, 1.0, 1.0]),
            vec![
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(0.5, 1.0, 0.0),
                Point3::new(1.0, 0.0, 0.0),
            ],
        ));
        let frame = PlaneFrameMm::from_plane(PlaneMm {
            origin_mm: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
        })
        .unwrap();
        let mut native = GeometryWorkBudget::new(1_000_000);
        let prepared = crate::parametric::prepare_curve_enclosure(
            &curve,
            (0.0, 1.0),
            MAX_CURVE_CAPTURE_BYTES,
            &mut native,
            &AtomicBool::new(false),
        )
        .unwrap();
        let mut points = Vec::new();
        sample_curve(
            &CurveSamplingQuery {
                prepared: &prepared,
                range: (0.0, 1.0),
                endpoint_strata: &[],
                front: [0.0; 3],
                back: [1.0, 0.0, 0.0],
                frame: &frame,
                maximum_plane_error_mm: SECTION_PLANE_TOLERANCE_MM,
                maximum_chord_error_mm: SECTION_SAMPLING_TOLERANCE_MM,
                reference_mm: [0.0; 3],
                budget: &SectionBudget::default(),
                new_curve: false,
            },
            &mut SamplingWork::new(&mut native, MAX_CURVE_CAPTURE_BYTES),
            &mut points,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(points.iter().any(|point| point[1] > 0.99));
    }

    #[test]
    fn sampling_proof_stack_refuses_tiny_scratch_and_honors_cancellation() {
        use monstertruck_modeling::Line;
        let curve = Curve::Line(Line(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0)));
        let mut native = GeometryWorkBudget::new(100_000);
        let prepared = crate::parametric::prepare_curve_enclosure(
            &curve,
            (0.0, 1.0),
            MAX_CURVE_CAPTURE_BYTES,
            &mut native,
            &AtomicBool::new(false),
        )
        .unwrap();
        let frame = PlaneFrameMm::from_plane(PlaneMm {
            origin_mm: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
        })
        .unwrap();
        for (cancelled, expected) in [
            (false, GeometryErrorCode::ResourceLimit),
            (true, GeometryErrorCode::Cancelled),
        ] {
            let mut points = Vec::new();
            let mut work = SamplingWork::new(&mut native, 256);
            let error = sample_curve(
                &CurveSamplingQuery {
                    prepared: &prepared,
                    range: (0.0, 1.0),
                    endpoint_strata: &[],
                    front: [0.0; 3],
                    back: [1.0, 0.0, 0.0],
                    frame: &frame,
                    maximum_plane_error_mm: SECTION_PLANE_TOLERANCE_MM,
                    maximum_chord_error_mm: SECTION_SAMPLING_TOLERANCE_MM,
                    reference_mm: [0.0; 3],
                    budget: &SectionBudget::default(),
                    new_curve: false,
                },
                &mut work,
                &mut points,
                &AtomicBool::new(cancelled),
            )
            .unwrap_err();
            assert_eq!(error.code, expected);
        }
    }

    #[test]
    fn sampling_work_is_cumulative_and_cancellation_wins_over_exhaustion() {
        let mut native = GeometryWorkBudget::new(1);
        let mut work = SamplingWork::new(&mut native, MAX_CURVE_CAPTURE_BYTES);
        work.nodes = MAX_SAMPLING_NODES - 1;
        work.charge(&AtomicBool::new(false)).unwrap();
        assert_eq!(
            work.charge(&AtomicBool::new(false)).unwrap_err().code,
            GeometryErrorCode::ResourceLimit
        );
        assert_eq!(
            work.charge(&AtomicBool::new(true)).unwrap_err().code,
            GeometryErrorCode::Cancelled
        );
    }

    #[test]
    fn sampled_capacity_uses_only_remaining_shared_scratch_before_allocation() {
        let mut native = GeometryWorkBudget::new(1);
        let mut none = SamplingWork::new(&mut native, 0);
        let mut points = Vec::new();
        assert_eq!(
            none.reserve_point(&mut points).unwrap_err().code,
            GeometryErrorCode::ResourceLimit
        );
        assert_eq!(points.capacity(), 0);
        let mut boundaries = Vec::new();
        assert_eq!(
            none.reserve_loop(&mut boundaries).unwrap_err().code,
            GeometryErrorCode::ResourceLimit
        );
        assert_eq!(boundaries.capacity(), 0);
        let mut owners = Vec::new();
        assert_eq!(
            none.push_source_face(&mut owners, 0, &AtomicBool::new(false))
                .unwrap_err()
                .code,
            GeometryErrorCode::ResourceLimit,
        );
        assert_eq!(owners.capacity(), 0);

        let mut native = GeometryWorkBudget::new(1);
        let mut exact = SamplingWork::new(&mut native, 4 * std::mem::size_of::<[f64; 3]>());
        exact.reserve_point(&mut points).unwrap();
        assert_eq!(exact.remaining_bytes(), 0);
        points.resize(4, [0.0; 3]);
        assert_eq!(
            exact.reserve_point(&mut points).unwrap_err().code,
            GeometryErrorCode::ResourceLimit
        );
        assert_eq!(points.capacity(), 4);
    }
}
