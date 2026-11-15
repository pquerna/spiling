// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Disposable display candidates are independently audited against the borrowed
//! native coefficients and topology. Mesher tolerances and diagnostics are not
//! certificates, and this module never creates a manufacturing face domain.

mod audit;
mod candidate;
mod preview;
mod singular;
mod sweep;
mod trim;

use crate::{GeometryWorkBudget, MeshArtifact, check_cancel, kernel::Definition};
use candidate::FaceCandidate;
use solidcore_tess::MeshTolerance;
use spiling_contracts::display::{MeshBudget, MeshChunkBuilder};
use spiling_contracts::geometry::{
    DisplayProfile, GeometryError, GeometryErrorCode, MAX_NATIVE_FACES, MESH_CARRIER_TOLERANCE_MM,
};
use std::sync::atomic::AtomicBool;

const PROOF_BYTES: usize = crate::compute::DISPLAY_PROOF_BYTES;

/// Refinement rounds after the first plan: each plans the body four times finer.
const MAX_REFINEMENTS: i32 = 8;

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
                if cause.code == GeometryErrorCode::ShapeErrorExceeded
                    && self.attempt < MAX_REFINEMENTS =>
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
    let mut first_refinement: Option<String> = None;
    for attempt in 0..=MAX_REFINEMENTS {
        // The whole body is planned at one chord; only an independent shape-error audit asks
        // for the next, four times finer, and every attempt shares the cumulative allowance.
        let chord = MESH_CARRIER_TOLERANCE_MM * 0.25 * 0.25_f64.powi(attempt);
        let mut phase = DisplayProgress {
            stage: DisplayStage::PreparingEdges,
            attempt: attempt as u32,
            completed_faces: 0,
            total_faces: faces.len() as u32,
            consumed_work: work.consumed(),
        };
        progress(phase);
        let candidate::Candidates {
            edges,
            faces: candidates,
            stats,
        } = candidate::plan(
            definition,
            MeshTolerance::display(chord),
            PROOF_BYTES,
            work,
            cancel,
        )
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
        let candidate_bytes = candidates.iter().fold(edges.retained_bytes(), |sum, face| {
            sum.saturating_add(face.retained_bytes())
        });
        check_memory(candidate_bytes)?;
        let mut builder = MeshChunkBuilder::with_budget(
            definition.id().clone(),
            faces.len() as u32,
            profile,
            budget,
        )?;
        let mut round = RefinementRound::new(attempt);
        let result = (|| {
            for (ordinal, mut candidate) in candidates.into_iter().enumerate() {
                work.charge(1, cancel)?;
                definition.faces()[ordinal].validate()?;
                if definition.face_ordinal(candidate.face) != Some(ordinal) {
                    return Err(invalid("native display face has ambiguous source ordinal"));
                }
                let occupied = candidate_bytes.saturating_add(builder.retained_bytes());
                phase.stage = DisplayStage::TessellatingFace;
                phase.consumed_work = work.consumed();
                progress(phase);
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
                    Ok(())
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
                    && attempt < MAX_REFINEMENTS
                    && round.retry_cause.is_some() =>
            {
                // A completed audit, never the planner's statistics, requests refinement.
                if first_refinement.is_none() {
                    first_refinement = Some(cause.message);
                }
            }
            Err(cause) => {
                let notes = candidate::plan_notes(definition, &stats);
                return Err(match first_refinement.as_deref() {
                    Some(previous) => GeometryError::new(
                        cause.code,
                        format!("{}; first refinement: {previous}{notes}", cause.message),
                    ),
                    None => GeometryError::new(cause.code, format!("{}{notes}", cause.message)),
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
    candidate: &mut FaceCandidate,
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
