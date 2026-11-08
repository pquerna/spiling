// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Authored FDM analysis intent. These records are not numerical results.
use crate::{geometry::*, project::*};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use ts_rs::TS;

#[cfg(test)]
#[path = "analysis_tests.rs"]
mod tests;

fn bounded<'de, D, T, const LIMIT: usize>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Items<T, const LIMIT: usize>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const LIMIT: usize> serde::de::Visitor<'de> for Items<T, LIMIT> {
        type Value = Vec<T>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a bounded analysis array")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut sequence: A,
        ) -> Result<Self::Value, A::Error> {
            let mut values = Vec::new();
            while let Some(value) = sequence.next_element()? {
                if values.len() == LIMIT {
                    return Err(serde::de::Error::custom("analysis array exceeds budget"));
                }
                values.push(value);
            }
            Ok(values)
        }
    }
    deserializer.deserialize_seq(Items::<T, LIMIT>(std::marker::PhantomData))
}

pub const MAX_ANALYSIS_CASES: usize = 8;
pub const MAX_ANALYSIS_PATCHES: usize = 16;
pub const MAX_ANALYSIS_DOCUMENT_BYTES: usize = 64 * 1024;
pub const MAX_ANALYSIS_RETAINED_BYTES: usize = 4 * 1024 * 1024;

fn invalid(message: &str) -> ProjectError {
    ProjectError::new(ProjectErrorCode::InvalidProject, message)
}

#[derive(Debug, Clone, Serialize, TS, PartialEq, Eq, PartialOrd, Ord)]
#[serde(transparent)]
#[ts(type = "string")]
pub struct AnalysisCaseId(String);
impl AnalysisCaseId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().hyphenated().to_string())
    }
    pub fn parse(value: String) -> Result<Self, ProjectError> {
        ProjectId::parse(value.clone())?;
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl Default for AnalysisCaseId {
    fn default() -> Self {
        Self::new()
    }
}
impl<'de> Deserialize<'de> for AnalysisCaseId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisMode {
    Basic,
    Enhanced,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AnalysisFaceLoad {
    pub face_id: FaceId,
    pub force_n: [f64; 3],
    pub moment_n_mm: [f64; 3],
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AnalysisFaceRestraint {
    pub face_id: FaceId,
    pub fixed_translation: [bool; 3],
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AnalysisCase {
    pub id: AnalysisCaseId,
    pub name: String,
    pub layout_id: PrintLayoutId,
    pub occurrence_id: OccurrenceId,
    pub definition_id: DefinitionId,
    pub source_hash: SourceHash,
    pub mode: AnalysisMode,
    #[serde(deserialize_with = "bounded::<_, _, MAX_ANALYSIS_PATCHES>")]
    pub loads: Vec<AnalysisFaceLoad>,
    #[serde(deserialize_with = "bounded::<_, _, MAX_ANALYSIS_PATCHES>")]
    pub restraints: Vec<AnalysisFaceRestraint>,
}
impl AnalysisCase {
    pub fn validate(&self) -> Result<(), ProjectError> {
        if self.name.is_empty()
            || self.name.len() > 80
            || self.name.trim() != self.name
            || self.name.chars().any(char::is_control)
        {
            return Err(invalid(
                "analysis case name must be 1..80 trimmed bytes without controls",
            ));
        }
        if self.loads.len() > MAX_ANALYSIS_PATCHES || self.restraints.len() > MAX_ANALYSIS_PATCHES {
            return Err(ProjectError::new(
                ProjectErrorCode::ResourceLimit,
                "analysis patch budget exceeded",
            ));
        }
        let mut load_faces = BTreeSet::new();
        for load in &self.loads {
            if !load_faces.insert(&load.face_id)
                || load
                    .force_n
                    .iter()
                    .chain(&load.moment_n_mm)
                    .any(|value| !value.is_finite())
                || load
                    .force_n
                    .iter()
                    .chain(&load.moment_n_mm)
                    .all(|value| *value == 0.0)
            {
                return Err(invalid(
                    "analysis loads require unique whole faces and a finite nonzero authored wrench",
                ));
            }
        }
        let mut restraint_faces = BTreeSet::new();
        for restraint in &self.restraints {
            if !restraint_faces.insert(&restraint.face_id)
                || !restraint.fixed_translation.contains(&true)
                || load_faces.contains(&restraint.face_id)
            {
                return Err(invalid(
                    "analysis restraints require unique whole faces, a fixed direction and no loaded-face overlap",
                ));
            }
        }
        Ok(())
    }
    pub fn patch_faces(&self) -> impl Iterator<Item = &FaceId> {
        self.loads
            .iter()
            .map(|load| &load.face_id)
            .chain(self.restraints.iter().map(|restraint| &restraint.face_id))
    }
    pub fn allocated_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.name.capacity()
            + self.loads.capacity() * std::mem::size_of::<AnalysisFaceLoad>()
            + self.restraints.capacity() * std::mem::size_of::<AnalysisFaceRestraint>()
            + self
                .patch_faces()
                .map(FaceId::allocated_bytes)
                .sum::<usize>()
            + self.id.0.capacity()
            + self.layout_id.allocated_bytes()
            + self.definition_id.allocated_bytes()
            + self.source_hash.allocated_bytes()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AnalysisCaseRecord {
    pub case: AnalysisCase,
    pub edited_revision: ProjectRevision,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AnalysisDocument {
    #[serde(deserialize_with = "bounded::<_, _, MAX_ANALYSIS_CASES>")]
    pub cases: Vec<AnalysisCaseRecord>,
}
impl AnalysisDocument {
    pub fn validate(&self) -> Result<(), ProjectError> {
        if self.cases.len() > MAX_ANALYSIS_CASES
            || self.allocated_bytes() > MAX_ANALYSIS_DOCUMENT_BYTES
        {
            return Err(ProjectError::new(
                ProjectErrorCode::ResourceLimit,
                "analysis document exceeds budget",
            ));
        }
        let mut ids = BTreeSet::new();
        let mut names = BTreeSet::new();
        for record in &self.cases {
            record.case.validate()?;
            if !ids.insert(&record.case.id)
                || !names.insert((&record.case.layout_id, &record.case.name))
            {
                return Err(invalid(
                    "duplicate analysis case identity or name within a print layout",
                ));
            }
        }
        Ok(())
    }
    pub fn validate_subjects(
        &self,
        definitions: &[StoredDefinition],
        layouts: &[PrintLayoutRecord],
        revision: ProjectRevision,
    ) -> Result<(), ProjectError> {
        self.validate()?;
        for record in &self.cases {
            let case = &record.case;
            if record.edited_revision > revision
                || !definitions.iter().any(|definition| {
                    definition.definition_id == case.definition_id
                        && definition.provenance.source_hash == case.source_hash
                })
                || !layouts.iter().any(|layout| {
                    layout.layout_id == case.layout_id
                        && layout.occurrences.iter().any(|occurrence| {
                            occurrence.occurrence_id == case.occurrence_id
                                && occurrence.definition_id == case.definition_id
                        })
                })
            {
                return Err(invalid(
                    "analysis case subject or revision differs from the saved native project",
                ));
            }
        }
        Ok(())
    }
    pub fn allocated_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.cases.capacity() * std::mem::size_of::<AnalysisCaseRecord>()
            + self
                .cases
                .iter()
                .map(|record| record.case.allocated_bytes())
                .sum::<usize>()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AnalysisEdit {
    Put { case: AnalysisCase },
    Remove { case_id: AnalysisCaseId },
}
impl AnalysisEdit {
    pub fn validate(&self) -> Result<(), ProjectError> {
        if let Self::Put { case } = self {
            case.validate()?;
            if case.allocated_bytes() > MAX_ANALYSIS_DOCUMENT_BYTES {
                return Err(ProjectError::new(
                    ProjectErrorCode::ResourceLimit,
                    "analysis case allocation exceeds budget",
                ));
            }
        }
        Ok(())
    }
}
