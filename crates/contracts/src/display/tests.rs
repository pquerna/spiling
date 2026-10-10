// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::geometry::*;

fn ids() -> (SessionId, ArtifactId, DefinitionId) {
    (
        SessionId::new(),
        ArtifactId::new(1).unwrap(),
        DefinitionId::from_source_sha256(&[7; 32]),
    )
}
struct AttachedMeshChunk {
    metadata: MeshChunkMetadata,
    bytes: Vec<u8>,
}
struct AttachedMesh {
    chunks: Vec<AttachedMeshChunk>,
    vertex_count: u32,
    triangle_count: u32,
    total_bytes: u32,
}
fn mesh(chunk_limit: u32) -> AttachedMesh {
    let (session, artifact, definition) = ids();
    let mut builder =
        MeshChunkBuilder::with_chunk_limit(definition, 2, DisplayProfile::MeshMm005V1, chunk_limit)
            .unwrap();
    let origin = 1e9;
    let positions = [
        [origin, 0.0, 0.0],
        [origin + 1.0, 0.0, 0.0],
        [origin, 1.0, 0.0],
        [origin + 1.0, 1.0, 0.0],
    ];
    builder
        .push_face(
            0,
            &positions,
            &[[0.0, 0.0, 1.0]; 4],
            &[[0, 1, 2], [1, 3, 2]],
            0.012,
        )
        .unwrap();
    builder
        .push_face(
            1,
            &[
                [origin, 0.0, 1.0],
                [origin + 1.0, 0.0, 1.0],
                [origin, 1.0, 1.0],
            ],
            &[[0.0, 0.0, -1.0]; 3],
            &[[0, 2, 1]],
            0.0,
        )
        .unwrap();
    let encoded = builder.finish().unwrap();
    AttachedMesh {
        chunks: encoded
            .chunks
            .into_iter()
            .map(|chunk| AttachedMeshChunk {
                metadata: chunk.descriptor.into_metadata(session.clone(), artifact),
                bytes: chunk.bytes,
            })
            .collect(),
        vertex_count: encoded.vertex_count,
        triangle_count: encoded.triangle_count,
        total_bytes: encoded.total_bytes,
    }
}
fn validate_mesh<'a>(
    bytes: &'a [u8],
    metadata: &MeshChunkMetadata,
) -> Result<ValidatedMeshChunk<'a>, GeometryError> {
    validate_mesh_chunk(
        bytes,
        metadata,
        &metadata.session_id,
        metadata.artifact_id,
        &metadata.definition_id,
        metadata.chunk_index,
    )
}
fn rehash_mesh(metadata: &MeshChunkMetadata, bytes: &[u8]) -> MeshChunkMetadata {
    let mut metadata = metadata.clone();
    metadata.sha256 = SourceHash::from_bytes(bytes);
    metadata
}

#[test]
fn mesh_encodes_all_faces_and_chunks_with_local_remapping_and_borrowed_views() {
    let encoded = mesh(152);
    assert_eq!(encoded.chunks.len(), 3);
    assert_eq!(encoded.triangle_count, 3);
    assert_eq!(encoded.vertex_count, 9);
    assert_eq!(encoded.total_bytes, 456);
    let descriptors: Vec<_> = encoded
        .chunks
        .iter()
        .map(|chunk| chunk.metadata.clone())
        .collect();
    validate_mesh_manifest(&descriptors).unwrap();
    let mut faces = Vec::new();
    for (index, chunk) in encoded.chunks.iter().enumerate() {
        assert_eq!(chunk.bytes.len(), 152);
        assert_eq!(chunk.metadata.chunk_index, index as u32);
        assert_eq!(chunk.metadata.chunk_count, 3);
        let decoded = validate_mesh(&chunk.bytes, &chunk.metadata).unwrap();
        assert_eq!(decoded.positions.len(), 3);
        assert_eq!(decoded.indices.len(), 3);
        assert_eq!(
            decoded.positions.bytes().as_ptr(),
            chunk.bytes[64..].as_ptr()
        );
        assert_eq!(
            decoded.local_origin_mm[0],
            1e9 + if index == 1 { 1.0 } else { 0.0 }
        );
        assert_eq!(decoded.indices.iter().collect::<Vec<_>>(), [0, 1, 2]);
        faces.extend(decoded.face_ordinals.iter());
        assert_eq!(decoded.positions.get(3), None);
    }
    assert_eq!(faces, [0, 0, 1]);
    let mut reordered = descriptors.clone();
    reordered.swap(0, 1);
    assert!(validate_mesh_manifest(&reordered).is_err());
    let encoded = mesh(MAX_GEOMETRY_CHUNK_BYTES);
    assert_eq!(encoded.chunks.len(), 1);
    assert_eq!(encoded.vertex_count, 7);
}

#[test]
fn mesh_rejects_every_truncation_identity_hash_and_scalar_corruption() {
    let encoded = mesh(152);
    let chunk = &encoded.chunks[0];
    for length in 0..chunk.bytes.len() {
        assert!(validate_mesh(&chunk.bytes[..length], &chunk.metadata).is_err());
    }
    let mut trailing = chunk.bytes.clone();
    trailing.push(0);
    assert!(validate_mesh(&trailing, &chunk.metadata).is_err());
    assert!(
        validate_mesh_chunk(
            &chunk.bytes,
            &chunk.metadata,
            &SessionId::new(),
            chunk.metadata.artifact_id,
            &chunk.metadata.definition_id,
            0
        )
        .is_err()
    );
    assert!(
        validate_mesh_chunk(
            &chunk.bytes,
            &chunk.metadata,
            &chunk.metadata.session_id,
            ArtifactId::new(2).unwrap(),
            &chunk.metadata.definition_id,
            0
        )
        .is_err()
    );
    assert!(
        validate_mesh_chunk(
            &chunk.bytes,
            &chunk.metadata,
            &chunk.metadata.session_id,
            chunk.metadata.artifact_id,
            &DefinitionId::from_source_sha256(&[8; 32]),
            0
        )
        .is_err()
    );
    assert!(
        validate_mesh_chunk(
            &chunk.bytes,
            &chunk.metadata,
            &chunk.metadata.session_id,
            chunk.metadata.artifact_id,
            &chunk.metadata.definition_id,
            1
        )
        .is_err()
    );
    let mut bytes = chunk.bytes.clone();
    bytes[64] ^= 1;
    assert!(validate_mesh(&bytes, &chunk.metadata).is_err());
    for (offset, replacement) in [
        (0, b"NOPE".to_vec()),
        (4, 2_u16.to_le_bytes().to_vec()),
        (6, 1_u16.to_le_bytes().to_vec()),
        (8, u32::MAX.to_le_bytes().to_vec()),
        (12, 0_u32.to_le_bytes().to_vec()),
        (16, u32::MAX.to_le_bytes().to_vec()),
        (20, 1_u32.to_le_bytes().to_vec()),
        (24, f64::INFINITY.to_le_bytes().to_vec()),
        (48, vec![1]),
        (64, f32::NAN.to_le_bytes().to_vec()),
        (64, 999.0_f32.to_le_bytes().to_vec()),
        (108, 0.0_f32.to_le_bytes().to_vec()),
        (136, 3_u32.to_le_bytes().to_vec()),
        (148, 2_u32.to_le_bytes().to_vec()),
    ] {
        let mut bytes = chunk.bytes.clone();
        bytes[offset..offset + replacement.len()].copy_from_slice(&replacement);
        let descriptor = rehash_mesh(&chunk.metadata, &bytes);
        assert!(
            validate_mesh(&bytes, &descriptor).is_err(),
            "accepted corruption at {offset}"
        );
    }
    let mut metadata = chunk.metadata.clone();
    metadata.quantization_error_mm = f64::NAN;
    assert!(validate_mesh(&chunk.bytes, &metadata).is_err());
    metadata = chunk.metadata.clone();
    metadata.carrier_deviation_mm = 0.026;
    assert!(validate_mesh(&chunk.bytes, &metadata).is_err());
    metadata = chunk.metadata.clone();
    metadata.bounds_mm.max[0] = 1e9 - 1.0;
    assert!(validate_mesh(&chunk.bytes, &metadata).is_err());
}

#[test]
fn native_face_failures_never_publish_a_partial_mesh() {
    let make = || {
        let (_, _, d) = ids();
        MeshChunkBuilder::new(d, 1, DisplayProfile::MeshMm005V1).unwrap()
    };
    let positions = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    let normals = [[0.0, 0.0, 1.0]; 3];
    assert!(make().finish().is_err());
    for (ordinal, triangles, carrier) in [
        (1, vec![[0, 1, 2]], 0.0),
        (0, vec![], 0.0),
        (0, vec![[0, 1, 3]], 0.0),
        (0, vec![[0, 1, 2]], 0.026),
    ] {
        let mut builder = make();
        assert!(
            builder
                .push_face(ordinal, &positions, &normals, &triangles, carrier)
                .is_err()
        );
        assert!(builder.finish().is_err());
    }
    let mut builder = make();
    let bad_normals = [[0.0; 3]; 3];
    assert!(
        builder
            .push_face(0, &positions, &bad_normals, &[[0, 1, 2]], 0.0)
            .is_err()
    );
    assert!(builder.finish().is_err());
    let mut builder = make();
    let huge = [[0.0; 3], [1e12 + 0.1, 0.0, 0.0], [0.0, 1.0, 0.0]];
    assert!(
        builder
            .push_face(0, &huge, &normals, &[[0, 1, 2]], 0.0)
            .is_err()
    );
    assert!(builder.finish().is_err());
}

#[test]
fn identity_free_mesh_reports_measured_f32_displacement_and_enforces_the_profile_boundary() {
    let definition = DefinitionId::from_source_sha256(&[17; 32]);
    let make =
        || MeshChunkBuilder::new(definition.clone(), 1, DisplayProfile::MeshMm005V1).unwrap();
    let source = [
        [1e9, 0.0, 0.0],
        [1e9 + 1e6 + 0.017, 0.0, 0.0],
        [1e9, 1.0, 0.0],
    ];
    let normals = [[0.0, 0.0, 1.0]; 3];
    let mut builder = make();
    builder
        .push_face(0, &source, &normals, &[[0, 1, 2]], 0.024)
        .unwrap();
    let mut encoded = builder.finish().unwrap();
    let chunk = encoded.chunks.pop().unwrap();
    assert!(chunk.descriptor.quantization_error_mm > 0.016);
    assert!(chunk.descriptor.quantization_error_mm < 0.018);
    let session = SessionId::new();
    let artifact = ArtifactId::new(19).unwrap();
    let bytes = chunk.bytes;
    let metadata = chunk.descriptor.into_metadata(session.clone(), artifact);
    let view = validate_mesh_chunk(&bytes, &metadata, &session, artifact, &definition, 0).unwrap();
    let measured = source
        .into_iter()
        .zip(view.positions.iter())
        .map(|(native, packed)| {
            norm3(std::array::from_fn(|axis| {
                view.local_origin_mm[axis] + f64::from(packed[axis]) - native[axis]
            }))
        })
        .fold(0.0_f64, f64::max);
    assert_eq!(measured, metadata.quantization_error_mm);
    assert!(metadata.carrier_deviation_mm + measured <= MESH_SURFACE_TOLERANCE_MM);
    let mut builder = make();
    let mut outside = source;
    outside[1][0] = 1e9 + 1e6 + 0.026;
    assert!(
        builder
            .push_face(0, &outside, &normals, &[[0, 1, 2]], 0.0)
            .is_err()
    );
    assert!(builder.finish().is_err());
}

// Deliberately small analytic loops test the wire layout, not native boolean accuracy.
fn section(chunk_limit: u32) -> EncodedSection {
    let (session, artifact, definition) = ids();
    let plane = PlaneMm {
        origin_mm: [1e9, 1e9, 4.0],
        normal: [0.0, 0.0, 2.0],
    };
    let mut builder = SectionChunkBuilder::with_chunk_limit(
        session,
        artifact,
        SceneRevision(7),
        plane,
        chunk_limit,
    )
    .unwrap();
    let world = |x: f64, y: f64| [1e9 + x, 1e9 + y, 4.0];
    builder
        .push_loop(
            OccurrenceId::new(1).unwrap(),
            definition.clone(),
            false,
            &[
                world(0.0, 0.0),
                world(10.0, 0.0),
                world(10.0, 10.0),
                world(0.0, 10.0),
                world(0.0, 0.0),
            ],
        )
        .unwrap();
    builder
        .push_loop(
            OccurrenceId::new(1).unwrap(),
            definition,
            true,
            &[
                world(2.0, 2.0),
                world(2.0, 3.0),
                world(3.0, 3.0),
                world(3.0, 2.0),
                world(2.0, 2.0),
            ],
        )
        .unwrap();
    builder.finish().unwrap()
}
fn validate_section<'a>(
    bytes: &'a [u8],
    descriptor: &SectionChunkMetadata,
    summary: &SectionSummary,
) -> Result<ValidatedSectionChunk<'a>, GeometryError> {
    validate_section_chunk(
        bytes,
        descriptor,
        summary,
        &summary.session_id,
        summary.artifact_id,
        descriptor.chunk_index,
    )
}

#[test]
fn sections_pack_whole_loops_and_validate_contiguous_identity_ranges() {
    let encoded = section(192);
    assert_eq!(encoded.chunks.len(), 2);
    assert_eq!(encoded.summary.total_loop_count, 2);
    assert_eq!(encoded.summary.total_bytes, 384);
    let descriptors: Vec<_> = encoded
        .chunks
        .iter()
        .map(|chunk| chunk.metadata.clone())
        .collect();
    validate_section_manifest(&encoded.summary, &descriptors, &encoded.loops).unwrap();
    for (index, chunk) in encoded.chunks.iter().enumerate() {
        let decoded = validate_section(&chunk.bytes, &chunk.metadata, &encoded.summary).unwrap();
        assert_eq!(decoded.loop_offsets.iter().collect::<Vec<_>>(), [0, 5]);
        assert_eq!(decoded.points_relative_mm.len(), 5);
        assert_eq!(decoded.first_loop_ordinal, index as u32);
        assert_eq!(
            decoded.points_relative_mm.bytes().as_ptr(),
            chunk.bytes[72..].as_ptr()
        );
        validate_section_loop_metadata(
            &decoded,
            &encoded.summary,
            &encoded.loops[index..index + 1],
        )
        .unwrap();
        let mut wrong = encoded.loops[index].clone();
        wrong.is_hole = !wrong.is_hole;
        assert!(validate_section_loop_metadata(&decoded, &encoded.summary, &[wrong]).is_err());
    }
    let mut swapped = descriptors.clone();
    swapped.swap(0, 1);
    assert!(validate_section_manifest(&encoded.summary, &swapped, &encoded.loops).is_err());
    let mut overlapping = descriptors.clone();
    overlapping[1].first_loop_ordinal = 0;
    assert!(validate_section_manifest(&encoded.summary, &overlapping, &encoded.loops).is_err());
    let mut wrong_counts = encoded.loops.clone();
    wrong_counts[0].point_count += 1;
    assert!(validate_section_manifest(&encoded.summary, &descriptors, &wrong_counts).is_err());
    let mut impossible = encoded.summary.clone();
    impossible.total_bytes = 1;
    assert!(validate_section_summary(&impossible).is_err());
    let encoded = section(MAX_GEOMETRY_CHUNK_BYTES);
    assert_eq!(encoded.chunks.len(), 1);
    // Three offsets occupy twelve bytes; padding at 76..80 is explicitly zero.
    assert_eq!(&encoded.chunks[0].bytes[76..80], &[0; 4]);
}

#[test]
fn section_rejects_every_truncation_bad_offsets_nonfinite_points_and_padding() {
    let encoded = section(MAX_GEOMETRY_CHUNK_BYTES);
    let chunk = &encoded.chunks[0];
    for length in 0..chunk.bytes.len() {
        assert!(
            validate_section(&chunk.bytes[..length], &chunk.metadata, &encoded.summary).is_err()
        );
    }
    for (offset, replacement) in [
        (0, b"NOPE".to_vec()),
        (4, 2_u16.to_le_bytes().to_vec()),
        (6, 1_u16.to_le_bytes().to_vec()),
        (8, u32::MAX.to_le_bytes().to_vec()),
        (12, u32::MAX.to_le_bytes().to_vec()),
        (16, f64::NAN.to_le_bytes().to_vec()),
        (40, 1_u32.to_le_bytes().to_vec()),
        (44, 1_u32.to_le_bytes().to_vec()),
        (48, vec![1]),
        (64, 1_u32.to_le_bytes().to_vec()),
        (68, 3_u32.to_le_bytes().to_vec()),
        (72, 9_u32.to_le_bytes().to_vec()),
        (76, vec![1]),
        (80, f64::INFINITY.to_le_bytes().to_vec()),
        (96, 1.0_f64.to_le_bytes().to_vec()),
        (176, 1.0_f64.to_le_bytes().to_vec()),
    ] {
        let mut bytes = chunk.bytes.clone();
        bytes[offset..offset + replacement.len()].copy_from_slice(&replacement);
        let mut descriptor = chunk.metadata.clone();
        descriptor.sha256 = SourceHash::from_bytes(&bytes);
        assert!(
            validate_section(&bytes, &descriptor, &encoded.summary).is_err(),
            "accepted corruption at {offset}"
        );
    }
    let mut bytes = chunk.bytes.clone();
    bytes[80] ^= 1;
    assert!(validate_section(&bytes, &chunk.metadata, &encoded.summary).is_err());
    let mut summary = encoded.summary.clone();
    summary.frame.x_axis = [1.0; 3];
    assert!(validate_section(&chunk.bytes, &chunk.metadata, &summary).is_err());
    assert!(
        validate_section_chunk(
            &chunk.bytes,
            &chunk.metadata,
            &encoded.summary,
            &SessionId::new(),
            encoded.summary.artifact_id,
            0
        )
        .is_err()
    );
    let mut summary = encoded.summary.clone();
    summary.sampling_tolerance_mm = 0.01;
    assert!(validate_section_summary(&summary).is_err());
}

#[test]
fn section_empty_closed_loop_and_resource_rules_are_explicit() {
    let make = |cap| {
        let (s, a, _) = ids();
        SectionChunkBuilder::with_chunk_limit(
            s,
            a,
            SceneRevision::ZERO,
            PlaneMm {
                origin_mm: [0.0; 3],
                normal: [0.0, 0.0, 1.0],
            },
            cap,
        )
        .unwrap()
    };
    let empty = make(MAX_GEOMETRY_CHUNK_BYTES).finish().unwrap();
    assert!(empty.chunks.is_empty());
    assert!(empty.loops.is_empty());
    validate_section_manifest(&empty.summary, &[], &[]).unwrap();
    let (_, _, definition) = ids();
    let occurrence = OccurrenceId::new(1).unwrap();
    for points in [
        vec![[0.0; 3]; 3],
        vec![[0.0; 3]; 4],
        vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ],
        vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 1.0],
            [0.0, 0.0, 0.0],
        ],
    ] {
        let mut builder = make(MAX_GEOMETRY_CHUNK_BYTES);
        assert!(
            builder
                .push_loop(occurrence, definition.clone(), false, &points)
                .is_err()
        );
        assert!(builder.finish().is_err());
    }
    let too_many = vec![[0.0; 3]; MAX_LOOP_POINTS as usize + 1];
    let mut builder = make(MAX_GEOMETRY_CHUNK_BYTES);
    assert_eq!(
        builder
            .push_loop(occurrence, definition.clone(), false, &too_many)
            .unwrap_err()
            .code,
        GeometryErrorCode::ResourceLimit
    );
    assert!(builder.finish().is_err());
    let square = [
        [0.0; 3],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0; 3],
    ];
    let mut builder = make(168);
    assert_eq!(
        builder
            .push_loop(occurrence, definition.clone(), false, &square)
            .unwrap_err()
            .code,
        GeometryErrorCode::ResourceLimit
    );
    assert!(builder.finish().is_err());
    let mut builder = make(MAX_GEOMETRY_CHUNK_BYTES);
    assert!(
        builder
            .push_loop(occurrence, definition, true, &square)
            .is_err()
    );
    assert!(builder.finish().is_err());
}

fn fixture_bytes(value: &serde_json::Value) -> Vec<u8> {
    let hex = value.as_str().unwrap();
    (0..hex.len())
        .step_by(2)
        .map(|offset| u8::from_str_radix(&hex[offset..offset + 2], 16).unwrap())
        .collect()
}

#[test]
fn native_mesh_encoder_matches_original_algebraic_cross_language_golden() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../../../fixtures/protocol/mesh.json")).unwrap();
    let session = SessionId::parse("550e8400-e29b-41d4-a716-446655440000").unwrap();
    let definition = DefinitionId::from_source_sha256(&[7; 32]);
    let mut builder =
        MeshChunkBuilder::new(definition.clone(), 2, DisplayProfile::MeshMm005V1).unwrap();
    builder
        .push_face(
            0,
            &[[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]],
            &[[0.0, 0.0, 1.0]; 3],
            &[[0, 1, 2]],
            0.0,
        )
        .unwrap();
    builder
        .push_face(
            1,
            &[[0.0, 0.0, 1.0], [2.0, 0.0, 1.0], [0.0, 2.0, 1.0]],
            &[[0.0, 0.0, -1.0]; 3],
            &[[0, 2, 1]],
            0.0,
        )
        .unwrap();
    let encoded = builder.finish().unwrap();
    assert_eq!(fixture["face_table"][0]["face_id"], "step:9007199254740993");
    assert_eq!(
        fixture["chunks"].as_array().unwrap().len(),
        encoded.chunks.len()
    );
    for (value, chunk) in fixture["chunks"]
        .as_array()
        .unwrap()
        .iter()
        .zip(&encoded.chunks)
    {
        let bytes = fixture_bytes(&value["data"]);
        let descriptor: MeshChunkMetadata =
            serde_json::from_value(value["metadata"].clone()).unwrap();
        assert_eq!(bytes, chunk.bytes);
        assert_eq!(
            descriptor,
            chunk
                .descriptor
                .clone()
                .into_metadata(session.clone(), ArtifactId::new(1).unwrap())
        );
        validate_mesh_chunk(
            &bytes,
            &descriptor,
            &session,
            descriptor.artifact_id,
            &definition,
            descriptor.chunk_index,
        )
        .unwrap();
    }
}

#[test]
fn native_section_encoder_matches_original_algebraic_single_and_multichunk_goldens() {
    // These establish exact encoding only, not a successful native boolean section.
    for (source, artifact, cap) in [
        (
            include_str!("../../../../fixtures/protocol/section.json"),
            2,
            MAX_GEOMETRY_CHUNK_BYTES,
        ),
        (
            include_str!("../../../../fixtures/protocol/section-multichunk.json"),
            3,
            192,
        ),
    ] {
        let fixture: serde_json::Value = serde_json::from_str(source).unwrap();
        let summary: SectionSummary = serde_json::from_value(fixture["summary"].clone()).unwrap();
        let loops: Vec<SectionLoopMetadata> =
            serde_json::from_value(fixture["loops"].clone()).unwrap();
        let definition = DefinitionId::from_source_sha256(&[7; 32]);
        let mut builder = SectionChunkBuilder::with_chunk_limit(
            summary.session_id.clone(),
            ArtifactId::new(artifact).unwrap(),
            SceneRevision(7),
            PlaneMm {
                origin_mm: [0.0, 0.0, 4.0],
                normal: [0.0, 0.0, 1.0],
            },
            cap,
        )
        .unwrap();
        builder
            .push_loop(
                OccurrenceId::new(1).unwrap(),
                definition.clone(),
                false,
                &[
                    [0.0, 0.0, 4.0],
                    [20.0, 0.0, 4.0],
                    [20.0, 10.0, 4.0],
                    [0.0, 10.0, 4.0],
                    [0.0, 0.0, 4.0],
                ],
            )
            .unwrap();
        builder
            .push_loop(
                OccurrenceId::new(1).unwrap(),
                definition,
                true,
                &[
                    [2.0, 2.0, 4.0],
                    [2.0, 4.0, 4.0],
                    [4.0, 4.0, 4.0],
                    [4.0, 2.0, 4.0],
                    [2.0, 2.0, 4.0],
                ],
            )
            .unwrap();
        let encoded = builder.finish().unwrap();
        assert_eq!(summary, encoded.summary);
        assert_eq!(loops, encoded.loops);
        assert_eq!(
            fixture["chunks"].as_array().unwrap().len(),
            encoded.chunks.len()
        );
        let mut descriptors = Vec::new();
        for (value, chunk) in fixture["chunks"]
            .as_array()
            .unwrap()
            .iter()
            .zip(&encoded.chunks)
        {
            let bytes = fixture_bytes(&value["data"]);
            let descriptor: SectionChunkMetadata =
                serde_json::from_value(value["metadata"].clone()).unwrap();
            assert_eq!(bytes, chunk.bytes);
            assert_eq!(descriptor, chunk.metadata);
            let validated = validate_section_chunk(
                &bytes,
                &descriptor,
                &summary,
                &summary.session_id,
                summary.artifact_id,
                descriptor.chunk_index,
            )
            .unwrap();
            let first = descriptor.first_loop_ordinal as usize;
            let end = first + descriptor.loop_count as usize;
            validate_section_loop_metadata(&validated, &summary, &loops[first..end]).unwrap();
            descriptors.push(descriptor);
        }
        validate_section_manifest(&summary, &descriptors, &loops).unwrap();
    }
}

#[test]
fn a_later_failed_face_or_loop_cannot_publish_the_already_encoded_prefix() {
    let (session, artifact, definition) = ids();
    let mut mesh =
        MeshChunkBuilder::with_chunk_limit(definition.clone(), 2, DisplayProfile::MeshMm005V1, 152)
            .unwrap();
    let positions = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    mesh.push_face(0, &positions, &[[0.0, 0.0, 1.0]; 3], &[[0, 1, 2]], 0.0)
        .unwrap();
    // Triangle one flushes the valid first chunk; a vertex of the second face then
    // exceeds the quantization profile after remapping has begun.
    let positions = [[0.0, 0.0, 0.0], [1e12 + 0.1, 0.0, 0.0], [0.0, 1.0, 0.0]];
    assert!(
        mesh.push_face(1, &positions, &[[0.0, 0.0, 1.0]; 3], &[[0, 1, 2]], 0.0)
            .is_err()
    );
    assert!(mesh.finish().is_err());
    let mut section = SectionChunkBuilder::with_chunk_limit(
        session,
        artifact,
        SceneRevision::ZERO,
        PlaneMm {
            origin_mm: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
        },
        192,
    )
    .unwrap();
    let square = [
        [0.0; 3],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0; 3],
    ];
    section
        .push_loop(
            OccurrenceId::new(1).unwrap(),
            definition.clone(),
            false,
            &square,
        )
        .unwrap();
    section
        .push_loop(
            OccurrenceId::new(2).unwrap(),
            definition.clone(),
            false,
            &square,
        )
        .unwrap();
    // Two valid occurrences have staged an encoded prefix. Oversized loop admission
    // must invalidate the entire artifact, not silently retain only those occurrences.
    let too_many = vec![[0.0; 3]; MAX_LOOP_POINTS as usize + 1];
    assert_eq!(
        section
            .push_loop(OccurrenceId::new(3).unwrap(), definition, false, &too_many)
            .unwrap_err()
            .code,
        GeometryErrorCode::ResourceLimit
    );
    assert!(section.finish().is_err());
}
