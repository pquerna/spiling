// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::required;
use crate::{geometry::FaceId, manufacturing::*, optical::*, rpc};

fn channels(value: rpc::AppearanceChannels) -> [f64; 3] {
    [value.first, value.second, value.third]
}
fn channels_wire([first, second, third]: [f64; 3]) -> rpc::AppearanceChannels {
    rpc::AppearanceChannels {
        first,
        second,
        third,
    }
}
fn exposure(value: rpc::OpticalCycleExposure) -> Result<CycleExposure, String> {
    Ok(CycleExposure {
        period_mm: value.period_mm,
        phase_at_patch_mm: value.phase_at_patch_mm,
        complete_periods: value.complete_periods,
        exposed_height_mm: channels(required(
            value.exposed_height_mm,
            "optical exposed heights",
        )?),
        fractions: channels(required(value.fractions, "optical fractions")?),
    })
}
fn exposure_wire(value: CycleExposure) -> rpc::OpticalCycleExposure {
    rpc::OpticalCycleExposure {
        period_mm: value.period_mm,
        phase_at_patch_mm: value.phase_at_patch_mm,
        complete_periods: value.complete_periods,
        exposed_height_mm: Some(channels_wire(value.exposed_height_mm)),
        fractions: Some(channels_wire(value.fractions)),
    }
}
fn assessment(value: rpc::OpticalInspectionAssessmentValue) -> Result<OpticalAssessment, String> {
    use rpc::optical_inspection_assessment_value::Kind;
    Ok(
        match required(value.kind, "optical inspection assessment")? {
            Kind::Unassessed(value) => OpticalAssessment::Unassessed {
                reason: match rpc::OpticalUnknownReason::try_from(value.reason)
                    .map_err(|_| "unknown optical reason")?
                {
                    rpc::OpticalUnknownReason::MissingEvidence => {
                        OpticalUnknownReason::MissingEvidence
                    }
                    rpc::OpticalUnknownReason::UnsupportedConditions => {
                        OpticalUnknownReason::UnsupportedConditions
                    }
                    rpc::OpticalUnknownReason::UnsupportedColorDomain => {
                        OpticalUnknownReason::UnsupportedColorDomain
                    }
                    rpc::OpticalUnknownReason::Unspecified => {
                        return Err("missing optical unknown reason".into());
                    }
                },
                exposure: exposure(required(value.exposure, "unassessed exposure")?)?,
            },
            Kind::Estimate(value) => {
                if value.display_srgb8.len() != 3 {
                    return Err("display requires exactly three bytes".into());
                }
                let display_srgb8: [u8; 3] = value
                    .display_srgb8
                    .into_iter()
                    .map(|v| u8::try_from(v).map_err(|_| "invalid display byte"))
                    .collect::<Result<Vec<_>, _>>()?
                    .try_into()
                    .map_err(|_| "display requires exactly three bytes")?;
                OpticalAssessment::Estimate {
                    exposure: exposure(required(value.exposure, "estimated exposure")?)?,
                    predicted_lab: channels(required(value.predicted_lab, "predicted Lab")?),
                    display_srgb8,
                    delta_e00: value.delta_e00,
                    delta_lightness: value.delta_lightness,
                    within_tolerance: value.within_tolerance,
                }
            }
        },
    )
}
fn assessment_wire(
    value: OpticalAssessment,
) -> Result<rpc::OpticalInspectionAssessmentValue, String> {
    use rpc::optical_inspection_assessment_value::Kind;
    Ok(rpc::OpticalInspectionAssessmentValue {
        kind: Some(match value {
            OpticalAssessment::FilteredPhase => {
                return Err("applied recipe cannot be a filtered grid phase".into());
            }
            OpticalAssessment::Unassessed { reason, exposure } => {
                Kind::Unassessed(rpc::OpticalUnassessedExposure {
                    reason: match reason {
                        OpticalUnknownReason::MissingEvidence => {
                            rpc::OpticalUnknownReason::MissingEvidence
                        }
                        OpticalUnknownReason::UnsupportedConditions => {
                            rpc::OpticalUnknownReason::UnsupportedConditions
                        }
                        OpticalUnknownReason::UnsupportedColorDomain => {
                            rpc::OpticalUnknownReason::UnsupportedColorDomain
                        }
                    } as i32,
                    exposure: Some(exposure_wire(exposure)),
                })
            }
            OpticalAssessment::Estimate {
                exposure,
                predicted_lab,
                display_srgb8,
                delta_e00,
                delta_lightness,
                within_tolerance,
            } => Kind::Estimate(rpc::OpticalReferenceEstimate {
                exposure: Some(exposure_wire(exposure)),
                predicted_lab: Some(channels_wire(predicted_lab)),
                display_srgb8: display_srgb8.map(u32::from).to_vec(),
                delta_e00,
                delta_lightness,
                within_tolerance,
            }),
        }),
    })
}
impl TryFrom<rpc::SurfaceColorInspection> for SurfaceColorInspection {
    type Error = String;
    fn try_from(v: rpc::SurfaceColorInspection) -> Result<Self, String> {
        let patch = required(v.realized_patch, "realized surface patch")?;
        let p = required(patch.origin_mm, "realized patch origin")?;
        Ok(Self {
            construction_index: u16::try_from(v.construction_index)
                .map_err(|_| "construction index overflow")?,
            face_id: FaceId::parse(v.face_id).map_err(|e| e.to_string())?,
            realized_patch: OpticalPatch {
                origin_mm: [p.x, p.y, p.z],
                normal_xy: [patch.normal_x, patch.normal_y],
                width_mm: patch.width_mm,
                height_mm: patch.height_mm,
            },
            interval_count: v.interval_count,
            exposed_height_by_tool_mm: [
                v.tool_zero_exposed_height_mm,
                v.tool_one_exposed_height_mm,
            ],
            fractions_by_tool: [v.tool_zero_fraction, v.tool_one_fraction],
            reference: v
                .reference
                .map(|r| -> Result<OpticalInspectionAssessment, String> {
                    if rpc::OpticalBackingModel::try_from(r.backing_model)
                        .map_err(|_| "unknown backing model")?
                        != rpc::OpticalBackingModel::Unmodelled
                    {
                        return Err("backing optics must be explicitly unmodelled".into());
                    }
                    Ok(OpticalInspectionAssessment {
                        producer: r.producer,
                        backing_model: OpticalBackingModel::Unmodelled,
                        patch_matches_request: r.patch_matches_request,
                        pose_matches_application: r.pose_matches_application,
                        requested: assessment(required(
                            r.requested,
                            "requested optical assessment",
                        )?)?,
                        realized: assessment(required(r.realized, "realized optical assessment")?)?,
                    })
                })
                .transpose()?,
        })
    }
}
impl TryFrom<SurfaceColorInspection> for rpc::SurfaceColorInspection {
    type Error = String;
    fn try_from(v: SurfaceColorInspection) -> Result<Self, String> {
        let [x, y, z] = v.realized_patch.origin_mm;
        Ok(Self {
            construction_index: u32::from(v.construction_index),
            face_id: v.face_id.as_str().into(),
            realized_patch: Some(rpc::OpticalPatch {
                origin_mm: Some(rpc::Vec3 { x, y, z }),
                normal_x: v.realized_patch.normal_xy[0],
                normal_y: v.realized_patch.normal_xy[1],
                width_mm: v.realized_patch.width_mm,
                height_mm: v.realized_patch.height_mm,
            }),
            interval_count: v.interval_count,
            tool_zero_exposed_height_mm: v.exposed_height_by_tool_mm[0],
            tool_one_exposed_height_mm: v.exposed_height_by_tool_mm[1],
            tool_zero_fraction: v.fractions_by_tool[0],
            tool_one_fraction: v.fractions_by_tool[1],
            reference: v
                .reference
                .map(|r| -> Result<rpc::OpticalInspectionAssessment, String> {
                    Ok(rpc::OpticalInspectionAssessment {
                        producer: r.producer,
                        backing_model: match r.backing_model {
                            OpticalBackingModel::Unmodelled => rpc::OpticalBackingModel::Unmodelled,
                        } as i32,
                        patch_matches_request: r.patch_matches_request,
                        pose_matches_application: r.pose_matches_application,
                        requested: Some(assessment_wire(r.requested)?),
                        realized: Some(assessment_wire(r.realized)?),
                    })
                })
                .transpose()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_surface_exposure_roundtrips_and_refuses_incomplete_or_nonfinite_tables() {
        let intent: ManufacturingIntent = serde_json::from_str(include_str!(
            "../../../../fixtures/manufacturing/surface-color-box.intent.json"
        ))
        .unwrap();
        let JobWorkflow::FixedDualMarlin { job } = &intent.printer.workflow else {
            unreachable!()
        };
        let face = SurfaceColorInspection {
            construction_index: 0,
            face_id: job.surface_constructions[0].faces[0].face_id.clone(),
            realized_patch: OpticalPatch {
                origin_mm: [20.0, 20.0, 0.0],
                normal_xy: [0.0, -1.0],
                width_mm: 20.0,
                height_mm: 8.0,
            },
            interval_count: 40,
            exposed_height_by_tool_mm: [4.0, 4.0],
            fractions_by_tool: [0.5, 0.5],
            reference: None,
        };
        let mut provenance = ManufacturingInspectionProvenance {
            project_id: crate::project::ProjectId::new(),
            input_revision: crate::project::ProjectRevision(1),
            input_hash: crate::geometry::SourceHash::from_bytes(b"input"),
            surface_overlays: crate::surface::SurfaceOverlays::empty(),
            process_document: crate::process::ProcessDocument::empty(),
            kernel: crate::geometry::KernelIdentity {
                name: "native".into(),
                version: "1".into(),
                revision: "1".into(),
            },
            planner_version: "32".into(),
            emitter_version: "11".into(),
            verifier_version: "16".into(),
            workflow: intent.printer.workflow,
            surface_color: vec![face],
        };
        let wire = rpc::ManufacturingInspectionProvenance::try_from(provenance.clone()).unwrap();
        assert_eq!(
            ManufacturingInspectionProvenance::try_from(wire.clone()).unwrap(),
            provenance
        );
        for change in 0..6 {
            let mut wrong = wire.clone();
            match change {
                0 => wrong.surface_color.clear(),
                1 => wrong.surface_color.push(wrong.surface_color[0].clone()),
                2 => wrong.surface_color[0].interval_count = 0,
                3 => wrong.surface_color[0].tool_zero_fraction = f64::NAN,
                4 => wrong.surface_color[0].realized_patch = None,
                _ => wrong.surface_color[0].tool_zero_exposed_height_mm = 5.0,
            }
            assert!(ManufacturingInspectionProvenance::try_from(wrong).is_err());
        }
        provenance.surface_color[0].fractions_by_tool[0] = 0.8;
        assert!(rpc::ManufacturingInspectionProvenance::try_from(provenance).is_err());
    }
    #[test]
    fn optical_assessment_wire_requires_explicit_reason_exposure_and_rgb_bytes() {
        let exposure = CycleExposure {
            period_mm: 0.4,
            phase_at_patch_mm: 0.0,
            complete_periods: 20,
            exposed_height_mm: [4.0, 4.0, 0.0],
            fractions: [0.5, 0.5, 0.0],
        };
        for reason in [
            OpticalUnknownReason::MissingEvidence,
            OpticalUnknownReason::UnsupportedConditions,
            OpticalUnknownReason::UnsupportedColorDomain,
        ] {
            let value = OpticalAssessment::Unassessed { reason, exposure };
            assert_eq!(
                assessment(assessment_wire(value.clone()).unwrap()).unwrap(),
                value
            );
        }
        assert!(assessment(rpc::OpticalInspectionAssessmentValue::default()).is_err());
        let value = OpticalAssessment::Estimate {
            exposure,
            predicted_lab: [50.0, 0.0, 0.0],
            display_srgb8: [100, 120, 130],
            delta_e00: 2.0,
            delta_lightness: 1.0,
            within_tolerance: true,
        };
        let wire = assessment_wire(value.clone()).unwrap();
        assert_eq!(assessment(wire.clone()).unwrap(), value);
        for bytes in [vec![1, 2], vec![1, 2, 256], vec![1, 2, 3, 4]] {
            let mut wrong = wire.clone();
            let Some(rpc::optical_inspection_assessment_value::Kind::Estimate(ref mut estimate)) =
                wrong.kind
            else {
                unreachable!()
            };
            estimate.display_srgb8 = bytes;
            assert!(assessment(wrong).is_err());
        }
    }
}
