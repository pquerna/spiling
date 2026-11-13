// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Immutable validated ownership: subsequent views borrow the same bytes without
//! rehashing or rescanning them. No unchecked constructor or mutable byte access.
use super::*;
use std::{ops::Range, sync::Arc};
#[derive(Debug, Clone)]
pub struct CheckedMeshChunk {
    bytes: Arc<[u8]>,
    metadata: MeshChunkMetadata,
    origin: [f64; 3],
    ranges: [Range<usize>; 4],
    radius_mm: f64,
}
impl CheckedMeshChunk {
    pub fn new(bytes: Arc<[u8]>, metadata: MeshChunkMetadata) -> Result<Self, GeometryError> {
        let view = validate_mesh_chunk(
            &bytes,
            &metadata,
            &metadata.session_id,
            metadata.artifact_id,
            &metadata.definition_id,
            metadata.chunk_index,
        )?;
        let sizes = [
            view.positions.bytes().len(),
            view.normals.bytes().len(),
            view.indices.bytes().len(),
            view.face_ordinals.bytes().len(),
        ];
        let mut offset = MESH_HEADER_BYTES;
        let ranges = sizes.map(|size| {
            let range = offset..offset + size;
            offset += size;
            range
        });
        let origin = view.local_origin_mm;
        let radius_mm = view
            .positions
            .iter()
            .map(|p| {
                p.into_iter()
                    .map(|v| f64::from(v).powi(2))
                    .sum::<f64>()
                    .sqrt()
            })
            .fold(0., f64::max)
            .next_up();
        Ok(Self {
            bytes,
            metadata,
            origin,
            ranges,
            radius_mm,
        })
    }
    pub fn metadata(&self) -> &MeshChunkMetadata {
        &self.metadata
    }
    pub fn bytes(&self) -> &Arc<[u8]> {
        &self.bytes
    }
    pub fn radius_mm(&self) -> f64 {
        self.radius_mm
    }
    pub fn view(&self) -> ValidatedMeshChunk<'_> {
        ValidatedMeshChunk {
            local_origin_mm: self.origin,
            chunk_index: self.metadata.chunk_index,
            positions: F32x3View::new(&self.bytes[self.ranges[0].clone()]),
            normals: F32x3View::new(&self.bytes[self.ranges[1].clone()]),
            indices: U32View::new(&self.bytes[self.ranges[2].clone()]),
            face_ordinals: U32View::new(&self.bytes[self.ranges[3].clone()]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owned_views_share_checked_bytes_and_refuse_corrupt_scalar_data() {
        let mut builder = MeshChunkBuilder::new(
            DefinitionId::from_source_sha256(&[9; 32]),
            1,
            DisplayProfile::MeshTrimmedMm005V2,
        )
        .unwrap();
        builder
            .push_face(
                0,
                &[[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
                &[[0., 0., 1.]; 3],
                &[[0, 1, 2]],
                0.,
                0.,
            )
            .unwrap();
        let chunk = builder.finish().unwrap().chunks.pop().unwrap();
        let metadata = chunk
            .descriptor
            .into_metadata(SessionId::new(), ArtifactId::new(1).unwrap());
        let checked = CheckedMeshChunk::new(chunk.bytes.into(), metadata.clone()).unwrap();
        let shared = checked.clone();
        assert!(Arc::ptr_eq(checked.bytes(), shared.bytes()));
        assert_eq!(shared.view().positions.get(1), Some([1., 0., 0.]));
        assert!(shared.radius_mm() >= 1.);
        let mut corrupt = checked.bytes().to_vec();
        let last = corrupt.len() - 4;
        corrupt[last..].copy_from_slice(&2u32.to_le_bytes());
        let mut forged = metadata;
        forged.sha256 = SourceHash::from_bytes(&corrupt); // Hash is valid; the face ordinal is not.
        assert!(CheckedMeshChunk::new(corrupt.into(), forged).is_err());
    }
}
