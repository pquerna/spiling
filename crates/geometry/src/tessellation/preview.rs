// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{DisplayProgress, DisplayStage, candidate_query, check_memory, invalid, resource};
use crate::{GeometryWorkBudget, MeshArtifact, check_cancel, kernel::Definition};
use solidcore_mesh::{EdgeOptions, EdgeSet, FaceMesh, OutputOptions, TessellateOptions, Tolerance};
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
    if faces.is_empty()
        || faces.len() != definition.faces().len()
        || faces.len() != definition.face_step_ids().len()
    {
        return Err(invalid("preview requires the complete source face table"));
    }
    let kernel = crate::make_kernel_context();
    let options = TessellateOptions {
        chord_tolerance: Tolerance::Absolute(MESH_CARRIER_TOLERANCE_MM * 0.25),
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
    let trim = edge_deviation(definition, &kernel, &edges, work, cancel)?;
    let mut topology = Topology::default();
    let mut builder = MeshChunkBuilder::with_budget(
        definition.id().clone(),
        faces.len() as u32,
        DisplayProfile::MeshTolerantMm005V1,
        budget,
    )?;
    for (ordinal, &face) in faces.iter().enumerate() {
        update.stage = DisplayStage::TessellatingFace;
        update.completed_faces = ordinal as u32;
        update.consumed_work = work.consumed();
        progress(update);
        let outside = edges
            .retained_bytes()
            .saturating_add(topology.retained_bytes())
            .saturating_add(builder.retained_bytes());
        let candidate = candidate_query(outside, work, cancel, |native| {
            solidcore_mesh::tessellate_face_with_budget(
                &kernel,
                definition.brep(),
                face,
                &edges,
                &options,
                native,
            )
        })
        .map_err(|failure| {
            GeometryError::new(
                failure.code,
                format!(
                    "preview face {ordinal}/{}: {}",
                    faces.len(),
                    failure.message
                ),
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
        validate_measurement_samples(definition, &kernel, &candidate, work, cancel)?;
        let carrier = candidate
            .deviation
            .filter(|value| value.is_finite() && *value >= 0.0)
            .ok_or_else(|| invalid("preview face has no finite measured deviation"))?;
        if !trim.is_finite() || trim < 0.0 || carrier + trim > MESH_CARRIER_TOLERANCE_MM {
            return Err(GeometryError::new(
                GeometryErrorCode::ShapeErrorExceeded,
                "measured preview deviation exceeds display profile",
            ));
        }
        update.stage = DisplayStage::AuditingFace;
        update.consumed_work = work.consumed();
        progress(update);
        let adapter = adapt_face(&candidate, outside, work, cancel)?;
        drop(candidate);
        let outside = edges
            .retained_bytes()
            .saturating_add(builder.retained_bytes())
            .saturating_add(adapter.retained_bytes());
        topology.include(&adapter, outside, work, cancel)?;
        let growth = packing_growth(adapter.positions.len(), adapter.indices.len())
            .ok_or_else(|| resource("preview packing growth overflow"))?;
        check_memory(
            outside
                .saturating_add(topology.retained_bytes())
                .saturating_add(growth),
        )?;
        builder.push_face(
            ordinal as u32,
            &adapter.positions,
            &adapter.normals,
            &adapter.indices,
            carrier,
            trim,
        )?;
        update.stage = DisplayStage::FaceComplete;
        update.completed_faces = ordinal as u32 + 1;
        update.consumed_work = work.consumed();
        progress(update);
    }
    drop(edges);
    topology.finish(builder.retained_bytes(), work, cancel)?;
    check_cancel(cancel)?;
    let result = builder.finish()?;
    update.stage = DisplayStage::Complete;
    update.consumed_work = work.consumed();
    progress(update);
    check_cancel(cancel)?;
    Ok(result)
}

fn validate_measurement_samples(
    definition: &Definition,
    kernel: &solidcore::kernel::Kernel,
    candidate: &FaceMesh,
    work: &GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    let surface = definition
        .brep()
        .face(candidate.face)
        .map_err(|cause| invalid(cause.to_string()))?
        .surface
        .as_ref()
        .ok_or_else(|| invalid("preview face has no surface"))?;
    for triangle in &candidate.triangles {
        let mut points = [solidcore::math::Point3::default(); 3];
        let mut parameters = [solidcore::math::Point2::default(); 3];
        for (corner, &index) in triangle.iter().enumerate() {
            points[corner] = *candidate
                .positions
                .get(index as usize)
                .ok_or_else(|| invalid("preview measurement has an invalid position index"))?;
            parameters[corner] = *candidate
                .uvs
                .get(index as usize)
                .ok_or_else(|| invalid("preview measurement has an invalid parameter index"))?;
        }
        for weights in [
            [1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0],
            [0.5, 0.5, 0.0],
            [0.0, 0.5, 0.5],
            [0.5, 0.0, 0.5],
            [2.0 / 3.0, 1.0 / 6.0, 1.0 / 6.0],
            [1.0 / 6.0, 2.0 / 3.0, 1.0 / 6.0],
            [1.0 / 6.0, 1.0 / 6.0, 2.0 / 3.0],
        ] {
            work.charge(1, cancel)?;
            let point = points[0] * weights[0] + points[1] * weights[1] + points[2] * weights[2];
            let parameter = solidcore::math::Point2::new(
                parameters[0].x * weights[0]
                    + parameters[1].x * weights[1]
                    + parameters[2].x * weights[2],
                parameters[0].y * weights[0]
                    + parameters[1].y * weights[1]
                    + parameters[2].y * weights[2],
            );
            if !parameter.x.is_finite() || !parameter.y.is_finite() {
                return Err(invalid("preview measurement has nonfinite parameters"));
            }
            let measured = surface
                .evaluate_point(kernel, parameter)
                .map_err(|cause| invalid(cause.to_string()))?;
            if !measured.distance(point).is_finite() {
                return Err(invalid("preview sample has no finite surface measurement"));
            }
        }
    }
    Ok(())
}

struct AdaptedFace {
    positions: Vec<[f64; 3]>,
    normals: Vec<[f64; 3]>,
    indices: Vec<[u32; 3]>,
}
impl AdaptedFace {
    fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            .saturating_add(
                self.positions
                    .capacity()
                    .saturating_mul(size_of::<[f64; 3]>()),
            )
            .saturating_add(
                self.normals
                    .capacity()
                    .saturating_mul(size_of::<[f64; 3]>()),
            )
            .saturating_add(
                self.indices
                    .capacity()
                    .saturating_mul(size_of::<[u32; 3]>()),
            )
    }
}

type TriangleCorners = ([solidcore::math::Vec3; 3], [solidcore::math::Vec3; 3]);

fn corners(
    candidate: &FaceMesh,
    triangle: [u32; 3],
) -> Result<Option<TriangleCorners>, GeometryError> {
    if triangle.iter().any(|&index| {
        index as usize >= candidate.positions.len() || index as usize >= candidate.normals.len()
    }) {
        return Err(invalid("preview face has an invalid corner index"));
    }
    let points = triangle.map(|index| candidate.positions[index as usize]);
    if points[0] == points[1] || points[1] == points[2] || points[2] == points[0] {
        return Ok(None);
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
    Ok(Some((points, normals)))
}

fn adapt_face(
    candidate: &FaceMesh,
    outside: usize,
    work: &GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<AdaptedFace, GeometryError> {
    for point in &candidate.positions {
        work.charge(1, cancel)?;
        if [point.x, point.y, point.z]
            .iter()
            .any(|value| !value.is_finite())
        {
            return Err(invalid("preview contains a nonfinite position"));
        }
    }
    let mut maximum_vertices = candidate.positions.len();
    for &triangle in &candidate.triangles {
        work.charge(1, cancel)?;
        if let Some((_, normals)) = corners(candidate, triangle)? {
            for (index, normal) in triangle.into_iter().zip(normals) {
                let original = candidate.normals[index as usize];
                if [normal.x, normal.y, normal.z].map(f64::to_bits)
                    != [original.x, original.y, original.z].map(f64::to_bits)
                {
                    maximum_vertices = maximum_vertices
                        .checked_add(1)
                        .ok_or_else(|| resource("preview repaired vertex count overflow"))?;
                }
            }
        }
    }
    if maximum_vertices > u32::MAX as usize {
        return Err(resource("preview exceeds vertex index representation"));
    }
    let growth = maximum_vertices
        .checked_mul(2 * size_of::<[f64; 3]>() + 4 * (size_of::<((u32, [u64; 3]), u32)>() + 1))
        .and_then(|bytes| {
            bytes.checked_add(
                candidate
                    .triangles
                    .len()
                    .checked_mul(size_of::<[u32; 3]>())?,
            )
        })
        .and_then(|bytes| bytes.checked_add(candidate.retained_bytes()))
        .and_then(|bytes| bytes.checked_add(outside))
        .ok_or_else(|| resource("preview adapter storage overflow"))?;
    check_memory(growth)?;
    let mut adapter = AdaptedFace {
        positions: Vec::new(),
        normals: Vec::new(),
        indices: Vec::new(),
    };
    adapter
        .positions
        .try_reserve_exact(maximum_vertices)
        .map_err(|_| resource("preview position allocation failed"))?;
    adapter
        .normals
        .try_reserve_exact(maximum_vertices)
        .map_err(|_| resource("preview normal allocation failed"))?;
    adapter
        .indices
        .try_reserve_exact(candidate.triangles.len())
        .map_err(|_| resource("preview index allocation failed"))?;
    let mut vertices = HashMap::new();
    vertices
        .try_reserve(maximum_vertices)
        .map_err(|_| resource("preview normal index allocation failed"))?;
    for &triangle in &candidate.triangles {
        work.charge(1, cancel)?;
        let Some((points, normals)) = corners(candidate, triangle)? else {
            continue;
        };
        let mut indices = [0; 3];
        for corner in 0..3 {
            let point = points[corner];
            let normal = normals[corner];
            let normal = [normal.x, normal.y, normal.z];
            let key = (triangle[corner], normal.map(f64::to_bits));
            indices[corner] = *vertices.entry(key).or_insert_with(|| {
                let index = adapter.positions.len() as u32;
                adapter.positions.push([point.x, point.y, point.z]);
                adapter.normals.push(normal);
                index
            });
        }
        adapter.indices.push(indices);
    }
    check_memory(
        outside
            .saturating_add(candidate.retained_bytes())
            .saturating_add(adapter.retained_bytes())
            .saturating_add(map_bytes(&vertices)),
    )?;
    Ok(adapter)
}

fn map_bytes<Key, Value>(map: &HashMap<Key, Value>) -> usize {
    map.capacity()
        .saturating_mul(size_of::<(Key, Value)>() + 1)
        .saturating_mul(2)
        .saturating_add(size_of::<HashMap<Key, Value>>() + 32)
}

fn grow<T>(values: &mut Vec<T>, additional: usize, outside: usize) -> Result<(), GeometryError> {
    let required = values
        .len()
        .checked_add(additional)
        .ok_or_else(|| resource("preview topology capacity overflow"))?;
    if required <= values.capacity() {
        return Ok(());
    }
    let capacity = required.max(
        values
            .capacity()
            .checked_mul(2)
            .ok_or_else(|| resource("preview topology capacity overflow"))?,
    );
    let bytes = values
        .capacity()
        .checked_add(capacity)
        .and_then(|count| count.checked_mul(size_of::<T>()))
        .and_then(|bytes| bytes.checked_add(outside))
        .ok_or_else(|| resource("preview topology growth overflow"))?;
    check_memory(bytes)?;
    values
        .try_reserve_exact(capacity - values.len())
        .map_err(|_| resource("preview topology allocation failed"))?;
    check_memory(outside.saturating_add(values.capacity().saturating_mul(size_of::<T>())))
}

#[derive(Default)]
struct Topology {
    vertices: Vec<[u64; 3]>,
    edges: Vec<[u32; 2]>,
}
impl Topology {
    fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            .saturating_add(
                self.vertices
                    .capacity()
                    .saturating_mul(size_of::<[u64; 3]>()),
            )
            .saturating_add(self.edges.capacity().saturating_mul(size_of::<[u32; 2]>()))
    }
    fn include(
        &mut self,
        face: &AdaptedFace,
        outside: usize,
        work: &GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        let maximum_edges = face
            .indices
            .len()
            .checked_mul(3)
            .ok_or_else(|| resource("preview topology edge overflow"))?;
        let count = self
            .vertices
            .len()
            .checked_add(face.positions.len())
            .ok_or_else(|| resource("preview topology vertex overflow"))?;
        if count > u32::MAX as usize {
            return Err(resource("preview topology exceeds index representation"));
        }
        work.charge(1, cancel)?;
        grow(
            &mut self.vertices,
            face.positions.len(),
            outside
                .saturating_add(size_of::<Self>())
                .saturating_add(self.edges.capacity().saturating_mul(size_of::<[u32; 2]>())),
        )?;
        grow(
            &mut self.edges,
            maximum_edges,
            outside.saturating_add(size_of::<Self>()).saturating_add(
                self.vertices
                    .capacity()
                    .saturating_mul(size_of::<[u64; 3]>()),
            ),
        )?;
        let offset = self.vertices.len() as u32;
        for &point in &face.positions {
            work.charge(1, cancel)?;
            if point.iter().any(|value| !value.is_finite()) {
                return Err(invalid("preview contains a nonfinite position"));
            }
            self.vertices
                .push(point.map(|value| if value == 0.0 { 0 } else { value.to_bits() }));
        }
        for triangle in &face.indices {
            work.charge(1, cancel)?;
            for corner in 0..3 {
                self.edges.push([
                    triangle[corner] + offset,
                    triangle[(corner + 1) % 3] + offset,
                ]);
            }
        }
        check_memory(outside.saturating_add(self.retained_bytes()))?;
        Ok(())
    }
    fn finish(
        mut self,
        outside: usize,
        work: &GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        check_memory(
            outside
                .saturating_add(self.retained_bytes())
                .saturating_add(self.vertices.len().saturating_mul(2 * size_of::<u32>())),
        )?;
        let mut order = Vec::new();
        let mut canonical = Vec::new();
        order
            .try_reserve_exact(self.vertices.len())
            .map_err(|_| resource("preview topology ordering allocation failed"))?;
        canonical
            .try_reserve_exact(self.vertices.len())
            .map_err(|_| resource("preview topology canonical allocation failed"))?;
        check_memory(
            outside
                .saturating_add(self.retained_bytes())
                .saturating_add(order.capacity().saturating_mul(size_of::<u32>()))
                .saturating_add(canonical.capacity().saturating_mul(size_of::<u32>())),
        )?;
        for index in 0..self.vertices.len() {
            work.charge(1, cancel)?;
            order.push(index as u32);
            canonical.push(0u32);
        }
        crate::compute::sort_unstable_by(
            &mut order,
            |first, second| self.vertices[*first as usize].cmp(&self.vertices[*second as usize]),
            work,
            cancel,
        )?;
        let mut previous = None;
        let mut rank = 0u32;
        for index in order {
            work.charge(1, cancel)?;
            let point = self.vertices[index as usize];
            if previous.is_some_and(|previous| previous != point) {
                rank = rank
                    .checked_add(1)
                    .ok_or_else(|| resource("preview canonical index overflow"))?;
            }
            canonical[index as usize] = rank;
            previous = Some(point);
        }
        for edge in &mut self.edges {
            work.charge(1, cancel)?;
            *edge = edge.map(|index| canonical[index as usize]);
            if edge[0] == edge[1] {
                return Err(invalid("preview contains a collapsed physical triangle"));
            }
        }
        drop(self.vertices);
        drop(canonical);
        let key = |edge: &[u32; 2]| [edge[0].min(edge[1]), edge[0].max(edge[1])];
        crate::compute::sort_unstable_by(
            &mut self.edges,
            |first, second| key(first).cmp(&key(second)),
            work,
            cancel,
        )?;
        let mut previous = None;
        let mut uses = 0u32;
        let mut winding = 0i32;
        for edge in self.edges {
            work.charge(1, cancel)?;
            let current = key(&edge);
            if previous.is_some_and(|previous| previous != current) {
                if uses != 2 || winding != 0 {
                    return Err(invalid(
                        "preview does not retain a closed oriented source boundary",
                    ));
                }
                uses = 0;
                winding = 0;
            }
            previous = Some(current);
            uses = uses
                .checked_add(1)
                .ok_or_else(|| resource("preview edge incidence overflow"))?;
            winding += if edge[0] < edge[1] { 1 } else { -1 };
        }
        if uses != 2 || winding != 0 {
            return Err(invalid(
                "preview does not retain a closed oriented source boundary",
            ));
        }
        Ok(())
    }
}

fn packing_growth(vertices: usize, triangles: usize) -> Option<usize> {
    let packed = triangles.checked_mul(3 * 24 + 16)?;
    let chunks = packed / (spiling_contracts::geometry::MAX_GEOMETRY_CHUNK_BYTES as usize - 88) + 2;
    packed
        .checked_add(chunks.checked_mul(1024)?)?
        .checked_add(vertices.checked_mul(2 * size_of::<u32>())?)?
        .checked_add(2 * spiling_contracts::geometry::MAX_GEOMETRY_CHUNK_BYTES as usize)
}

fn edge_deviation(
    definition: &Definition,
    kernel: &solidcore::kernel::Kernel,
    edges: &EdgeSet,
    work: &GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<f64, GeometryError> {
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
            .map_err(|cause| invalid(cause.to_string()))?;
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
                    .map_err(|cause| invalid(cause.to_string()))?;
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
    Ok(trim)
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
    fn preview_refuses_unmeasurable_surface_samples_instead_of_zero_deviation() {
        let source = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/geometry/through-hole.step"),
        )
        .unwrap();
        let cancel = AtomicBool::new(false);
        let definition = crate::from_step_bytes_with_cancel(&source, &cancel).unwrap();
        let kernel = crate::make_kernel_context();
        let options = TessellateOptions::default();
        let mut work = GeometryWorkBudget::query();
        let edges = candidate_query(0, &mut work, &cancel, |native| {
            solidcore_mesh::tessellate_edge_set_with_budget(
                &kernel,
                definition.brep(),
                &options,
                native,
            )
        })
        .unwrap();
        let mut candidate = candidate_query(edges.retained_bytes(), &mut work, &cancel, |native| {
            solidcore_mesh::tessellate_face_with_budget(
                &kernel,
                definition.brep(),
                definition.face_ids()[0],
                &edges,
                &options,
                native,
            )
        })
        .unwrap();
        validate_measurement_samples(&definition, &kernel, &candidate, &work, &cancel).unwrap();
        candidate.deviation = Some(0.0);
        candidate
            .uvs
            .fill(solidcore::math::Point2::new(f64::NAN, 0.0));
        assert!(
            validate_measurement_samples(&definition, &kernel, &candidate, &work, &cancel).is_err()
        );
        candidate.uvs.clear();
        assert!(
            validate_measurement_samples(&definition, &kernel, &candidate, &work, &cancel).is_err()
        );
    }

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
