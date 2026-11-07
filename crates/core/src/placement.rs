// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Shared atomic placement/copy semantics for detached trials and authoritative Apply.
use super::*;

#[derive(Clone)]
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
    /// Detached additional-job trial. Preserve global assets/history context, but compile
    /// only the selected original and its scoped contributions.
    pub fn template_only(&self, template: OccurrenceId) -> Result<Self, ProjectError> {
        if !self.snapshot.occurrences.contains_key(&template) {
            return Err(invalid("Unknown layout template"));
        }
        let mut snapshot = (*self.snapshot).clone();
        snapshot.occurrences.retain(|id, _| *id == template);
        snapshot.manufacturing_artifact = None;
        Ok(Self {
            snapshot: Arc::new(snapshot),
            next: self.next,
        })
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
/// One transaction applies Current and every additional job. Copy inheritance and
/// allocator ownership remain identical to ordinary placement and plate transfer.
pub(crate) fn apply_across_plates(
    snapshot: &mut Snapshot,
    placements: &[LayoutPlacement],
    additional: &[Vec<LayoutPlacement>],
    next: [&mut u32; 4],
) -> Result<(), ProjectError> {
    if placements.is_empty() && additional.is_empty() {
        return Err(invalid("Layout has no placement changes"));
    }
    if snapshot.layouts.len() + additional.len() > MAX_PRINT_LAYOUTS {
        return Err(error(
            ProjectErrorCode::ResourceLimit,
            "Additional plates exceed the project plate budget",
        ));
    }
    let [occurrence, surface, layer, operation] = next;
    let source = snapshot.focused_layout.clone();
    if !placements.is_empty() {
        apply(snapshot, placements, occurrence, surface, layer, operation)?;
    }
    for rows in additional {
        validate_layout_placements(rows).map_err(|e| invalid(e.message))?;
        if rows.iter().any(|p| !p.copy) {
            return Err(invalid("Additional plates may only contain new copies"));
        }
        let begin = *occurrence;
        apply(snapshot, rows, occurrence, surface, layer, operation)?;
        let copied = (begin..*occurrence)
            .map(|id| OccurrenceId::new(id).expect("allocated occurrence"))
            .collect();
        print_layouts::apply(
            snapshot,
            PrintLayoutEdit::Create {
                name: format!("Plate {}", snapshot.layouts.len() + 1),
                copy_setup_from: Some(source.clone()),
            },
            occurrence,
            surface,
            layer,
            operation,
        )?;
        let target = snapshot.focused_layout.clone();
        print_layouts::apply(
            snapshot,
            PrintLayoutEdit::Transfer {
                source: source.clone(),
                target,
                occurrences: copied,
                copy: false,
            },
            occurrence,
            surface,
            layer,
            operation,
        )?;
        snapshot.focused_layout = source.clone();
    }
    Ok(())
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
    let mut routed = Vec::with_capacity(placements.len());
    for p in placements {
        let source = original
            .occurrences
            .get(&p.source)
            .ok_or_else(|| invalid("Unknown placement source"))?;
        if !p.copy {
            routed.push((p.source, p.source, p.pose));
            snapshot
                .occurrences
                .get_mut(&p.source)
                .ok_or_else(|| invalid("Unknown moved occurrence"))?
                .pose = p.pose;
            continue;
        }
        let id = OccurrenceId::new(*next_occurrence).map_err(|e| invalid(e.message))?;
        routed.push((p.source, id, p.pose));
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
    crate::routed_placement::apply(snapshot, &original, &routed)?;
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
                additional: vec![],
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
        commit(
            &mut project,
            Edit::ApplyPlacement {
                additional: vec![],
                placements: edits,
            },
        );
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
                    additional: vec![],
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
                    additional: vec![],
                    placements: vec![p.clone(); MAX_OCCURRENCES as usize]
                })
                .is_err()
        );
        assert_eq!(project.snapshot().occurrences.len(), 1);
        commit(
            &mut project,
            Edit::ApplyPlacement {
                additional: vec![],
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

#[cfg(test)]
mod distribution_tests {
    use super::*;
    #[test]
    fn additional_jobs_share_sources_and_one_undo_and_failed_admission_has_no_effect() {
        let mut project = crate::tests::imported();
        let original = project.snapshot();
        let source = original.occurrences.values().next().unwrap().clone();
        let copy = LayoutPlacement {
            source: source.occurrence_id,
            copy: true,
            pose: source.pose,
        };
        let prepared = project
            .prepare(Edit::ApplyPlacement {
                placements: vec![],
                additional: vec![vec![copy.clone()], vec![copy.clone()]],
            })
            .unwrap();
        assert_eq!(project.snapshot().layouts.len(), 1);
        project.commit(prepared).unwrap();
        let distributed = project.snapshot();
        assert_eq!(distributed.layouts.len(), 3);
        assert_eq!(distributed.all_occurrences().count(), 3);
        assert_eq!(distributed.focused_layout, original.focused_layout);
        assert!(Arc::ptr_eq(
            &distributed.sources[&source.definition_id],
            &original.sources[&source.definition_id]
        ));
        let ids = distributed
            .all_occurrences()
            .map(|o| o.occurrence_id)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(ids.len(), 3);
        let before = project.info();
        let mut bad = copy.clone();
        bad.source = OccurrenceId::new(u32::MAX).unwrap();
        assert!(
            project
                .prepare(Edit::ApplyPlacement {
                    placements: vec![],
                    additional: vec![vec![copy.clone()], vec![bad]],
                })
                .is_err()
        );
        assert_eq!(project.info(), before);
        assert_eq!(project.snapshot().layouts.len(), 3);
        project.undo().unwrap();
        assert_eq!(
            project.snapshot().layout_records(),
            original.layout_records()
        );
        project.redo().unwrap();
        assert_eq!(
            project.snapshot().layout_records(),
            distributed.layout_records()
        );
        let before = project.info();
        assert!(
            project
                .prepare(Edit::ApplyPlacement {
                    placements: vec![],
                    additional: vec![vec![copy]; MAX_PRINT_LAYOUTS]
                })
                .is_err()
        );
        assert_eq!(project.info(), before);
    }
}

#[cfg(test)]
mod routed_tests {
    use super::*;
    use spiling_contracts::manufacturing::*;
    fn commit(project: &mut Project, edit: Edit) {
        let prepared = project.prepare(edit).unwrap();
        project.commit(prepared).unwrap();
    }
    #[test]
    fn routed_copies_transport_explicit_reservations_share_setup_and_undo_atomically() {
        let mut project = crate::tests::imported();
        let id = OccurrenceId::new(1).unwrap();
        let pose = RigidPoseMm {
            translation_mm: [20.0, 20.0, 0.0],
            ..RigidPoseMm::IDENTITY
        };
        commit(
            &mut project,
            Edit::SetPose {
                occurrence_id: id,
                pose,
            },
        );
        let intent = decode_intent(include_bytes!(
            "../../../fixtures/manufacturing/mixed-width-combined.intent.json"
        ))
        .unwrap();
        commit(
            &mut project,
            Edit::SetManufacturingIntent {
                intent: Arc::new(intent),
            },
        );
        let baseline = project.snapshot();
        let copy_pose = RigidPoseMm {
            translation_mm: [70.0, 20.0, 0.0],
            ..pose
        };
        let placements = vec![LayoutPlacement {
            source: id,
            copy: true,
            pose: copy_pose,
        }];
        let trial = project.placement_draft().preview(&placements).unwrap();
        assert!(Arc::ptr_eq(&baseline, &project.snapshot()));
        commit(
            &mut project,
            Edit::ApplyPlacement {
                placements,
                additional: vec![],
            },
        );
        let copied = project.snapshot();
        assert_eq!(copied.manufacturing_intent, trial.manufacturing_intent);
        assert!(Arc::ptr_eq(
            baseline.sources.values().next().unwrap(),
            copied.sources.values().next().unwrap()
        ));
        let JobWorkflow::FixedDualMarlin { job } = &copied
            .manufacturing_intent
            .as_ref()
            .unwrap()
            .printer
            .workflow
        else {
            unreachable!()
        };
        assert_eq!(job.assignments.len(), 2);
        assert_eq!(job.assignments[0].tool_id, job.assignments[1].tool_id);
        assert_eq!(job.assignments[0].interior, job.assignments[1].interior);
        assert!(
            job.assignments[1].work_envelope_mm.min[0] <= 69.0
                && job.assignments[1].work_envelope_mm.min[0] > 68.999999999
        );
        assert!(
            job.assignments[1].work_envelope_mm.max[0] >= 91.0
                && job.assignments[1].work_envelope_mm.max[0] < 91.000000001
        );
        project.undo().unwrap();
        assert_eq!(
            project.snapshot().manufacturing_intent,
            baseline.manufacturing_intent
        );
        assert_eq!(project.snapshot().occurrences.len(), 1);
        project.redo().unwrap();
        assert_eq!(
            project.snapshot().manufacturing_intent,
            copied.manufacturing_intent
        );
        let before = project.snapshot();
        let overflow = (0..32)
            .map(|i| LayoutPlacement {
                source: id,
                copy: true,
                pose: RigidPoseMm {
                    translation_mm: [200.0 + i as f64 * 50.0, 20.0, 0.0],
                    ..pose
                },
            })
            .collect();
        assert!(
            project
                .prepare(Edit::ApplyPlacement {
                    placements: overflow,
                    additional: vec![]
                })
                .is_err()
        );
        assert!(Arc::ptr_eq(&before, &project.snapshot()));
    }
}
