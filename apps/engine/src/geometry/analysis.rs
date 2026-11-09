// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use spiling_contracts::analysis::{mechanics::*, *};

impl Runtime {
    pub(super) fn attach_analysis_result(
        &mut self,
        session: &SessionId,
        base: ProjectRevision,
        input: &SourceHash,
    ) -> Result<Reply, ProjectError> {
        self.metadata_base(session, base)?;
        let product = self
            .analysis_candidate
            .as_ref()
            .filter(|product| {
                &product.summary.input_hash == input
                    && product.summary.captured_revision == base
                    && product.summary.project_id == self.project.info().project_id
            })
            .ok_or_else(|| {
                project_error(
                    ProjectErrorCode::StaleRevision,
                    "no matching current successful analysis completion to attach",
                )
            })?;
        let record = AnalysisResultRecord {
            case_id: product.summary.case.case.id.clone(),
            hash: SourceHash::from_bytes(&product.bytes),
            byte_count: product.bytes.len() as u32,
            input_hash: input.clone(),
            captured_revision: base,
        };
        let asset = Arc::new(spiling_core::AnalysisResultAsset::new(
            record,
            product.bytes.clone(),
        )?);
        self.commit_metadata_edit(Edit::AttachAnalysisResult { asset })
    }
    pub(super) fn get_analysis_result(
        &mut self,
        session: &SessionId,
        base: ProjectRevision,
        id: &AnalysisCaseId,
    ) -> Result<Reply, ProjectError> {
        self.appearance_base(session, base)?;
        let snapshot = self.project.snapshot();
        let asset = snapshot
            .analysis
            .results
            .iter()
            .find(|record| &record.case_id == id)
            .and_then(|record| snapshot.analysis_results.get(&record.hash))
            .cloned()
            .ok_or_else(|| {
                project_error(
                    ProjectErrorCode::MissingAsset,
                    "no portable result attached to this case",
                )
            })?;
        self.start(Purpose::GetAnalysisResult, 0, move |_, _, _| {
            Work::GetAnalysisResult(asset)
        })
        .map_err(project_geometry_error)
    }
    fn analysis_capture(
        &self,
        session: &SessionId,
        base: ProjectRevision,
        id: &AnalysisCaseId,
        settings: Arc<AnalysisRunSettings>,
    ) -> Result<spiling_analysis::run::RunCapture, ProjectError> {
        self.appearance_base(session, base)?;
        settings.validate()?;
        let snapshot = self.project.snapshot();
        let case = snapshot
            .analysis
            .cases
            .iter()
            .find(|record| &record.case.id == id)
            .cloned()
            .ok_or_else(|| {
                project_error(
                    ProjectErrorCode::MissingAsset,
                    "saved analysis case is absent",
                )
            })?;
        if case.case.mode != AnalysisMode::Basic {
            return Err(project_error(
                ProjectErrorCode::InvalidProject,
                "Enhanced detail is not admitted by the assumed-law macro operator; no Basic fallback",
            ));
        }
        let layout = snapshot
            .layouts
            .iter()
            .find(|layout| layout.layout_id == case.case.layout_id)
            .ok_or_else(|| {
                project_error(ProjectErrorCode::MissingAsset, "analysis layout absent")
            })?;
        let occurrence = layout
            .occurrences
            .get(&case.case.occurrence_id)
            .ok_or_else(|| {
                project_error(ProjectErrorCode::MissingAsset, "analysis occurrence absent")
            })?;
        if occurrence.definition_id != case.case.definition_id
            || !snapshot.sources.contains_key(&case.case.definition_id)
            || !self.definitions.contains_key(&case.case.definition_id)
        {
            return Err(project_error(
                ProjectErrorCode::StaleRevision,
                "analysis subject/native source changed",
            ));
        }
        if !snapshot.surface_overlays.layers.is_empty()
            || !snapshot.process_document.layers.is_empty()
        {
            return Err(project_error(
                ProjectErrorCode::InvalidProject,
                "assumed source-only construction does not admit surface additions or regional process layers; nothing is silently dropped",
            ));
        }
        if case.case.loads.is_empty() || case.case.restraints.is_empty() {
            return Err(project_error(
                ProjectErrorCode::InvalidProject,
                "mechanical execution requires explicit finite loads and restraints",
            ));
        }
        Ok(spiling_analysis::run::RunCapture {
            project_id: self.project.info().project_id,
            case,
            revision: base,
            pose: occurrence.pose,
            settings: (*settings).clone(),
        })
    }
    pub(crate) fn validate_analysis_admission(
        &self,
        command: &ProjectCommand,
    ) -> Result<(), ProjectError> {
        if let ProjectCommand::RunAnalysis {
            session_id,
            base_revision,
            case_id,
            settings,
        } = command
        {
            self.analysis_capture(session_id, *base_revision, case_id, settings.clone())?;
        }
        Ok(())
    }
    pub(super) fn run_analysis(
        &mut self,
        session: &SessionId,
        base: ProjectRevision,
        id: &AnalysisCaseId,
        settings: Arc<AnalysisRunSettings>,
    ) -> Result<Reply, ProjectError> {
        let capture = self.analysis_capture(session, base, id, settings)?;
        self.start(
            Purpose::RunAnalysis {
                maximum_seconds: capture.settings.maximum_seconds,
            },
            0,
            move |_, _, _| Work::RunAnalysis(capture),
        )
        .map_err(project_geometry_error)
    }
    pub fn analysis_publication(
        &self,
        id: JobId,
    ) -> Option<&Arc<spiling_analysis::run::RunProduct>> {
        self.analysis_publication
            .as_ref()
            .filter(|(job, _)| *job == id)
            .map(|(_, report)| report)
    }
    pub fn release_analysis_publication(&mut self, id: JobId) {
        if self
            .analysis_publication
            .as_ref()
            .is_some_and(|(job, _)| *job == id)
        {
            self.analysis_publication = None;
        }
    }
}
pub(super) fn mechanical_error(message: String, cancel: &AtomicBool) -> ProjectError {
    let code = if cancel.load(Ordering::Acquire) {
        ProjectErrorCode::Cancelled
    } else if message.starts_with("ResourceLimit:") {
        ProjectErrorCode::ResourceLimit
    } else {
        ProjectErrorCode::InvalidProject
    };
    project_error(code, &message)
}
