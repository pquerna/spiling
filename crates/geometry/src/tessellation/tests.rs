// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use spiling_contracts::display::{ValidatedMeshChunk, validate_mesh_chunk, validate_mesh_manifest};
use spiling_contracts::geometry::{ArtifactId, MeshChunkMetadata, SessionId, MAX_GEOMETRY_CHUNK_BYTES, MESH_QUANTIZATION_TOLERANCE_MM, MESH_SURFACE_TOLERANCE_MM};
use std::path::Path;

fn fixture(name: &str) -> Definition {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/geometry").join(name);
    let bytes = std::fs::read(&path).unwrap_or_else(|error| panic!("original fixture {}: {error}", path.display()));
    crate::from_step_bytes_with_cancel(&bytes, &AtomicBool::new(false)).unwrap()
}
fn metadata(mesh: &MeshArtifact) -> Vec<MeshChunkMetadata> {
    let session = SessionId::new();
    let artifact = ArtifactId::new(1).unwrap();
    mesh.chunks.iter().map(|chunk| chunk.descriptor.clone().into_metadata(session.clone(), artifact)).collect()
}
fn decoded<'a>(bytes: &'a [u8], descriptor: &MeshChunkMetadata) -> ValidatedMeshChunk<'a> {
    validate_mesh_chunk(bytes, descriptor, &descriptor.session_id, descriptor.artifact_id, &descriptor.definition_id, descriptor.chunk_index).unwrap()
}
fn point(mesh: &ValidatedMeshChunk<'_>, index: u32) -> [f64; 3] {
    let p = mesh.positions.get(index as usize).unwrap();
    std::array::from_fn(|a| mesh.local_origin_mm[a] + f64::from(p[a]))
}
fn audit_encoded(definition: &Definition, encoded: &MeshArtifact) {
    let descriptors = metadata(encoded);
    validate_mesh_manifest(&descriptors).unwrap();
    let mut seen = vec![false; definition.face_ids().len()];
    let mut previous = 0;
    let mut triangles = 0;
    for (chunk, descriptor) in encoded.chunks.iter().zip(&descriptors) {
        assert_eq!(&descriptor.definition_id, definition.id());
        assert_eq!(descriptor.face_count as usize, definition.face_ids().len());
        assert!(chunk.bytes.len() <= MAX_GEOMETRY_CHUNK_BYTES as usize);
        assert!(descriptor.carrier_deviation_mm + descriptor.trim_deviation_mm <= MESH_CARRIER_TOLERANCE_MM);
        assert!(descriptor.quantization_error_mm <= MESH_QUANTIZATION_TOLERANCE_MM);
        assert!(descriptor.carrier_deviation_mm + descriptor.trim_deviation_mm + descriptor.quantization_error_mm <= MESH_SURFACE_TOLERANCE_MM);
        let view = decoded(&chunk.bytes, descriptor);
        triangles += view.face_ordinals.len() as u32;
        for (triangle_index, ordinal) in view.face_ordinals.iter().enumerate() {
            assert!(ordinal >= previous);
            previous = ordinal;
            seen[ordinal as usize] = true;
            let native = definition.face_ids()[ordinal as usize];
            assert_eq!(definition.face_ordinal(native), Some(ordinal as usize));
            let points: [[f64; 3]; 3] = std::array::from_fn(|i| point(&view, view.indices.get(3 * triangle_index + i).unwrap()));
            let a = audit::sub(points[1], points[0]);
            let b = audit::sub(points[2], points[0]);
            let cross = [a[1]*b[2]-a[2]*b[1], a[2]*b[0]-a[0]*b[2], a[0]*b[1]-a[1]*b[0]];
            for slot in 0..3 {
                let index = view.indices.get(3 * triangle_index + slot).unwrap();
                let normal = view.normals.get(index as usize).unwrap();
                assert!((0..3).map(|i| cross[i] * f64::from(normal[i])).sum::<f64>() > 0.0);
            }
        }
    }
    assert!(seen.into_iter().all(|v| v), "every original source-face ordinal must be published");
    assert_eq!(triangles, encoded.triangle_count);
    assert!(triangles > 0);
}

#[test]
fn original_parts_preserve_all_source_faces_analytic_normals_and_profile_precision() {
    for name in ["box-mm.step", "box-inch.step", "cylinder.step", "through-hole.step", "independent-exporters/occt-box-mm.step", "independent-exporters/occt-cylinder-mm.step", "independent-exporters/occt-through-hole-mm.step", "independent-exporters/occt-rounded-housing-mm.step", "independent-exporters/occt-tapered-fitting-mm.step"] {
        let definition = fixture(name);
        let mesh = tessellate(&definition, DisplayProfile::MeshTrimmedMm005V2, &AtomicBool::new(false), MeshBudget::FROZEN).unwrap_or_else(|cause| panic!("{name}: {cause:?}"));
        audit_encoded(&definition, &mesh);
    }
}

#[test]
fn unchanged_independent_fitting_display_preserves_physical_taper_and_bore() {
    let definition = fixture("independent-exporters/occt-tapered-fitting-mm.step");
    let mesh = tessellate(&definition, DisplayProfile::MeshTrimmedMm005V2, &AtomicBool::new(false), MeshBudget::FROZEN).unwrap();
    audit_encoded(&definition, &mesh);
    for (chunk, descriptor) in mesh.chunks.iter().zip(metadata(&mesh)) {
        let view = decoded(&chunk.bytes, &descriptor);
        for triangle in 0..view.face_ordinals.len() {
            let points: [[f64; 3]; 3] = std::array::from_fn(|i| point(&view, view.indices.get(3 * triangle + i).unwrap()));
            for weights in [[1.0,0.0,0.0],[0.0,1.0,0.0],[0.0,0.0,1.0],[0.5,0.5,0.0],[0.0,0.5,0.5],[0.5,0.0,0.5],[1.0/3.0;3]] {
                let p:[f64;3]=std::array::from_fn(|axis|(0..3).map(|i|weights[i]*points[i][axis]).sum());
                let ordinal=view.face_ordinals.get(triangle).unwrap();
                use spiling_contracts::geometry::AnalyticSurfaceDescription as Carrier;
                let distance=match definition.faces()[ordinal as usize].geometry.analytic.as_ref().unwrap() {
                    Carrier::Plane { origin_mm,.. } => (p[2]-origin_mm[2]).abs(),
                    Carrier::Cylinder { radius_mm,.. } => {
                        assert!((*radius_mm-2.0).abs()<1e-10);
                        (p[0].hypot(p[1])-2.0).abs()
                    }
                    Carrier::Cone {..} => (p[0].hypot(p[1])-(6.0-p[2]/6.0)).abs()/(1.0_f64+1.0/36.0).sqrt(),
                    _=>panic!("unexpected unchanged fitting carrier"),
                };
                assert!(distance<=descriptor.carrier_deviation_mm+descriptor.quantization_error_mm+1e-10);
            }
            let [a,b,c] = points;
            // Complete projected-triangle distance: a radial hole cannot be
            // checked at vertices alone. The origin must remain outside every
            // triangle, with all three edges clear of the physical radius-two bore.
            let orient = |a:[f64;3],b:[f64;3]| a[0]*b[1]-a[1]*b[0];
            let signs = [orient(a,b),orient(b,c),orient(c,a)];
            assert!(!(signs.iter().all(|s|*s>0.0)||signs.iter().all(|s|*s<0.0)));
            for (a,b) in [(a,b),(b,c),(c,a)] {
                let d=[b[0]-a[0],b[1]-a[1]];
                let length=d[0]*d[0]+d[1]*d[1];
                let t=if length>0.0 { (-(a[0]*d[0]+a[1]*d[1])/length).clamp(0.0,1.0) }else{0.0};
                assert!((a[0]+t*d[0]).hypot(a[1]+t*d[1]) >= 2.0-MESH_SURFACE_TOLERANCE_MM);
            }
        }
    }
}

#[test]
fn complete_native_trim_proof_rejects_missing_and_duplicate_on_carrier_triangles() {
    let definition = fixture("box-mm.step");
    let kernel = crate::make_kernel_context();
    let options = TessellateOptions::default().with_chord(MESH_CARRIER_TOLERANCE_MM*0.25);
    let edges = solidcore_mesh::tessellate_edge_set(&kernel, definition.brep(), &options).unwrap();
    let face = definition.face_ids()[0];
    let original = solidcore_mesh::tessellate_face(&kernel, definition.brep(), face, &edges, &options).unwrap();
    for duplicate in [false,true] {
        let mut candidate=original.clone();
        if duplicate { candidate.triangles.push(candidate.triangles[0]); } else { candidate.triangles.remove(0); }
        let error=audit::face(&definition,0,&candidate,&edges,MeshBudget::FROZEN,0,&mut GeometryWorkBudget::new(20_000_000),&AtomicBool::new(false)).err().expect("invalid triangle chain must refuse");
        assert_eq!(error.code,GeometryErrorCode::InvalidGeometry);
    }
}

#[test]
fn face_slot_and_resource_refusals_never_return_partial_artifacts() {
    let definition=fixture("box-mm.step");
    let error=tessellate(&definition,DisplayProfile::MeshTrimmedMm005V2,&AtomicBool::new(false),MeshBudget {vertices:1,triangles:1,bytes:152}).unwrap_err();
    assert!(matches!(error.code,GeometryErrorCode::ResourceLimit|GeometryErrorCode::ShapeErrorExceeded|GeometryErrorCode::InvalidGeometry));
    assert!(tessellate(&definition,DisplayProfile::MeshTrimmedMm005V2,&AtomicBool::new(true),MeshBudget::FROZEN).is_err());
    assert!(tessellate(&definition,DisplayProfile::MeshTrimmedMm005V2,&AtomicBool::new(false),MeshBudget::FROZEN).is_ok());
}
