// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Disposable display candidates are independently audited against the borrowed
//! native coefficients and topology. Mesher tolerances and diagnostics are not
//! certificates, and this module never creates a manufacturing face domain.

mod audit;
mod trim;

use crate::{GeometryWorkBudget, MeshArtifact, check_cancel, kernel::Definition};
use solidcore_mesh::{
    EdgeOptions, OutputOptions, QueryBudget, QueryError, QueryLimit, TessellateOptions, Tolerance,
};
use spiling_contracts::display::{MeshBudget, MeshChunkBuilder};
use spiling_contracts::geometry::{
    DisplayProfile, GeometryError, GeometryErrorCode, MAX_NATIVE_FACES, MESH_CARRIER_TOLERANCE_MM,
};
use std::sync::atomic::AtomicBool;

const PROOF_BYTES: usize = 32 * 1024 * 1024;

pub(crate) fn tessellate(
    definition: &Definition,
    profile: DisplayProfile,
    cancel: &AtomicBool,
    budget: MeshBudget,
) -> Result<MeshArtifact, GeometryError> {
    check_cancel(cancel)?;
    budget.validate()?;
    let faces = definition.face_ids();
    if faces.is_empty() || faces.len() > MAX_NATIVE_FACES as usize {
        return Err(invalid(
            "native display requires a nonempty bounded face table",
        ));
    }
    if faces.len() != definition.faces().len() || faces.len() != definition.face_step_ids().len() {
        return Err(invalid("native display source-face tables disagree"));
    }
    let brep = definition.brep();
    let kernel = crate::make_kernel_context();
    let mut work = GeometryWorkBudget::new(20_000_000);
    let mut options = TessellateOptions {
        chord_tolerance: Tolerance::Absolute(MESH_CARRIER_TOLERANCE_MM * 0.25),
        max_triangles_per_face: budget.triangles as usize,
        edge: EdgeOptions {
            max_segments: budget.vertices,
            ..EdgeOptions::default()
        },
        output: OutputOptions {
            normals: true,
            uvs: true,
            face_ids: true,
            edge_polylines: true,
        },
        // Native sampled deviation is deliberately not used as authority.
        measure_deviation: false,
        ..TessellateOptions::default()
    };
    for attempt in 0..=8 {
        work.charge(1, cancel)?;
        let edges = candidate_query(0, &mut work, cancel, |native| {
            solidcore_mesh::tessellate_edge_set_with_budget(&kernel, brep, &options, native)
        })?;
        check_cancel(cancel)?;
        if let Some(failure) = edges.failures().first() {
            return Err(invalid(format!(
                "native display edge candidate failed: {}",
                failure.error
            )));
        }
        let edge_bytes = edges.retained_bytes();
        check_memory(edge_bytes)?;
        let mut builder = MeshChunkBuilder::with_budget(
            definition.id().clone(),
            faces.len() as u32,
            profile,
            budget,
        )?;
        let result = (|| {
            for (ordinal, &face) in faces.iter().enumerate() {
                work.charge(1, cancel)?;
                definition.faces()[ordinal].validate()?;
                if definition.face_ordinal(face) != Some(ordinal) {
                    return Err(invalid("native display face has ambiguous source ordinal"));
                }
                let mut candidate_options = options.clone();
                candidate_options.max_triangles_per_face =
                    builder.remaining_budget().triangles as usize;
                let occupied = edge_bytes.saturating_add(builder.retained_bytes());
                let candidate = candidate_query(occupied, &mut work, cancel, |native| {
                    solidcore_mesh::tessellate_face_with_budget(
                        &kernel,
                        brep,
                        face,
                        &edges,
                        &candidate_options,
                        native,
                    )
                })?;
                check_cancel(cancel)?;
                if candidate.face != face {
                    return Err(invalid("native mesher changed source face identity"));
                }
                let data = audit::face(
                    definition, ordinal, &candidate, &edges, builder.remaining_budget(),
                    occupied, &mut work, cancel,
                ).map_err(|cause| GeometryError::new(cause.code, format!(
                    "native display source={} face={ordinal} source_face={} attempt={attempt} work={}: {}",
                    definition.id().as_str(), definition.faces()[ordinal].face_id.as_str(),
                    work.consumed(), cause.message,
                )))?;
                builder.push_face(
                    ordinal as u32,
                    &data.positions,
                    &data.normals,
                    &candidate.triangles,
                    data.carrier_error,
                    data.trim_error,
                )?;
            }
            check_cancel(cancel)?;
            builder.finish()
        })();
        match result {
            Err(cause) if cause.code == GeometryErrorCode::ShapeErrorExceeded && attempt < 8 => {
                // A completed audit, never sampled mesher statistics, requests
                // refinement. All attempts share the same cumulative allowance.
                options.chord_tolerance =
                    Tolerance::Absolute(MESH_CARRIER_TOLERANCE_MM * 0.25_f64.powi(attempt + 2));
            }
            result => return result,
        }
    }
    unreachable!("the last candidate returns its actual audit result")
}

fn candidate_query<T>(
    outside: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    query: impl FnOnce(&QueryBudget<'_>) -> Result<T, QueryError>,
) -> Result<T, GeometryError> {
    check_memory(outside)?;
    let native = QueryBudget::new(work.remaining(), PROOF_BYTES - outside, cancel);
    let result = query(&native);
    // Debit completed/admitted native work even when candidate generation fails.
    work.charge(native.consumed(), cancel)?;
    result.map_err(|error| match error {
        QueryError::Geometry(error) => native_error(error),
        QueryError::Limit(QueryLimit::Cancelled) => GeometryError::new(
            GeometryErrorCode::Cancelled,
            "native display candidate cancelled",
        ),
        QueryError::Limit(limit) => {
            resource(format!("native display candidate allowance: {limit:?}"))
        }
    })
}

pub(super) fn check_memory(bytes: usize) -> Result<(), GeometryError> {
    if bytes > PROOF_BYTES {
        Err(resource(
            "native display retained proof workspace exceeds 32 MiB",
        ))
    } else {
        Ok(())
    }
}
pub(super) fn invalid(message: impl AsRef<str>) -> GeometryError {
    GeometryError::new(GeometryErrorCode::InvalidGeometry, message)
}
pub(super) fn resource(message: impl AsRef<str>) -> GeometryError {
    GeometryError::new(GeometryErrorCode::ResourceLimit, message)
}
pub(super) fn shape(message: impl AsRef<str>) -> GeometryError {
    GeometryError::new(GeometryErrorCode::ShapeErrorExceeded, message)
}
fn native_error(error: solidcore::SolidError) -> GeometryError {
    invalid(format!("native display candidate: {error}"))
}

#[cfg(test)]
mod tests;
