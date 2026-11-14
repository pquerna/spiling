// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::{Edit, Project, materials::MaterialAsset, tests::*};
use spiling_contracts::{
    geometry::{AabbMm, FaceId, OccurrenceId, RigidPoseMm},
    manufacturing::{MachineComponent, MachineComponentRole, MachineComponentShape},
    process::{ProcessDocument, ProcessEdit},
    project::{MAX_MANUFACTURING_OPTIONS, MAX_MANUFACTURING_OPTIONS_BYTES},
    surface::{SurfaceFinish, SurfaceScope},
};

fn commit(project: &mut Project, edit: Edit) {
    let prepared = project.prepare(edit).unwrap();
    project.commit(prepared).unwrap();
}

fn option(project: &Project, name: &str) -> ManufacturingOption {
    ManufacturingOption::new(
        name.into(),
        context_hash(&project.snapshot()).unwrap(),
        Arc::new(manufacturing_intent()),
    )
    .unwrap()
}

fn retained(project: &mut Project, name: &str) -> ManufacturingOption {
    let option = option(project, name);
    commit(
        project,
        Edit::RetainManufacturingOption {
            option: option.clone(),
        },
    );
    option
}

#[test]
fn retain_inspect_remove_and_history_preserve_current_configuration_and_artifact() {
    let mut project = manufactured();
    let before = project.snapshot();
    let revision = project.info().revision;
    let option = retained(&mut project, "Fine alternative");
    assert_eq!(project.info().revision, revision.next().unwrap());
    assert!(project.info().dirty);
    let after = project.snapshot();
    assert!(Arc::ptr_eq(
        before.manufacturing_intent.as_ref().unwrap(),
        after.manufacturing_intent.as_ref().unwrap()
    ));
    assert!(Arc::ptr_eq(
        before.manufacturing_artifact.as_ref().unwrap(),
        after.manufacturing_artifact.as_ref().unwrap()
    ));
    assert_eq!(before.occurrences, after.occurrences);
    assert!(Arc::ptr_eq(
        before.sources.values().next().unwrap(),
        after.sources.values().next().unwrap()
    ));
    assert!(Arc::ptr_eq(
        &before.surface_overlays,
        &after.surface_overlays
    ));
    assert!(Arc::ptr_eq(
        &before.process_document,
        &after.process_document
    ));
    let info = project.info();
    assert_eq!(info.manufacturing_options, vec![option.info()]);
    let inspected = project
        .manufacturing_option(info.revision, &option.id)
        .unwrap();
    assert_eq!(inspected, option);
    assert!(Arc::ptr_eq(&inspected.intent, &option.intent));
    assert_eq!(project.info(), info);
    assert!(Arc::ptr_eq(&project.snapshot(), &after));
    assert_eq!(
        context_hash(&before).unwrap(),
        context_hash(&after).unwrap()
    );
    commit(
        &mut project,
        Edit::RemoveManufacturingOption {
            id: option.id.clone(),
        },
    );
    assert!(project.snapshot().manufacturing_options.is_empty());
    assert!(Arc::ptr_eq(
        project.snapshot().manufacturing_artifact.as_ref().unwrap(),
        before.manufacturing_artifact.as_ref().unwrap()
    ));
    project.undo().unwrap();
    assert_eq!(
        project.snapshot().manufacturing_options.as_ref(),
        &vec![option]
    );
    project.redo().unwrap();
    assert!(project.snapshot().manufacturing_options.is_empty());
    project.undo().unwrap();
    project.undo().unwrap();
    assert!(project.snapshot().manufacturing_options.is_empty());
    assert!(project.snapshot().manufacturing_artifact.is_some());
}

#[test]
fn applying_is_one_revision_one_undoable_intent_transaction() {
    let mut project = manufactured();
    let mut candidate = manufacturing_intent();
    candidate.recipe.print_speed_mm_s = 9.0;
    let option = ManufacturingOption::new(
        "Slower".into(),
        context_hash(&project.snapshot()).unwrap(),
        Arc::new(candidate),
    )
    .unwrap();
    commit(
        &mut project,
        Edit::RetainManufacturingOption {
            option: option.clone(),
        },
    );
    let before = project.snapshot();
    let revision = project.info().revision;
    commit(
        &mut project,
        Edit::ApplyManufacturingOption {
            id: option.id.clone(),
        },
    );
    assert_eq!(project.info().revision, revision.next().unwrap());
    assert!(Arc::ptr_eq(
        project.snapshot().manufacturing_intent.as_ref().unwrap(),
        &option.intent
    ));
    assert!(project.snapshot().manufacturing_artifact.is_none());
    assert_eq!(project.snapshot().occurrences, before.occurrences);
    assert_eq!(
        project.snapshot().manufacturing_options,
        before.manufacturing_options
    );
    project.undo().unwrap();
    assert!(Arc::ptr_eq(&project.snapshot(), &before));
    project.redo().unwrap();
    assert!(project.snapshot().manufacturing_artifact.is_none());
    assert_eq!(project.snapshot().manufacturing_intent, Some(option.intent));
}

#[test]
fn detached_trial_shares_inputs_but_cannot_publish_or_mutate_current() {
    let project = manufactured();
    let before = project.snapshot();
    let info = project.info();
    let mut intent = manufacturing_intent();
    intent.recipe.print_speed_mm_s = 9.0;
    let intent = Arc::new(intent);
    let trial = trial_snapshot(&before, intent.clone()).unwrap();
    assert!(trial.manufacturing_artifact.is_none());
    assert!(Arc::ptr_eq(
        trial.manufacturing_intent.as_ref().unwrap(),
        &intent
    ));
    assert!(Arc::ptr_eq(
        trial.sources.values().next().unwrap(),
        before.sources.values().next().unwrap()
    ));
    assert!(Arc::ptr_eq(
        &trial.surface_overlays,
        &before.surface_overlays
    ));
    assert!(Arc::ptr_eq(
        &trial.process_document,
        &before.process_document
    ));
    assert_eq!(
        context_hash(&trial).unwrap(),
        context_hash(&before).unwrap()
    );
    assert!(Arc::ptr_eq(&project.snapshot(), &before));
    assert_eq!(project.info(), info);
    let mut invalid_intent = manufacturing_intent();
    invalid_intent.recipe.layer_height_mm = f64::NAN;
    assert!(trial_snapshot(&before, Arc::new(invalid_intent)).is_err());
    let mut invalid_source = (*before).clone();
    Arc::make_mut(invalid_source.sources.values_mut().next().unwrap())
        .record
        .provenance
        .source_hash = SourceHash::from_bytes(b"corrupt");
    assert!(trial_snapshot(&invalid_source, intent).is_err());
    assert_eq!(project.info(), info);
}

#[test]
fn old_context_is_inspectable_but_apply_rejects_pose_rebound_and_foreign_source() {
    let mut project = imported();
    let option = retained(&mut project, "Original");
    commit(
        &mut project,
        Edit::SetPose {
            occurrence_id: OccurrenceId::new(1).unwrap(),
            pose: RigidPoseMm {
                translation_mm: [2.0, 0.0, 0.0],
                ..RigidPoseMm::IDENTITY
            },
        },
    );
    let before = project.info();
    assert_eq!(
        project
            .manufacturing_option(before.revision, &option.id)
            .unwrap(),
        option
    );
    assert_eq!(
        project
            .prepare(Edit::ApplyManufacturingOption {
                id: option.id.clone()
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::InvalidProject
    );
    assert_eq!(project.info(), before);
    project.undo().unwrap();
    commit(
        &mut project,
        Edit::ApplyManufacturingOption {
            id: option.id.clone(),
        },
    );
    project.undo().unwrap();
    let source = project.snapshot().sources.values().next().unwrap().clone();
    commit(
        &mut project,
        Edit::Remove {
            occurrence_id: OccurrenceId::new(1).unwrap(),
        },
    );
    commit(
        &mut project,
        Edit::Import {
            source,
            pose: RigidPoseMm::IDENTITY,
        },
    );
    assert_ne!(
        context_hash(&project.snapshot()).unwrap(),
        option.context_hash
    );
    assert!(
        project
            .prepare(Edit::ApplyManufacturingOption {
                id: option.id.clone()
            })
            .is_err()
    );
    let mut foreign = Project::new();
    commit(
        &mut foreign,
        Edit::Import {
            source: asset(b"foreign source".to_vec(), "foreign.step"),
            pose: RigidPoseMm::IDENTITY,
        },
    );
    assert!(
        foreign
            .prepare(Edit::RetainManufacturingOption { option })
            .is_err()
    );
    assert!(foreign.snapshot().manufacturing_options.is_empty());
}

#[test]
fn context_binds_all_manufacturing_inputs_but_not_active_or_appearance_state() {
    let mut project = imported();
    let baseline = context_hash(&project.snapshot()).unwrap();
    commit(
        &mut project,
        Edit::SetManufacturingIntent {
            intent: Arc::new(manufacturing_intent()),
        },
    );
    assert_eq!(baseline, context_hash(&project.snapshot()).unwrap());
    let mut snapshot = (*project.snapshot()).clone();
    Arc::make_mut(snapshot.sources.values_mut().next().unwrap())
        .record
        .provenance
        .source_name = "changed.step".into();
    assert_ne!(baseline, context_hash(&snapshot).unwrap());
    let mut snapshot = (*project.snapshot()).clone();
    snapshot.materials = Some(Arc::new(MaterialAsset::new(&Default::default()).unwrap()));
    assert_ne!(baseline, context_hash(&snapshot).unwrap());
    let definition_id = project.snapshot().sources.keys().next().unwrap().clone();
    commit(
        &mut project,
        Edit::AddSurfaceFinish {
            scope: SurfaceScope::Definition {
                definition_id: definition_id.clone(),
            },
            face_id: FaceId::from_step_entity(7).unwrap(),
            finish: SurfaceFinish::Smooth,
        },
    );
    assert_ne!(baseline, context_hash(&project.snapshot()).unwrap());
    project.undo().unwrap();
    commit(
        &mut project,
        Edit::Process {
            edit: ProcessEdit::SetSparseFraction {
                scope: SurfaceScope::Definition { definition_id },
                fraction: 0.5,
            },
        },
    );
    assert_ne!(baseline, context_hash(&project.snapshot()).unwrap());
    project.undo().unwrap();
    let edit = image_edit(&project, image_asset([9; 4], "context.pgm"));
    commit(&mut project, edit);
    assert_ne!(baseline, context_hash(&project.snapshot()).unwrap());
    project.undo().unwrap();
    let mut snapshot = (*project.snapshot()).clone();
    Arc::make_mut(&mut snapshot.appearance_study)
        .limits
        .max_alternatives = 1;
    assert_eq!(baseline, context_hash(&snapshot).unwrap());
    assert_eq!(*snapshot.process_document, ProcessDocument::empty());
}

#[test]
fn stale_prepared_and_inspection_revisions_refuse_without_changing_options() {
    let mut project = imported();
    let revision = project.info().revision;
    let option = option(&project, "Staged");
    let prepared = project
        .prepare(Edit::RetainManufacturingOption {
            option: option.clone(),
        })
        .unwrap();
    commit(
        &mut project,
        Edit::SetManufacturingIntent {
            intent: Arc::new(manufacturing_intent()),
        },
    );
    let before = project.info();
    assert_eq!(
        project.commit(prepared).unwrap_err().code,
        ProjectErrorCode::StaleRevision
    );
    assert_eq!(
        project
            .manufacturing_option(revision, &option.id)
            .unwrap_err()
            .code,
        ProjectErrorCode::StaleRevision
    );
    assert_eq!(project.info(), before);
    let foreign = imported();
    let option = self::option(&foreign, "Foreign owner");
    let prepared = foreign
        .prepare(Edit::RetainManufacturingOption { option })
        .unwrap();
    assert_eq!(
        project.commit(prepared).unwrap_err().code,
        ProjectErrorCode::StaleRevision
    );
    assert_eq!(project.info(), before);
}

fn large_intent() -> Arc<ManufacturingIntent> {
    let mut intent = manufacturing_intent();
    intent.printer.components = (0..64)
        .map(|index| MachineComponent {
            id: format!("{index:03}"),
            role: MachineComponentRole::Fixture,
            pose: RigidPoseMm::IDENTITY,
            shape: MachineComponentShape::Box {
                bounds_mm: AabbMm {
                    min: [0.0; 3],
                    max: [1.0; 3],
                },
            },
        })
        .collect();
    let mut bytes = serde_json::to_vec(&intent).unwrap().len();
    for component in &mut intent.printer.components {
        while component.id.len() < 256 && bytes < 32740 {
            let character = if bytes + 2 <= 32740 { '"' } else { 'x' };
            component.id.push(character);
            bytes += if character == '"' { 2 } else { 1 };
        }
    }
    assert_eq!(serde_json::to_vec(&intent).unwrap().len(), 32740);
    intent.validate().unwrap();
    Arc::new(intent)
}

#[test]
fn count_names_content_identity_and_total_bytes_are_atomic_bounds() {
    let mut project = imported();
    for index in 0..MAX_MANUFACTURING_OPTIONS {
        retained(&mut project, &format!("Option {index}"));
    }
    let before = project.info();
    assert_eq!(
        project
            .prepare(Edit::RetainManufacturingOption {
                option: option(&project, "Overflow")
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::ResourceLimit
    );
    assert_eq!(project.info(), before);
    let mut option = project.snapshot().manufacturing_options[0].clone();
    option.name = "Forged".into();
    assert!(option.validate().is_err());
    option = project.snapshot().manufacturing_options[0].clone();
    Arc::make_mut(&mut option.intent).recipe.print_speed_mm_s = 9.0;
    assert!(option.validate().is_err());
    option = project.snapshot().manufacturing_options[0].clone();
    option.context_hash = SourceHash::from_bytes(b"rebound context");
    assert!(option.validate().is_err());
    let mut project = imported();
    let intent = large_intent();
    let context = context_hash(&project.snapshot()).unwrap();
    let options: Vec<_> = (0..MAX_MANUFACTURING_OPTIONS)
        .map(|index| {
            ManufacturingOption::new(
                format!("{index:03}{}", "n".repeat(125)),
                context.clone(),
                intent.clone(),
            )
            .unwrap()
        })
        .collect();
    assert!(serde_json::to_vec(&options).unwrap().len() > MAX_MANUFACTURING_OPTIONS_BYTES);
    for option in &options[..7] {
        commit(
            &mut project,
            Edit::RetainManufacturingOption {
                option: option.clone(),
            },
        );
    }
    let before = project.info();
    assert_eq!(
        project
            .prepare(Edit::RetainManufacturingOption {
                option: options[7].clone()
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::ResourceLimit
    );
    assert_eq!(project.info(), before);
}

#[test]
fn no_active_configuration_is_created_by_retaining_on_an_empty_project() {
    let mut project = Project::new();
    let before = project.info();
    assert!(!before.dirty);
    retained(&mut project, "Not applied");
    assert!(project.info().dirty);
    assert!(project.snapshot().manufacturing_intent.is_none());
    assert!(project.snapshot().manufacturing_artifact.is_none());
    project.undo().unwrap();
    assert!(!project.info().dirty);
}

#[test]
fn complete_options_and_trials_cannot_replace_or_inherit_current_hardware() {
    let mut project = imported();
    let catalog = spiling_contracts::hardware::builtin_catalog().unwrap();
    let capture = Arc::new(spiling_contracts::hardware::ProjectHardwareCapture {
        setup: catalog.setups[0].clone(),
        plate: catalog.plates[0].clone(),
        face: "textured".into(),
        accessories: vec![],
    });
    let no_hardware = context_hash(&project.snapshot()).unwrap();
    commit(
        &mut project,
        Edit::SetHardware {
            capture: Some(capture.clone()),
        },
    );
    assert_ne!(no_hardware, context_hash(&project.snapshot()).unwrap());
    let intent = Arc::new(
        serde_json::from_str(include_str!(
            "../../../fixtures/manufacturing/solid-fill.intent.json"
        ))
        .unwrap(),
    );
    commit(&mut project, Edit::SetManufacturingIntent { intent });
    let current = project.snapshot();
    let option = ManufacturingOption::new(
        "Captured".into(),
        context_hash(&current).unwrap(),
        current.manufacturing_intent.as_ref().unwrap().clone(),
    )
    .unwrap();
    commit(
        &mut project,
        Edit::RetainManufacturingOption {
            option: option.clone(),
        },
    );
    let mut missing = option.intent.as_ref().clone();
    missing.hardware = None;
    assert!(trial_snapshot(&current, Arc::new(missing.clone())).is_err());
    let incomplete = ManufacturingOption::new(
        "Uncaptured".into(),
        option.context_hash.clone(),
        Arc::new(missing),
    )
    .unwrap();
    assert!(
        project
            .prepare(Edit::RetainManufacturingOption { option: incomplete })
            .is_err()
    );
    let mut changed = capture.as_ref().clone();
    changed.plate.label = "Different captured plate definition".into();
    changed.validate().unwrap();
    commit(
        &mut project,
        Edit::SetHardware {
            capture: Some(Arc::new(changed)),
        },
    );
    let before = project.info();
    assert!(
        project
            .prepare(Edit::ApplyManufacturingOption {
                id: option.id.clone()
            })
            .is_err()
    );
    assert_eq!(project.info(), before);
    project.undo().unwrap();
    commit(
        &mut project,
        Edit::ApplyManufacturingOption { id: option.id },
    );
    assert_eq!(
        project.snapshot().hardware.as_deref(),
        Some(capture.as_ref())
    );
}

#[test]
fn options_still_fit_the_complete_manifest_envelope_not_just_their_own_budget() {
    let project = imported();
    let mut manifest = project.capture().manifest;
    manifest.layouts[0]
        .manufacturing_options
        .push(option(&project, "Retained settings"));
    manifest.appearance_study.selection = Some(String::new());
    let base = serde_json::to_vec_pretty(&manifest).unwrap().len();
    manifest.appearance_study.selection = Some(
        "x".repeat(spiling_contracts::project::MAX_PROJECT_MANIFEST_BYTES as usize - base - 44),
    );
    validate_manufacturing_options(&manifest.layouts[0].manufacturing_options).unwrap();
    assert_eq!(
        crate::validate_manifest_size(&manifest).unwrap_err().code,
        ProjectErrorCode::ResourceLimit
    );
}

#[test]
fn retained_option_census_shares_intents_and_charges_live_prepared_name_capacity() {
    let mut project = imported();
    let option = option(&project, "Shared intent");
    let mut first = (*project.snapshot()).clone();
    first.manufacturing_options = Arc::new(vec![option.clone()]);
    let single = retained_bytes(std::iter::once(&first), None).unwrap();
    assert_eq!(
        retained_bytes([&first, &first].into_iter(), None).unwrap(),
        single
    );
    let mut second = first.clone();
    second.manufacturing_options = Arc::new(vec![option]);
    let both = retained_bytes([&first, &second].into_iter(), None).unwrap();
    assert!(both > single);
    assert!(both < single * 2);
    // Retained options are admitted against the host allowance, not a fixed
    // aggregate: history beyond the former 16 MiB census is retained.
    const LARGE: usize = 12 * 1024 * 1024;
    let large_name = |name: &str| {
        let mut value = String::with_capacity(LARGE);
        value.push_str(name);
        value
    };
    let context = context_hash(&project.snapshot()).unwrap();
    let large = |name: &str| {
        ManufacturingOption::new(
            large_name(name),
            context.clone(),
            Arc::new(manufacturing_intent()),
        )
        .unwrap()
    };
    let first_option = large("First");
    let first_id = first_option.id.clone();
    commit(
        &mut project,
        Edit::RetainManufacturingOption {
            option: first_option,
        },
    );
    commit(
        &mut project,
        Edit::RemoveManufacturingOption { id: first_id },
    );
    commit(
        &mut project,
        Edit::RetainManufacturingOption {
            option: large("Second"),
        },
    );
    let history =
        std::iter::once(project.current.as_ref()).chain(project.undo.iter().map(Arc::as_ref));
    assert!(retained_bytes(history, None).unwrap() > 2 * LARGE);
    assert_eq!(project.snapshot().manufacturing_options.len(), 1);

    // Growth beyond the fresh host allowance refuses before mutation; edits
    // that add no option allocation are unaffected.
    fn exhausted() -> usize {
        1024
    }
    project.observe_retained_growth(exhausted);
    let before = project.info();
    assert_eq!(
        project
            .prepare(Edit::RetainManufacturingOption {
                option: large("Third"),
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::ResourceLimit
    );
    assert_eq!(project.info(), before);
    let second = project.snapshot().manufacturing_options[0].id.clone();
    commit(&mut project, Edit::RemoveManufacturingOption { id: second });
    assert!(project.snapshot().manufacturing_options.is_empty());
}
