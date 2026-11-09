// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use spiling_contracts::analysis::{mechanics::*, *};

pub(super) struct FaceRequest {
    pub project_id: ProjectId,
    pub project_revision: ProjectRevision,
    pub reference: FaceRef,
    pub source_hash: SourceHash,
    pub pose: RigidPoseMm,
}

pub(super) fn validate_contact(
    definition: &spiling_geometry::Definition,
    load: &AnalysisFaceLoad,
    cancel: &AtomicBool,
) -> Result<(), ProjectError> {
    let load::ForceFootprint::Disk { center_mm, .. } = load.footprint else {
        return Ok(());
    };
    let mut work = spiling_geometry::GeometryWorkBudget::query();
    let face = definition
        .vertical_rectangular_face_with_work(&load.face_id, &mut work, cancel)
        .map_err(project_geometry_error)?;
    let horizontal: f64 = (0..3)
        .map(|axis| (center_mm[axis] - face.origin_mm[axis]) * face.horizontal_direction[axis])
        .sum();
    let plane: f64 = (0..3)
        .map(|axis| (center_mm[axis] - face.origin_mm[axis]) * face.outward_normal[axis])
        .sum();
    if plane.abs() > 1e-7
        || horizontal < -1e-7
        || horizontal > face.width_mm + 1e-7
        || center_mm[2] < face.min_z_mm - 1e-7
        || center_mm[2] > face.max_z_mm + 1e-7
    {
        return Err(project_error(
            ProjectErrorCode::InvalidProject,
            "stored contact is outside its authoritative native trimmed face",
        ));
    }
    Ok(())
}

pub(super) fn prepare_face(
    definition: &spiling_geometry::Definition,
    request: FaceRequest,
    cancel: &AtomicBool,
) -> Result<load::AnalysisFacePreparation, JobError> {
    let mut work = spiling_geometry::GeometryWorkBudget::query();
    let face = definition.vertical_rectangular_face_with_work(
        &request.reference.face_id,
        &mut work,
        cancel,
    )?;
    let height = face.max_z_mm - face.min_z_mm;
    let mut centroid_mm = std::array::from_fn(|axis| {
        face.origin_mm[axis] + face.horizontal_direction[axis] * face.width_mm * 0.5
    });
    centroid_mm[2] = face.min_z_mm + height * 0.5;
    let preparation = load::AnalysisFacePreparation {
        project_id: request.project_id,
        project_revision: request.project_revision,
        reference: request.reference,
        source_hash: request.source_hash,
        pose: request.pose,
        area_mm2: face.width_mm * height,
        centroid_mm,
        outward_normal: face.outward_normal,
        origin_mm: face.origin_mm,
        horizontal_direction: face.horizontal_direction,
        width_mm: face.width_mm,
        height_mm: height,
    };
    preparation.validate()?;
    Ok(preparation)
}

impl Runtime {
    pub(super) fn prepare_analysis_face(
        &mut self,
        session: &SessionId,
        base: ProjectRevision,
        reference: FaceRef,
    ) -> Result<Reply, ProjectError> {
        self.appearance_base(session, base)?;
        self.live_surface_target(&reference)?;
        let snapshot = self.project.snapshot();
        let occurrence = &snapshot.occurrences[&reference.occurrence_id];
        let source_hash = self.definitions[&reference.definition_id]
            .record
            .provenance
            .source_hash
            .clone();
        let request = FaceRequest {
            project_id: self.project.info().project_id,
            project_revision: base,
            reference,
            source_hash,
            pose: occurrence.pose,
        };
        self.start(Purpose::PrepareAnalysisFace, 0, move |_, _, _| {
            Work::PrepareAnalysisFace(request)
        })
        .map_err(project_geometry_error)
    }
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
        if let ProjectCommand::PrepareAnalysisFace {
            session_id,
            base_revision,
            reference,
        } = command
        {
            self.appearance_base(session_id, *base_revision)?;
            self.live_surface_target(reference)?;
        }
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
