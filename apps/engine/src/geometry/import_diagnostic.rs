// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Rejected-source display is a leased session artifact, never a DefinitionState.
use super::*;
use spiling_contracts::display::{EncodedMesh, MeshBudget, MeshChunkBuilder};

pub(super) fn encode(
    mesh: Option<&spiling_geometry::source::RejectedMesh>,
    source: &[u8],
    budget: MeshBudget,
    cancel: &AtomicBool,
) -> Option<EncodedMesh> {
    let mesh = mesh?;
    if cancel.load(Ordering::Acquire) || mesh.validate().is_err() {
        return None;
    }
    let id = DefinitionId::for_source(&<sha2::Sha256 as sha2::Digest>::digest(source).into(), None);
    let mut builder =
        MeshChunkBuilder::with_budget(id, 1, DisplayProfile::MeshTrimmedMm005V2, budget).ok()?;
    if mesh.vertices.len() > budget.vertices as usize
        || mesh.triangles.len() > budget.triangles as usize
    {
        return None;
    }
    let mut normals = Vec::new();
    normals.try_reserve_exact(mesh.vertices.len()).ok()?;
    normals.resize(mesh.vertices.len(), [0.; 3]);
    let mut triangles = Vec::new();
    triangles.try_reserve_exact(mesh.triangles.len()).ok()?;
    for (i, t) in mesh.triangles.iter().enumerate() {
        if i % 1024 == 0 && cancel.load(Ordering::Acquire) {
            return None;
        }
        let p = t.map(|v| mesh.vertices[v as usize]);
        let u = std::array::from_fn::<_, 3, _>(|a| p[1][a] - p[0][a]);
        let v = std::array::from_fn::<_, 3, _>(|a| p[2][a] - p[0][a]);
        let n = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let length = n.iter().map(|v| v * v).sum::<f64>().sqrt();
        if !length.is_finite() || length == 0. {
            continue;
        } // Only the diagnostic display omits undrawable facets.
        for &id in t {
            for a in 0..3 {
                normals[id as usize][a] += n[a] / length;
            }
        }
        triangles.push(*t);
    }
    for n in &mut normals {
        let len = n.iter().map(|v| v * v).sum::<f64>().sqrt();
        *n = if len > 0. {
            n.map(|v| v / len)
        } else {
            [0., 0., 1.]
        };
    }
    builder
        .push_face(0, &mesh.vertices, &normals, &triangles, 0., 0.)
        .ok()?;
    builder.finish().ok()
}
impl Runtime {
    pub(super) fn clear_import_preview(&mut self) {
        if let Some((id, _, _)) = self.rejected_import_artifact.take() {
            self.artifacts.remove(&id);
            self.released.push_back(id);
            if self.released.len() > MAX_TERMINAL_JOBS as usize {
                self.released.pop_front();
            }
        }
    }
    pub(super) fn attach_import_preview(
        &mut self,
        error: &mut GeometryError,
        mesh: EncodedMesh,
    ) -> Result<(), String> {
        self.clear_import_preview();
        if mesh.total_bytes > MAX_ENGINE_ARTIFACT_BYTES.saturating_sub(self.artifact_bytes()) {
            return Ok(());
        }
        let Some(first) = mesh.chunks.first() else {
            return Ok(());
        };
        let definition_id = first.descriptor.definition_id.clone();
        let id = ArtifactId::new(
            allocate(&mut self.next_artifact, "artifact ID").map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let mut chunks = Vec::new();
        let mut bytes = Vec::new();
        for chunk in mesh.chunks {
            chunks.push(ArtifactChunkMetadata::Mesh {
                metadata: chunk.descriptor.into_metadata(self.session_id.clone(), id),
            });
            bytes.push(chunk.bytes);
        }
        self.artifacts.insert(
            id,
            Artifact {
                summary: ArtifactSummary::Mesh {
                    artifact_id: id,
                    definition_id: definition_id.clone(),
                    chunk_count: chunks.len() as u32,
                    total_bytes: mesh.total_bytes,
                },
                chunks,
                bytes,
                loops: Vec::new(),
                leased: true,
                pinned: false,
            },
        );
        self.rejected_import_artifact = Some((id, self.revision, self.project.info().revision));
        if let Some(info) = &mut error.import_diagnostic {
            info.preview = Some(id);
            info.preview_definition = Some(definition_id);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostic_mesh_accepts_open_facets_but_rejects_invalid_indices() {
        let mut mesh = spiling_geometry::source::RejectedMesh {
            vertices: vec![[0., 0., 0.], [10., 0., 0.], [0., 10., 0.]],
            triangles: vec![[0, 1, 2]],
        };
        let cancel = AtomicBool::new(false);
        let encoded = encode(Some(&mesh), b"source", MeshBudget::FROZEN, &cancel).unwrap();
        assert_eq!(encoded.triangle_count, 1);
        mesh.triangles[0][2] = 10;
        assert!(encode(Some(&mesh), b"source", MeshBudget::FROZEN, &cancel).is_none());
    }
}
