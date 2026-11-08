// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Authoritative independent print jobs. Focus is a transient projection, never saved content.
use super::*;

#[derive(Debug, Clone)]
pub struct PrintLayoutSnapshot {
    pub layout_id: PrintLayoutId,
    pub name: String,
    pub occurrences: BTreeMap<OccurrenceId, OccurrenceRecord>,
    pub manufacturing_intent: Option<Arc<ManufacturingIntent>>,
    pub manufacturing_options: Arc<Vec<ManufacturingOption>>,
    pub manufacturing_artifact: Option<Arc<ManufacturingAsset>>,
    pub hardware: Option<Arc<spiling_contracts::hardware::ProjectHardwareCapture>>,
}
impl Default for PrintLayoutSnapshot {
    fn default() -> Self {
        Self {
            layout_id: PrintLayoutId::new(),
            name: "Plate 1".into(),
            occurrences: BTreeMap::new(),
            manufacturing_intent: None,
            manufacturing_options: Arc::new(vec![]),
            manufacturing_artifact: None,
            hardware: None,
        }
    }
}
impl PrintLayoutSnapshot {
    pub fn record(&self) -> PrintLayoutRecord {
        PrintLayoutRecord {
            layout_id: self.layout_id.clone(),
            name: self.name.clone(),
            occurrences: self.occurrences.values().cloned().collect(),
            hardware: self.hardware.clone(),
            manufacturing_intent: self.manufacturing_intent.clone(),
            manufacturing_options: self.manufacturing_options.as_ref().clone(),
            manufacturing_artifact: self
                .manufacturing_artifact
                .as_ref()
                .map(|a| a.record.clone()),
        }
    }
    pub fn info(&self) -> PrintLayoutInfo {
        PrintLayoutInfo {
            layout_id: self.layout_id.clone(),
            name: self.name.clone(),
            occurrence_count: self.occurrences.len() as u32,
            configured: self.manufacturing_intent.is_some(),
            has_result: self.manufacturing_artifact.is_some(),
        }
    }
}
impl Default for Snapshot {
    fn default() -> Self {
        let layout = PrintLayoutSnapshot::default();
        Self {
            focused_layout: layout.layout_id.clone(),
            layouts: vec![layout],
            sources: BTreeMap::new(),
            surface_overlays: Default::default(),
            process_document: Default::default(),
            surface_images: BTreeMap::new(),
            appearance_study: Default::default(),
            analysis: Default::default(),
            materials: None,
            appearance_palettes: BTreeMap::new(),
        }
    }
}
// Existing single-job authoring code operates on this explicit transient projection.
// There is one stored record for each job; selection never copies or duplicates job authority.
impl std::ops::Deref for Snapshot {
    type Target = PrintLayoutSnapshot;
    fn deref(&self) -> &Self::Target {
        self.layouts
            .iter()
            .find(|l| l.layout_id == self.focused_layout)
            .expect("validated focused print layout")
    }
}
impl std::ops::DerefMut for Snapshot {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.layouts
            .iter_mut()
            .find(|l| l.layout_id == self.focused_layout)
            .expect("validated focused print layout")
    }
}
impl Snapshot {
    pub fn active_sources(&self) -> impl Iterator<Item = &Arc<SourceAsset>> {
        self.sources
            .iter()
            .filter(|(id, _)| self.occurrences.values().any(|o| &o.definition_id == *id))
            .map(|(_, a)| a)
    }
    pub fn all_occurrences(&self) -> impl Iterator<Item = &OccurrenceRecord> {
        self.layouts.iter().flat_map(|l| l.occurrences.values())
    }
    pub fn layout_records(&self) -> Vec<PrintLayoutRecord> {
        self.layouts
            .iter()
            .map(PrintLayoutSnapshot::record)
            .collect()
    }
    pub fn scoped_documents(&self) -> (SurfaceOverlays, ProcessDocument) {
        layout_documents(
            &self.occurrences.values().cloned().collect::<Vec<_>>(),
            &self.surface_overlays,
            &self.process_document,
        )
    }
    /// Preserve immutable results only when their full scoped semantic input is unchanged.
    /// Fresh actual-program inspection remains mandatory before delivery.
    pub fn invalidate_changed_layout_results(
        &mut self,
        project: &ProjectId,
    ) -> Result<(), ProjectError> {
        let definitions = self
            .sources
            .values()
            .map(|a| a.record.clone())
            .collect::<Vec<_>>();
        for layout in &mut self.layouts {
            let Some(asset) = &layout.manufacturing_artifact else {
                continue;
            };
            let rows = layout.occurrences.values().cloned().collect::<Vec<_>>();
            let used = rows
                .iter()
                .map(|o| &o.definition_id)
                .collect::<std::collections::BTreeSet<_>>();
            let definitions = definitions
                .iter()
                .filter(|d| used.contains(&d.definition_id))
                .cloned()
                .collect::<Vec<_>>();
            let (surface, process) =
                layout_documents(&rows, &self.surface_overlays, &self.process_document);
            let hash = layout
                .manufacturing_intent
                .as_ref()
                .map(|intent| {
                    input_fingerprint(project, &definitions, &rows, intent, &surface, &process)
                })
                .transpose()
                .map_err(manufacturing_error)?;
            if hash.as_ref() != Some(&asset.record.input_hash) {
                layout.manufacturing_artifact = None;
            }
        }
        Ok(())
    }
}
impl Project {
    pub fn select_layout(&mut self, layout_id: &PrintLayoutId) -> Result<(), ProjectError> {
        if !self
            .current
            .layouts
            .iter()
            .any(|l| &l.layout_id == layout_id)
        {
            return Err(invalid("unknown print layout"));
        }
        Arc::make_mut(&mut self.current).focused_layout = layout_id.clone();
        Ok(())
    }
}

pub(crate) fn apply(
    snapshot: &mut Snapshot,
    edit: PrintLayoutEdit,
    next_occurrence: &mut u32,
    next_surface: &mut u32,
    next_process: &mut u32,
    next_operation: &mut u32,
) -> Result<(), ProjectError> {
    edit.validate()?;
    let index = |s: &Snapshot, id: &PrintLayoutId| {
        s.layouts
            .iter()
            .position(|l| &l.layout_id == id)
            .ok_or_else(|| invalid("unknown print layout"))
    };
    match edit {
        PrintLayoutEdit::Create {
            name,
            copy_setup_from,
        } => {
            if snapshot.layouts.len() == MAX_PRINT_LAYOUTS {
                return Err(error(
                    ProjectErrorCode::ResourceLimit,
                    "project already has 16 plates",
                ));
            }
            let mut layout = PrintLayoutSnapshot {
                name,
                ..Default::default()
            };
            if let Some(id) = copy_setup_from {
                let source = &snapshot.layouts[index(snapshot, &id)?];
                layout.hardware = source.hardware.clone();
                layout.manufacturing_intent = source.manufacturing_intent.clone();
                // Occurrence-bound routes cannot be copied as an apparently configured empty job.
                if layout.manufacturing_intent.as_ref().is_some_and(|i| {
                    matches!(
                        i.printer.workflow,
                        spiling_contracts::manufacturing::JobWorkflow::FixedDualMarlin { .. }
                    )
                }) {
                    layout.manufacturing_intent = None;
                }
            }
            snapshot.focused_layout = layout.layout_id.clone();
            snapshot.layouts.push(layout);
        }
        PrintLayoutEdit::Duplicate { source, name } => {
            let a = index(snapshot, &source)?;
            let occurrences = snapshot.layouts[a]
                .occurrences
                .keys()
                .copied()
                .collect::<Vec<_>>();
            if snapshot.layouts[a]
                .manufacturing_intent
                .as_ref()
                .is_some_and(|i| {
                    matches!(
                        i.printer.workflow,
                        spiling_contracts::manufacturing::JobWorkflow::FixedDualMarlin { .. }
                    )
                })
            {
                return Err(invalid(
                    "remove fixed-dual assignments before duplicating a plate",
                ));
            }
            apply(
                snapshot,
                PrintLayoutEdit::Create {
                    name,
                    copy_setup_from: Some(source.clone()),
                },
                next_occurrence,
                next_surface,
                next_process,
                next_operation,
            )?;
            let target = snapshot.focused_layout.clone();
            if !occurrences.is_empty() {
                apply(
                    snapshot,
                    PrintLayoutEdit::Transfer {
                        source,
                        target,
                        occurrences,
                        copy: true,
                    },
                    next_occurrence,
                    next_surface,
                    next_process,
                    next_operation,
                )?;
            }
        }
        PrintLayoutEdit::Rename { layout_id, name } => {
            let i = index(snapshot, &layout_id)?;
            snapshot.layouts[i].name = name;
        }
        PrintLayoutEdit::Reorder { layout_ids } => {
            if layout_ids.len() != snapshot.layouts.len() {
                return Err(invalid("plate order must include every plate"));
            }
            for id in &layout_ids {
                index(snapshot, id)?;
            }
            snapshot.layouts.sort_by_key(|l| {
                layout_ids
                    .iter()
                    .position(|id| id == &l.layout_id)
                    .expect("checked plate order")
            });
        }
        PrintLayoutEdit::Remove { layout_id } => {
            if snapshot.layouts.len() == 1 {
                return Err(invalid("the project must retain one plate"));
            }
            let i = index(snapshot, &layout_id)?;
            snapshot.layouts.remove(i);
            if snapshot.focused_layout == layout_id {
                snapshot.focused_layout = snapshot.layouts[i.min(snapshot.layouts.len() - 1)]
                    .layout_id
                    .clone();
            }
            prune(snapshot);
        }
        PrintLayoutEdit::Transfer {
            source,
            target,
            occurrences,
            copy,
        } => {
            let a = index(snapshot, &source)?;
            let b = index(snapshot, &target)?;
            if [a, b].iter().any(|i| {
                snapshot.layouts[*i]
                    .manufacturing_intent
                    .as_ref()
                    .is_some_and(|i| {
                        matches!(
                            i.printer.workflow,
                            spiling_contracts::manufacturing::JobWorkflow::FixedDualMarlin { .. }
                        )
                    })
            }) {
                return Err(invalid(
                    "fixed-dual occurrence assignments must be removed before transferring parts",
                ));
            }
            if occurrences
                .iter()
                .any(|id| !snapshot.layouts[a].occurrences.contains_key(id))
            {
                return Err(invalid(
                    "a transferred part no longer belongs to the source plate",
                ));
            }
            let original_focus = snapshot.focused_layout.clone();
            snapshot.focused_layout = source;
            let ids = if copy {
                let begin = *next_occurrence;
                let placements = occurrences
                    .iter()
                    .map(|id| LayoutPlacement {
                        source: *id,
                        copy: true,
                        pose: snapshot.occurrences[id].pose,
                    })
                    .collect::<Vec<_>>();
                placement::apply(
                    snapshot,
                    &placements,
                    next_occurrence,
                    next_surface,
                    next_process,
                    next_operation,
                )?;
                (begin..*next_occurrence)
                    .map(|id| OccurrenceId::new(id).expect("allocated occurrence"))
                    .collect::<Vec<_>>()
            } else {
                occurrences
            };
            for id in ids {
                let row = snapshot.layouts[a]
                    .occurrences
                    .remove(&id)
                    .expect("checked source part");
                snapshot.layouts[b].occurrences.insert(id, row);
            }
            snapshot.focused_layout = original_focus;
        }
    }
    Ok(())
}
fn prune(snapshot: &mut Snapshot) {
    let used: std::collections::BTreeSet<_> = snapshot
        .all_occurrences()
        .map(|o| o.definition_id.clone())
        .collect();
    let occurrences: std::collections::BTreeSet<_> = snapshot
        .all_occurrences()
        .map(|o| o.occurrence_id)
        .collect();
    snapshot.sources.retain(|id, _| used.contains(id));
    let keep = |scope: &SurfaceScope| match scope {
        SurfaceScope::Definition { definition_id } => used.contains(definition_id),
        SurfaceScope::Occurrence { occurrence_id, .. } => occurrences.contains(occurrence_id),
    };
    Arc::make_mut(&mut snapshot.surface_overlays)
        .layers
        .retain(|l| keep(&l.scope));
    Arc::make_mut(&mut snapshot.process_document)
        .layers
        .retain(|l| keep(&l.scope));
}

#[cfg(test)]
mod tests {
    use super::*;
    fn edit(p: &mut Project, edit: PrintLayoutEdit) {
        let prepared = p.prepare(Edit::PrintLayout { edit }).unwrap();
        p.commit(prepared).unwrap();
    }
    #[test]
    fn separate_plates_share_sources_transfer_atomically_and_reopen() {
        let mut p = crate::tests::imported();
        let source = p.info().layout_id;
        let original = p.snapshot().occurrences.values().next().unwrap().clone();
        edit(
            &mut p,
            PrintLayoutEdit::Create {
                name: "Spare parts".into(),
                copy_setup_from: Some(source.clone()),
            },
        );
        let target = p.info().layout_id;
        assert_ne!(source, target);
        assert!(p.snapshot().occurrences.is_empty());
        assert_eq!(p.snapshot().all_occurrences().count(), 1);
        let revision = p.info().revision;
        let before = p.info();
        p.select_layout(&source).unwrap();
        p.select_layout(&target).unwrap();
        assert_eq!(
            p.info(),
            before,
            "navigation changed semantic project state"
        );
        assert_eq!(p.info().revision, revision);
        edit(
            &mut p,
            PrintLayoutEdit::Transfer {
                source: source.clone(),
                target: target.clone(),
                occurrences: vec![original.occurrence_id],
                copy: true,
            },
        );
        assert_eq!(p.snapshot().all_occurrences().count(), 2);
        assert_eq!(p.snapshot().sources.len(), 1);
        let copied = p.snapshot().occurrences.values().next().unwrap().clone();
        assert_ne!(copied.occurrence_id, original.occurrence_id);
        assert_eq!(copied.definition_id, original.definition_id);
        assert_eq!(copied.pose, original.pose);
        p.undo().unwrap();
        assert_eq!(p.snapshot().all_occurrences().count(), 1);
        p.redo().unwrap();
        assert_eq!(p.snapshot().all_occurrences().count(), 2);
        edit(
            &mut p,
            PrintLayoutEdit::Transfer {
                source: source.clone(),
                target: target.clone(),
                occurrences: vec![original.occurrence_id],
                copy: false,
            },
        );
        assert_eq!(p.snapshot().all_occurrences().count(), 2);
        assert!(
            p.snapshot()
                .layouts
                .iter()
                .find(|l| l.layout_id == source)
                .unwrap()
                .occurrences
                .is_empty()
        );
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("plates.spil");
        let receipt = storage::save(
            p.capture(),
            None,
            Some(path.clone()),
            &std::sync::atomic::AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        p.mark_saved(receipt).unwrap();
        assert!(!p.info().dirty);
        p.select_layout(&source).unwrap();
        assert!(!p.info().dirty);
        drop(p);
        let p = Project::from_loaded(
            storage::open(
                &path,
                true,
                false,
                &std::sync::atomic::AtomicBool::new(false),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(p.snapshot().layouts.len(), 2);
        assert_eq!(p.snapshot().all_occurrences().count(), 2);
        assert!(!p.info().dirty);
    }
    #[test]
    fn unrelated_job_edits_preserve_exact_result_and_shared_defaults_invalidate_it() {
        let mut p = crate::tests::manufactured();
        let a = p.info().layout_id;
        let artifact = p.snapshot().manufacturing_artifact.clone().unwrap();
        let definition = p
            .snapshot()
            .occurrences
            .values()
            .next()
            .unwrap()
            .definition_id
            .clone();
        edit(
            &mut p,
            PrintLayoutEdit::Create {
                name: "Empty work".into(),
                copy_setup_from: Some(a.clone()),
            },
        );
        let b = p.info().layout_id;
        let prepared = p
            .prepare(Edit::SetManufacturingIntent {
                intent: {
                    let mut i = crate::tests::manufacturing_intent();
                    i.recipe.print_speed_mm_s += 1.;
                    Arc::new(i)
                },
            })
            .unwrap();
        p.commit(prepared).unwrap();
        p.select_layout(&a).unwrap();
        assert!(Arc::ptr_eq(
            p.snapshot().manufacturing_artifact.as_ref().unwrap(),
            &artifact
        ));
        edit(
            &mut p,
            PrintLayoutEdit::Rename {
                layout_id: b,
                name: "Other job".into(),
            },
        );
        assert!(Arc::ptr_eq(
            p.snapshot().manufacturing_artifact.as_ref().unwrap(),
            &artifact
        ));
        let prepared = p
            .prepare(Edit::Process {
                edit: ProcessEdit::SetSparseFraction {
                    scope: SurfaceScope::Definition {
                        definition_id: definition,
                    },
                    fraction: 0.6,
                },
            })
            .unwrap();
        p.commit(prepared).unwrap();
        assert!(p.snapshot().manufacturing_artifact.is_none());
        p.undo().unwrap();
        assert!(Arc::ptr_eq(
            p.snapshot().manufacturing_artifact.as_ref().unwrap(),
            &artifact
        ));
    }
    #[test]
    fn unsaved_empty_plate_edits_are_dirty_but_navigation_is_not_content() {
        let mut p = Project::new();
        assert!(!p.info().dirty);
        edit(
            &mut p,
            PrintLayoutEdit::Create {
                name: "Empty".into(),
                copy_setup_from: None,
            },
        );
        assert!(p.info().dirty);
        p.undo().unwrap();
        assert!(!p.info().dirty);
        let id = p.info().layout_id;
        edit(
            &mut p,
            PrintLayoutEdit::Rename {
                layout_id: id,
                name: "Named empty".into(),
            },
        );
        assert!(p.info().dirty);
    }
    #[test]
    fn remove_is_undoable_and_does_not_prune_another_plates_shared_source() {
        let mut p = crate::tests::imported();
        let source = p.info().layout_id;
        edit(
            &mut p,
            PrintLayoutEdit::Create {
                name: "Second".into(),
                copy_setup_from: None,
            },
        );
        let target = p.info().layout_id;
        let id = OccurrenceId::new(1).unwrap();
        edit(
            &mut p,
            PrintLayoutEdit::Transfer {
                source: source.clone(),
                target,
                occurrences: vec![id],
                copy: true,
            },
        );
        edit(&mut p, PrintLayoutEdit::Remove { layout_id: source });
        assert_eq!(p.snapshot().layouts.len(), 1);
        assert_eq!(p.snapshot().sources.len(), 1);
        assert_eq!(p.snapshot().all_occurrences().count(), 1);
        p.undo().unwrap();
        assert_eq!(p.snapshot().layouts.len(), 2);
        assert_eq!(p.snapshot().all_occurrences().count(), 2);
        let before = p.info();
        let empty = p.prepare(Edit::PrintLayout {
            edit: PrintLayoutEdit::Transfer {
                source: before.layout_id.clone(),
                target: before.layout_id.clone(),
                occurrences: vec![id],
                copy: false,
            },
        });
        assert!(empty.is_err());
        assert_eq!(p.info(), before);
    }
}
