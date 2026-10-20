// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Appearance composition uses the existing session worker, not geometry or Compile.
use super::*;
use spiling_contracts::appearance::*;
use spiling_core::PaletteAsset;

pub(crate) struct ReportPublication {
    pub report: Arc<AppearanceReport>,
    pub record: AppearanceReportRecord,
    pub bytes: Vec<u8>,
}
impl ReportPublication {
    pub fn new(report: Arc<AppearanceReport>) -> Result<Self, ProjectError> {
        let bytes = report.encode()?;
        // encode validates the report. Bind that exact owned buffer directly,
        // rather than serializing the unchanged report again for its identity.
        let record = AppearanceReportRecord {
            hash: SourceHash::from_bytes(&bytes),
            byte_count: bytes.len() as u64,
            input_hash: report.input_hash.clone(),
            producer: report.producer.clone(),
            complete: report.is_complete(),
        };
        Ok(Self {
            report,
            record,
            bytes,
        })
    }
}

pub(super) fn import_palette(
    path: NativePath,
    remaining: usize,
    cancel: &AtomicBool,
) -> Result<Arc<PaletteAsset>, ProjectError> {
    // Original, canonical and bounded parsed records coexist. Do not release
    // history or the old project to admit a new import.
    let input_limit = (remaining / 3).min(MAX_PALETTE_BYTES as usize);
    let captured =
        source::capture(path, input_limit as u32, cancel).map_err(super::project_geometry_error)?;
    let metadata_reserve = std::mem::size_of::<PaletteRecord>() + 2048;
    let parsed_budget = remaining
        .saturating_sub(captured.bytes.capacity())
        .saturating_sub(metadata_reserve);
    if PreparedPalette::allocation_bound(&captured.bytes)? > parsed_budget {
        return Err(project_error(
            ProjectErrorCode::ResourceLimit,
            "palette prepared records exceed remaining retained capacity",
        ));
    }
    let document = PaletteDocument::decode_bounded(&captured.bytes, parsed_budget)?;
    let cache_reserve = PreparedPalette::cache_allocation_bound(document.rows.len());
    let canonical_budget = parsed_budget
        .saturating_sub(document.allocated_bytes())
        .saturating_sub(cache_reserve);
    let canonical = document.canonical_bytes_bounded(canonical_budget)?;
    if captured
        .bytes
        .capacity()
        .checked_add(canonical.capacity())
        .and_then(|bytes| bytes.checked_add(document.allocated_bytes()))
        .and_then(|bytes| bytes.checked_add(cache_reserve))
        .and_then(|bytes| bytes.checked_add(metadata_reserve))
        .is_none_or(|bytes| bytes > remaining)
    {
        return Err(project_error(
            ProjectErrorCode::ResourceLimit,
            "palette original/canonical/parsed allocations exceed remaining retained capacity",
        ));
    }
    let record = PaletteRecord::new(
        &document,
        &captured.bytes,
        &canonical,
        document.name.clone(),
    )?;
    check_cancel(cancel).map_err(super::project_geometry_error)?;
    PaletteAsset::from_document(record, captured.bytes, canonical, document).map(Arc::new)
}

pub(super) fn compare_palette(
    asset: Arc<PaletteAsset>,
    target: AppearanceTarget,
    limits: SearchLimits,
    cancel: &AtomicBool,
) -> Result<Arc<ReportPublication>, ProjectError> {
    let report = spiling_appearance::select_single(asset.prepared(), &target, limits, &|| {
        cancel.load(Ordering::Acquire)
    })?;
    ReportPublication::new(Arc::new(report)).map(Arc::new)
}

impl Runtime {
    fn appearance_base(
        &self,
        session: &SessionId,
        base: ProjectRevision,
    ) -> Result<(), ProjectError> {
        if session != &self.session_id || base != self.project.info().revision {
            return Err(project_error(
                ProjectErrorCode::StaleRevision,
                "appearance session or project revision is stale",
            ));
        }
        if self.active.is_some() {
            return Err(project_error(
                ProjectErrorCode::Busy,
                "an engine operation is active",
            ));
        }
        Ok(())
    }
    pub(crate) fn validate_appearance_admission(
        &self,
        command: &ProjectCommand,
    ) -> Result<(), ProjectError> {
        match command {
            ProjectCommand::ImportAppearancePalette {
                session_id,
                base_revision,
                source,
            } => {
                self.appearance_base(session_id, *base_revision)?;
                self.metadata_base(session_id, *base_revision)?;
                source.validate().map_err(super::project_geometry_error)
            }
            ProjectCommand::CompareAppearance {
                session_id,
                base_revision,
            } => {
                self.appearance_base(session_id, *base_revision)?;
                let study = self.project.appearance_study();
                if study.palette.is_none() || study.target.is_none() {
                    return Err(project_error(
                        ProjectErrorCode::InvalidProject,
                        "appearance comparison requires an imported palette and explicit target",
                    ));
                }
                Ok(())
            }
            ProjectCommand::GetAppearanceReport {
                session_id,
                base_revision,
            } => {
                self.appearance_base(session_id, *base_revision)?;
                if self.project.appearance_study().report.is_none() {
                    return Err(project_error(
                        ProjectErrorCode::InvalidProject,
                        "appearance study has no accepted report",
                    ));
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
    pub(super) fn import_appearance_palette(
        &mut self,
        session: &SessionId,
        base: ProjectRevision,
        source: NativePath,
    ) -> Result<Reply, ProjectError> {
        self.appearance_base(session, base)?;
        self.metadata_base(session, base)?;
        source.validate().map_err(super::project_geometry_error)?;
        let remaining = spiling_core::MAX_APPEARANCE_RETAINED_BYTES
            .checked_sub(self.project.retained_appearance_bytes()?)
            .ok_or_else(|| {
                project_error(
                    ProjectErrorCode::ResourceLimit,
                    "retained appearance allocation budget exceeded",
                )
            })?;
        self.start(Purpose::ImportAppearancePalette, 0, move |_, _, _| {
            Work::ImportAppearancePalette {
                source,
                retained_budget: remaining,
            }
        })
        .map_err(super::project_geometry_error)
    }
    pub(super) fn compare_appearance(
        &mut self,
        session: &SessionId,
        base: ProjectRevision,
    ) -> Result<Reply, ProjectError> {
        self.compare_appearance_with_initial_cancel(session, base, false)
    }
    pub(crate) fn compare_appearance_with_initial_cancel(
        &mut self,
        session: &SessionId,
        base: ProjectRevision,
        initially_cancelled: bool,
    ) -> Result<Reply, ProjectError> {
        self.appearance_base(session, base)?;
        let study = self.project.appearance_study();
        let palette = study.palette.as_ref().ok_or_else(|| {
            project_error(
                ProjectErrorCode::InvalidProject,
                "appearance study has no imported palette",
            )
        })?;
        let asset = self
            .project
            .snapshot()
            .appearance_palettes
            .get(&palette.canonical_hash)
            .cloned()
            .ok_or_else(|| {
                project_error(
                    ProjectErrorCode::CorruptAsset,
                    "appearance palette asset missing",
                )
            })?;
        let target = study.target.clone().ok_or_else(|| {
            project_error(
                ProjectErrorCode::InvalidProject,
                "appearance study has no explicit target",
            )
        })?;
        let limits = study.limits;
        self.appearance_comparison = None;
        self.start_with_cancel(
            Purpose::CompareAppearance,
            0,
            initially_cancelled,
            move |_, _, _| Work::CompareAppearance {
                asset,
                target,
                limits,
            },
        )
        .map_err(super::project_geometry_error)
    }
    pub(super) fn get_appearance_report(
        &mut self,
        session: &SessionId,
        base: ProjectRevision,
    ) -> Result<Reply, ProjectError> {
        self.appearance_base(session, base)?;
        let report = self
            .project
            .appearance_study()
            .report
            .clone()
            .ok_or_else(|| {
                project_error(
                    ProjectErrorCode::InvalidProject,
                    "appearance study has no accepted report",
                )
            })?;
        self.start(Purpose::GetAppearanceReport, 0, move |_, _, _| {
            Work::GetAppearanceReport { report }
        })
        .map_err(super::project_geometry_error)
    }
    pub(super) fn accept_appearance_report(
        &mut self,
        session: &SessionId,
        base: ProjectRevision,
        hash: &SourceHash,
        selection: Option<String>,
    ) -> Result<Reply, ProjectError> {
        self.appearance_base(session, base)?;
        self.metadata_base(session, base)?;
        let (project_id, revision, publication) =
            self.appearance_comparison.as_ref().ok_or_else(|| {
                project_error(
                    ProjectErrorCode::StaleRevision,
                    "no complete current appearance comparison to accept",
                )
            })?;
        if project_id != &self.project.info().project_id
            || *revision != base
            || &publication.record.hash != hash
            || !publication.record.complete
        {
            return Err(project_error(
                ProjectErrorCode::StaleRevision,
                "appearance comparison is incomplete or no longer current",
            ));
        }
        self.commit_metadata_edit(Edit::AcceptAppearanceReport {
            report: publication.report.clone(),
            selection,
        })
    }
    pub fn appearance_publication(&self, id: JobId) -> Option<&Arc<ReportPublication>> {
        self.appearance_publication
            .as_ref()
            .filter(|(job, _)| *job == id)
            .map(|(_, report)| report)
    }
    pub fn release_appearance_publication(&mut self, id: JobId) {
        if self
            .appearance_publication
            .as_ref()
            .is_some_and(|(job, _)| *job == id)
        {
            self.appearance_publication = None;
        }
    }
}
