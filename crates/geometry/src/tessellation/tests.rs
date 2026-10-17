// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::tessellate;
use monstertruck_meshing::prelude::cshell_triangulation_with;
use monstertruck_modeling::{Point3, Shell, Vector3};
use spiling_contracts::display::{
    EncodedMesh, ValidatedMeshChunk, validate_mesh_chunk, validate_mesh_manifest,
};
use spiling_contracts::geometry::{
    ArtifactId, MAX_GEOMETRY_CHUNK_BYTES, MAX_SCENE_MESH_BYTES, MESH_QUANTIZATION_TOLERANCE_MM,
    MESH_SURFACE_TOLERANCE_MM, MeshChunkMetadata, SessionId,
};
use std::path::Path;

// Independent closed-form physical-distance oracles for encoded output only;
// production carrier admission uses the bidirectional native UV certificate.
/// The radial norm is convex: its maximum over a triangle is at a vertex.
/// Its minimum is zero when the projected triangle contains the axis, otherwise
/// it is the minimum distance on all three projected edges (including endpoints).
fn triangle_carrier_deviation(
    carrier: &AnalyticSurfaceDescription,
    triangle: [[f64; 3]; 3],
) -> Result<f64, GeometryError> {
    let deviation = match carrier {
        AnalyticSurfaceDescription::Plane { origin_mm, normal } => {
            let mut maximum = 0.0_f64;
            for point in triangle {
                let relative = sub(point, *origin_mm);
                let distance = dot(relative, *normal).abs();
                if !distance.is_finite() {
                    return Err(invalid("nonfinite plane carrier distance"));
                }
                let rounding = 16.0 * f64::EPSILON * length(relative);
                maximum = maximum.max(distance + rounding);
            }
            maximum
        }
        AnalyticSurfaceDescription::Cylinder {
            axis_origin_mm,
            axis_direction,
            radius_mm,
        } => {
            let projected =
                triangle.map(|point| project_radial(sub(point, *axis_origin_mm), *axis_direction));
            if projected.iter().any(|&point| !finite(point)) {
                return Err(invalid("nonfinite cylinder triangle projection"));
            }
            let maximum = projected.into_iter().map(length).fold(0.0_f64, f64::max);
            let minimum = projected_triangle_distance(projected);
            if !maximum.is_finite() || !minimum.is_finite() {
                return Err(invalid("nonfinite projected triangle distance"));
            }
            // Outward rounding allowance protects the conservative bound from
            // scalar projection/distance arithmetic, separately from f32 packing.
            let rounding = 64.0 * f64::EPSILON * (maximum + radius_mm);
            (maximum + rounding - radius_mm)
                .max(radius_mm - (minimum - rounding).max(0.0))
                .max(0.0)
        }
        AnalyticSurfaceDescription::Sphere { frame, radius_mm } => {
            let relative = triangle.map(|p| sub(p, frame.origin_mm));
            let maximum = relative.into_iter().map(length).fold(0.0_f64, f64::max);
            let minimum = triangle_distance(relative);
            let rounding = arithmetic_reserve(triangle, frame.origin_mm, *radius_mm);
            (maximum + rounding - radius_mm)
                .max(radius_mm - (minimum - rounding).max(0.0))
                .max(0.0)
        }
        _ => unreachable!("physical oracle is only used for planes, cylinders and spheres"),
    };
    if !deviation.is_finite() {
        return Err(invalid("nonfinite analytic carrier deviation"));
    }
    Ok(deviation)
}

fn arithmetic_reserve(points: [[f64; 3]; 3], origin: [f64; 3], radius: f64) -> f64 {
    256.0
        * f64::EPSILON
        * (points
            .into_iter()
            .map(length)
            .fold(length(origin) + radius.abs(), f64::max)
            + 1.0)
}

/// Distance to a spatial triangle, including the interior projection.
fn triangle_distance(points: [[f64; 3]; 3]) -> f64 {
    let normal = cross(sub(points[1], points[0]), sub(points[2], points[0]));
    let norm2 = dot(normal, normal);
    if norm2 > 0.0 {
        let projection = scale(normal, dot(points[0], normal) / norm2);
        if [(0, 1), (1, 2), (2, 0)].into_iter().all(|(a, b)| {
            dot(
                cross(sub(points[b], points[a]), sub(projection, points[a])),
                normal,
            ) >= 0.0
        }) {
            return length(projection);
        }
    }
    [(0, 1), (1, 2), (2, 0)]
        .into_iter()
        .map(|(a, b)| segment_distance(points[a], points[b]))
        .fold(f64::INFINITY, f64::min)
}

fn projected_triangle_distance(points: [[f64; 3]; 3]) -> f64 {
    let normal = cross(sub(points[1], points[0]), sub(points[2], points[0]));
    let edges = [
        (points[0], points[1]),
        (points[1], points[2]),
        (points[2], points[0]),
    ];
    if dot(normal, normal) > 0.0
        && edges
            .iter()
            .all(|&(a, b)| dot(cross(sub(b, a), scale(a, -1.0)), normal) >= 0.0)
    {
        0.0
    } else {
        edges
            .into_iter()
            .map(|(a, b)| segment_distance(a, b))
            .fold(f64::INFINITY, f64::min)
    }
}

fn segment_distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    let direction = sub(b, a);
    let denominator = dot(direction, direction);
    let t = if denominator == 0.0 {
        0.0
    } else {
        (-dot(a, direction) / denominator).clamp(0.0, 1.0)
    };
    length(add(a, scale(direction, t)))
}

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
        assert_eq!(descriptor.definition_id, definition.id);
        assert_eq!(descriptor.face_count as usize, definition.faces.len());
        assert!(chunk.bytes.len() <= MAX_GEOMETRY_CHUNK_BYTES as usize);
        assert!(descriptor.carrier_deviation_mm <= MESH_CARRIER_TOLERANCE_MM);
        assert!(
            descriptor.carrier_deviation_mm + descriptor.trim_deviation_mm
                <= MESH_CARRIER_TOLERANCE_MM
        );
        assert!(descriptor.quantization_error_mm <= MESH_QUANTIZATION_TOLERANCE_MM);
        assert!(
            descriptor.carrier_deviation_mm
                + descriptor.trim_deviation_mm
                + descriptor.quantization_error_mm
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
                if let Some(carrier) = &info.geometry.analytic {
                    let expected = scale(
                        carrier_normal(carrier, position).unwrap(),
                        if info.orientation { 1.0 } else { -1.0 },
                    );
                    assert!(length(sub(normal, expected)) < 2.0 * NORMAL_NORM_TOLERANCE);
                }
                assert!(dot(geometric_normal, normal) > 0.0);
            }
            if let Some(carrier) = &info.geometry.analytic {
                if !matches!(
                    carrier,
                    AnalyticSurfaceDescription::Plane { .. }
                        | AnalyticSurfaceDescription::Cylinder { .. }
                        | AnalyticSurfaceDescription::Sphere { .. }
                ) {
                    continue;
                }
                let measured = triangle_carrier_deviation(carrier, points).unwrap();
                // Distance to the analytic carrier is 1-Lipschitz in positions.
                assert!(
                    measured
                        <= descriptor.carrier_deviation_mm
                            + descriptor.quantization_error_mm
                            + 1e-8
                );
            }
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
        "independent-exporters/occt-box-mm.step",
        "independent-exporters/occt-cylinder-mm.step",
        "independent-exporters/occt-through-hole-mm.step",
    ] {
        let definition = fixture(name);
        if name.starts_with("box-") {
            assert_eq!(definition.faces.len(), 6);
        }
        let encoded = tessellate(
            &definition,
            DisplayProfile::MeshTrimmedMm005V2,
            &AtomicBool::new(false),
        )
        .unwrap();
        audit_encoded(&definition, &encoded);
        let repeated = tessellate(
            &definition,
            DisplayProfile::MeshTrimmedMm005V2,
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

fn assert_through_hole_caps_keep_trim_and_inward_normals(name: &str) {
    let definition = fixture(name);
    let encoded = tessellate(
        &definition,
        DisplayProfile::MeshTrimmedMm005V2,
        &AtomicBool::new(false),
    )
    .unwrap();
    let hole = definition
        .faces
        .iter()
        .find_map(|info| match info.geometry.analytic.clone().unwrap() {
            AnalyticSurfaceDescription::Cylinder {
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
            match info.geometry.analytic.clone().unwrap() {
                AnalyticSurfaceDescription::Cylinder { .. } => {
                    hole_triangles += 1;
                    for (position, index) in points.into_iter().zip(indices) {
                        let radial = project_radial(sub(position, hole.0), hole.1);
                        let normal = view.normals.get(index as usize).unwrap().map(f64::from);
                        assert!(dot(normal, radial) < 0.0);
                    }
                }
                AnalyticSurfaceDescription::Plane { normal, .. }
                    if dot(normal, hole.1).abs() > 1.0 - 1e-10 =>
                {
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
fn through_hole_exporters_keep_cap_trims_and_inward_cylinder_normals() {
    for name in [
        "through-hole.step",
        "independent-exporters/occt-through-hole-mm.step",
    ] {
        assert_through_hole_caps_keep_trim_and_inward_normals(name);
    }
}

#[test]
fn projected_cylinder_audit_catches_edge_and_interior_sag_not_visible_at_vertices() {
    let carrier = AnalyticSurfaceDescription::Cylinder {
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
    let rotated = AnalyticSurfaceDescription::Cylinder {
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
        tessellate(&definition, DisplayProfile::MeshTrimmedMm005V2, &cancel)
            .unwrap_err()
            .code,
        GeometryErrorCode::InvalidGeometry
    );
    definition
        .faces
        .resize(MAX_NATIVE_FACES as usize + 1, first);
    assert_eq!(
        tessellate(&definition, DisplayProfile::MeshTrimmedMm005V2, &cancel)
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
    let input = FaceAuditInput {
        polygon: &polygon,
        surface: source,
        native_orientation: face.orientation(),
        info,
        budget: MeshBudget::FROZEN,
        sphere_chart: None,
        boundary_face: None,
        extra_retained_bytes: 0,
    };
    assert!(
        audit_face(
            &input,
            &mut crate::GeometryWorkBudget::new(20_000_000),
            &cancel,
        )
        .is_ok()
    );
    let mut bad = polygon.clone();
    bad.normals_mut()[0] = Vector3::new(0.0, 0.0, 0.0);
    assert!(
        audit_face(
            &FaceAuditInput {
                polygon: &bad,
                ..input
            },
            &mut crate::GeometryWorkBudget::new(20_000_000),
            &cancel,
        )
        .is_err()
    );
    let mut bad = polygon.clone();
    bad.positions_mut()[0] = Point3::new(f64::NAN, 0.0, 0.0);
    assert!(
        audit_face(
            &FaceAuditInput {
                polygon: &bad,
                ..input
            },
            &mut crate::GeometryWorkBudget::new(20_000_000),
            &cancel,
        )
        .is_err()
    );
    let mut bad = polygon.clone();
    bad.uncheck_editor().faces.tri_faces_mut()[0][0].pos = polygon.positions().len();
    assert!(
        audit_face(
            &FaceAuditInput {
                polygon: &bad,
                ..input
            },
            &mut crate::GeometryWorkBudget::new(20_000_000),
            &cancel,
        )
        .is_err()
    );
    let mut bad = polygon.clone();
    bad.uncheck_editor().faces.tri_faces_mut()[0][0].nor = None;
    assert!(
        audit_face(
            &FaceAuditInput {
                polygon: &bad,
                ..input
            },
            &mut crate::GeometryWorkBudget::new(20_000_000),
            &cancel,
        )
        .is_err()
    );
    let mut bad = polygon.clone();
    bad.uncheck_editor().faces.tri_faces_mut()[0][0].uv = None;
    assert!(
        audit_face(
            &FaceAuditInput {
                polygon: &bad,
                ..input
            },
            &mut crate::GeometryWorkBudget::new(20_000_000),
            &cancel,
        )
        .is_err()
    );
    let mut bad = polygon.clone();
    bad.uncheck_editor().faces.tri_faces_mut()[0].swap(0, 1);
    assert!(
        audit_face(
            &FaceAuditInput {
                polygon: &bad,
                ..input
            },
            &mut crate::GeometryWorkBudget::new(20_000_000),
            &cancel,
        )
        .is_err()
    );
    let mut bad_info = info.clone();
    bad_info.orientation = !bad_info.orientation;
    assert!(
        audit_face(
            &FaceAuditInput {
                info: &bad_info,
                ..input
            },
            &mut crate::GeometryWorkBudget::new(20_000_000),
            &cancel,
        )
        .is_err()
    );
}

#[test]
fn frozen_perforated_plate_is_a_real_bounded_multichunk_transfer_workload() {
    let definition = fixture("perforated-plate.step");
    let encoded = tessellate(
        &definition,
        DisplayProfile::MeshTrimmedMm005V2,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(
        encoded.total_bytes > 4 * 1024 * 1024,
        "frozen workload must exceed diagnostic binary cap"
    );
    audit_encoded(&definition, &encoded);
}

fn analytic_frame() -> spiling_contracts::geometry::PlaneFrameMm {
    spiling_contracts::geometry::PlaneFrameMm {
        origin_mm: [0.0; 3],
        x_axis: [1.0, 0.0, 0.0],
        y_axis: [0.0, 1.0, 0.0],
        z_axis: [0.0, 0.0, 1.0],
    }
}

#[test]
fn spherical_triangle_interior_sag_is_not_a_vertex_certificate() {
    let sphere = AnalyticSurfaceDescription::Sphere {
        frame: analytic_frame(),
        radius_mm: 5.0,
    };
    let triangle = [[5.0, 0.0, 0.0], [0.0, 5.0, 0.0], [0.0, 0.0, 5.0]];
    assert!((triangle_distance(triangle) - 5.0 / 3.0_f64.sqrt()).abs() < 1e-12);
    assert!(triangle_carrier_deviation(&sphere, triangle).unwrap() > 2.0);
}

#[test]
fn whole_native_triangle_certificate_converges_without_slicing_a_taper() {
    use monstertruck_modeling::{Curve, Line, Processor, RevolutionSurface};
    let surface = Surface::RevolutionSurface(Processor::new(RevolutionSurface::by_revolution(
        Curve::Line(Line(
            Point3::new(6.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 12.0),
        )),
        Point3::new(0.0, 0.0, 0.0),
        Vector3::new(0.0, 0.0, 1.0),
    )));
    let angle = 0.002_f64;
    let uv = [[0.0, 0.0], [1.0, 0.0], [1.0, angle]];
    let points = [
        [6.0, 0.0, 0.0],
        [1.0, 0.0, 12.0],
        [angle.cos(), angle.sin(), 12.0],
    ];
    let bound = crate::parametric::triangle_error_bound(
        &surface,
        &uv,
        &points,
        &mut crate::GeometryWorkBudget::new(50_000),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(bound > 0.0 && bound < MESH_CARRIER_TOLERANCE_MM);
    // Independent physical world geometry, not a native-evaluation echo:
    // the same weights pair source-image and mesh-triangle points both ways.
    for i in 0..=16 {
        for j in 0..=16 - i {
            let weights = [
                1.0 - f64::from(i + j) / 16.0,
                f64::from(i) / 16.0,
                f64::from(j) / 16.0,
            ];
            let u = weights[1] + weights[2];
            let theta = weights[2] * angle;
            let radius = 6.0 - 5.0 * u;
            let expected = [radius * theta.cos(), radius * theta.sin(), 12.0 * u];
            let mesh = std::array::from_fn(|axis| {
                (0..3).map(|k| weights[k] * points[k][axis]).sum::<f64>()
            });
            assert!(length(sub(mesh, expected)) <= bound);
        }
    }
}

#[test]
fn non_ring_carrier_normals_remain_refused() {
    let horn = AnalyticSurfaceDescription::Torus {
        frame: analytic_frame(),
        major_radius_mm: 2.0,
        minor_radius_mm: 2.0,
        torus_kind: TorusKind::Horn,
    };
    assert!(carrier_normal(&horn, [4.0, 0.0, 0.0]).is_err());
}

#[test]
fn genuine_regular_native_families_complete_import_picking_bounds_and_sections() {
    let cancel = AtomicBool::new(false);
    let originals =
        crate::parity_fixtures::recipes()
            .into_iter()
            .map(|(name, solid, expectation)| {
                (
                    name,
                    crate::parity_fixtures::original(name, solid),
                    expectation,
                )
            });
    let independent = crate::parity_fixtures::independent_recipes()
        .into_iter()
        .map(|(name, expectation)| {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/geometry")
                .join(name);
            (
                name,
                std::fs::read_to_string(path).expect("frozen independent OCCT source"),
                expectation,
            )
        });
    for (name, bytes, expectation) in originals.chain(independent) {
        let definition = crate::import_step(bytes.as_bytes(), &cancel)
            .unwrap_or_else(|error| panic!("{name} import: {error:?}"));
        if let Some(count) = expectation["face_count"].as_u64() {
            assert_eq!(
                definition.faces().len() as u64,
                count,
                "{name} original source face count"
            );
        }
        let mesh = tessellate(&definition, DisplayProfile::MeshTrimmedMm005V2, &cancel)
            .unwrap_or_else(|error| panic!("{name} certified display: {error:?}"));
        audit_encoded(&definition, &mesh);
        let mut work = crate::GeometryWorkBudget::new(1_000_000);
        for (axis, direction) in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
            .into_iter()
            .enumerate()
        {
            let bounds = crate::projected_bounds(
                &definition,
                direction,
                crate::BoundsPolicy {
                    maximum_endpoint_uncertainty_mm: 1e-6,
                },
                &mut work,
                &cancel,
            )
            .unwrap_or_else(|error| panic!("{name} bounds: {error:?}"));
            assert!(bounds.minimum[0] <= bounds.minimum[1]);
            assert!(bounds.maximum[0] <= bounds.maximum[1]);
            assert!(bounds.achieved_uncertainty_mm <= 1e-6);
            if let (Some(minimum), Some(maximum)) = (
                expectation["physical_bounds_mm"]["min"][axis].as_f64(),
                expectation["physical_bounds_mm"]["max"][axis].as_f64(),
            ) {
                assert!(
                    bounds.minimum[0] <= minimum && minimum <= bounds.minimum[1],
                    "{name} axis {axis} true minimum missing from certified bracket"
                );
                assert!(
                    bounds.maximum[0] <= maximum && maximum <= bounds.maximum[1],
                    "{name} axis {axis} true maximum missing from certified bracket"
                );
            }
        }
        let section = crate::section(
            &definition,
            spiling_contracts::geometry::PlaneMm {
                origin_mm: [0.0, 0.0, expectation["section_z_mm"].as_f64().unwrap()],
                normal: [0.0, 0.0, 1.0],
            },
            &cancel,
        )
        .unwrap_or_else(|error| panic!("{name} native section: {error:?}"));
        assert!(
            !section.loops.is_empty(),
            "{name} loses native interior material"
        );
        if let Some(count) = expectation["section_loop_count"].as_u64() {
            assert_eq!(section.loops.len() as u64, count, "{name}");
        }
        let area: f64 = section
            .loops
            .iter()
            .map(|boundary| {
                boundary
                    .points_mm
                    .windows(2)
                    .map(|pair| pair[0][0] * pair[1][1] - pair[1][0] * pair[0][1])
                    .sum::<f64>()
                    * 0.5
            })
            .sum();
        let expected_area = expectation["section_area_mm2"].as_f64().or_else(|| {
            expectation["section_area_expression"]
                .as_str()
                .map(|expression| match expression {
                    "16*pi" => 16.0 * std::f64::consts::PI,
                    "25*pi" => 25.0 * std::f64::consts::PI,
                    "40*pi" => 40.0 * std::f64::consts::PI,
                    "48*pi" => 48.0 * std::f64::consts::PI,
                    "12*pi" => 12.0 * std::f64::consts::PI,
                    "221-4*pi" => 221.0 - 4.0 * std::f64::consts::PI,
                    "400-9*pi" => 400.0 - 9.0 * std::f64::consts::PI,
                    other => panic!("unknown original recipe expression {other}"),
                })
        });
        if let Some(expected) = expected_area {
            let perimeter: f64 = section
                .loops
                .iter()
                .flat_map(|boundary| boundary.points_mm.windows(2))
                .map(|pair| length(sub(pair[1], pair[0])))
                .sum();
            let tolerance = section.sampling_tolerance_mm;
            let area_bound = 2.0 * perimeter * tolerance
                + std::f64::consts::TAU * tolerance * tolerance * section.loops.len() as f64
                + 1e-8;
            assert!(
                (area - expected).abs() <= area_bound,
                "{name}: native area {area} expected {expected}, bound {area_bound}"
            );
        }
    }
}

#[test]
fn denominator_zero_is_a_conditioning_refusal_not_a_geometry_label_gate() {
    let bytes = crate::parity_fixtures::conditioning_source();
    let error = crate::import_step(bytes.as_bytes(), &AtomicBool::new(false)).unwrap_err();
    assert_eq!(error.code, GeometryErrorCode::ConditioningFailure);
}

#[test]
fn unsampled_polynomial_oscillation_never_receives_a_sample_only_certificate() {
    let cancel = AtomicBool::new(false);
    let bytes = crate::parity_fixtures::aliasing_source();
    let definition = crate::import_step(bytes.as_bytes(), &cancel).unwrap();
    match tessellate(&definition, DisplayProfile::MeshTrimmedMm005V2, &cancel) {
        Ok(mesh) => audit_encoded(&definition, &mesh),
        Err(error) => assert!(matches!(
            error.code,
            GeometryErrorCode::ShapeErrorExceeded
                | GeometryErrorCode::ConditioningFailure
                | GeometryErrorCode::AnalysisIndeterminate
                | GeometryErrorCode::ResourceLimit
        )),
    }
}

#[test]
fn complete_native_trim_proof_rejects_missing_and_duplicate_on_carrier_triangles() {
    let definition = fixture("box-mm.step");
    let face = definition.solid.face_iter().next().unwrap();
    let shell: Shell = std::iter::once(face.clone()).collect();
    let compressed = shell.compress();
    let mut meshed = cshell_triangulation_with(
        &compressed,
        TessellationOptions {
            tolerance: MESH_CARRIER_TOLERANCE_MM * 0.25,
            search_trials: 2,
            primitive: TessellationPrimitiveOptions {
                mode: TessellationPrimitiveMode::Triangles,
                ..Default::default()
            },
        },
    );
    let polygon = meshed.faces[0].surface.take().unwrap();
    let cancel = AtomicBool::new(false);
    coverage::certify(
        &definition,
        0,
        &polygon,
        0,
        &mut crate::GeometryWorkBudget::new(20_000_000),
        &cancel,
    )
    .unwrap();
    let mut missing = polygon.clone();
    let mut triangles = polygon.tri_faces().clone();
    triangles.pop().unwrap();
    *missing.uncheck_editor().faces =
        monstertruck_meshing::prelude::Faces::from_tri_and_quad_faces(triangles, Vec::new());
    assert!(
        coverage::certify(
            &definition,
            0,
            &missing,
            0,
            &mut crate::GeometryWorkBudget::new(20_000_000),
            &cancel
        )
        .is_err()
    );
    let mut duplicated = polygon.clone();
    let duplicate = polygon.tri_faces()[0];
    let mut triangles = polygon.tri_faces().clone();
    triangles.push(duplicate);
    *duplicated.uncheck_editor().faces =
        monstertruck_meshing::prelude::Faces::from_tri_and_quad_faces(triangles, Vec::new());
    assert!(
        coverage::certify(
            &definition,
            0,
            &duplicated,
            0,
            &mut crate::GeometryWorkBudget::new(20_000_000),
            &cancel
        )
        .is_err()
    );
}

#[test]
fn frozen_nurbs_representation_preserves_face_table_display_and_native_section() {
    let cancel = AtomicBool::new(false);
    let definition = fixture("unsupported-nurbs.step");
    assert_eq!(definition.faces.len(), 6);
    assert_eq!(
        definition.faces[0].geometry.family,
        spiling_contracts::geometry::SurfaceFamily::Nurbs
    );
    assert!(definition.faces[0].geometry.analytic.is_none());
    let mesh = tessellate(&definition, DisplayProfile::MeshTrimmedMm005V2, &cancel).unwrap();
    audit_encoded(&definition, &mesh);
    let native = crate::section(
        &definition,
        spiling_contracts::geometry::PlaneMm {
            origin_mm: [0.0, 0.0, 4.0],
            normal: [0.0, 0.0, 1.0],
        },
        &cancel,
    )
    .unwrap();
    assert_eq!(native.loops.len(), 1);
    let mut work = crate::GeometryWorkBudget::new(1_000_000);
    let bounds = crate::projected_bounds(
        &definition,
        [0.0, 0.0, 1.0],
        crate::BoundsPolicy {
            maximum_endpoint_uncertainty_mm: 1e-6,
        },
        &mut work,
        &cancel,
    )
    .unwrap();
    assert!(bounds.minimum[0] <= 0.0 && bounds.minimum[1] >= 0.0);
    assert!(bounds.maximum[0] <= 8.0 && bounds.maximum[1] >= 8.0);
}

#[test]
fn whole_coedge_source_chart_uncertainty_is_charged_before_packing() {
    let cancel = AtomicBool::new(false);
    let mut definition = fixture("box-mm.step");
    let original = tessellate(&definition, DisplayProfile::MeshTrimmedMm005V2, &cancel).unwrap();
    for boundary in &mut definition.face_boundaries {
        for coedge in boundary.wires.iter_mut().flatten() {
            coedge.source_chart_margin_uv =
                coedge.source_chart_margin_uv.map(|value| value.max(1e-5));
        }
    }
    let widened = tessellate(&definition, DisplayProfile::MeshTrimmedMm005V2, &cancel).unwrap();
    let before = original
        .chunks
        .iter()
        .map(|chunk| chunk.descriptor.trim_deviation_mm)
        .fold(0.0_f64, f64::max);
    let after = widened
        .chunks
        .iter()
        .map(|chunk| chunk.descriptor.trim_deviation_mm)
        .fold(0.0_f64, f64::max);
    assert!(after > before);
    assert_eq!(original.vertex_count, widened.vertex_count);
    assert_eq!(original.triangle_count, widened.triangle_count);
    for boundary in &mut definition.face_boundaries {
        for coedge in boundary.wires.iter_mut().flatten() {
            coedge.source_chart_margin_uv = [0.1; 2];
        }
    }
    assert_eq!(
        tessellate(&definition, DisplayProfile::MeshTrimmedMm005V2, &cancel)
            .unwrap_err()
            .code,
        GeometryErrorCode::ShapeErrorExceeded
    );
}

#[test]
fn independent_spline_housing_natural_poles_keep_generic_source_identity() {
    let cancel = AtomicBool::new(false);
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/geometry/independent-exporters/occt-spline-housing-mm.step");
    let bytes = std::fs::read(path).expect("unchanged independent OCCT spline housing source");
    let definition = crate::import_step(&bytes, &cancel).unwrap();
    let mut work = crate::GeometryWorkBudget::new(20_000_000);
    let mut natural = 0;
    for ordinal in 0..definition.faces().len() {
        let trim = crate::kernel::prepare_face_trim_domain_with_byte_limit(
            &definition,
            ordinal,
            coverage::PROOF_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        for joint in trim.native().joints() {
            if let monstertruck_modeling::TrimJointKind::NaturalBoundary {
                world_enclosure, ..
            } = &joint.kind
            {
                natural += 1;
                assert!(
                    definition.faces()[ordinal].geometry.analytic.is_none(),
                    "generic source pole must not be coerced into an analytic sphere"
                );
                assert!(matches!(
                    definition.faces()[ordinal].geometry.family,
                    spiling_contracts::geometry::SurfaceFamily::BSpline
                        | spiling_contracts::geometry::SurfaceFamily::Nurbs
                ));
                assert!(world_enclosure.iter().all(|axis| axis.lower.is_finite()
                    && axis.upper.is_finite()
                    && axis.lower <= axis.upper));
            }
        }
    }
    assert!(
        natural > 0,
        "actual converted corner patches must exercise native natural-boundary authority"
    );
    let mesh = tessellate(&definition, DisplayProfile::MeshTrimmedMm005V2, &cancel).unwrap();
    audit_encoded(&definition, &mesh);
    for chunk in &mesh.chunks {
        assert!(
            chunk.descriptor.carrier_deviation_mm + chunk.descriptor.trim_deviation_mm <= 0.025
        );
        assert!(chunk.descriptor.quantization_error_mm <= 0.025);
    }
}

#[test]
fn reversed_native_face_joints_preserve_independent_box_area_and_display_error() {
    for name in ["box-mm.step", "independent-exporters/occt-box-mm.step"] {
        let definition = fixture(name);
        let encoded = tessellate(
            &definition,
            DisplayProfile::MeshTrimmedMm005V2,
            &AtomicBool::new(false),
        )
        .expect(name);
        audit_encoded(&definition, &encoded);
        let mut areas = vec![0.0; definition.faces.len()];
        let mut volume = 0.0;
        for (chunk, descriptor) in encoded.chunks.iter().zip(metadata(&encoded)) {
            assert!(
                descriptor.trim_deviation_mm < 1e-6,
                "a reversed planar joint must be compared with its incident mesh vertex, not the opposite edge endpoint: {name}"
            );
            let view = decoded(&chunk.bytes, &descriptor);
            for (triangle_index, ordinal) in view.face_ordinals.iter().enumerate() {
                let points = std::array::from_fn::<_, 3, _>(|slot| {
                    point(&view, view.indices.get(3 * triangle_index + slot).unwrap())
                });
                areas[ordinal as usize] +=
                    0.5 * length(cross(sub(points[1], points[0]), sub(points[2], points[0])));
                volume += dot(points[0], cross(points[1], points[2])) / 6.0;
            }
        }
        areas.sort_by(f64::total_cmp);
        assert_eq!(areas.len(), 6);
        for (area, expected) in areas.into_iter().zip([80., 80., 160., 160., 200., 200.]) {
            assert!(
                (area - expected).abs() < 1e-5,
                "independent 20×10×8 box face-area oracle: {name}"
            );
        }
        assert!(
            (volume - 1600.).abs() < 1e-5,
            "closed outward box volume oracle: {name}"
        );
    }
}

#[test]
fn unchanged_independent_fitting_display_preserves_physical_taper_and_bore() {
    let definition = fixture("independent-exporters/occt-tapered-fitting-mm.step");
    assert_eq!(definition.faces.len(), 4);
    let encoded = tessellate(
        &definition,
        DisplayProfile::MeshTrimmedMm005V2,
        &AtomicBool::new(false),
    )
    .unwrap();
    audit_encoded(&definition, &encoded);
    for (chunk, descriptor) in encoded.chunks.iter().zip(metadata(&encoded)) {
        let view = decoded(&chunk.bytes, &descriptor);
        for (triangle_index, ordinal) in view.face_ordinals.iter().enumerate() {
            let points: [[f64; 3]; 3] = std::array::from_fn(|slot| {
                point(&view, view.indices.get(3 * triangle_index + slot).unwrap())
            });
            for weights in [
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [0.5, 0.5, 0.0],
                [0.0, 0.5, 0.5],
                [0.5, 0.0, 0.5],
                [1.0 / 3.0; 3],
            ] {
                let p: [f64; 3] =
                    std::array::from_fn(|axis| (0..3).map(|i| weights[i] * points[i][axis]).sum());
                // This unchanged independent export has radius 6 -> 4 over
                // Z=0..12, a radius-two bore, and the two physical end planes.
                let distance = match definition.faces[ordinal as usize]
                    .geometry
                    .analytic
                    .as_ref()
                    .unwrap()
                {
                    AnalyticSurfaceDescription::Plane { origin_mm, .. } => {
                        let z = if origin_mm[2].abs() < 1e-10 {
                            0.0
                        } else {
                            assert!((origin_mm[2] - 12.0).abs() < 1e-10);
                            12.0
                        };
                        (p[2] - z).abs()
                    }
                    AnalyticSurfaceDescription::Cylinder { radius_mm, .. } => {
                        assert!((radius_mm - 2.0).abs() < 1e-10);
                        (p[0].hypot(p[1]) - 2.0).abs()
                    }
                    AnalyticSurfaceDescription::Cone { .. } => {
                        (p[0].hypot(p[1]) - (6.0 - p[2] / 6.0)).abs()
                            / (1.0_f64 + 1.0 / 36.0).sqrt()
                    }
                    _ => panic!("unexpected physical fitting carrier"),
                };
                assert!(
                    distance
                        <= descriptor.carrier_deviation_mm
                            + descriptor.quantization_error_mm
                            + 1e-10
                );
            }
        }
    }
}

#[test]
fn correspondence_admission_bounds_actual_live_mask_storage() {
    let circuits = vec![vec![
        coverage::Corner {
            uv: [0.0; 2],
            position: 0,
            bounds: (),
        };
        16_385
    ]];
    let edge_count = circuits[0].len() - 1;
    let endpoints = Vec::<(usize, usize)>::with_capacity(edge_count);
    let used = vec![vec![false; edge_count]];
    // Measure the actual simultaneously retained vector capacities, without
    // relying on the estimator's assumptions about mask representation.
    let actual_live = coverage::vector_bytes(&circuits)
        + circuits
            .iter()
            .map(|circuit| circuit.capacity() * std::mem::size_of::<coverage::Corner>())
            .sum::<usize>()
        + coverage::vector_bytes(&endpoints)
        + coverage::vector_bytes(&used)
        + used
            .iter()
            .map(|mask| mask.capacity() * std::mem::size_of::<bool>())
            .sum::<usize>();
    let admitted = coverage::correspondence_bytes(&circuits, 4, 1);
    let reserved = coverage::PROOF_BYTES.checked_sub(admitted).unwrap();
    coverage::check_memory(reserved + admitted).unwrap();
    assert!(
        reserved + actual_live <= coverage::PROOF_BYTES,
        "admitted correspondence storage exceeds the shared retained-byte cap"
    );
}
