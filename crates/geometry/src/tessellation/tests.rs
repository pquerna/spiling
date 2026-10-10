// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::tessellate;
use monstertruck_modeling::{Point3, Vector3};
use spiling_contracts::display::{
    EncodedMesh, ValidatedMeshChunk, validate_mesh_chunk, validate_mesh_manifest,
};
use spiling_contracts::geometry::{
    ArtifactId, MAX_GEOMETRY_CHUNK_BYTES, MAX_SCENE_MESH_BYTES, MESH_QUANTIZATION_TOLERANCE_MM,
    MESH_SURFACE_TOLERANCE_MM, MeshChunkMetadata, SessionId,
};
use std::path::Path;

fn fixture(name: &str) -> Definition {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/geometry")
        .join(name);
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("original fixture {}: {error}", path.display()));
    crate::import_step(&bytes, &AtomicBool::new(false)).unwrap()
}
fn metadata(mesh: &EncodedMesh) -> Vec<MeshChunkMetadata> {
    // This is the downstream consumer attachment boundary, not native geometry
    // fabricating engine handles. All chunks share one real test-consumer lease.
    let session = SessionId::new();
    let artifact = ArtifactId::new(1).unwrap();
    mesh.chunks
        .iter()
        .map(|chunk| {
            chunk
                .descriptor
                .clone()
                .into_metadata(session.clone(), artifact)
        })
        .collect()
}
fn decoded<'a>(bytes: &'a [u8], metadata: &MeshChunkMetadata) -> ValidatedMeshChunk<'a> {
    validate_mesh_chunk(
        bytes,
        metadata,
        &metadata.session_id,
        metadata.artifact_id,
        &metadata.definition_id,
        metadata.chunk_index,
    )
    .unwrap()
}
fn point(mesh: &ValidatedMeshChunk<'_>, index: u32) -> [f64; 3] {
    let p = mesh.positions.get(index as usize).unwrap();
    std::array::from_fn(|axis| mesh.local_origin_mm[axis] + f64::from(p[axis]))
}

fn audit_encoded(definition: &Definition, encoded: &EncodedMesh) {
    let descriptors = metadata(encoded);
    validate_mesh_manifest(&descriptors).unwrap();
    let mut seen = vec![false; definition.faces.len()];
    let mut previous = 0;
    let mut vertices = 0;
    let mut triangles = 0;
    let mut bytes = 0;
    for (chunk, descriptor) in encoded.chunks.iter().zip(&descriptors) {
        assert!(chunk.bytes.len() <= MAX_GEOMETRY_CHUNK_BYTES as usize);
        assert!(descriptor.carrier_deviation_mm <= MESH_CARRIER_TOLERANCE_MM);
        assert!(descriptor.quantization_error_mm <= MESH_QUANTIZATION_TOLERANCE_MM);
        assert!(
            descriptor.carrier_deviation_mm + descriptor.quantization_error_mm
                <= MESH_SURFACE_TOLERANCE_MM
        );
        let view = decoded(&chunk.bytes, descriptor);
        vertices += view.positions.len() as u32;
        triangles += view.face_ordinals.len() as u32;
        bytes += chunk.bytes.len() as u32;
        for (triangle_index, ordinal) in view.face_ordinals.iter().enumerate() {
            assert!(
                ordinal >= previous,
                "source face traversal must remain ordered across chunks"
            );
            previous = ordinal;
            seen[ordinal as usize] = true;
            let info = &definition.faces[ordinal as usize];
            let indices = std::array::from_fn::<_, 3, _>(|slot| {
                view.indices.get(3 * triangle_index + slot).unwrap()
            });
            let points = indices.map(|index| point(&view, index));
            let geometric_normal = cross(sub(points[1], points[0]), sub(points[2], points[0]));
            for (position, index) in points.into_iter().zip(indices) {
                let normal = view.normals.get(index as usize).unwrap().map(f64::from);
                let radial_or_plane = carrier_normal(&info.carrier, position).unwrap();
                let expected = scale(radial_or_plane, if info.orientation { 1.0 } else { -1.0 });
                assert!(length(sub(normal, expected)) < 2.0 * NORMAL_NORM_TOLERANCE);
                assert!(dot(geometric_normal, normal) > 0.0);
            }
            let measured = triangle_carrier_deviation(&info.carrier, points).unwrap();
            // Radial distance and plane distance are 1-Lipschitz in positions;
            // quantization moves the whole triangle by at most this displacement.
            assert!(
                measured
                    <= descriptor.carrier_deviation_mm + descriptor.quantization_error_mm + 1e-8
            );
        }
    }
    assert!(
        seen.into_iter().all(|present| present),
        "no native source-face slot may be dropped"
    );
    assert_eq!(vertices, encoded.vertex_count);
    assert_eq!(triangles, encoded.triangle_count);
    assert_eq!(bytes, encoded.total_bytes);
    assert!(encoded.total_bytes <= MAX_SCENE_MESH_BYTES);
}

#[test]
fn original_parts_preserve_all_source_faces_analytic_normals_and_profile_precision() {
    for name in [
        "box-mm.step",
        "box-inch.step",
        "cylinder.step",
        "through-hole.step",
    ] {
        let definition = fixture(name);
        if name.starts_with("box-") {
            assert_eq!(definition.faces.len(), 6);
        }
        let encoded = tessellate(
            &definition,
            DisplayProfile::MeshMm005V1,
            &AtomicBool::new(false),
        )
        .unwrap();
        audit_encoded(&definition, &encoded);
        let repeated = tessellate(
            &definition,
            DisplayProfile::MeshMm005V1,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(encoded.vertex_count, repeated.vertex_count);
        assert_eq!(encoded.triangle_count, repeated.triangle_count);
        assert_eq!(encoded.total_bytes, repeated.total_bytes);
        assert_eq!(encoded.chunks.len(), repeated.chunks.len());
        for (a, b) in encoded.chunks.iter().zip(&repeated.chunks) {
            assert_eq!(a.descriptor, b.descriptor);
            assert_eq!(a.bytes, b.bytes);
        }
    }
}

#[test]
fn through_hole_caps_keep_the_trim_and_cylinder_normals_point_into_the_hole() {
    let definition = fixture("through-hole.step");
    let encoded = tessellate(
        &definition,
        DisplayProfile::MeshMm005V1,
        &AtomicBool::new(false),
    )
    .unwrap();
    let hole = definition
        .faces
        .iter()
        .find_map(|info| match info.carrier {
            FaceCarrier::Cylinder {
                axis_origin_mm,
                axis_direction,
                radius_mm,
            } => {
                assert!(
                    !info.orientation,
                    "hole wall must be inward relative to outward radial carrier"
                );
                assert!((radius_mm - 3.0).abs() <= 1e-6);
                Some((axis_origin_mm, axis_direction, radius_mm))
            }
            _ => None,
        })
        .expect("mandatory native hole carrier");
    let descriptors = metadata(&encoded);
    let mut cap_areas = vec![0.0; definition.faces.len()];
    let mut hole_triangles = 0;
    for (chunk, descriptor) in encoded.chunks.iter().zip(&descriptors) {
        let view = decoded(&chunk.bytes, descriptor);
        for (triangle_index, ordinal) in view.face_ordinals.iter().enumerate() {
            let info = &definition.faces[ordinal as usize];
            let indices = std::array::from_fn::<_, 3, _>(|slot| {
                view.indices.get(3 * triangle_index + slot).unwrap()
            });
            let points = indices.map(|index| point(&view, index));
            match info.carrier {
                FaceCarrier::Cylinder { .. } => {
                    hole_triangles += 1;
                    for (position, index) in points.into_iter().zip(indices) {
                        let radial = project_radial(sub(position, hole.0), hole.1);
                        let normal = view.normals.get(index as usize).unwrap().map(f64::from);
                        assert!(dot(normal, radial) < 0.0);
                    }
                }
                FaceCarrier::Plane { normal, .. } if dot(normal, hole.1).abs() > 1.0 - 1e-10 => {
                    let projected = points.map(|p| project_radial(sub(p, hole.0), hole.1));
                    assert!(
                        projected_triangle_distance(projected)
                            >= hole.2
                                - MESH_CARRIER_TOLERANCE_MM
                                - descriptor.quantization_error_mm
                                - 1e-8,
                        "a cap triangle must not bridge the circular trim"
                    );
                    cap_areas[ordinal as usize] +=
                        0.5 * length(cross(sub(points[1], points[0]), sub(points[2], points[0])));
                }
                _ => {}
            }
        }
    }
    assert!(hole_triangles > 0);
    let areas: Vec<_> = cap_areas.into_iter().filter(|&area| area > 0.0).collect();
    assert_eq!(areas.len(), 2);
    let expected = 400.0 - std::f64::consts::PI * 9.0;
    let sampling_area_bound = 2.0 * std::f64::consts::PI * 3.0 * MESH_CARRIER_TOLERANCE_MM
        + std::f64::consts::PI * MESH_CARRIER_TOLERANCE_MM.powi(2);
    for area in areas {
        assert!((area - expected).abs() <= sampling_area_bound + 1e-5);
    }
}

#[test]
fn projected_cylinder_audit_catches_edge_and_interior_sag_not_visible_at_vertices() {
    let carrier = FaceCarrier::Cylinder {
        axis_origin_mm: [0.0; 3],
        axis_direction: [0.0, 0.0, 1.0],
        radius_mm: 5.0,
    };
    let angle = 0.15_f64;
    let triangle = [
        [5.0 * angle.cos(), 5.0 * angle.sin(), 0.0],
        [5.0 * angle.cos(), -5.0 * angle.sin(), 0.0],
        [5.0 * angle.cos(), 5.0 * angle.sin(), 8.0],
    ];
    for p in triangle {
        assert!((p[0].hypot(p[1]) - 5.0).abs() < 1e-12);
    }
    let measured = triangle_carrier_deviation(&carrier, triangle).unwrap();
    assert!(measured >= 5.0 * (1.0 - angle.cos()));
    assert!(measured > MESH_CARRIER_TOLERANCE_MM);
    let spanning = [
        [5.0, 0.0, 0.0],
        [-2.5, 5.0 * 3.0_f64.sqrt() / 2.0, 0.0],
        [-2.5, -5.0 * 3.0_f64.sqrt() / 2.0, 0.0],
    ];
    assert_eq!(projected_triangle_distance(spanning), 0.0);
    assert!(triangle_carrier_deviation(&carrier, spanning).unwrap() >= 5.0);
    let rotated = FaceCarrier::Cylinder {
        axis_origin_mm: [1e6, -2e6, 3e6],
        axis_direction: [1.0, 0.0, 0.0],
        radius_mm: 5.0,
    };
    let transformed = triangle.map(|p| [p[2] + 1e6, p[0] - 2e6, p[1] + 3e6]);
    assert!((triangle_carrier_deviation(&rotated, transformed).unwrap() - measured).abs() < 1e-8);
}

#[test]
fn face_slot_and_resource_refusals_never_return_partial_artifacts() {
    let mut definition = fixture("box-mm.step");
    let cancel = AtomicBool::new(false);
    let first = definition.faces[0].clone();
    definition.faces.pop();
    assert_eq!(
        tessellate(&definition, DisplayProfile::MeshMm005V1, &cancel)
            .unwrap_err()
            .code,
        GeometryErrorCode::InvalidGeometry
    );
    definition
        .faces
        .resize(MAX_NATIVE_FACES as usize + 1, first);
    assert_eq!(
        tessellate(&definition, DisplayProfile::MeshMm005V1, &cancel)
            .unwrap_err()
            .code,
        GeometryErrorCode::ResourceLimit
    );
}

#[test]
fn malformed_native_attributes_and_winding_are_refused_without_repair() {
    let definition = fixture("box-mm.step");
    let face = definition.solid.face_iter().next().unwrap();
    let shell: Shell = std::iter::once(face.clone()).collect();
    let compressed = shell.compress();
    let mut meshed = cshell_triangulation_with(
        &compressed,
        TessellationOptions {
            tolerance: MESH_CARRIER_TOLERANCE_MM,
            search_trials: 2,
            primitive: TessellationPrimitiveOptions {
                mode: TessellationPrimitiveMode::Triangles,
                ..Default::default()
            },
        },
    );
    let polygon = meshed.faces[0].surface.take().unwrap();
    let source = &compressed.faces[0].surface;
    let info = &definition.faces[0];
    let cancel = AtomicBool::new(false);
    assert!(
        audit_face(
            &polygon,
            source,
            face.orientation(),
            info,
            &cancel,
            MeshBudget::FROZEN
        )
        .is_ok()
    );
    let mut bad = polygon.clone();
    bad.normals_mut()[0] = Vector3::new(0.0, 0.0, 0.0);
    assert!(
        audit_face(
            &bad,
            source,
            face.orientation(),
            info,
            &cancel,
            MeshBudget::FROZEN
        )
        .is_err()
    );
    let mut bad = polygon.clone();
    bad.positions_mut()[0] = Point3::new(f64::NAN, 0.0, 0.0);
    assert!(
        audit_face(
            &bad,
            source,
            face.orientation(),
            info,
            &cancel,
            MeshBudget::FROZEN
        )
        .is_err()
    );
    let mut bad = polygon.clone();
    bad.uncheck_editor().faces.tri_faces_mut()[0][0].pos = polygon.positions().len();
    assert!(
        audit_face(
            &bad,
            source,
            face.orientation(),
            info,
            &cancel,
            MeshBudget::FROZEN
        )
        .is_err()
    );
    let mut bad = polygon.clone();
    bad.uncheck_editor().faces.tri_faces_mut()[0][0].nor = None;
    assert!(
        audit_face(
            &bad,
            source,
            face.orientation(),
            info,
            &cancel,
            MeshBudget::FROZEN
        )
        .is_err()
    );
    let mut bad = polygon.clone();
    bad.uncheck_editor().faces.tri_faces_mut()[0][0].uv = None;
    assert!(
        audit_face(
            &bad,
            source,
            face.orientation(),
            info,
            &cancel,
            MeshBudget::FROZEN
        )
        .is_err()
    );
    let mut bad = polygon.clone();
    bad.uncheck_editor().faces.tri_faces_mut()[0].swap(0, 1);
    assert!(
        audit_face(
            &bad,
            source,
            face.orientation(),
            info,
            &cancel,
            MeshBudget::FROZEN
        )
        .is_err()
    );
    let mut bad_info = info.clone();
    bad_info.orientation = !bad_info.orientation;
    assert!(
        audit_face(
            &polygon,
            source,
            face.orientation(),
            &bad_info,
            &cancel,
            MeshBudget::FROZEN
        )
        .is_err()
    );
}

#[test]
fn frozen_perforated_plate_is_a_real_bounded_multichunk_transfer_workload() {
    let definition = fixture("perforated-plate.step");
    let encoded = tessellate(
        &definition,
        DisplayProfile::MeshMm005V1,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(
        encoded.total_bytes > 4 * 1024 * 1024,
        "frozen workload must exceed diagnostic binary cap"
    );
    audit_encoded(&definition, &encoded);
}
