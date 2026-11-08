// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::required;
use crate::{analysis::*, geometry::*, project::*, rpc};

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
                .map(|load| rpc::AnalysisFaceLoad {
                    face_id: load.face_id.as_str().into(),
                    force_n: load.force_n.to_vec(),
                    moment_n_mm: load.moment_n_mm.to_vec(),
                })
                .collect(),
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
        if value.cases.len() > MAX_ANALYSIS_CASES {
            return Err("analysis case budget exceeded".into());
        }
        let document = Self {
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
