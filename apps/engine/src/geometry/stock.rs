// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
//! Saved-stock composition on the existing native worker/Operations publication path.
use super::*;
use spiling_contracts::{library::*, stock::*};
use std::{path::PathBuf, sync::RwLock};
pub(crate) type LibraryLocation = Arc<RwLock<Option<PathBuf>>>;
pub(crate) struct StockPublication {
    pub record: StockMatchRecord,
    pub bytes: Vec<u8>,
}
pub(super) fn compare(
    materials: Option<Arc<spiling_core::materials::MaterialAsset>>,
    library: Option<PathBuf>,
    query: StockQuery,
    cancel: &AtomicBool,
) -> Result<StockPublication, ProjectError> {
    let invalid = |e: String| project_error(ProjectErrorCode::InvalidProject, &e);
    query.validate().map_err(invalid)?;
    if cancel.load(Ordering::Acquire) {
        let report =
            spiling_appearance::stock::compare(query, None, vec![], &|| true).map_err(invalid)?;
        return StockPublication::new(report);
    }
    let personal = if query.scope != StockScope::Project || query.on_hand_only {
        let path = library.ok_or_else(|| {
            invalid("Choose a personal library before searching this scope or stock".into())
        })?;
        spiling_core::library::LibraryStore::new(path)
            .read(cancel)
            .map_err(|e| {
                project_error(
                    if e.code == LibraryErrorCode::Cancelled {
                        ProjectErrorCode::Cancelled
                    } else {
                        ProjectErrorCode::InvalidProject
                    },
                    &e.to_string(),
                )
            })?
    } else {
        None
    };
    let identity = personal.as_ref().map(|d| LibraryIdentity {
        id: d.id.clone(),
        revision: d.revision,
    });
    let mut candidates = BTreeMap::<FilamentRef, StockCandidate>::new();
    let mut append = |collection: &MaterialCollection, project: bool| -> Result<(), ProjectError> {
        for entry in &collection.entries {
            if entry.archived {
                continue;
            }
            let d = collection
                .resolve(&entry.current)
                .ok_or_else(|| invalid("unresolved stock definition".into()))?;
            let count = personal.as_ref().map_or(0, |p| {
                p.spools
                    .iter()
                    .filter(|s| s.state == StockState::OnHand && s.filament.id == entry.current.id)
                    .count() as u32
            });
            if query.on_hand_only && count == 0 {
                continue;
            }
            let row = candidates
                .entry(entry.current.clone())
                .or_insert_with(|| StockCandidate {
                    filament: entry.current.clone(),
                    name: d.name.clone(),
                    appearance: d.appearance.clone(),
                    hueforge_td_mm: d.declaration.hueforge_td_mm,
                    in_project: false,
                    in_library: false,
                    on_hand_spools: count,
                    profile_count: d.profiles.len() as u32,
                });
            if project {
                row.in_project = true;
            } else {
                row.in_library = true;
            }
        }
        Ok(())
    };
    if query.scope != StockScope::MyLibrary
        && let Some(asset) = materials
    {
        append(&asset.document()?, true)?;
    }
    if query.scope != StockScope::Project
        && let Some(p) = &personal
    {
        append(&p.materials, false)?;
    }
    let report = spiling_appearance::stock::compare(
        query,
        identity,
        candidates.into_values().collect(),
        &|| cancel.load(Ordering::Acquire),
    )
    .map_err(invalid)?;
    StockPublication::new(report)
}
impl StockPublication {
    fn new(report: StockMatchReport) -> Result<Self, ProjectError> {
        let bytes = report
            .encode()
            .map_err(|e| project_error(ProjectErrorCode::InvalidProject, &e))?;
        let record = StockMatchRecord {
            hash: SourceHash::from_bytes(&bytes),
            byte_count: bytes.len() as u64,
            input_hash: report.input_hash,
            producer: STOCK_PRODUCER.into(),
            complete: report.status == StockSearchStatus::Complete,
        };
        Ok(Self { record, bytes })
    }
}

impl Runtime {
    pub(crate) fn match_stock(
        &mut self,
        session: &SessionId,
        base: ProjectRevision,
        query: StockQuery,
        initially_cancelled: bool,
    ) -> Result<Reply, ProjectError> {
        self.appearance_base(session, base)?;
        query
            .validate()
            .map_err(|e| project_error(ProjectErrorCode::InvalidProject, &e))?;
        let materials = self.project.snapshot().materials.clone();
        let library = self
            .stock_library
            .as_ref()
            .and_then(|l| l.read().ok().and_then(|p| p.clone()));
        self.start_with_cancel(
            Purpose::MatchStock,
            0,
            initially_cancelled,
            move |_, _, _| Work::MatchStock {
                materials,
                library,
                query,
            },
        )
        .map_err(super::project_geometry_error)
    }
    pub fn stock_publication(&self, id: JobId) -> Option<&Arc<StockPublication>> {
        self.stock_publication
            .as_ref()
            .filter(|(job, _)| *job == id)
            .map(|(_, r)| r)
    }
    pub fn release_stock_publication(&mut self, id: JobId) {
        if self
            .stock_publication
            .as_ref()
            .is_some_and(|(job, _)| *job == id)
        {
            self.stock_publication = None;
        }
    }
}
