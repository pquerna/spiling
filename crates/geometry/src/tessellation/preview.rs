// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{DisplayProgress, DisplayStage, candidate_query, check_memory, invalid, resource};
use crate::{GeometryWorkBudget, MeshArtifact, check_cancel, kernel::Definition};
use solidcore_mesh::{EdgeOptions, OutputOptions, TessellateOptions, Tolerance};
use spiling_contracts::{
    display::{MeshBudget, MeshChunkBuilder},
    geometry::{DisplayProfile, GeometryError, GeometryErrorCode, MESH_CARRIER_TOLERANCE_MM},
};
use std::{collections::HashMap, sync::atomic::AtomicBool};

pub(super) fn tessellate(
    definition: &Definition,
    cancel: &AtomicBool,
    budget: MeshBudget,
    work: &mut GeometryWorkBudget,
    progress: &mut impl FnMut(DisplayProgress),
) -> Result<MeshArtifact, GeometryError> {
    work.charge(1, cancel)?;
    if budget.vertices == 0 || budget.triangles == 0 || budget.bytes == 0 {
        return Err(resource("no display capacity remains"));
    }
    let faces = definition.face_ids();
    if faces.is_empty() || faces.len() != definition.faces().len() {
        return Err(invalid("preview requires the complete source face table"));
    }
    let kernel = crate::make_kernel_context();
    let options = TessellateOptions {
        chord_tolerance: Tolerance::Absolute(
            crate::tolerant::TolerantPolicy::default().profile_mm * 0.5,
        ),
        max_triangles_per_face: budget.triangles as usize,
        edge: EdgeOptions {
            max_segments: budget.vertices,
            ..EdgeOptions::default()
        },
        output: OutputOptions {
            normals: true,
            uvs: false,
            face_ids: true,
            edge_polylines: true,
        },
        measure_deviation: true,
        ..TessellateOptions::default()
    };
    let mut update = DisplayProgress {
        stage: DisplayStage::PreparingEdges,
        attempt: 0,
        completed_faces: 0,
        total_faces: faces.len() as u32,
        consumed_work: work.consumed(),
    };
    progress(update);
    let edges = candidate_query(0, work, cancel, |native| {
        solidcore_mesh::tessellate_edge_set_with_budget(
            &kernel,
            definition.brep(),
            &options,
            native,
        )
    })?;
    if !edges.failures().is_empty() {
        return Err(invalid("preview omitted a native boundary edge"));
    }
    let mut candidates = Vec::new();
    candidates
        .try_reserve_exact(faces.len())
        .map_err(|_| resource("preview face inventory allocation failed"))?;
    let mut retained = edges
        .retained_bytes()
        .checked_add(candidates.capacity() * std::mem::size_of::<solidcore_mesh::FaceMesh>())
        .ok_or_else(|| resource("preview retained size overflow"))?;
    for (ordinal, &face) in faces.iter().enumerate() {
        update.stage = DisplayStage::TessellatingFace;
        update.completed_faces = ordinal as u32;
        update.consumed_work = work.consumed();
        progress(update);
        let candidate = candidate_query(retained, work, cancel, |native| {
            solidcore_mesh::tessellate_face_with_budget(
                &kernel,
                definition.brep(),
                face,
                &edges,
                &options,
                native,
            )
        })?;
        if candidate.face != face
            || candidate.positions.is_empty()
            || candidate.triangles.is_empty()
        {
            return Err(invalid(
                "preview omitted or changed an original source face",
            ));
        }
        retained = retained
            .checked_add(candidate.retained_bytes())
            .ok_or_else(|| resource("preview face storage overflow"))?;
        check_memory(retained)?;
        candidates.push(candidate);
    }
    check_cancel(cancel)?;
    let mesh = assemble_preview(
        definition, &kernel, &edges, candidates, retained, work, cancel,
    )?;
    drop(edges);
    check_cancel(cancel)?;
    if !mesh.failures.is_empty()
        || mesh.triangles.len() != mesh.triangle_faces.len()
        || mesh.normals.len() != mesh.triangles.len()
    {
        return Err(invalid(
            "preview does not cover the complete native face inventory",
        ));
    }
    let carrier = mesh
        .stats
        .face_deviation
        .ok_or_else(|| invalid("preview has no measured surface deviation"))?;
    let trim = mesh
        .stats
        .edge_deviation
        .ok_or_else(|| invalid("preview has no measured boundary deviation"))?;
    if !carrier.is_finite()
        || !trim.is_finite()
        || carrier < 0.0
        || trim < 0.0
        || carrier + trim > MESH_CARRIER_TOLERANCE_MM
    {
        return Err(GeometryError::new(
            GeometryErrorCode::ShapeErrorExceeded,
            "measured preview deviation exceeds display profile",
        ));
    }
    check_memory(mesh.retained_bytes())?;
    let face_index_bytes = faces
        .len()
        .checked_mul(
            2 * std::mem::size_of::<(solidcore::FaceId, usize)>()
                + 2 * std::mem::size_of::<Vec<usize>>()
                + std::mem::size_of::<usize>(),
        )
        .and_then(|bytes| bytes.checked_add(mesh.triangles.len() * std::mem::size_of::<usize>()))
        .ok_or_else(|| resource("preview face index storage overflow"))?;
    check_memory(mesh.retained_bytes().saturating_add(face_index_bytes))?;
    let mut face_ordinals = HashMap::new();
    face_ordinals
        .try_reserve(faces.len())
        .map_err(|_| resource("preview source index allocation failed"))?;
    face_ordinals.extend(
        faces
            .iter()
            .enumerate()
            .map(|(ordinal, &face)| (face, ordinal)),
    );
    let mut counts = Vec::new();
    counts
        .try_reserve_exact(faces.len())
        .map_err(|_| resource("preview face count allocation failed"))?;
    counts.resize(faces.len(), 0usize);
    for face in &mesh.triangle_faces {
        work.charge(1, cancel)?;
        let ordinal = *face_ordinals
            .get(face)
            .ok_or_else(|| invalid("preview triangle lost native face ownership"))?;
        counts[ordinal] = counts[ordinal]
            .checked_add(1)
            .ok_or_else(|| resource("preview face count overflow"))?;
    }
    let mut by_face = Vec::new();
    by_face
        .try_reserve_exact(faces.len())
        .map_err(|_| resource("preview face index allocation failed"))?;
    for count in counts {
        let mut triangles = Vec::new();
        triangles
            .try_reserve_exact(count)
            .map_err(|_| resource("preview triangle index allocation failed"))?;
        by_face.push(triangles);
    }
    for (triangle_index, face) in mesh.triangle_faces.iter().enumerate() {
        work.charge(1, cancel)?;
        let ordinal = *face_ordinals
            .get(face)
            .ok_or_else(|| invalid("preview triangle lost native face ownership"))?;
        by_face[ordinal].push(triangle_index);
    }
    drop(face_ordinals);
    validate_closed(&mesh, face_index_bytes, work, cancel)?;
    let mut builder = MeshChunkBuilder::with_budget(
        definition.id().clone(),
        faces.len() as u32,
        DisplayProfile::MeshTolerantMm005V1,
        budget,
    )?;
    for (ordinal, triangles) in by_face.into_iter().enumerate() {
        work.charge(1, cancel)?;
        update.stage = DisplayStage::AuditingFace;
        update.completed_faces = ordinal as u32;
        update.consumed_work = work.consumed();
        progress(update);
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut indices = Vec::new();
        let mut vertices = HashMap::new();
        let maximum_vertices = triangles
            .len()
            .checked_mul(3)
            .ok_or_else(|| resource("preview vertex count overflow"))?;
        let adapter_bytes = maximum_vertices
            .checked_mul(
                2 * std::mem::size_of::<[f64; 3]>()
                    + 2 * std::mem::size_of::<((u32, [u64; 3]), u32)>(),
            )
            .and_then(|bytes| bytes.checked_add(triangles.len() * std::mem::size_of::<[u32; 3]>()))
            .and_then(|bytes| bytes.checked_add(mesh.retained_bytes()))
            .and_then(|bytes| bytes.checked_add(face_index_bytes))
            .and_then(|bytes| bytes.checked_add(builder.retained_bytes()))
            .ok_or_else(|| resource("preview adapter storage overflow"))?;
        check_memory(adapter_bytes)?;
        positions
            .try_reserve_exact(maximum_vertices)
            .map_err(|_| resource("preview position allocation failed"))?;
        normals
            .try_reserve_exact(maximum_vertices)
            .map_err(|_| resource("preview normal allocation failed"))?;
        vertices
            .try_reserve(maximum_vertices)
            .map_err(|_| resource("preview vertex map allocation failed"))?;
        indices
            .try_reserve_exact(triangles.len())
            .map_err(|_| resource("preview index allocation failed"))?;
        for triangle_index in triangles {
            work.charge(1, cancel)?;
            let triangle = mesh.triangles[triangle_index];
            let mut local = [0; 3];
            for corner in 0..3 {
                let source_index = triangle[corner];
                let normal = mesh.normals[triangle_index][corner];
                let normal = [normal.x, normal.y, normal.z];
                let key = (source_index, normal.map(f64::to_bits));
                local[corner] = *vertices.entry(key).or_insert_with(|| {
                    let index = positions.len() as u32;
                    let point = mesh.positions[source_index as usize];
                    positions.push([point.x, point.y, point.z]);
                    normals.push(normal);
                    index
                });
            }
            indices.push(local);
        }
        let adapter_bytes = positions.capacity() * std::mem::size_of::<[f64; 3]>()
            + normals.capacity() * std::mem::size_of::<[f64; 3]>()
            + indices.capacity() * std::mem::size_of::<[u32; 3]>()
            + vertices.capacity() * std::mem::size_of::<((u32, [u64; 3]), u32)>();
        check_memory(
            mesh.retained_bytes()
                .checked_add(adapter_bytes)
                .and_then(|bytes| bytes.checked_add(face_index_bytes))
                .and_then(|bytes| bytes.checked_add(builder.retained_bytes()))
                .and_then(|bytes| {
                    bytes.checked_add(packing_growth(positions.len(), indices.len())?)
                })
                .ok_or_else(|| resource("preview packing storage overflow"))?,
        )?;
        builder.push_face(
            ordinal as u32,
            &positions,
            &normals,
            &indices,
            carrier,
            trim,
        )?;
        update.stage = DisplayStage::FaceComplete;
        update.completed_faces = ordinal as u32 + 1;
        update.consumed_work = work.consumed();
        progress(update);
    }
    let result = builder.finish()?;
    update.stage = DisplayStage::Complete;
    progress(update);
    check_cancel(cancel)?;
    Ok(result)
}

fn packing_growth(vertices: usize, triangles: usize) -> Option<usize> {
    let packed = triangles.checked_mul(3 * 24 + 16)?;
    let chunks = packed / (spiling_contracts::geometry::MAX_GEOMETRY_CHUNK_BYTES as usize - 88) + 2;
    packed
        .checked_add(chunks.checked_mul(1024)?)?
        .checked_add(vertices.checked_mul(2 * std::mem::size_of::<u32>())?)?
        .checked_add(2 * spiling_contracts::geometry::MAX_GEOMETRY_CHUNK_BYTES as usize)
}

fn assemble_preview(
    definition: &Definition,
    kernel: &solidcore::kernel::Kernel,
    edges: &solidcore_mesh::EdgeSet,
    candidates: Vec<solidcore_mesh::FaceMesh>,
    retained: usize,
    work: &GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<solidcore_mesh::Mesh, GeometryError> {
    let mut vertices = 0usize;
    let mut triangles = 0usize;
    for candidate in &candidates {
        work.charge(1, cancel)?;
        vertices = vertices
            .checked_add(candidate.positions.len())
            .ok_or_else(|| resource("preview assembly vertex overflow"))?;
        triangles = triangles
            .checked_add(candidate.triangles.len())
            .ok_or_else(|| resource("preview assembly triangle overflow"))?;
    }
    if vertices > u32::MAX as usize {
        return Err(resource("preview assembly exceeds index representation"));
    }
    let bytes = vertices
        .checked_mul(std::mem::size_of::<solidcore::math::Point3>())
        .and_then(|bytes| {
            bytes.checked_add(triangles.checked_mul(
                std::mem::size_of::<[u32; 3]>()
                    + std::mem::size_of::<[solidcore::math::Vec3; 3]>()
                    + std::mem::size_of::<solidcore::FaceId>(),
            )?)
        })
        .and_then(|bytes| bytes.checked_add(retained))
        .ok_or_else(|| resource("preview assembly storage overflow"))?;
    check_memory(bytes)?;
    let mut mesh = solidcore_mesh::Mesh::default();
    mesh.positions
        .try_reserve_exact(vertices)
        .map_err(|_| resource("preview assembly position allocation failed"))?;
    mesh.triangles
        .try_reserve_exact(triangles)
        .map_err(|_| resource("preview assembly index allocation failed"))?;
    mesh.normals
        .try_reserve_exact(triangles)
        .map_err(|_| resource("preview assembly normal allocation failed"))?;
    mesh.triangle_faces
        .try_reserve_exact(triangles)
        .map_err(|_| resource("preview assembly ownership allocation failed"))?;
    let mut carrier = 0.0_f64;
    for candidate in candidates {
        work.charge(1, cancel)?;
        let deviation = candidate
            .deviation
            .filter(|value| value.is_finite() && *value >= 0.0)
            .ok_or_else(|| invalid("preview face has no finite measured deviation"))?;
        carrier = carrier.max(deviation);
        let offset = mesh.positions.len() as u32;
        for &point in &candidate.positions {
            work.charge(1, cancel)?;
            mesh.positions.push(point);
        }
        for &triangle in &candidate.triangles {
            work.charge(1, cancel)?;
            if triangle.iter().any(|&index| {
                index as usize >= candidate.positions.len()
                    || index as usize >= candidate.normals.len()
            }) {
                return Err(invalid("preview face has an invalid corner index"));
            }
            let points = triangle.map(|index| candidate.positions[index as usize]);
            if points[0] == points[1] || points[1] == points[2] || points[2] == points[0] {
                continue;
            }
            let geometric = (points[1] - points[0]).cross(points[2] - points[0]);
            let length = geometric.length();
            if !length.is_finite() || length <= 0.0 {
                return Err(invalid("preview contains a degenerate physical triangle"));
            }
            let normals = triangle.map(|index| {
                let normal = candidate.normals[index as usize];
                if normal.length_squared() == 0.0 || normal.dot(geometric) <= 0.0 {
                    geometric / length
                } else {
                    normal
                }
            });
            mesh.triangles.push(triangle.map(|index| index + offset));
            mesh.normals.push(normals);
            mesh.triangle_faces.push(candidate.face);
        }
    }
    let mut trim = 0.0_f64;
    for polyline in edges.polylines() {
        work.charge(1, cancel)?;
        if polyline.degenerate {
            continue;
        }
        if polyline.params.len() != polyline.vertices.len() || polyline.params.len() < 2 {
            return Err(invalid("preview edge has incomplete parameter ownership"));
        }
        let edge = definition
            .brep()
            .edge(polyline.edge)
            .map_err(|cause| invalid(&cause.to_string()))?;
        let curve = edge
            .curve
            .as_ref()
            .ok_or_else(|| invalid("preview edge has no curve"))?;
        for (parameters, indices) in polyline.params.windows(2).zip(polyline.vertices.windows(2)) {
            let first = *edges
                .positions()
                .get(indices[0] as usize)
                .ok_or_else(|| invalid("preview edge index is outside its positions"))?;
            let last = *edges
                .positions()
                .get(indices[1] as usize)
                .ok_or_else(|| invalid("preview edge index is outside its positions"))?;
            for fraction in [0.25, 0.5, 0.75] {
                work.charge(1, cancel)?;
                let point = curve
                    .evaluate_point(
                        kernel,
                        parameters[0] + (parameters[1] - parameters[0]) * fraction,
                    )
                    .map_err(|cause| invalid(&cause.to_string()))?;
                let chord = last - first;
                let squared = chord.length_squared();
                let fraction = if squared > 0.0 {
                    ((point - first).dot(chord) / squared).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let distance = (point - (first + chord * fraction)).length();
                if !distance.is_finite() {
                    return Err(invalid("preview edge has nonfinite measured deviation"));
                }
                trim = trim.max(distance);
            }
        }
    }
    mesh.stats.face_deviation = Some(carrier);
    mesh.stats.edge_deviation = Some(trim);
    check_cancel(cancel)?;
    Ok(mesh)
}

fn validate_closed(
    mesh: &solidcore_mesh::Mesh,
    outside: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    let mut positions = HashMap::new();
    let mut canonical = Vec::new();
    let maximum_edges = mesh
        .triangles
        .len()
        .checked_mul(3)
        .ok_or_else(|| resource("preview topology count overflow"))?;
    let topology_bytes = mesh
        .positions
        .len()
        .checked_mul(2 * std::mem::size_of::<([u64; 3], u32)>() + std::mem::size_of::<u32>())
        .and_then(|bytes| {
            bytes.checked_add(
                maximum_edges.checked_mul(2 * std::mem::size_of::<((u32, u32), (u32, i32))>())?,
            )
        })
        .and_then(|bytes| bytes.checked_add(mesh.retained_bytes()))
        .and_then(|bytes| bytes.checked_add(outside))
        .ok_or_else(|| resource("preview topology storage overflow"))?;
    check_memory(topology_bytes)?;
    positions
        .try_reserve(mesh.positions.len())
        .map_err(|_| resource("preview topology allocation failed"))?;
    canonical
        .try_reserve_exact(mesh.positions.len())
        .map_err(|_| resource("preview canonical index allocation failed"))?;
    for point in &mesh.positions {
        work.charge(1, cancel)?;
        let values = [point.x, point.y, point.z];
        if values.iter().any(|value| !value.is_finite()) {
            return Err(invalid("preview contains a nonfinite position"));
        }
        let key = values.map(|value| if value == 0.0 { 0 } else { value.to_bits() });
        let next = positions.len() as u32;
        canonical.push(*positions.entry(key).or_insert(next));
    }
    let mut edges = HashMap::<(u32, u32), (u32, i32)>::new();
    edges
        .try_reserve(maximum_edges)
        .map_err(|_| resource("preview edge inventory allocation failed"))?;
    for triangle in &mesh.triangles {
        work.charge(1, cancel)?;
        if triangle
            .iter()
            .any(|&index| index as usize >= canonical.len())
        {
            return Err(invalid(
                "preview triangle index is outside its vertex buffer",
            ));
        }
        let triangle = triangle.map(|index| canonical[index as usize]);
        if triangle[0] == triangle[1] || triangle[1] == triangle[2] || triangle[2] == triangle[0] {
            return Err(invalid("preview contains a collapsed physical triangle"));
        }
        for corner in 0..3 {
            let first = triangle[corner];
            let second = triangle[(corner + 1) % 3];
            let entry = edges
                .entry((first.min(second), first.max(second)))
                .or_default();
            entry.0 = entry
                .0
                .checked_add(1)
                .ok_or_else(|| resource("preview edge incidence overflow"))?;
            entry.1 += if first < second { 1 } else { -1 };
        }
    }
    if edges
        .values()
        .any(|&(uses, winding)| uses != 2 || winding != 0)
    {
        return Err(invalid(
            "preview does not retain a closed oriented source boundary",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiling_contracts::{
        display::CheckedMeshChunk,
        geometry::{ArtifactId, SessionId},
    };
    use std::{collections::BTreeSet, path::Path};

    #[test]
    fn preview_preserves_source_faces_and_the_through_hole_in_checked_buffers() {
        let source = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/geometry/through-hole.step"),
        )
        .unwrap();
        let cancel = AtomicBool::new(false);
        let definition = crate::from_step_bytes_with_cancel(&source, &cancel).unwrap();
        let artifact = tessellate(
            &definition,
            &cancel,
            MeshBudget::FROZEN,
            &mut GeometryWorkBudget::query(),
            &mut |_| {},
        )
        .unwrap();
        let mut visible_faces = BTreeSet::new();
        let mut hole_hits = 0;
        let mut material_hits = 0;
        for chunk in artifact.chunks {
            assert_eq!(
                chunk.descriptor.mesh_profile,
                DisplayProfile::MeshTolerantMm005V1
            );
            let checked = CheckedMeshChunk::new(
                chunk.bytes.into(),
                chunk
                    .descriptor
                    .into_metadata(SessionId::new(), ArtifactId::new(1).unwrap()),
            )
            .unwrap();
            let view = checked.view();
            for triangle_index in 0..view.face_ordinals.len() {
                visible_faces.insert(view.face_ordinals.get(triangle_index).unwrap());
                let triangle = std::array::from_fn(|corner| {
                    let point = view
                        .positions
                        .get(view.indices.get(triangle_index * 3 + corner).unwrap() as usize)
                        .unwrap();
                    std::array::from_fn::<_, 2, _>(|axis| {
                        view.local_origin_mm[axis] + f64::from(point[axis])
                    })
                });
                hole_hits += usize::from(contains_projection(triangle, [10.0, 10.0]));
                material_hits += usize::from(contains_projection(triangle, [2.0, 3.0]));
            }
        }
        assert_eq!(visible_faces.len(), definition.faces().len());
        assert_eq!(hole_hits, 0, "display must not cap the native through-hole");
        assert!(
            material_hits >= 2,
            "both sides of the material must be visible"
        );
        assert_eq!(
            tessellate(
                &definition,
                &AtomicBool::new(true),
                MeshBudget::FROZEN,
                &mut GeometryWorkBudget::query(),
                &mut |_| {}
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::Cancelled
        );
        let late_cancel = AtomicBool::new(false);
        assert_eq!(
            tessellate(
                &definition,
                &late_cancel,
                MeshBudget::FROZEN,
                &mut GeometryWorkBudget::query(),
                &mut |progress| {
                    if progress.stage == DisplayStage::Complete {
                        late_cancel.store(true, std::sync::atomic::Ordering::Release);
                    }
                },
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::Cancelled
        );
    }

    fn contains_projection(triangle: [[f64; 2]; 3], point: [f64; 2]) -> bool {
        let orientation = |first: [f64; 2], second: [f64; 2], third: [f64; 2]| {
            (second[0] - first[0]) * (third[1] - first[1])
                - (second[1] - first[1]) * (third[0] - first[0])
        };
        if orientation(triangle[0], triangle[1], triangle[2]) == 0.0 {
            return false;
        }
        let signs = std::array::from_fn::<_, 3, _>(|corner| {
            orientation(triangle[corner], triangle[(corner + 1) % 3], point)
        });
        signs.iter().all(|value| *value >= 0.0) || signs.iter().all(|value| *value <= 0.0)
    }
}
