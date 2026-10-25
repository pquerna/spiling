// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Capture original STEP operands; never substitute the topology reader's fitted PCurves.

use std::sync::Arc;

use solidcore::FaceId;
use solidcraft_step::geom::{Associated, Curve, Pcurve};
use solidcraft_step::p21::Exchange;
use solidcraft_step::read::{Reader, Units};
use spiling_contracts::geometry::{FaceInfo, GeometryError, StepEntityId};
use std::sync::atomic::AtomicBool;

use super::ast::{Entities, at, enumeration, list, reference};
use super::{check_cancel, invalid, resolve_face};
use crate::kernel::source::{
    OriginalCoedgeSource, OriginalFaceSource, OriginalPcurveSource, OriginalSourceTopology,
    OriginalWireSource,
};

pub(super) fn capture(
    entities: &Entities<'_>,
    exchange: &Exchange,
    faces: &[FaceInfo],
    native_faces: &[FaceId],
    length_scale_to_mm: f64,
    cancel: &AtomicBool,
) -> Result<OriginalSourceTopology, GeometryError> {
    if faces.len() != native_faces.len() {
        return Err(invalid("original/native face census mismatch"));
    }
    let mut units = Units::default();
    units.len = length_scale_to_mm;
    let mut reader = Reader::new(exchange, units);
    let mut result = OriginalSourceTopology {
        faces: Vec::with_capacity(faces.len()),
        coedges: Vec::new(),
    };
    for (info, &native_face) in faces.iter().zip(native_faces) {
        check_cancel(cancel)?;
        let source_face = info.face_id.step_entity();
        let (resolved, orientation) = resolve_face(entities, source_face)?;
        let face = entities.records(resolved)?.iter().copied()
            .find(|record| matches!(record.name.as_str(), "ADVANCED_FACE" | "FACE_SURFACE"))
            .ok_or_else(|| invalid("original face has no surface-use record"))?;
        let carrier_id = reference(at(face, 2)?)?;
        let carrier = reader.surface(carrier_id)
            .map_err(|cause| invalid(&format!("original surface #{carrier_id}: {cause}")))?;
        let mut captured = OriginalFaceSource {
            native_face,
            source_face: StepEntityId(source_face),
            face_entity: StepEntityId(resolved),
            carrier_id: StepEntityId(carrier_id),
            carrier,
            same_sense: source_bool(at(face, 3)?)?,
            face_use_forward: orientation == source_bool(at(face, 3)?)?,
            wires: Vec::new(),
        };
        for bound in list(at(face, 1)?)? {
            check_cancel(cancel)?;
            let bound_id = reference(bound)?;
            let bound = entities.records(bound_id)?.iter().copied()
                .find(|record| matches!(record.name.as_str(), "FACE_BOUND" | "FACE_OUTER_BOUND"))
                .ok_or_else(|| invalid("original face bound has no supported loop use"))?;
            let loop_id = reference(at(bound, 1)?)?;
            let mut wire = OriginalWireSource {
                bound_id: StepEntityId(bound_id),
                loop_id: StepEntityId(loop_id),
                is_outer: bound.name == "FACE_OUTER_BOUND",
                forward: source_bool(at(bound, 2)?)?,
                coedges: Vec::new(),
                vertex_loop: None,
            };
            if let Some(vertex_loop) = entities.records(loop_id)?.iter()
                .find(|record| record.name == "VERTEX_LOOP")
            {
                let vertex_id = reference(at(vertex_loop, 1)?)?;
                let vertex = entities.record(vertex_id, "VERTEX_POINT")?;
                let point_id = reference(at(vertex, 1)?)?;
                let point = reader.point(point_id)
                    .map_err(|cause| invalid(&format!("original vertex #{vertex_id}: {cause}")))?;
                wire.vertex_loop = Some((StepEntityId(vertex_id), point));
                captured.wires.push(wire);
                continue;
            }
            let edge_loop = entities.record(loop_id, "EDGE_LOOP")?;
            for oriented in list(at(edge_loop, 1)?)? {
                check_cancel(cancel)?;
                let oriented_edge_id = reference(oriented)?;
                let (edge_curve_id, forward) = original_edge_use(entities, oriented_edge_id)?;
                let edge = entities.record(edge_curve_id, "EDGE_CURVE")?;
                let vertex_ids = [reference(at(edge, 1)?)?, reference(at(edge, 2)?)?];
                let mut vertex_points_mm = [[0.0; 3]; 2];
                for (slot, id) in vertex_points_mm.iter_mut().zip(vertex_ids) {
                    let vertex = entities.record(id, "VERTEX_POINT")?;
                    let point = reference(at(vertex, 1)?)?;
                    *slot = reader.point(point)
                        .map_err(|cause| invalid(&format!("original vertex #{id}: {cause}")))?;
                }
                let curve_id = reference(at(edge, 3)?)?;
                let spatial_curve = reader.curve(curve_id)
                    .map_err(|cause| invalid(&format!("original edge curve #{curve_id}: {cause}")))?;
                let pcurves = original_pcurves(entities, curve_id, &spatial_curve, 0)?;
                let index = result.coedges.len();
                result.coedges.push(OriginalCoedgeSource {
                    source_face: StepEntityId(source_face),
                    bound_id: StepEntityId(bound_id),
                    loop_id: StepEntityId(loop_id),
                    oriented_edge_id: StepEntityId(oriented_edge_id),
                    edge_curve_id: StepEntityId(edge_curve_id),
                    curve_id: StepEntityId(curve_id),
                    vertex_ids: vertex_ids.map(StepEntityId),
                    vertex_points_mm,
                    forward,
                    same_sense: source_bool(at(edge, 4)?)?,
                    spatial_curve,
                    pcurves,
                });
                wire.coedges.push(index);
            }
            if wire.coedges.is_empty() {
                return Err(invalid("original EDGE_LOOP has no coedge uses"));
            }
            for position in 0..wire.coedges.len() {
                let current = &result.coedges[wire.coedges[position]];
                let next = &result.coedges[wire.coedges[(position + 1) % wire.coedges.len()]];
                let end = current.vertex_ids[usize::from(current.forward)];
                let start = next.vertex_ids[usize::from(!next.forward)];
                if end != start {
                    return Err(invalid("original EDGE_LOOP is not closed by vertex identity"));
                }
            }
            captured.wires.push(wire);
        }
        result.faces.push(captured);
    }
    Ok(result)
}

pub(super) fn source_bool(parameter: &step_p21::ast::Parameter) -> Result<bool, GeometryError> {
    if enumeration(parameter, "T") {
        Ok(true)
    } else if enumeration(parameter, "F") {
        Ok(false)
    } else {
        Err(invalid("original topology orientation is not a STEP boolean"))
    }
}

fn original_edge_use(entities: &Entities<'_>, original: u64) -> Result<(u64, bool), GeometryError> {
    let mut id = original;
    let mut forward = true;
    for _ in 0..32 {
        let records = entities.records(id)?;
        if records.iter().any(|record| record.name == "EDGE_CURVE") {
            return Ok((id, forward));
        }
        let oriented = entities.record(id, "ORIENTED_EDGE")?;
        forward = forward == source_bool(at(oriented, 4)?)?;
        id = reference(at(oriented, 3)?)?;
    }
    Err(invalid("original oriented-edge nesting exceeds the source cap"))
}

fn original_pcurves(
    entities: &Entities<'_>,
    curve_id: u64,
    curve: &Arc<Curve>,
    depth: usize,
) -> Result<Vec<OriginalPcurveSource>, GeometryError> {
    if depth > 32 {
        return Err(invalid("original PCurve selector nesting exceeds the source cap"));
    }
    if let Curve::Trimmed { basis, .. } = curve.as_ref() {
        let trimmed = entities.record(curve_id, "TRIMMED_CURVE")?;
        return original_pcurves(entities, reference(at(trimmed, 1)?)?, basis, depth + 1);
    }
    let Some(record) = entities.records(curve_id)?.iter().copied().find(|record| {
        matches!(record.name.as_str(), "SURFACE_CURVE" | "SEAM_CURVE" | "INTERSECTION_CURVE" | "BOUNDED_SURFACE_CURVE")
    }) else {
        return Ok(Vec::new());
    };
    let Curve::SurfaceCurve { associated, .. } = curve.as_ref() else {
        return Err(invalid("original associated geometry lost its surface-curve operand"));
    };
    let declarations = list(at(record, 2)?)?;
    if declarations.len() != associated.len() {
        return Err(invalid("original associated-geometry census changed during decoding"));
    }
    let mut result = Vec::new();
    for (association_index, (declaration, operand)) in declarations.iter().zip(associated).enumerate() {
        let Associated::Pcurve(Pcurve { surface, curve }) = operand else {
            continue;
        };
        let source_pcurve = reference(declaration)?;
        let pcurve = entities.record(source_pcurve, "PCURVE")?;
        let carrier_id = reference(at(pcurve, 1)?)?;
        result.push(OriginalPcurveSource {
            source_pcurve: StepEntityId(source_pcurve),
            carrier_id: StepEntityId(carrier_id),
            association_index,
            operands: Pcurve { surface: Arc::clone(surface), curve: Arc::clone(curve) },
        });
    }
    Ok(result)
}
