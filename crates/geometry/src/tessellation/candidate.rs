// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Display candidates from SolidCore's planner (`solidcore-tess`), read off the definition's
//! own record. The planner meshes every face in its own (u, v) and samples every edge once, so
//! neighbouring faces share their boundary nodes bit for bit; this module only gives those
//! shared nodes identities and the faces their kernel names. Nothing here is authority: the
//! audits decide publication against the native coefficients.

use super::{invalid, resource};
use crate::{GeometryWorkBudget, kernel::Definition, parametric};
use solidcore::math::{Point2, Point3, Vec3};
use solidcore::{EdgeId, FaceId};
use solidcore_tess::{Control, MeshTolerance, PlanStats, Stop, TessError, TessPlan};
use spiling_contracts::geometry::{GeometryError, GeometryErrorCode};
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// The caller's work budget, cancellation and retained-byte allowance as the planner's control.
/// The first refusal is kept so its cause, not only the planner's stop, is reported.
struct Ledger<'a> {
    work: &'a GeometryWorkBudget,
    cancel: &'a AtomicBool,
    bytes: AtomicUsize,
    limit: usize,
    refusal: Mutex<Option<GeometryError>>,
}

impl Ledger<'_> {
    fn refuse(&self, cause: GeometryError) {
        self.refusal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_or_insert(cause);
    }

    /// The planner's stop as the error the caller reports.
    fn stopped(&self, stop: Stop) -> GeometryError {
        if let Some(cause) = self
            .refusal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            return cause;
        }
        match stop {
            Stop::Cancelled => GeometryError::new(
                GeometryErrorCode::Cancelled,
                "native display plan cancelled",
            ),
            Stop::Work | Stop::Bytes => resource(format!("native display plan stopped: {stop:?}")),
        }
    }
}

impl Control for Ledger<'_> {
    fn charge(&self, work: u64) -> Result<(), Stop> {
        self.work.charge(work, self.cancel).map_err(|cause| {
            let stop = if cause.code == GeometryErrorCode::Cancelled {
                Stop::Cancelled
            } else {
                Stop::Work
            };
            self.refuse(cause);
            stop
        })
    }

    fn admit(&self, bytes: usize) -> Result<(), Stop> {
        let before = self.bytes.fetch_add(bytes, Ordering::AcqRel);
        if before.saturating_add(bytes) > self.limit {
            self.refuse(resource(format!(
                "native display plan exceeds its retained allowance of {} bytes",
                self.limit
            )));
            return Err(Stop::Bytes);
        }
        Ok(())
    }
}

/// One edge's published nodes: its kernel edge, each node's curve parameter and shared id.
pub(crate) struct EdgePolyline {
    pub edge: EdgeId,
    pub params: Vec<f64>,
    pub vertices: Vec<u32>,
    /// Every node at one point (a collapsed edge): it has no chord to stand for.
    pub degenerate: bool,
}

/// Every edge's nodes, each node once with its shared id, as the faces meet them.
pub(crate) struct DisplayEdges {
    positions: Vec<Point3>,
    /// Sorted by kernel edge.
    edges: Vec<EdgePolyline>,
}

impl DisplayEdges {
    pub(crate) fn polyline(&self, edge: EdgeId) -> Option<&EdgePolyline> {
        self.edges
            .binary_search_by_key(&edge, |polyline| polyline.edge)
            .ok()
            .map(|index| &self.edges[index])
    }

    pub(crate) fn polylines(&self) -> &[EdgePolyline] {
        &self.edges
    }

    pub(crate) fn positions(&self) -> &[Point3] {
        &self.positions
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.edges.iter().fold(
            size_of::<Self>()
                .saturating_add(self.positions.capacity() * size_of::<Point3>())
                .saturating_add(self.edges.capacity() * size_of::<EdgePolyline>()),
            |sum, polyline| {
                sum.saturating_add(polyline.params.capacity() * size_of::<f64>())
                    .saturating_add(polyline.vertices.capacity() * size_of::<u32>())
            },
        )
    }
}

/// One face's candidate: its nodes' (u, v) on the face's carrier, positions (a boundary node's
/// is its edge node's point), unit outward normals (zero where the carrier has none), shared
/// ids (boundary nodes only) and triangles, counter-clockwise seen from outside.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FaceCandidate {
    pub face: FaceId,
    pub uvs: Vec<Point2>,
    pub positions: Vec<Point3>,
    pub normals: Vec<Vec3>,
    pub shared: Vec<Option<u32>>,
    pub triangles: Vec<[u32; 3]>,
    /// The planner filled it leniently: some of its boundary sides are not triangle sides.
    pub lenient: bool,
}

impl FaceCandidate {
    pub(crate) fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            .saturating_add(self.uvs.capacity() * size_of::<Point2>())
            .saturating_add(self.positions.capacity() * size_of::<Point3>())
            .saturating_add(self.normals.capacity() * size_of::<Vec3>())
            .saturating_add(self.shared.capacity() * size_of::<Option<u32>>())
            .saturating_add(self.triangles.capacity() * size_of::<[u32; 3]>())
    }
}

/// A whole body's candidates, faces by definition ordinal.
pub(crate) struct Candidates {
    pub edges: DisplayEdges,
    pub faces: Vec<FaceCandidate>,
    pub stats: PlanStats,
}

/// The definition's body as tess sees it: its box diagonal, which the planner's tolerance
/// rules read.
fn body_size(definition: &Definition) -> f64 {
    let bounds = &definition.bounds;
    let d: [f64; 3] = std::array::from_fn(|i| bounds.max[i] - bounds.min[i]);
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// Plan the definition's record at `tolerance` under the caller's work, cancellation and
/// `allowance` of retained bytes, and read it off as candidates. A face the planner could not
/// mesh, an edge it could not sample, or a face its evaluation refuses is a reported failure,
/// never a dropped face.
pub(crate) fn plan(
    definition: &Definition,
    tolerance: MeshTolerance,
    allowance: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Candidates, GeometryError> {
    let ledger = Ledger {
        work,
        cancel,
        bytes: AtomicUsize::new(0),
        limit: allowance,
        refusal: Mutex::new(None),
    };
    let record = definition.record();
    let (plan, stats) =
        solidcore_tess::plan_with(record, &tolerance, body_size(definition), &ledger).map_err(
            |cause| match cause {
                TessError::Stopped(stop) => ledger.stopped(stop),
                other => invalid(format!("native display plan: {other}")),
            },
        )?;
    refuse_unplanned(definition, &stats)?;
    let order = definition.record_order();
    if plan.faces.len() != record.faces.len()
        || plan.edges.len() != record.edges.len()
        || plan.edge_params.len() != record.edges.len()
        || order.faces.len() != record.faces.len()
        || order.edges.len() != record.edges.len()
    {
        return Err(invalid(
            "native display plan lost the record's face or edge order",
        ));
    }
    let (edges, ids) = display_edges(&plan, order, work, cancel)?;
    let mut faces: Vec<Option<FaceCandidate>> =
        (0..definition.face_ids().len()).map(|_| None).collect();
    for face_plan in &plan.faces {
        work.charge(1, cancel)?;
        let record_face = face_plan.face as usize;
        let face = *order
            .faces
            .get(record_face)
            .ok_or_else(|| invalid("native display plan names a face outside the record"))?;
        let ordinal = definition
            .face_ordinal(face)
            .ok_or_else(|| invalid("native display plan face has no source ordinal"))?;
        let lenient = stats
            .lenient
            .iter()
            .any(|refused| refused.face == record_face);
        let candidate = face_candidate(definition, face, face_plan, &ids, lenient, work, cancel)?;
        let slot = faces
            .get_mut(ordinal)
            .ok_or_else(|| invalid("native display face ordinal is outside the face table"))?;
        if slot.replace(candidate).is_some() {
            return Err(invalid("native display plan names one face twice"));
        }
    }
    let faces = faces
        .into_iter()
        .map(|face| face.ok_or_else(|| invalid("native display plan omitted a source face")))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Candidates {
        edges,
        faces,
        stats,
    })
}

/// What the planner flagged on faces it did mesh, by source face: filled leniently, left past
/// its tolerance by a memory bound, pole normals averaged, edges whose sampling a bound stopped.
/// Empty when it flagged nothing. A display failure carries it, so a refusal names what the
/// planner itself reported.
pub(crate) fn plan_notes(definition: &Definition, stats: &PlanStats) -> String {
    let named = |record_face: usize| {
        definition
            .record_order()
            .faces
            .get(record_face)
            .and_then(|face| definition.face_ordinal(*face))
            .map_or_else(
                || format!("record face {record_face}"),
                |ordinal| definition.faces()[ordinal].face_id.as_str().to_owned(),
            )
    };
    let mut notes = Vec::new();
    if !stats.lenient.is_empty() {
        let faces: Vec<String> = stats.lenient.iter().map(|f| named(f.face)).collect();
        notes.push(format!("lenient={faces:?}"));
    }
    if !stats.over_tolerance.is_empty() {
        let faces: Vec<String> = stats
            .over_tolerance
            .iter()
            .map(|f| format!("{}:sag={}", named(f.face), f.sag))
            .collect();
        notes.push(format!("over_tolerance={faces:?}"));
    }
    if !stats.averaged_poles.is_empty() {
        notes.push(format!("averaged_poles={}", stats.averaged_poles.len()));
    }
    if !stats.edges_capped.is_empty() {
        notes.push(format!("edges_capped={}", stats.edges_capped.len()));
    }
    if notes.is_empty() {
        String::new()
    } else {
        format!("; plan: {}", notes.join(" "))
    }
}

/// The planner's own refusals, by source face: each is a failure of the display, named.
fn refuse_unplanned(definition: &Definition, stats: &PlanStats) -> Result<(), GeometryError> {
    let named = |record_face: usize| {
        definition
            .record_order()
            .faces
            .get(record_face)
            .and_then(|face| definition.face_ordinal(*face))
            .map_or_else(
                || format!("record face {record_face}"),
                |ordinal| definition.faces()[ordinal].face_id.as_str().to_owned(),
            )
    };
    if let Some(face) = stats.unmeshed.first() {
        return Err(invalid(format!(
            "native display could not mesh {} face(s); first {}: {}",
            stats.unmeshed.len(),
            named(face.face),
            face.reason
        )));
    }
    if let Some(edge) = stats.unsampled_edges.first() {
        return Err(invalid(format!(
            "native display could not sample {} edge(s); first record edge {}: {}",
            stats.unsampled_edges.len(),
            edge.edge,
            edge.reason
        )));
    }
    Ok(())
}

/// The bits a shared node is known by: its position exactly, with the two zeros one.
fn node_key(p: solidcore_record::Vec3) -> [u64; 3] {
    [p.x, p.y, p.z].map(|v| if v == 0.0 { 0 } else { v.to_bits() })
}

fn display_edges(
    plan: &TessPlan,
    order: &solidcore_convert::brep::Order,
    work: &GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(DisplayEdges, HashMap<[u64; 3], u32>), GeometryError> {
    let mut ids: HashMap<[u64; 3], u32> = HashMap::new();
    let mut positions = Vec::new();
    let mut edges = Vec::with_capacity(plan.edges.len());
    for (record_edge, (points, params)) in plan.edges.iter().zip(&plan.edge_params).enumerate() {
        work.charge(points.len() as u64 + 1, cancel)?;
        let edge = *order
            .edges
            .get(record_edge)
            .ok_or_else(|| invalid("native display plan names an edge outside the record"))?;
        if points.len() != params.len() {
            return Err(invalid(
                "native display edge has incomplete parameter ownership",
            ));
        }
        let mut vertices = Vec::with_capacity(points.len());
        for &point in points {
            if ![point.x, point.y, point.z].iter().all(|v| v.is_finite()) {
                return Err(invalid("native display edge node is not finite"));
            }
            let next = u32::try_from(positions.len())
                .map_err(|_| resource("native display shared node index overflow"))?;
            let id = *ids.entry(node_key(point)).or_insert_with(|| {
                positions.push(Point3::new(point.x, point.y, point.z));
                next
            });
            vertices.push(id);
        }
        let degenerate = vertices.windows(2).all(|pair| pair[0] == pair[1]);
        edges.push(EdgePolyline {
            edge,
            params: params.clone(),
            vertices,
            degenerate,
        });
    }
    edges.sort_unstable_by_key(|polyline| polyline.edge);
    if edges.windows(2).any(|pair| pair[0].edge == pair[1].edge) {
        return Err(invalid("native display plan names one kernel edge twice"));
    }
    Ok((DisplayEdges { positions, edges }, ids))
}

/// One face's plan as a candidate: boundary nodes take their edge node's point and shared id,
/// inner nodes are evaluated on the face's own carrier at their (u, v), and every node gets the
/// carrier's unit normal there, turned outward as the planner oriented the face.
fn face_candidate(
    definition: &Definition,
    face: FaceId,
    face_plan: &solidcore_tess::FacePlan,
    ids: &HashMap<[u64; 3], u32>,
    lenient: bool,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<FaceCandidate, GeometryError> {
    let surface = definition
        .face_surface(face)
        .ok_or_else(|| invalid("native display candidate has no carrier"))?;
    let count = face_plan.uv.len();
    if face_plan.fixed.len() > count {
        return Err(invalid(
            "native display face plan has more fixed nodes than nodes",
        ));
    }
    let mut candidate = FaceCandidate {
        face,
        uvs: Vec::with_capacity(count),
        positions: Vec::with_capacity(count),
        normals: Vec::with_capacity(count),
        shared: Vec::with_capacity(count),
        triangles: Vec::new(),
        lenient,
    };
    let sign = if face_plan.flip { -1.0 } else { 1.0 };
    // The planner's parameters at most four representable steps past the carrier's natural
    // domain are its rounding: evaluated on the domain's end (the rule of
    // `prepare_candidate_parameters`, which the audit applies again). Farther stays, and refuses.
    let domain = surface.natural_uv_domain_if_built();
    let bounds = [
        [domain.u_min(), domain.u_max()],
        [domain.v_min(), domain.v_max()],
    ];
    let roundoff = bounds.map(|[lower, upper]| {
        let mut enclosure = [lower, upper];
        for _ in 0..4 {
            enclosure = [parametric::down(enclosure[0]), parametric::up(enclosure[1])];
        }
        enclosure
    });
    for (index, planned) in face_plan.uv.iter().enumerate() {
        work.charge(1, cancel)?;
        if !planned.iter().all(|v| v.is_finite()) {
            return Err(invalid("native display face node has nonfinite parameters"));
        }
        let uv: [f64; 2] = std::array::from_fn(|axis| {
            let value = planned[axis];
            if roundoff[axis][0] <= value && value <= roundoff[axis][1] {
                value.clamp(bounds[axis][0], bounds[axis][1])
            } else {
                value
            }
        });
        let jet =
            parametric::surface_enclosure(surface, [[uv[0], uv[0]], [uv[1], uv[1]]], work, cancel)?;
        let (position, shared) = match face_plan.fixed.get(index) {
            Some(&point) => {
                let id = *ids.get(&node_key(point)).ok_or_else(|| {
                    invalid("native display boundary node is on no published edge")
                })?;
                (Point3::new(point.x, point.y, point.z), Some(id))
            }
            None => {
                let p = jet.position.map(parametric::Interval::mid);
                (Point3::new(p[0], p[1], p[2]), None)
            }
        };
        let du = jet.du.map(parametric::Interval::mid);
        let dv = jet.dv.map(parametric::Interval::mid);
        let n = Vec3::new(
            du[1] * dv[2] - du[2] * dv[1],
            du[2] * dv[0] - du[0] * dv[2],
            du[0] * dv[1] - du[1] * dv[0],
        );
        let length = n.length();
        let normal = if length.is_finite() && length > 0.0 {
            n * (sign / length)
        } else {
            Vec3::new(0.0, 0.0, 0.0)
        };
        candidate.uvs.push(Point2::new(uv[0], uv[1]));
        candidate.positions.push(position);
        candidate.normals.push(normal);
        candidate.shared.push(shared);
    }
    if face_plan
        .triangles
        .iter()
        .any(|t| t.iter().any(|&i| (i as usize) >= count))
    {
        return Err(invalid(
            "native display face plan names a node it does not have",
        ));
    }
    candidate.triangles.clone_from(&face_plan.triangles);
    chart_pole_fans(&mut candidate, surface, bounds, work, cancel)?;
    Ok(candidate)
}

/// How far apart a row's points may be for its nodes to be taken for one pole: a proposal
/// only, which the singular audit proves or refuses.
const COLLAPSED_ROW_MM: f64 = 1e-9;

/// A pole is one point of a whole carrier row, so its node's (u, v) is a choice of chart: the
/// planner fans a pole's ring to one copy of it, which leaves each fan triangle's preimage a long
/// sliver across the row. Each such triangle gets its own copy of the pole on the row, at the
/// middle of its ring side: the same point, shared id and published triangle, a local preimage.
/// The singular audit proves every copy is its collapsed native chart point.
fn chart_pole_fans(
    candidate: &mut FaceCandidate,
    surface: &solidcore::surface::surface::Surface,
    bounds: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    // A pole node: a boundary node whose shared id another node of the face has at another
    // (u, v), both on one row at the end of the domain along which the carrier is one point
    // (a seam's corner is on such a row too, but its row is a whole curve). The axis it runs
    // along is free.
    let mut by_shared: HashMap<u32, Vec<usize>> = HashMap::new();
    for (local, shared) in candidate.shared.iter().enumerate() {
        if let Some(shared) = shared {
            by_shared.entry(*shared).or_default().push(local);
        }
    }
    let mut free: Vec<Option<usize>> = vec![None; candidate.uvs.len()];
    for locals in by_shared.values() {
        work.charge(locals.len() as u64, cancel)?;
        let uv = |local: usize| [candidate.uvs[local].x, candidate.uvs[local].y];
        for axis in 0..2 {
            let row = 1 - axis;
            let first = uv(locals[0]);
            let on_row = locals.len() > 1
                && bounds[row].contains(&first[row])
                && locals.iter().all(|&local| uv(local)[row] == first[row])
                && locals.iter().any(|&local| uv(local)[axis] != first[axis]);
            if !on_row {
                continue;
            }
            let mut domain = [[first[row]; 2]; 2];
            domain[axis] = locals
                .iter()
                .fold([f64::INFINITY, f64::NEG_INFINITY], |range, &local| {
                    [range[0].min(uv(local)[axis]), range[1].max(uv(local)[axis])]
                });
            let image = parametric::surface_position_enclosure(surface, domain, work, cancel)?;
            if image.iter().all(|c| c.hi - c.lo <= COLLAPSED_ROW_MM) {
                for &local in locals {
                    free[local] = Some(axis);
                }
            }
        }
    }
    for triangle in 0..candidate.triangles.len() {
        work.charge(1, cancel)?;
        let corners = candidate.triangles[triangle];
        for corner in 0..3 {
            let pole = corners[corner] as usize;
            let Some(axis) = free[pole] else { continue };
            let others = [corners[(corner + 1) % 3], corners[(corner + 2) % 3]].map(|i| i as usize);
            if others.iter().any(|&other| free[other].is_some()) {
                continue;
            }
            let along = others.map(|other| [candidate.uvs[other].x, candidate.uvs[other].y][axis]);
            let (low, high) = (along[0].min(along[1]), along[0].max(along[1]));
            let at = [candidate.uvs[pole].x, candidate.uvs[pole].y];
            if low <= at[axis] && at[axis] <= high {
                continue;
            }
            let mut copy = at;
            copy[axis] = low * 0.5 + high * 0.5;
            let index = u32::try_from(candidate.uvs.len())
                .map_err(|_| resource("native display pole copy index overflow"))?;
            candidate.uvs.push(Point2::new(copy[0], copy[1]));
            candidate.positions.push(candidate.positions[pole]);
            candidate.normals.push(candidate.normals[pole]);
            candidate.shared.push(candidate.shared[pole]);
            free.push(Some(axis));
            candidate.triangles[triangle][corner] = index;
        }
    }
    Ok(())
}
