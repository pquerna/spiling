// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Shared atomic placement/copy semantics for detached trials and authoritative Apply.
use super::*;

pub struct PlacementDraft {
    snapshot: Arc<Snapshot>,
    next: [u32; 4],
}
impl PlacementDraft {
    pub(crate) fn new(project: &Project) -> Self {
        Self {
            snapshot: project.snapshot(),
            next: [
                project.next_occurrence,
                project.next_surface_layer,
                project.next_process_layer,
                project.next_process_operation,
            ],
        }
    }
    pub fn snapshot(&self) -> &Arc<Snapshot> {
        &self.snapshot
    }
    pub fn preview(&self, placements: &[LayoutPlacement]) -> Result<Snapshot, ProjectError> {
        let mut snapshot = (*self.snapshot).clone();
        let [mut occurrence, mut surface, mut layer, mut operation] = self.next;
        if placements.is_empty() {
            snapshot.manufacturing_artifact = None;
            return Ok(snapshot);
        }
        apply(
            &mut snapshot,
            placements,
            &mut occurrence,
            &mut surface,
            &mut layer,
            &mut operation,
        )?;
        snapshot.manufacturing_artifact = None;
        Ok(snapshot)
    }
}
fn allocate(next: &mut u32) -> Result<u32, ProjectError> {
    let value = *next;
    *next = next.checked_add(1).ok_or_else(|| {
        error(
            ProjectErrorCode::ResourceLimit,
            "Placement identity allocator exhausted",
        )
    })?;
    Ok(value)
}
pub(crate) fn apply(
    snapshot: &mut Snapshot,
    placements: &[LayoutPlacement],
    next_occurrence: &mut u32,
    next_surface: &mut u32,
    next_layer: &mut u32,
    next_operation: &mut u32,
) -> Result<(), ProjectError> {
    validate_layout_placements(placements).map_err(|e| invalid(e.message))?;
    // Capture originals before moving or copying any: order must never change copy inheritance.
    let original = snapshot.clone();
    for p in placements {
        let source = original
            .occurrences
            .get(&p.source)
            .ok_or_else(|| invalid("Unknown placement source"))?;
        if !p.copy {
            snapshot
                .occurrences
                .get_mut(&p.source)
                .ok_or_else(|| invalid("Unknown moved occurrence"))?
                .pose = p.pose;
            continue;
        }
        let id = OccurrenceId::new(*next_occurrence).map_err(|e| invalid(e.message))?;
        add_occurrence(
            snapshot,
            source.definition_id.clone(),
            p.pose,
            next_occurrence,
        )?;
        for old in original.surface_overlays.layers.iter().filter(|l| matches!(l.scope, SurfaceScope::Occurrence { occurrence_id, .. } if occurrence_id == p.source)) {
            if snapshot.surface_overlays.layers.len() >= MAX_SURFACE_LAYERS {
                return Err(error(ProjectErrorCode::ResourceLimit, "Copies exceed the surface-layer budget"));
            }
            let mut layer = old.clone();
            layer.layer_id = SurfaceLayerId::new(allocate(next_surface)?).map_err(|e| invalid(e.message))?;
            layer.scope = SurfaceScope::Occurrence { occurrence_id: id, expected_definition_id: source.definition_id.clone() };
            Arc::make_mut(&mut snapshot.surface_overlays).layers.push(layer);
        }
        for old in original.process_document.layers.iter().filter(|l| matches!(l.scope, SurfaceScope::Occurrence { occurrence_id, .. } if occurrence_id == p.source)) {
            if snapshot.process_document.layers.len() >= MAX_PROCESS_LAYERS
                || snapshot.process_document.layers.iter().map(|l| l.operations.len()).sum::<usize>() + old.operations.len() > MAX_PROCESS_OPERATIONS {
                return Err(error(ProjectErrorCode::ResourceLimit, "Copies exceed the process-layer/operation budget"));
            }
            let mut layer = old.clone();
            layer.layer_id = ProcessLayerId::new(allocate(next_layer)?)?;
            layer.scope = SurfaceScope::Occurrence { occurrence_id: id, expected_definition_id: source.definition_id.clone() };
            for operation in &mut layer.operations { operation.operation_id = ProcessOperationId::new(allocate(next_operation)?)?; }
            Arc::make_mut(&mut snapshot.process_document).layers.push(layer);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn commit(project: &mut Project, edit: Edit) {
        let p = project.prepare(edit).unwrap();
        project.commit(p).unwrap();
    }
    #[test]
    fn copying_preserves_scoped_intent_and_assets_as_one_undoable_transaction() {
        let mut project = crate::tests::imported();
        let source = project
            .snapshot()
            .occurrences
            .values()
            .next()
            .unwrap()
            .clone();
        let scope = SurfaceScope::Occurrence {
            occurrence_id: source.occurrence_id,
            expected_definition_id: source.definition_id.clone(),
        };
        commit(
            &mut project,
            Edit::AddImageSurfaceFinish {
                asset: crate::tests::image_asset([1, 2, 3, 4], "shared.pgm"),
                scope: scope.clone(),
                face_id: FaceId::from_step_entity(7).unwrap(),
                parameters: crate::tests::image_parameters(),
            },
        );
        commit(
            &mut project,
            Edit::Process {
                edit: ProcessEdit::SetSparseFraction {
                    scope,
                    fraction: 0.4,
                },
            },
        );
        let original = project.snapshot();
        let mut pose = source.pose;
        pose.translation_mm[0] = 30.0;
        let edits = vec![LayoutPlacement {
            source: source.occurrence_id,
            copy: true,
            pose,
        }];
        let draft = project.placement_draft();
        let trial = draft.preview(&edits).unwrap();
        assert_eq!(project.snapshot().occurrences.len(), 1);
        let prepared = project
            .prepare(Edit::ApplyPlacement {
                placements: edits.clone(),
            })
            .unwrap();
        assert_eq!(trial.occurrences, prepared.snapshot().occurrences);
        project.commit(prepared).unwrap();
        let copied = project.snapshot();
        assert_eq!(copied.occurrences.len(), 2);
        assert_eq!(copied.surface_overlays.layers.len(), 2);
        assert_ne!(
            copied.surface_overlays.layers[0].layer_id,
            copied.surface_overlays.layers[1].layer_id
        );
        assert_ne!(
            copied.process_document.layers[0].operations[0].operation_id,
            copied.process_document.layers[1].operations[0].operation_id
        );
        let new_id = copied.occurrences.keys().copied().max().unwrap();
        assert!(
            matches!(copied.surface_overlays.layers[1].scope,SurfaceScope::Occurrence { occurrence_id,.. } if occurrence_id == new_id)
        );
        assert!(
            matches!(copied.process_document.layers[1].scope,SurfaceScope::Occurrence { occurrence_id,.. } if occurrence_id == new_id)
        );
        assert!(Arc::ptr_eq(
            copied.sources.values().next().unwrap(),
            original.sources.values().next().unwrap()
        ));
        assert!(Arc::ptr_eq(
            copied.surface_images.values().next().unwrap(),
            original.surface_images.values().next().unwrap()
        ));
        project.undo().unwrap();
        assert_eq!(project.snapshot().occurrences, original.occurrences);
        assert_eq!(
            project.snapshot().process_document,
            original.process_document
        );
        project.redo().unwrap();
        assert_eq!(project.snapshot().occurrences, copied.occurrences);
        project.undo().unwrap();
        commit(&mut project, Edit::ApplyPlacement { placements: edits });
        assert!(
            project
                .snapshot()
                .occurrences
                .keys()
                .copied()
                .max()
                .unwrap()
                > new_id
        );
    }
    #[test]
    fn duplicate_moves_or_failed_copy_budget_never_partially_apply_or_consume_ids() {
        let mut project = crate::tests::imported();
        let id = *project.snapshot().occurrences.keys().next().unwrap();
        let p = LayoutPlacement {
            source: id,
            copy: false,
            pose: RigidPoseMm::IDENTITY,
        };
        assert!(
            project
                .prepare(Edit::ApplyPlacement {
                    placements: vec![p.clone(), p]
                })
                .is_err()
        );
        let p = LayoutPlacement {
            source: id,
            copy: true,
            pose: RigidPoseMm::IDENTITY,
        };
        assert!(
            project
                .prepare(Edit::ApplyPlacement {
                    placements: vec![p.clone(); MAX_OCCURRENCES as usize]
                })
                .is_err()
        );
        assert_eq!(project.snapshot().occurrences.len(), 1);
        commit(
            &mut project,
            Edit::ApplyPlacement {
                placements: vec![p],
            },
        );
        assert!(
            project
                .snapshot()
                .occurrences
                .contains_key(&OccurrenceId::new(2).unwrap())
        );
    }
}
