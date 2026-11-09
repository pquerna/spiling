// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::required;
use super::{request_id, session_parent};
use crate::analysis::mechanics::*;
use crate::{analysis::*, geometry::*, project::*, rpc};

impl TryFrom<rpc::ForceFootprint> for load::ForceFootprint {
    type Error = String;
    fn try_from(value: rpc::ForceFootprint) -> Result<Self, String> {
        let footprint = match required(value.kind, "force footprint kind")? {
            rpc::force_footprint::Kind::WholeFace(true) => Self::WholeFace {},
            rpc::force_footprint::Kind::Disk(disk) => Self::Disk {
                center_mm: required(disk.center_mm, "contact center")?.try_into()?,
                diameter_mm: disk.diameter_mm,
            },
            _ => return Err("whole-face selector must be explicit true".into()),
        };
        footprint.validate().map_err(|error| error.to_string())?;
        Ok(footprint)
    }
}
impl TryFrom<load::ForceFootprint> for rpc::ForceFootprint {
    type Error = String;
    fn try_from(value: load::ForceFootprint) -> Result<Self, String> {
        value.validate().map_err(|error| error.to_string())?;
        Ok(Self {
            kind: Some(match value {
                load::ForceFootprint::WholeFace {} => rpc::force_footprint::Kind::WholeFace(true),
                load::ForceFootprint::Disk {
                    center_mm,
                    diameter_mm,
                } => rpc::force_footprint::Kind::Disk(rpc::ForceDisk {
                    center_mm: Some(center_mm.try_into()?),
                    diameter_mm,
                }),
            }),
        })
    }
}

impl TryFrom<rpc::PrepareAnalysisFaceRequest> for ProjectCommand {
    type Error = String;
    fn try_from(value: rpc::PrepareAnalysisFaceRequest) -> Result<Self, String> {
        super::request_id(&value.request_id)?;
        let session_id = super::session_parent(&value.parent)?;
        let reference: crate::geometry::FaceRef =
            required(value.reference, "analysis face reference")?.try_into()?;
        if session_id != reference.session_id {
            return Err("analysis face session mismatch".into());
        }
        Ok(Self::PrepareAnalysisFace {
            session_id,
            base_revision: ProjectRevision(value.base_revision),
            reference,
        })
    }
}

impl TryFrom<rpc::AnalysisFacePreparation> for crate::analysis::load::AnalysisFacePreparation {
    type Error = String;
    fn try_from(value: rpc::AnalysisFacePreparation) -> Result<Self, String> {
        let prepared = Self {
            project_id: ProjectId::parse(value.project_id).map_err(|error| error.to_string())?,
            project_revision: ProjectRevision(value.project_revision),
            reference: required(value.reference, "prepared analysis face")?.try_into()?,
            source_hash: SourceHash::parse(value.source_hash).map_err(|error| error.to_string())?,
            pose: required(value.pose, "prepared face pose")?.try_into()?,
            area_mm2: value.area_mm2,
            centroid_mm: required(value.centroid_mm, "native face centroid")?.try_into()?,
            outward_normal: required(value.outward_normal, "native face normal")?.try_into()?,
            origin_mm: required(value.origin_mm, "native face origin")?.try_into()?,
            horizontal_direction: required(value.horizontal_direction, "native face horizontal")?
                .try_into()?,
            width_mm: value.width_mm,
            height_mm: value.height_mm,
        };
        prepared.validate().map_err(|error| error.to_string())?;
        Ok(prepared)
    }
}
impl TryFrom<crate::analysis::load::AnalysisFacePreparation> for rpc::AnalysisFacePreparation {
    type Error = String;
    fn try_from(value: crate::analysis::load::AnalysisFacePreparation) -> Result<Self, String> {
        value.validate().map_err(|error| error.to_string())?;
        Ok(Self {
            project_id: value.project_id.as_str().into(),
            project_revision: value.project_revision.0,
            reference: Some(value.reference.try_into()?),
            source_hash: value.source_hash.as_str().into(),
            pose: Some(value.pose.try_into()?),
            area_mm2: value.area_mm2,
            centroid_mm: Some(value.centroid_mm.try_into()?),
            outward_normal: Some(value.outward_normal.try_into()?),
            origin_mm: Some(value.origin_mm.try_into()?),
            horizontal_direction: Some(value.horizontal_direction.try_into()?),
            width_mm: value.width_mm,
            height_mm: value.height_mm,
        })
    }
}

impl TryFrom<rpc::GetAnalysisResultRequest> for ProjectCommand {
    type Error = String;
    fn try_from(value: rpc::GetAnalysisResultRequest) -> Result<Self, String> {
        request_id(&value.request_id)?;
        Ok(Self::GetAnalysisResult {
            session_id: session_parent(&value.parent)?,
            base_revision: ProjectRevision(value.base_revision),
            case_id: AnalysisCaseId::parse(value.case_id).map_err(|error| error.to_string())?,
        })
    }
}
impl TryFrom<rpc::AnalysisResultRecord> for AnalysisResultRecord {
    type Error = String;
    fn try_from(value: rpc::AnalysisResultRecord) -> Result<Self, String> {
        if value.byte_count == 0 || value.byte_count as usize > MAX_MECHANICAL_FIELD_BYTES {
            return Err("saved field byte budget differs".into());
        }
        Ok(Self {
            case_id: AnalysisCaseId::parse(value.case_id).map_err(|error| error.to_string())?,
            hash: SourceHash::parse(value.hash).map_err(|error| error.to_string())?,
            byte_count: value.byte_count,
            input_hash: SourceHash::parse(value.input_hash).map_err(|error| error.to_string())?,
            captured_revision: ProjectRevision(value.captured_revision),
        })
    }
}
impl From<AnalysisResultRecord> for rpc::AnalysisResultRecord {
    fn from(value: AnalysisResultRecord) -> Self {
        Self {
            case_id: value.case_id.as_str().into(),
            hash: value.hash.as_str().into(),
            byte_count: value.byte_count,
            input_hash: value.input_hash.as_str().into(),
            captured_revision: value.captured_revision.0,
        }
    }
}

impl TryFrom<rpc::AnalysisRunSettings> for AnalysisRunSettings {
    type Error = String;
    fn try_from(value: rpc::AnalysisRunSettings) -> Result<Self, String> {
        fn tensor(values: Vec<f64>) -> Result<[[f64; 6]; 6], String> {
            if values.len() != 36 {
                return Err("assumed engineering tensor needs 36 row-major coefficients".into());
            }
            Ok(std::array::from_fn(|row| {
                std::array::from_fn(|column| values[6 * row + column])
            }))
        }
        let laws = required(value.laws, "assumed printed laws")?;
        let settings = Self {
            recipe: required(value.recipe, "explicit analysis print recipe")?.try_into()?,
            laws: AssumedPrintedLaws {
                name: laws.name,
                justification: laws.justification,
                compatible_recipe: required(laws.compatible_recipe, "law-compatible recipe")?
                    .try_into()?,
                wall_mpa: tensor(laws.wall_mpa)?,
                skin_mpa: tensor(laws.skin_mpa)?,
                effective_core_mpa: tensor(laws.effective_core_mpa)?,
            },
            maximum_span_mm: value.maximum_span_mm,
            maximum_seconds: value.maximum_seconds,
        };
        settings.validate().map_err(|error| error.to_string())?;
        Ok(settings)
    }
}
impl TryFrom<AnalysisRunSettings> for rpc::AnalysisRunSettings {
    type Error = String;
    fn try_from(value: AnalysisRunSettings) -> Result<Self, String> {
        value.validate().map_err(|error| error.to_string())?;
        Ok(Self {
            recipe: Some(value.recipe.try_into()?),
            laws: Some(rpc::AssumedPrintedLaws {
                name: value.laws.name,
                justification: value.laws.justification,
                compatible_recipe: Some(value.laws.compatible_recipe.try_into()?),
                wall_mpa: value.laws.wall_mpa.into_iter().flatten().collect(),
                skin_mpa: value.laws.skin_mpa.into_iter().flatten().collect(),
                effective_core_mpa: value
                    .laws
                    .effective_core_mpa
                    .into_iter()
                    .flatten()
                    .collect(),
            }),
            maximum_span_mm: value.maximum_span_mm,
            maximum_seconds: value.maximum_seconds,
        })
    }
}
impl TryFrom<rpc::RunAnalysisRequest> for ProjectCommand {
    type Error = String;
    fn try_from(value: rpc::RunAnalysisRequest) -> Result<Self, String> {
        super::request_id(&value.request_id)?;
        Ok(Self::RunAnalysis {
            session_id: super::session_parent(&value.parent)?,
            base_revision: ProjectRevision(value.base_revision),
            case_id: AnalysisCaseId::parse(value.case_id).map_err(|error| error.to_string())?,
            settings: std::sync::Arc::new(
                required(value.settings, "analysis settings")?.try_into()?,
            ),
        })
    }
}

impl TryFrom<rpc::AnalysisCase> for AnalysisCase {
    type Error = String;
    fn try_from(value: rpc::AnalysisCase) -> Result<Self, String> {
        if value.loads.len() > MAX_ANALYSIS_PATCHES || value.restraints.len() > MAX_ANALYSIS_PATCHES
        {
            return Err("analysis patch budget exceeded".into());
        }
        let case = Self {
            id: AnalysisCaseId::parse(value.id).map_err(|error| error.to_string())?,
            name: value.name,
            layout_id: PrintLayoutId::parse(value.layout_id).map_err(|error| error.to_string())?,
            occurrence_id: OccurrenceId::new(value.occurrence_id)
                .map_err(|error| error.to_string())?,
            definition_id: DefinitionId::parse(value.definition_id)
                .map_err(|error| error.to_string())?,
            source_hash: SourceHash::parse(value.source_hash).map_err(|error| error.to_string())?,
            mode: match rpc::AnalysisMode::try_from(value.mode) {
                Ok(rpc::AnalysisMode::Basic) => AnalysisMode::Basic,
                Ok(rpc::AnalysisMode::Enhanced) => AnalysisMode::Enhanced,
                _ => return Err("unknown or missing FDM analysis mode".into()),
            },
            loads: value
                .loads
                .into_iter()
                .map(|load| {
                    Ok(AnalysisFaceLoad {
                        face_id: FaceId::parse(load.face_id).map_err(|error| error.to_string())?,
                        footprint: required(load.footprint, "force footprint")?.try_into()?,
                        force_n: load
                            .force_n
                            .try_into()
                            .map_err(|_| "analysis force requires three components")?,
                        moment_n_mm: load
                            .moment_n_mm
                            .try_into()
                            .map_err(|_| "analysis moment requires three components")?,
                    })
                })
                .collect::<Result<_, String>>()?,
            restraints: value
                .restraints
                .into_iter()
                .map(|restraint| {
                    Ok(AnalysisFaceRestraint {
                        face_id: FaceId::parse(restraint.face_id)
                            .map_err(|error| error.to_string())?,
                        fixed_translation: restraint
                            .fixed_translation
                            .try_into()
                            .map_err(|_| "analysis restraint requires three directions")?,
                    })
                })
                .collect::<Result<_, String>>()?,
        };
        case.validate().map_err(|error| error.to_string())?;
        Ok(case)
    }
}
impl TryFrom<AnalysisCase> for rpc::AnalysisCase {
    type Error = String;
    fn try_from(value: AnalysisCase) -> Result<Self, String> {
        value.validate().map_err(|error| error.to_string())?;
        Ok(Self {
            id: value.id.as_str().into(),
            name: value.name,
            layout_id: value.layout_id.as_str().into(),
            occurrence_id: value.occurrence_id.get(),
            definition_id: value.definition_id.as_str().into(),
            source_hash: value.source_hash.as_str().into(),
            mode: match value.mode {
                AnalysisMode::Basic => rpc::AnalysisMode::Basic,
                AnalysisMode::Enhanced => rpc::AnalysisMode::Enhanced,
            } as i32,
            loads: value
                .loads
                .into_iter()
                .map(|load| {
                    Ok(rpc::AnalysisFaceLoad {
                        face_id: load.face_id.as_str().into(),
                        footprint: Some(load.footprint.try_into()?),
                        force_n: load.force_n.to_vec(),
                        moment_n_mm: load.moment_n_mm.to_vec(),
                    })
                })
                .collect::<Result<_, String>>()?,
            restraints: value
                .restraints
                .into_iter()
                .map(|restraint| rpc::AnalysisFaceRestraint {
                    face_id: restraint.face_id.as_str().into(),
                    fixed_translation: restraint.fixed_translation.to_vec(),
                })
                .collect(),
        })
    }
}
impl TryFrom<rpc::AnalysisDocument> for AnalysisDocument {
    type Error = String;
    fn try_from(value: rpc::AnalysisDocument) -> Result<Self, String> {
        if value.cases.len() > MAX_ANALYSIS_CASES || value.results.len() > MAX_ANALYSIS_CASES {
            return Err("analysis case budget exceeded".into());
        }
        let document = Self {
            results: value
                .results
                .into_iter()
                .map(|record| {
                    Ok(AnalysisResultRecord {
                        case_id: AnalysisCaseId::parse(record.case_id)
                            .map_err(|error| error.to_string())?,
                        hash: SourceHash::parse(record.hash).map_err(|error| error.to_string())?,
                        byte_count: record.byte_count,
                        input_hash: SourceHash::parse(record.input_hash)
                            .map_err(|error| error.to_string())?,
                        captured_revision: ProjectRevision(record.captured_revision),
                    })
                })
                .collect::<Result<_, String>>()?,
            cases: value
                .cases
                .into_iter()
                .map(|record| {
                    Ok(AnalysisCaseRecord {
                        case: required(record.case, "analysis case")?.try_into()?,
                        edited_revision: ProjectRevision(record.edited_revision),
                    })
                })
                .collect::<Result<_, String>>()?,
        };
        document.validate().map_err(|error| error.to_string())?;
        Ok(document)
    }
}
impl TryFrom<AnalysisDocument> for rpc::AnalysisDocument {
    type Error = String;
    fn try_from(value: AnalysisDocument) -> Result<Self, String> {
        value.validate().map_err(|error| error.to_string())?;
        Ok(Self {
            results: value
                .results
                .into_iter()
                .map(|record| rpc::AnalysisResultRecord {
                    case_id: record.case_id.as_str().into(),
                    hash: record.hash.as_str().into(),
                    byte_count: record.byte_count,
                    input_hash: record.input_hash.as_str().into(),
                    captured_revision: record.captured_revision.0,
                })
                .collect(),
            cases: value
                .cases
                .into_iter()
                .map(|record| {
                    Ok(rpc::AnalysisCaseRecord {
                        case: Some(record.case.try_into()?),
                        edited_revision: record.edited_revision.get(),
                    })
                })
                .collect::<Result<_, String>>()?,
        })
    }
}
impl TryFrom<rpc::AnalysisEdit> for AnalysisEdit {
    type Error = String;
    fn try_from(value: rpc::AnalysisEdit) -> Result<Self, String> {
        let edit = match required(value.edit, "analysis edit")? {
            rpc::analysis_edit::Edit::Put(case) => Self::Put {
                case: case.try_into()?,
            },
            rpc::analysis_edit::Edit::Remove(id) => Self::Remove {
                case_id: AnalysisCaseId::parse(id).map_err(|error| error.to_string())?,
            },
        };
        edit.validate().map_err(|error| error.to_string())?;
        Ok(edit)
    }
}
impl TryFrom<AnalysisEdit> for rpc::AnalysisEdit {
    type Error = String;
    fn try_from(value: AnalysisEdit) -> Result<Self, String> {
        value.validate().map_err(|error| error.to_string())?;
        Ok(Self {
            edit: Some(match value {
                AnalysisEdit::Put { case } => rpc::analysis_edit::Edit::Put(case.try_into()?),
                AnalysisEdit::Remove { case_id } => {
                    rpc::analysis_edit::Edit::Remove(case_id.as_str().into())
                }
            }),
        })
    }
}
