// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Study-only optical composition on the existing worker and promotion ACK path.
use super::*;
use spiling_contracts::optical::*;
use spiling_core::PaletteAsset;

pub(crate) struct OpticalReportPublication {
    pub report: Arc<OpticalReport>,
    pub record: OpticalReportRecord,
    pub bytes: Vec<u8>,
}
impl OpticalReportPublication {
    pub fn new(report: Arc<OpticalReport>) -> Result<Self, ProjectError> {
        let bytes = report.encode()?;
        let record = OpticalReportRecord {
            hash: SourceHash::from_bytes(&bytes),
            byte_count: bytes.len() as u64,
            input_hash: report.input_hash.clone(),
            producer: report.producer.clone(),
            complete: report.is_complete(),
        };
        Ok(Self { report, record, bytes })
    }
}

pub(super) fn compare_palette(
    asset: Arc<PaletteAsset>,
    request: Arc<OpticalRequest>,
    cancel: &AtomicBool,
) -> Result<Arc<OpticalReportPublication>, ProjectError> {
    let report = spiling_appearance::optical::compare(asset.prepared(), request, &|| {
        cancel.load(Ordering::Acquire)
    })?;
    OpticalReportPublication::new(Arc::new(report)).map(Arc::new)
}

impl Runtime {
    fn optical_inputs(&self) -> Result<(Arc<PaletteAsset>, Arc<OpticalRequest>), ProjectError> {
        let study = self.project.appearance_study();
        let palette = study.palette.as_ref().ok_or_else(|| {
            project_error(ProjectErrorCode::InvalidProject, "optical study has no imported palette")
        })?;
        let optical = study.optical.as_ref().ok_or_else(|| {
            project_error(ProjectErrorCode::InvalidProject, "optical study has no explicit request")
        })?;
        let asset = self.project.snapshot().appearance_palettes.get(&palette.canonical_hash)
            .cloned().ok_or_else(|| {
                project_error(ProjectErrorCode::CorruptAsset, "optical palette asset is missing")
            })?;
        optical.request.validate_palette(asset.prepared())?;
        Ok((asset, optical.request.clone()))
    }

    pub(crate) fn validate_optical_admission(&self, command: &ProjectCommand) -> Result<(), ProjectError> {
        match command {
            ProjectCommand::SetOpticalStudy { session_id, base_revision, request } => {
                self.metadata_base(session_id, *base_revision)?;
                let snapshot = self.project.snapshot();
                let palette = snapshot.appearance_study.palette.as_ref().ok_or_else(|| {
                    project_error(ProjectErrorCode::InvalidProject, "optical study requires an imported palette")
                })?;
                let asset = snapshot.appearance_palettes.get(&palette.canonical_hash).ok_or_else(|| {
                    project_error(ProjectErrorCode::CorruptAsset, "optical palette asset is missing")
                })?;
                request.validate_palette(asset.prepared())
            }
            ProjectCommand::CompareOptical { session_id, base_revision } => {
                self.appearance_base(session_id, *base_revision)?;
                self.optical_inputs().map(|_| ())
            }
            ProjectCommand::GetOpticalReport { session_id, base_revision } => {
                self.appearance_base(session_id, *base_revision)?;
                if self.project.appearance_study().optical.as_ref().and_then(|study| study.report.as_ref()).is_none() {
                    return Err(project_error(ProjectErrorCode::InvalidProject, "optical study has no accepted report"));
                }
                Ok(())
            }
            ProjectCommand::AcceptOpticalReport { session_id, base_revision, .. } => {
                self.appearance_base(session_id, *base_revision)?;
                self.metadata_base(session_id, *base_revision)
            }
            _ => Ok(()),
        }
    }

    pub(super) fn compare_optical(&mut self, session: &SessionId, base: ProjectRevision) -> Result<Reply, ProjectError> {
        self.compare_optical_with_initial_cancel(session, base, false)
    }
    pub(crate) fn compare_optical_with_initial_cancel(
        &mut self,
        session: &SessionId,
        base: ProjectRevision,
        initially_cancelled: bool,
    ) -> Result<Reply, ProjectError> {
        self.appearance_base(session, base)?;
        let (asset, request) = self.optical_inputs()?;
        self.optical_comparison = None;
        self.start_with_cancel(Purpose::CompareOptical, 0, initially_cancelled, move |_, _, _| {
            Work::CompareOptical { asset, request }
        }).map_err(super::project_geometry_error)
    }
    pub(super) fn get_optical_report(&mut self, session: &SessionId, base: ProjectRevision) -> Result<Reply, ProjectError> {
        self.appearance_base(session, base)?;
        let report = self.project.appearance_study().optical.as_ref().and_then(|study| study.report.clone())
            .ok_or_else(|| project_error(ProjectErrorCode::InvalidProject, "optical study has no accepted report"))?;
        self.start(Purpose::GetOpticalReport, 0, move |_, _, _| Work::GetOpticalReport { report })
            .map_err(super::project_geometry_error)
    }
    pub(super) fn accept_optical_report(
        &mut self,
        session: &SessionId,
        base: ProjectRevision,
        hash: &SourceHash,
        selection: Option<OpticalSelection>,
    ) -> Result<Reply, ProjectError> {
        self.appearance_base(session, base)?;
        self.metadata_base(session, base)?;
        let (project_id, revision, publication) = self.optical_comparison.as_ref().ok_or_else(|| {
            project_error(ProjectErrorCode::StaleRevision, "no complete current optical comparison to accept")
        })?;
        if project_id != &self.project.info().project_id || *revision != base
            || &publication.record.hash != hash || !publication.record.complete
        {
            return Err(project_error(ProjectErrorCode::StaleRevision, "optical comparison is incomplete or no longer current"));
        }
        self.commit_metadata_edit(Edit::AcceptOpticalReport { report: publication.report.clone(), selection })
    }
    pub fn optical_publication(&self, id: JobId) -> Option<&Arc<OpticalReportPublication>> {
        self.optical_publication.as_ref().filter(|(job, _)| *job == id).map(|(_, report)| report)
    }
    pub fn release_optical_publication(&mut self, id: JobId) {
        if self.optical_publication.as_ref().is_some_and(|(job, _)| *job == id) {
            self.optical_publication = None;
        }
    }
}
