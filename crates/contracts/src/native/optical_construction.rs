// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Typed evidence closure for retained-study manufacturing applications.
use super::required;
use crate::{appearance::*, geometry::SourceHash, manufacturing::*, optical::*, rpc};

impl TryFrom<rpc::AppearancePaletteRow> for PaletteRow {
    type Error = String;
    fn try_from(v: rpc::AppearancePaletteRow) -> Result<Self, String> {
        use rpc::appearance_palette_row::Evidence;
        let evidence = match required(v.evidence, "palette evidence")? {
            Evidence::Catalog(v) => {
                let rgb = required(v.srgb, "catalog sRGB")?;
                AppearanceEvidence::CatalogEstimate {
                    srgb: [rgb.first, rgb.second, rgb.third],
                }
            }
            Evidence::Measured(v) => AppearanceEvidence::Measured {
                color: required(v.color, "measured color")?.try_into()?,
                conditions: required(v.conditions, "measurement conditions")?.try_into()?,
                material_lot: v.material_lot,
            },
            Evidence::Unknown(v) => AppearanceEvidence::Unknown { reason: v.reason },
        };
        let row = Self {
            key: v.key,
            material_id: v.material_id,
            evidence_id: v.evidence_id,
            name: v.name,
            evidence,
        };
        row.validate().map_err(|e| e.to_string())?;
        Ok(row)
    }
}
impl TryFrom<PaletteRow> for rpc::AppearancePaletteRow {
    type Error = String;
    fn try_from(v: PaletteRow) -> Result<Self, String> {
        v.validate().map_err(|e| e.to_string())?;
        use rpc::appearance_palette_row::Evidence;
        let evidence = match v.evidence {
            AppearanceEvidence::CatalogEstimate {
                srgb: [first, second, third],
            } => Evidence::Catalog(rpc::AppearanceCatalogEvidence {
                srgb: Some(rpc::AppearanceChannels {
                    first,
                    second,
                    third,
                }),
            }),
            AppearanceEvidence::Measured {
                color,
                conditions,
                material_lot,
            } => Evidence::Measured(rpc::AppearanceMeasuredEvidence {
                color: Some(color.try_into()?),
                conditions: Some(conditions.try_into()?),
                material_lot,
            }),
            AppearanceEvidence::Unknown { reason } => {
                Evidence::Unknown(rpc::AppearanceUnknownEvidence { reason })
            }
        };
        Ok(Self {
            key: v.key,
            material_id: v.material_id,
            evidence_id: v.evidence_id,
            name: v.name,
            evidence: Some(evidence),
        })
    }
}
impl TryFrom<rpc::OpticalConstructionOrigin> for OpticalConstructionOrigin {
    type Error = String;
    fn try_from(v: rpc::OpticalConstructionOrigin) -> Result<Self, String> {
        if v.materials.len() != 2 {
            return Err("optical construction requires two evidence/material captures".into());
        }
        let model = match rpc::OpticalModel::try_from(v.model)
            .map_err(|_| "unknown optical construction model")?
        {
            rpc::OpticalModel::PrusaV7Reference => OpticalModel::PrusaV7_007bb922,
            rpc::OpticalModel::Unspecified => {
                return Err("missing optical construction model".into());
            }
        };
        let target = required(v.target, "optical construction target")?;
        let patch = required(v.requested_patch, "optical construction patch")?;
        let origin = required(patch.origin_mm, "optical construction patch origin")?;
        let materials = v
            .materials
            .into_iter()
            .map(|m| {
                Ok(AppliedOpticalMaterial {
                    evidence: required(m.evidence, "applied palette row")?.try_into()?,
                    evidence_hash: SourceHash::parse(m.evidence_hash).map_err(|e| e.to_string())?,
                    loaded_material: required(m.loaded_material, "applied loaded material")?
                        .try_into()?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?
            .try_into()
            .map_err(|_| "optical capture count changed")?;
        let result = Self {
            report: required(v.report, "applied optical report")?.try_into()?,
            palette_hash: SourceHash::parse(v.palette_hash).map_err(|e| e.to_string())?,
            selection: required(v.selection, "applied optical selection")?.try_into()?,
            model,
            observation: super::optical::observation(required(
                v.observation,
                "applied observation",
            )?)?,
            target: OpticalTarget {
                color: required(target.color, "applied target color")?.try_into()?,
                delta_e00_tolerance: target.delta_e00_tolerance,
                lightness_tolerance: target.lightness_tolerance,
            },
            requested_patch: OpticalPatch {
                origin_mm: [origin.x, origin.y, origin.z],
                normal_xy: [patch.normal_x, patch.normal_y],
                width_mm: patch.width_mm,
                height_mm: patch.height_mm,
            },
            recipe: required(v.recipe, "applied recipe")?.try_into()?,
            materials,
            applied_pose: required(v.applied_pose, "applied source pose")?.try_into()?,
        };
        result.validate_capture().map_err(|e| e.to_string())?;
        Ok(result)
    }
}
impl TryFrom<OpticalConstructionOrigin> for rpc::OpticalConstructionOrigin {
    type Error = String;
    fn try_from(v: OpticalConstructionOrigin) -> Result<Self, String> {
        v.validate_capture().map_err(|e| e.to_string())?;
        let patch = v.requested_patch;
        let [x, y, z] = patch.origin_mm;
        Ok(Self {
            report: Some(v.report.try_into()?),
            palette_hash: v.palette_hash.into(),
            selection: Some(v.selection.try_into()?),
            model: match v.model {
                OpticalModel::PrusaV7_007bb922 => rpc::OpticalModel::PrusaV7Reference,
            } as i32,
            observation: Some(super::optical::observation_wire(&v.observation)),
            target: Some(rpc::OpticalTarget {
                color: Some(v.target.color.try_into()?),
                delta_e00_tolerance: v.target.delta_e00_tolerance,
                lightness_tolerance: v.target.lightness_tolerance,
            }),
            requested_patch: Some(rpc::OpticalPatch {
                origin_mm: Some(rpc::Vec3 { x, y, z }),
                normal_x: patch.normal_xy[0],
                normal_y: patch.normal_xy[1],
                width_mm: patch.width_mm,
                height_mm: patch.height_mm,
            }),
            recipe: Some((&v.recipe).try_into()?),
            materials: v
                .materials
                .into_iter()
                .map(|m| {
                    Ok(rpc::AppliedOpticalMaterial {
                        evidence: Some(m.evidence.try_into()?),
                        evidence_hash: m.evidence_hash.into(),
                        loaded_material: Some(m.loaded_material.try_into()?),
                    })
                })
                .collect::<Result<_, String>>()?,
            applied_pose: Some(v.applied_pose.try_into()?),
        })
    }
}

impl TryFrom<rpc::ApplyOpticalConstructionParameters> for ApplyOpticalConstruction {
    type Error = String;
    fn try_from(v: rpc::ApplyOpticalConstructionParameters) -> Result<Self, String> {
        if v.bindings.len() != 2 {
            return Err("optical application requires exactly two explicit bindings".into());
        }
        let result = Self {
            construction_index: v
                .construction_index
                .try_into()
                .map_err(|_| "construction index overflow")?,
            report_hash: SourceHash::parse(v.report_hash).map_err(|e| e.to_string())?,
            selection: required(v.selection, "optical application selection")?.try_into()?,
            bindings: v
                .bindings
                .into_iter()
                .map(|m| {
                    Ok(OpticalMaterialBinding {
                        evidence_key: m.evidence_key,
                        evidence_hash: SourceHash::parse(m.evidence_hash)
                            .map_err(|e| e.to_string())?,
                        loaded_material_id: m.loaded_material_id,
                        loaded_declaration_hash: SourceHash::parse(m.loaded_declaration_hash)
                            .map_err(|e| e.to_string())?,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?
                .try_into()
                .map_err(|_| "binding count changed")?,
        };
        result.validate().map_err(|e| e.to_string())?;
        Ok(result)
    }
}
impl TryFrom<ApplyOpticalConstruction> for rpc::ApplyOpticalConstructionParameters {
    type Error = String;
    fn try_from(v: ApplyOpticalConstruction) -> Result<Self, String> {
        v.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            construction_index: u32::from(v.construction_index),
            report_hash: v.report_hash.into(),
            selection: Some(v.selection.try_into()?),
            bindings: v
                .bindings
                .into_iter()
                .map(|m| rpc::OpticalMaterialBinding {
                    evidence_key: m.evidence_key,
                    evidence_hash: m.evidence_hash.into(),
                    loaded_material_id: m.loaded_material_id,
                    loaded_declaration_hash: m.loaded_declaration_hash.into(),
                })
                .collect(),
        })
    }
}
