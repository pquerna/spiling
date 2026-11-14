// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Disposable display candidates are independently audited against the borrowed
//! native coefficients and topology. Mesher tolerances and diagnostics are not
//! certificates, and this module never creates a manufacturing face domain.

mod audit;
mod preview;
mod singular;
mod sweep;
mod trim;

use crate::{GeometryWorkBudget, MeshArtifact, check_cancel, kernel::Definition};
use solidcore_mesh::{
    EdgeOptions, FaceOverride, OutputOptions, QueryBudget, QueryError, QueryLimit,
    TessellateOptions, Tolerance,
};
use spiling_contracts::display::{MeshBudget, MeshChunkBuilder};
use spiling_contracts::geometry::{
    DisplayProfile, GeometryError, GeometryErrorCode, MAX_NATIVE_FACES, MESH_CARRIER_TOLERANCE_MM,
};
use std::sync::atomic::AtomicBool;

const PROOF_BYTES: usize = crate::compute::DISPLAY_PROOF_BYTES;

struct RefinementRound {
    attempt: i32,
    retry_cause: Option<GeometryError>,
}

impl RefinementRound {
    fn new(attempt: i32) -> Self {
        Self {
            attempt,
            retry_cause: None,
        }
    }

    fn audit<T>(
        &mut self,
        result: Result<T, GeometryError>,
        refine: impl FnOnce(&GeometryError) -> Result<(), GeometryError>,
    ) -> Result<Option<T>, GeometryError> {
        match result {
            Ok(data) => Ok(self.retry_cause.is_none().then_some(data)),
            Err(cause)
                if cause.code == GeometryErrorCode::ShapeErrorExceeded && self.attempt < 8 =>
            {
                refine(&cause)?;
                self.retry_cause.get_or_insert(cause);
                Ok(None)
            }
            Err(cause) => Err(self.failure(cause)),
        }
    }

    fn failure(&self, cause: GeometryError) -> GeometryError {
        if !matches!(
            cause.code,
            GeometryErrorCode::ResourceLimit | GeometryErrorCode::Cancelled
        ) && let Some(previous) = &self.retry_cause
        {
            return previous.clone();
        }
        cause
    }

    fn finish(&self) -> Result<(), GeometryError> {
        match &self.retry_cause {
            Some(cause) => Err(cause.clone()),
            None => Ok(()),
        }
    }
}

struct FaceRefinements {
    tolerances: Vec<Option<f64>>,
}

impl FaceRefinements {
    fn new(
        count: usize,
        work: &GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        work.charge(1, cancel)?;
        let expected = count
            .checked_mul(size_of::<Option<f64>>())
            .and_then(|bytes| bytes.checked_add(size_of::<Self>()))
            .ok_or_else(|| resource("native display refinement storage overflow"))?;
        check_memory(expected)?;
        let mut tolerances = Vec::new();
        tolerances
            .try_reserve_exact(count)
            .map_err(|_| resource("native display refinement allocation failed"))?;
        let result = Self { tolerances };
        check_memory(result.retained_bytes())?;
        let mut result = result;
        for _ in 0..count {
            work.charge(1, cancel)?;
            result.tolerances.push(None);
        }
        Ok(result)
    }

    fn retained_bytes(&self) -> usize {
        size_of::<Self>().saturating_add(
            self.tolerances
                .capacity()
                .saturating_mul(size_of::<Option<f64>>()),
        )
    }

    fn refine(
        &mut self,
        ordinal: usize,
        work: &GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        work.charge(1, cancel)?;
        let tolerance = self
            .tolerances
            .get_mut(ordinal)
            .ok_or_else(|| invalid("native display refinement has a foreign face ordinal"))?;
        *tolerance = Some(tolerance.unwrap_or(MESH_CARRIER_TOLERANCE_MM * 0.25) * 0.25);
        Ok(())
    }

    fn install(
        &self,
        options: &mut TessellateOptions,
        faces: &[solidcore::FaceId],
        work: &GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<usize, GeometryError> {
        work.charge(1, cancel)?;
        if faces.len() != self.tolerances.len() {
            return Err(invalid("native display refinement face inventory differs"));
        }
        options.face_hook = None;
        let mut count = 0;
        for tolerance in &self.tolerances {
            work.charge(1, cancel)?;
            count += usize::from(tolerance.is_some());
        }
        if count == 0 {
            return Ok(self.retained_bytes());
        }
        let outside = self
            .retained_bytes()
            .saturating_add(size_of::<Vec<(solidcore::FaceId, f64)>>() + 2 * size_of::<usize>());
        check_memory(
            outside.saturating_add(count.saturating_mul(size_of::<(solidcore::FaceId, f64)>())),
        )?;
        let mut overrides = Vec::new();
        overrides
            .try_reserve_exact(count)
            .map_err(|_| resource("native display refinement hook allocation failed"))?;
        let bytes = outside.saturating_add(
            overrides
                .capacity()
                .saturating_mul(size_of::<(solidcore::FaceId, f64)>()),
        );
        check_memory(bytes)?;
        for (&face, &tolerance) in faces.iter().zip(&self.tolerances) {
            work.charge(1, cancel)?;
            if let Some(tolerance) = tolerance {
                overrides.push((face, tolerance));
            }
        }
        crate::compute::sort_unstable_by(
            &mut overrides,
            |left, right| left.0.cmp(&right.0),
            work,
            cancel,
        )?;
        options.face_hook = Some(std::sync::Arc::new(move |face| {
            overrides
                .binary_search_by_key(&face, |&(id, _)| id)
                .ok()
                .map(|index| FaceOverride {
                    chord_tolerance: Some(overrides[index].1),
                    ..FaceOverride::default()
                })
        }));
        Ok(bytes)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayStage {
    PreparingEdges,
    TessellatingFace,
    AuditingFace,
    FaceComplete,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayProgress {
    pub stage: DisplayStage,
    pub attempt: u32,
    pub completed_faces: u32,
    pub total_faces: u32,
    pub consumed_work: u64,
}

pub(crate) fn tessellate(
    definition: &Definition,
    profile: DisplayProfile,
    cancel: &AtomicBool,
    budget: MeshBudget,
) -> Result<MeshArtifact, GeometryError> {
    tessellate_with_work(
        definition,
        profile,
        cancel,
        budget,
        &mut GeometryWorkBudget::query(),
        &mut |_| {},
    )
}

pub(crate) fn tessellate_with_work(
    definition: &Definition,
    profile: DisplayProfile,
    cancel: &AtomicBool,
    budget: MeshBudget,
    work: &mut GeometryWorkBudget,
    progress: &mut impl FnMut(DisplayProgress),
) -> Result<MeshArtifact, GeometryError> {
    check_cancel(cancel)?;
    budget.validate()?;
    if profile.is_tolerant_preview() {
        return preview::tessellate(definition, profile, cancel, budget, work, progress);
    }
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
    let mut first_refinement = None;
    let mut refinements = FaceRefinements::new(faces.len(), work, cancel)
        .map_err(|cause| display_context(cause, definition, None, 0, work.consumed(), None))?;
    for attempt in 0..=8 {
        let refinement_bytes = work
            .charge(1, cancel)
            .and_then(|()| refinements.install(&mut options, faces, work, cancel))
            .map_err(|cause| {
                display_context(
                    cause,
                    definition,
                    None,
                    attempt,
                    work.consumed(),
                    first_refinement.as_deref(),
                )
            })?;
        let mut phase = DisplayProgress {
            stage: DisplayStage::PreparingEdges,
            attempt: attempt as u32,
            completed_faces: 0,
            total_faces: faces.len() as u32,
            consumed_work: work.consumed(),
        };
        progress(phase);
        let edges = candidate_query(refinement_bytes, work, cancel, |native| {
            solidcore_mesh::tessellate_edge_set_with_budget(&kernel, brep, &options, native)
        })
        .map_err(|cause| {
            display_context(
                cause,
                definition,
                None,
                attempt,
                work.consumed(),
                first_refinement.as_deref(),
            )
        })?;
        check_cancel(cancel)?;
        if let Some(failure) = edges.failures().first() {
            return Err(invalid(format!(
                "native display edge candidate failed: {}",
                failure.error
            )));
        }
        let edge_bytes = edges.retained_bytes().saturating_add(refinement_bytes);
        check_memory(edge_bytes)?;
        let mut builder = MeshChunkBuilder::with_budget(
            definition.id().clone(),
            faces.len() as u32,
            profile,
            budget,
        )?;
        let mut round = RefinementRound::new(attempt);
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
                phase.stage = DisplayStage::TessellatingFace;
                phase.consumed_work = work.consumed();
                progress(phase);
                let mut candidate = candidate_query(occupied, work, cancel, |native| {
                    solidcore_mesh::tessellate_face_with_budget(
                        &kernel,
                        brep,
                        face,
                        &edges,
                        &candidate_options,
                        native,
                    )
                })
                .map_err(|cause| {
                    round.failure(display_context(
                        cause,
                        definition,
                        Some(ordinal),
                        attempt,
                        work.consumed(),
                        None,
                    ))
                })?;
                check_cancel(cancel)?;
                if candidate.face != face {
                    return Err(invalid("native mesher changed source face identity"));
                }
                prepare_candidate_parameters(definition, &mut candidate, work, cancel)?;
                phase.stage = DisplayStage::AuditingFace;
                phase.consumed_work = work.consumed();
                progress(phase);
                let data = audit::face(
                    definition,
                    ordinal,
                    &candidate,
                    &edges,
                    builder.remaining_budget(),
                    occupied,
                    work,
                    cancel,
                )
                .map_err(|cause| {
                    display_context(
                        cause,
                        definition,
                        Some(ordinal),
                        attempt,
                        work.consumed(),
                        None,
                    )
                });
                let data = round.audit(data, |cause| {
                    first_refinement.get_or_insert_with(|| cause.message.clone());
                    refinements.refine(ordinal, work, cancel).map_err(|cause| {
                        display_context(
                            cause,
                            definition,
                            Some(ordinal),
                            attempt,
                            work.consumed(),
                            None,
                        )
                    })
                })?;
                if let Some(data) = data {
                    builder.push_face(
                        ordinal as u32,
                        &data.positions,
                        &data.normals,
                        &candidate.triangles,
                        data.carrier_error,
                        data.trim_error,
                    )?;
                }
                phase.stage = DisplayStage::FaceComplete;
                phase.completed_faces = ordinal as u32 + 1;
                phase.consumed_work = work.consumed();
                progress(phase);
            }
            check_cancel(cancel)?;
            round.finish()?;
            builder.finish()
        })();
        match result {
            Err(cause)
                if cause.code == GeometryErrorCode::ShapeErrorExceeded
                    && attempt < 8
                    && round.retry_cause.is_some() =>
            {
                // A completed audit, never sampled mesher statistics, requests
                // refinement. All attempts share the same cumulative allowance.
                if first_refinement.is_none() {
                    first_refinement = Some(cause.message);
                }
            }
            Err(cause) => {
                return Err(match first_refinement.as_deref() {
                    Some(previous) => GeometryError::new(
                        cause.code,
                        format!("{}; first refinement: {previous}", cause.message,),
                    ),
                    None => cause,
                });
            }
            Ok(mesh) => {
                phase.stage = DisplayStage::Complete;
                phase.consumed_work = work.consumed();
                progress(phase);
                check_cancel(cancel)?;
                return Ok(mesh);
            }
        }
    }
    unreachable!("the last candidate returns its actual audit result")
}

fn prepare_candidate_parameters(
    definition: &Definition,
    candidate: &mut solidcore_mesh::FaceMesh,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    let surface = definition
        .face_surface(candidate.face)
        .ok_or_else(|| invalid("native display candidate has no carrier"))?;
    let domain = surface.natural_uv_domain_if_built();
    let bounds = [
        [domain.u_min(), domain.u_max()],
        [domain.v_min(), domain.v_max()],
    ];
    for [lower, upper] in bounds {
        crate::parametric::Interval::new(lower, upper)?;
    }
    let roundoff = bounds.map(|[lower, upper]| {
        let mut enclosure = [lower, upper];
        for _ in 0..4 {
            enclosure = [
                crate::parametric::down(enclosure[0]),
                crate::parametric::up(enclosure[1]),
            ];
        }
        enclosure
    });
    for parameter in &mut candidate.uvs {
        work.charge(1, cancel)?;
        for (axis, value) in [&mut parameter.x, &mut parameter.y].into_iter().enumerate() {
            if roundoff[axis][0] <= *value && *value <= roundoff[axis][1] {
                *value = (*value).clamp(bounds[axis][0], bounds[axis][1]);
            }
        }
    }
    Ok(())
}

fn display_context(
    cause: GeometryError,
    definition: &Definition,
    ordinal: Option<usize>,
    attempt: i32,
    work: u64,
    refinement: Option<&str>,
) -> GeometryError {
    let stage = match ordinal {
        Some(ordinal) => format!(
            "face={ordinal} source_face={}",
            definition.faces()[ordinal].face_id.as_str(),
        ),
        None => "edges".to_owned(),
    };
    let previous = refinement.map_or_else(String::new, |previous| {
        format!("; first refinement: {previous}")
    });
    GeometryError::new(
        cause.code,
        format!(
            "native display source={} {stage} attempt={attempt} work={work}: {}{previous}",
            definition.id().as_str(),
            cause.message,
        ),
    )
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
        QueryError::Limit(limit) => resource(format!(
            "native display candidate allowance: {limit:?}; candidate_work={}",
            native.consumed(),
        )),
    })
}

pub(super) fn check_memory(bytes: usize) -> Result<(), GeometryError> {
    if bytes > PROOF_BYTES {
        Err(resource(format!(
            "native display retained proof workspace exceeds {} MiB",
            PROOF_BYTES / (1024 * 1024)
        )))
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
