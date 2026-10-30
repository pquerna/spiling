// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use spiling_contracts::geometry::{DisplayProfile, GeometryError, PlaneMm};
use spiling_geometry::{
    Definition, MeshArtifact, NativeSection, SectionLoopMm, from_step_bytes_with_cancel, section,
    tessellate,
};
use std::sync::atomic::AtomicBool;

pub fn assert_close(actual: f64, expected: f64, tolerance: f64, context: &str) {
    let diff = (actual - expected).abs();
    assert!(
        diff <= tolerance,
        "{context}: actual {actual} differs from expected {expected} by {diff} (tolerance: {tolerance})"
    );
}

pub fn import_test_step(bytes: &[u8]) -> Result<Definition, GeometryError> {
    from_step_bytes_with_cancel(bytes, &AtomicBool::new(false))
}

pub fn section_at_z(def: &Definition, z_mm: f64) -> Result<NativeSection, GeometryError> {
    section(
        def,
        PlaneMm {
            origin_mm: [0.0, 0.0, z_mm],
            normal: [0.0, 0.0, 1.0],
        },
        &AtomicBool::new(false),
    )
}

pub fn section_midplane(def: &Definition) -> Result<NativeSection, GeometryError> {
    let bounds = def.bounds_mm();
    let z_mid = (bounds.min[2] + bounds.max[2]) * 0.5;
    section_at_z(def, z_mid)
}

pub fn tessellate_display(def: &Definition) -> Result<MeshArtifact, GeometryError> {
    tessellate(
        def,
        DisplayProfile::MeshTrimmedMm005V2,
        &AtomicBool::new(false),
    )
}

pub fn loop_area(result: &NativeSection, boundary: &SectionLoopMm) -> f64 {
    let xy = |point: [f64; 3]| {
        let delta: [f64; 3] = std::array::from_fn(|i| point[i] - result.frame().origin_mm[i]);
        let dot = |axis: [f64; 3]| delta.iter().zip(axis).map(|(x, y)| x * y).sum::<f64>();
        [dot(result.frame().x_axis), dot(result.frame().y_axis)]
    };
    boundary
        .points_mm
        .windows(2)
        .map(|pair| {
            let a = xy(pair[0]);
            let b = xy(pair[1]);
            a[0] * b[1] - a[1] * b[0]
        })
        .sum::<f64>()
        * 0.5
}

/// Validate each packed chunk before test-only scalar inspection. Source coordinates
/// include the chunk's f64 origin; production consumers retain the borrowed views.
pub fn mesh_chunks(mesh: &MeshArtifact) -> Vec<spiling_contracts::display::ValidatedMeshChunk<'_>> {
    use spiling_contracts::{display::*, geometry::*};
    let session = SessionId::new();
    let artifact = ArtifactId::new(1).unwrap();
    let metadata: Vec<_> = mesh
        .chunks
        .iter()
        .map(|chunk| {
            chunk
                .descriptor
                .clone()
                .into_metadata(session.clone(), artifact)
        })
        .collect();
    validate_mesh_manifest(&metadata).unwrap();
    mesh.chunks
        .iter()
        .zip(&metadata)
        .map(|(chunk, meta)| {
            validate_mesh_chunk(
                &chunk.bytes,
                meta,
                &session,
                artifact,
                &meta.definition_id,
                meta.chunk_index,
            )
            .unwrap()
        })
        .collect()
}

pub fn mesh_positions(mesh: &MeshArtifact) -> Vec<f64> {
    mesh_chunks(mesh)
        .into_iter()
        .flat_map(|chunk| {
            chunk.positions.iter().flat_map(move |point| {
                std::array::from_fn::<_, 3, _>(|axis| {
                    chunk.local_origin_mm[axis] + f64::from(point[axis])
                })
            })
        })
        .collect()
}

pub fn mesh_normals(mesh: &MeshArtifact) -> Vec<f64> {
    mesh_chunks(mesh)
        .into_iter()
        .flat_map(|chunk| {
            chunk
                .normals
                .iter()
                .flat_map(|normal| normal.map(f64::from))
        })
        .collect()
}

pub fn mesh_indices(mesh: &MeshArtifact) -> Vec<u32> {
    let mut base = 0;
    let mut indices = Vec::new();
    for chunk in mesh_chunks(mesh) {
        indices.extend(chunk.indices.iter().map(|index| base + index));
        base += chunk.positions.len() as u32;
    }
    indices
}
