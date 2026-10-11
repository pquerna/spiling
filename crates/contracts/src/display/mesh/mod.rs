// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::geometry::*;
use std::collections::HashMap;

/// Identity-free producer description. The engine attaches its own session/artifact
/// handles after native work succeeds; attachment changes no packed mesh bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshChunkDescriptor {
    pub definition_id: DefinitionId,
    pub schema_version: u16,
    pub mesh_profile: DisplayProfile,
    pub chunk_index: u32,
    pub chunk_count: u32,
    pub byte_count: u32,
    pub sha256: SourceHash,
    pub face_count: u32,
    pub bounds_mm: AabbMm,
    pub carrier_deviation_mm: f64,
    pub quantization_error_mm: f64,
}
impl MeshChunkDescriptor {
    pub fn into_metadata(
        self,
        session_id: SessionId,
        artifact_id: ArtifactId,
    ) -> MeshChunkMetadata {
        MeshChunkMetadata {
            session_id,
            artifact_id,
            definition_id: self.definition_id,
            schema_version: self.schema_version,
            mesh_profile: self.mesh_profile,
            chunk_index: self.chunk_index,
            chunk_count: self.chunk_count,
            byte_count: self.byte_count,
            sha256: self.sha256,
            face_count: self.face_count,
            bounds_mm: self.bounds_mm,
            carrier_deviation_mm: self.carrier_deviation_mm,
            quantization_error_mm: self.quantization_error_mm,
        }
    }
}

#[derive(Debug)]
pub struct EncodedMeshChunk {
    pub descriptor: MeshChunkDescriptor,
    pub bytes: Vec<u8>,
}
#[derive(Debug)]
pub struct EncodedMesh {
    pub chunks: Vec<EncodedMeshChunk>,
    pub vertex_count: u32,
    pub triangle_count: u32,
    pub total_bytes: u32,
}

/// Remaining consumer-owned scene capacity; it may only narrow frozen support caps.
#[derive(Debug, Clone, Copy)]
pub struct MeshBudget {
    pub vertices: u32,
    pub triangles: u32,
    pub bytes: u32,
}
impl MeshBudget {
    pub const FROZEN: Self = Self {
        vertices: MAX_DISPLAY_VERTICES,
        triangles: MAX_DISPLAY_TRIANGLES,
        bytes: MAX_SCENE_MESH_BYTES,
    };
    pub fn validate(self) -> Result<(), GeometryError> {
        if self.vertices == 0
            || self.vertices > MAX_DISPLAY_VERTICES
            || self.triangles == 0
            || self.triangles > MAX_DISPLAY_TRIANGLES
            || self.bytes < 152
            || self.bytes > MAX_SCENE_MESH_BYTES
        {
            return Err(resource("remaining mesh capacity outside support limits"));
        }
        Ok(())
    }
}

/// Feed one strict native face at a time in face-table order. Only one bounded remapped
/// chunk is held in addition to the final encoded chunks; no complete flattened mesh.
/// An error poisons the builder: callers must discard it, never publish a partial face.
pub struct MeshChunkBuilder {
    definition: DefinitionId,
    face_count: u32,
    profile: DisplayProfile,
    next_face: u32,
    failed: bool,
    chunk_limit: usize,
    budget: MeshBudget,
    chunks: Vec<EncodedMeshChunk>,
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    indices: Vec<u32>,
    faces: Vec<u32>,
    remap: HashMap<u32, u32>,
    origin: [f64; 3],
    bounds: Option<AabbMm>,
    carrier: f64,
    quantization: f64,
    vertex_count: u32,
    triangle_count: u32,
    total_bytes: u32,
}
impl MeshChunkBuilder {
    pub fn new(
        definition: DefinitionId,
        face_count: u32,
        profile: DisplayProfile,
    ) -> Result<Self, GeometryError> {
        Self::with_chunk_limit(definition, face_count, profile, MAX_GEOMETRY_CHUNK_BYTES)
    }
    /// Smaller caps are useful for interoperability fixtures; the support cap cannot be raised.
    pub fn with_chunk_limit(
        definition: DefinitionId,
        face_count: u32,
        profile: DisplayProfile,
        chunk_limit: u32,
    ) -> Result<Self, GeometryError> {
        Self::with_limits(
            definition,
            face_count,
            profile,
            chunk_limit,
            MeshBudget::FROZEN,
        )
    }
    /// Enforce remaining scene capacity during packing, not only before publication.
    pub fn with_budget(
        definition: DefinitionId,
        face_count: u32,
        profile: DisplayProfile,
        budget: MeshBudget,
    ) -> Result<Self, GeometryError> {
        Self::with_limits(
            definition,
            face_count,
            profile,
            MAX_GEOMETRY_CHUNK_BYTES,
            budget,
        )
    }
    fn with_limits(
        definition: DefinitionId,
        face_count: u32,
        profile: DisplayProfile,
        chunk_limit: u32,
        budget: MeshBudget,
    ) -> Result<Self, GeometryError> {
        budget.validate()?;
        if face_count == 0 || face_count > MAX_NATIVE_FACES {
            return Err(resource("mesh face count outside support limits"));
        }
        if !(152..=MAX_GEOMETRY_CHUNK_BYTES).contains(&chunk_limit) {
            return Err(resource("invalid mesh chunk cap"));
        }
        Ok(Self {
            definition,
            face_count,
            profile,
            next_face: 0,
            failed: false,
            chunk_limit: chunk_limit as usize,
            budget,
            chunks: Vec::new(),
            positions: Vec::new(),
            normals: Vec::new(),
            indices: Vec::new(),
            faces: Vec::new(),
            remap: HashMap::new(),
            origin: [0.0; 3],
            bounds: None,
            carrier: 0.0,
            quantization: 0.0,
            vertex_count: 0,
            triangle_count: 0,
            total_bytes: 0,
        })
    }
    pub fn remaining_budget(&self) -> MeshBudget {
        MeshBudget {
            vertices: self.budget.vertices - self.vertex_count,
            triangles: self.budget.triangles - self.triangle_count,
            bytes: self.budget.bytes - self.total_bytes,
        }
    }
    pub fn push_face(
        &mut self,
        ordinal: u32,
        positions: &[[f64; 3]],
        normals: &[[f64; 3]],
        triangles: &[[u32; 3]],
        carrier_deviation_mm: f64,
    ) -> Result<(), GeometryError> {
        if self.failed {
            return Err(invalid("mesh builder was invalidated"));
        }
        let result =
            self.push_face_inner(ordinal, positions, normals, triangles, carrier_deviation_mm);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn push_face_inner(
        &mut self,
        ordinal: u32,
        positions: &[[f64; 3]],
        normals: &[[f64; 3]],
        triangles: &[[u32; 3]],
        carrier: f64,
    ) -> Result<(), GeometryError> {
        if ordinal != self.next_face || ordinal >= self.face_count {
            return Err(invalid(
                "mesh faces must be complete and in face-table order",
            ));
        }
        if positions.is_empty() || normals.len() != positions.len() || triangles.is_empty() {
            return Err(invalid("strict meshing dropped or malformed a face"));
        }
        if positions.len() > self.budget.vertices as usize
            || triangles.len() > self.budget.triangles as usize
        {
            return Err(resource("per-face mesh count exceeds limit"));
        }
        if !carrier.is_finite() || !(0.0..=MESH_CARRIER_TOLERANCE_MM).contains(&carrier) {
            return Err(invalid("carrier deviation exceeds profile"));
        }
        for (&point, &normal) in positions.iter().zip(normals) {
            if !finite3(point)
                || !finite3(normal)
                || (norm3(normal) - 1.0).abs() > NORMAL_NORM_TOLERANCE
            {
                return Err(invalid("invalid native mesh vertex or normal"));
            }
        }
        for triangle in triangles {
            if triangle
                .iter()
                .any(|&index| index as usize >= positions.len())
            {
                return Err(invalid("native mesh index out of range"));
            }
        }
        self.remap.clear();
        for triangle in triangles {
            let mut missing = 0;
            for (slot, &index) in triangle.iter().enumerate() {
                if !self.remap.contains_key(&index) && !triangle[..slot].contains(&index) {
                    missing += 1;
                }
            }
            let required = MESH_HEADER_BYTES
                + (self.positions.len() + missing) * 24
                + (self.faces.len() + 1) * 16;
            if required > self.chunk_limit {
                self.flush()?;
            }
            if self.positions.is_empty() {
                self.origin = positions[triangle[0] as usize];
            }
            let mut mapped = [0; 3];
            for (slot, &index) in triangle.iter().enumerate() {
                mapped[slot] = if let Some(&mapped) = self.remap.get(&index) {
                    mapped
                } else {
                    if self.vertex_count == self.budget.vertices {
                        return Err(resource("unique mesh vertex budget exceeded"));
                    }
                    let point = positions[index as usize];
                    let local = std::array::from_fn::<_, 3, _>(|axis| {
                        (point[axis] - self.origin[axis]) as f32
                    });
                    let reconstructed = std::array::from_fn::<_, 3, _>(|axis| {
                        self.origin[axis] + f64::from(local[axis])
                    });
                    let error = norm3(std::array::from_fn(|axis| {
                        reconstructed[axis] - point[axis]
                    }));
                    if local.iter().any(|v| !v.is_finite())
                        || !finite3(reconstructed)
                        || !error.is_finite()
                        || error > MESH_QUANTIZATION_TOLERANCE_MM
                        || carrier + error > MESH_SURFACE_TOLERANCE_MM
                    {
                        return Err(invalid("mesh quantization exceeds profile"));
                    }
                    let normal = normals[index as usize].map(|v| v as f32);
                    if normal.iter().any(|v| !v.is_finite())
                        || (norm3(normal.map(f64::from)) - 1.0).abs() > NORMAL_NORM_TOLERANCE
                    {
                        return Err(invalid("packed mesh normal exceeds norm tolerance"));
                    }
                    let mapped = self.positions.len() as u32;
                    self.positions.push(local);
                    self.normals.push(normal);
                    self.remap.insert(index, mapped);
                    if let Some(bounds) = &mut self.bounds {
                        bounds.include(point)?;
                    } else {
                        self.bounds = Some(AabbMm::new(point, point)?);
                    }
                    self.quantization = self.quantization.max(error);
                    self.vertex_count += 1;
                    mapped
                };
            }
            if self.triangle_count == self.budget.triangles {
                return Err(resource("unique mesh triangle budget exceeded"));
            }
            self.carrier = self.carrier.max(carrier);
            if self.carrier + self.quantization > MESH_SURFACE_TOLERANCE_MM {
                return Err(invalid("combined chunk error exceeds profile"));
            }
            self.indices.extend_from_slice(&mapped);
            self.faces.push(ordinal);
            self.triangle_count += 1;
            let pending = MESH_HEADER_BYTES + self.positions.len() * 24 + self.faces.len() * 16;
            if self.total_bytes as usize + pending > self.budget.bytes as usize {
                return Err(resource("mesh byte budget exceeded"));
            }
        }
        self.next_face += 1;
        Ok(())
    }
    fn flush(&mut self) -> Result<(), GeometryError> {
        if self.faces.is_empty() {
            return Ok(());
        }
        let length = MESH_HEADER_BYTES + self.positions.len() * 24 + self.faces.len() * 16;
        if length > self.chunk_limit
            || self.total_bytes as usize + length > self.budget.bytes as usize
        {
            return Err(resource("mesh chunk or artifact exceeds budget"));
        }
        let mut bytes = Vec::with_capacity(length);
        bytes.resize(MESH_HEADER_BYTES, 0);
        bytes[..4].copy_from_slice(b"SPLM");
        bytes[4..6].copy_from_slice(&MESH_SCHEMA_VERSION.to_le_bytes());
        put_u32(&mut bytes, 8, self.positions.len() as u32);
        put_u32(&mut bytes, 12, self.indices.len() as u32);
        put_u32(&mut bytes, 16, self.faces.len() as u32);
        put_u32(&mut bytes, 20, self.chunks.len() as u32);
        for axis in 0..3 {
            put_f64(&mut bytes, 24 + axis * 8, self.origin[axis]);
        }
        for values in [&self.positions, &self.normals] {
            for vector in values {
                for value in vector {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
            }
        }
        for values in [&self.indices, &self.faces] {
            for value in values {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        let descriptor = MeshChunkDescriptor {
            definition_id: self.definition.clone(),
            schema_version: MESH_SCHEMA_VERSION,
            mesh_profile: self.profile,
            chunk_index: self.chunks.len() as u32,
            chunk_count: 0,
            byte_count: length as u32,
            sha256: SourceHash::from_bytes(&bytes),
            face_count: self.face_count,
            bounds_mm: self.bounds.take().expect("nonempty chunk"),
            carrier_deviation_mm: self.carrier,
            quantization_error_mm: self.quantization,
        };
        self.total_bytes += length as u32;
        self.chunks.push(EncodedMeshChunk { descriptor, bytes });
        self.positions.clear();
        self.normals.clear();
        self.indices.clear();
        self.faces.clear();
        self.remap.clear();
        self.carrier = 0.0;
        self.quantization = 0.0;
        Ok(())
    }
    pub fn finish(mut self) -> Result<EncodedMesh, GeometryError> {
        if self.failed || self.next_face != self.face_count {
            return Err(invalid("mesh artifact has absent or invalid faces"));
        }
        self.flush()?;
        let count = self.chunks.len() as u32;
        for chunk in &mut self.chunks {
            chunk.descriptor.chunk_count = count;
        }
        Ok(EncodedMesh {
            chunks: self.chunks,
            vertex_count: self.vertex_count,
            triangle_count: self.triangle_count,
            total_bytes: self.total_bytes,
        })
    }
}

#[derive(Debug)]
pub struct ValidatedMeshChunk<'a> {
    pub local_origin_mm: [f64; 3],
    pub chunk_index: u32,
    pub positions: F32x3View<'a>,
    pub normals: F32x3View<'a>,
    pub indices: U32View<'a>,
    pub face_ordinals: U32View<'a>,
}
/// Validate identity, hash, sizes and every scalar before returning any borrowed views.
pub fn validate_mesh_chunk<'a>(
    bytes: &'a [u8],
    metadata: &MeshChunkMetadata,
    session: &SessionId,
    artifact: ArtifactId,
    definition: &DefinitionId,
    chunk_index: u32,
) -> Result<ValidatedMeshChunk<'a>, GeometryError> {
    if &metadata.session_id != session
        || metadata.artifact_id != artifact
        || &metadata.definition_id != definition
        || metadata.chunk_index != chunk_index
    {
        return Err(GeometryError::new(
            GeometryErrorCode::UnknownHandle,
            "mesh descriptor identity mismatch",
        ));
    }
    if metadata.schema_version != MESH_SCHEMA_VERSION
        || metadata.chunk_count == 0
        || chunk_index >= metadata.chunk_count
        || metadata.face_count == 0
        || metadata.face_count > MAX_NATIVE_FACES
    {
        return Err(invalid("invalid mesh descriptor counts or version"));
    }
    metadata.bounds_mm.validate()?;
    let carrier = metadata.carrier_deviation_mm;
    let quantization = metadata.quantization_error_mm;
    if !carrier.is_finite()
        || !quantization.is_finite()
        || !(0.0..=MESH_CARRIER_TOLERANCE_MM).contains(&carrier)
        || !(0.0..=MESH_QUANTIZATION_TOLERANCE_MM).contains(&quantization)
        || carrier + quantization > MESH_SURFACE_TOLERANCE_MM
    {
        return Err(invalid("invalid mesh error budget"));
    }
    if bytes.len() < MESH_HEADER_BYTES
        || bytes.len() > MAX_GEOMETRY_CHUNK_BYTES as usize
        || bytes.len() != metadata.byte_count as usize
    {
        return Err(invalid("mesh byte count mismatch"));
    }
    if &bytes[..4] != b"SPLM"
        || u16_at(bytes, 4) != MESH_SCHEMA_VERSION
        || bytes[6..8].iter().chain(&bytes[48..64]).any(|&v| v != 0)
    {
        return Err(invalid("mesh magic, schema or reserved bytes invalid"));
    }
    let vertices = u32_at(bytes, 8) as usize;
    let indices = u32_at(bytes, 12) as usize;
    let triangles = u32_at(bytes, 16) as usize;
    if vertices == 0
        || triangles == 0
        || vertices > MAX_DISPLAY_VERTICES as usize
        || triangles > MAX_DISPLAY_TRIANGLES as usize
        || triangles.checked_mul(3) != Some(indices)
        || u32_at(bytes, 20) != chunk_index
    {
        return Err(invalid("mesh header counts or index invalid"));
    }
    let normals_start = checked_size(MESH_HEADER_BYTES, vertices, 12)?;
    let indices_start = checked_size(normals_start, vertices, 12)?;
    let faces_start = checked_size(indices_start, indices, 4)?;
    let end = checked_size(faces_start, triangles, 4)?;
    if bytes.len() != end {
        return Err(invalid("mesh exact layout length mismatch"));
    }
    if !metadata.sha256.matches_bytes(bytes) {
        return Err(invalid("mesh SHA-256 mismatch"));
    }
    let origin = std::array::from_fn(|axis| f64_at(bytes, 24 + axis * 8));
    if !finite3(origin) {
        return Err(invalid("nonfinite mesh origin"));
    }
    let positions = F32x3View::new(&bytes[MESH_HEADER_BYTES..normals_start]);
    let normals = F32x3View::new(&bytes[normals_start..indices_start]);
    let indices = U32View::new(&bytes[indices_start..faces_start]);
    let faces = U32View::new(&bytes[faces_start..end]);
    for point in positions.iter() {
        for axis in 0..3 {
            let reconstructed = origin[axis] + f64::from(point[axis]);
            if !point[axis].is_finite()
                || !reconstructed.is_finite()
                || reconstructed < metadata.bounds_mm.min[axis] - quantization - 1e-9
                || reconstructed > metadata.bounds_mm.max[axis] + quantization + 1e-9
            {
                return Err(invalid("mesh point outside finite advertised bounds"));
            }
        }
    }
    for normal in normals.iter() {
        if normal.iter().any(|v| !v.is_finite())
            || (norm3(normal.map(f64::from)) - 1.0).abs() > NORMAL_NORM_TOLERANCE
        {
            return Err(invalid("invalid mesh normal"));
        }
    }
    if indices.iter().any(|index| index as usize >= vertices)
        || faces.iter().any(|ordinal| ordinal >= metadata.face_count)
    {
        return Err(invalid("mesh vertex index or face ordinal out of range"));
    }
    Ok(ValidatedMeshChunk {
        local_origin_mm: origin,
        chunk_index,
        positions,
        normals,
        indices,
        face_ordinals: faces,
    })
}

/// Check complete descriptor order and aggregate budgets without reading/concatenating bodies.
pub fn validate_mesh_manifest(metadata: &[MeshChunkMetadata]) -> Result<(), GeometryError> {
    let first = metadata
        .first()
        .ok_or_else(|| invalid("mesh has no chunks"))?;
    let mut bytes = 0_u32;
    for (index, chunk) in metadata.iter().enumerate() {
        if chunk.chunk_index as usize != index
            || chunk.chunk_count as usize != metadata.len()
            || chunk.session_id != first.session_id
            || chunk.artifact_id != first.artifact_id
            || chunk.definition_id != first.definition_id
            || chunk.face_count != first.face_count
            || chunk.schema_version != MESH_SCHEMA_VERSION
            || chunk.mesh_profile != first.mesh_profile
            || chunk.byte_count < MESH_HEADER_BYTES as u32
            || chunk.byte_count > MAX_GEOMETRY_CHUNK_BYTES
        {
            return Err(invalid("mesh manifest is inconsistent or reordered"));
        }
        bytes = bytes
            .checked_add(chunk.byte_count)
            .ok_or_else(|| resource("mesh byte count overflow"))?;
    }
    if bytes > MAX_SCENE_MESH_BYTES {
        return Err(resource("mesh manifest exceeds byte budget"));
    }
    Ok(())
}
