// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Replay-qualified nominal surface exposure, separate from optical prediction.
use super::*;
use crate::optical::*;
fn corrupt(message: &str) -> ManufacturingError {
    ManufacturingError::new(ManufacturingErrorCode::CorruptArtifact, message)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpticalBackingModel {
    Unmodelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OpticalInspectionAssessment {
    pub producer: String,
    pub backing_model: OpticalBackingModel,
    pub patch_matches_request: bool,
    pub pose_matches_application: bool,
    pub requested: OpticalAssessment,
    pub realized: OpticalAssessment,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SurfaceColorInspection {
    pub construction_index: u16,
    pub face_id: FaceId,
    pub realized_patch: OpticalPatch,
    /// One complete physical interval per actual model layer, never per stroke.
    pub interval_count: u32,
    pub exposed_height_by_tool_mm: [f64; 2],
    pub fractions_by_tool: [f64; 2],
    #[serde(deserialize_with = "required_option")]
    pub reference: Option<OpticalInspectionAssessment>,
}

pub(super) fn surface_inspections<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<SurfaceColorInspection>, D::Error> {
    bounded_vec::<D, _, MAX_SURFACE_COLOR_FACES>(d)
}

pub(super) fn validate_surface_inspections(
    records: &[SurfaceColorInspection],
    workflow: &JobWorkflow,
) -> Result<(), ManufacturingError> {
    let JobWorkflow::FixedDualMarlin { job } = workflow else {
        return if records.is_empty() {
            Ok(())
        } else {
            Err(corrupt(
                "surface exposure requires a fixed-dual construction",
            ))
        };
    };
    if records.len() > MAX_SURFACE_COLOR_FACES
        || records.len()
            != job
                .surface_constructions
                .iter()
                .map(|c| c.faces.len())
                .sum::<usize>()
    {
        return Err(corrupt(
            "surface exposure table must cover every constructed face",
        ));
    }
    let close = |a: f64, b: f64| (a - b).abs() <= 1e-10 * a.abs().max(b.abs()).max(1.0);
    for (index, record) in records.iter().enumerate() {
        record
            .realized_patch
            .validate()
            .map_err(|e| corrupt(&e.message))?;
        let construction = job
            .surface_constructions
            .get(usize::from(record.construction_index))
            .filter(|c| c.faces.iter().any(|f| f.face_id == record.face_id))
            .ok_or_else(|| corrupt("surface exposure has no source-face construction"))?;
        if record.interval_count == 0
            || record.interval_count > MAX_PRINT_LAYERS
            || records[..index].iter().any(|r| {
                r.construction_index == record.construction_index && r.face_id == record.face_id
            })
            || record
                .exposed_height_by_tool_mm
                .iter()
                .chain(&record.fractions_by_tool)
                .any(|v| !v.is_finite() || *v < 0.0)
            || !close(
                record.exposed_height_by_tool_mm.iter().sum(),
                record.realized_patch.height_mm,
            )
            || !close(record.fractions_by_tool.iter().sum(), 1.0)
            || (0..2).any(|i| {
                record.fractions_by_tool[i] > 1.0
                    || !close(
                        record.fractions_by_tool[i],
                        record.exposed_height_by_tool_mm[i] / record.realized_patch.height_mm,
                    )
            })
        {
            return Err(corrupt(
                "surface exposure has incomplete or inconsistent physical coverage",
            ));
        }
        if let Some(reference) = &record.reference {
            let origin = construction
                .optical_origin
                .as_ref()
                .ok_or_else(|| corrupt("optical reassessment has no captured evidence origin"))?;
            if reference.producer != OPTICAL_PRODUCER
                || reference.patch_matches_request
                    != (record.realized_patch == origin.requested_patch)
            {
                return Err(corrupt(
                    "optical reassessment producer or patch identity mismatch",
                ));
            }
            for (assessment, patch) in [
                (&reference.requested, &origin.requested_patch),
                (&reference.realized, &record.realized_patch),
            ] {
                crate::optical::validate_assessment(
                    assessment,
                    &origin.recipe.slots,
                    usize::from(origin.recipe.phase_slot),
                    origin.recipe.origin_z_mm,
                    patch,
                    &origin.observation,
                    &origin.target,
                )
                .map_err(|e| corrupt(&e.message))?;
            }
            let realized = match &reference.realized {
                OpticalAssessment::Estimate { exposure, .. }
                | OpticalAssessment::Unassessed { exposure, .. } => exposure,
                OpticalAssessment::FilteredPhase => {
                    return Err(corrupt("applied recipe cannot be a filtered grid phase"));
                }
            };
            for (material, captured) in origin.materials.iter().enumerate() {
                let tool = usize::from(job.tool_for_material(&captured.loaded_material.id)?);
                if !close(
                    realized.exposed_height_mm[material],
                    record.exposed_height_by_tool_mm[tool],
                ) || !close(realized.fractions[material], record.fractions_by_tool[tool])
                {
                    return Err(corrupt(
                        "optical reassessment does not use replay-qualified tool exposure",
                    ));
                }
            }
            if realized.exposed_height_mm[2] != 0.0 || realized.fractions[2] != 0.0 {
                return Err(corrupt(
                    "two-material construction has a third exposure component",
                ));
            }
        }
    }
    Ok(())
}
