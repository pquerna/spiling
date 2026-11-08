// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::*;
use spiling_contracts::analysis::*;

fn case(project: &Project) -> AnalysisCase {
    let snapshot = project.snapshot();
    let occurrence = snapshot.occurrences.values().next().unwrap();
    let source = snapshot.sources.get(&occurrence.definition_id).unwrap();
    AnalysisCase {
        id: AnalysisCaseId::new(),
        name: "Hook load".into(),
        layout_id: snapshot.focused_layout.clone(),
        occurrence_id: occurrence.occurrence_id,
        definition_id: occurrence.definition_id.clone(),
        source_hash: source.record.provenance.source_hash.clone(),
        mode: AnalysisMode::Basic,
        loads: vec![AnalysisFaceLoad {
            face_id: FaceId::from_step_entity(10).unwrap(),
            force_n: [0.0, -10.0, 0.0],
            moment_n_mm: [1.0, 2.0, 3.0],
        }],
        restraints: vec![AnalysisFaceRestraint {
            face_id: FaceId::from_step_entity(11).unwrap(),
            fixed_translation: [true; 3],
        }],
    }
}
fn put(project: &mut Project, case: AnalysisCase) {
    let prepared = project
        .prepare(Edit::Analysis {
            edit: AnalysisEdit::Put { case },
        })
        .unwrap();
    project.commit(prepared).unwrap();
}
#[test]
fn analysis_load_edits_preserve_source_print_inputs_and_manufacturing_artifact() {
    let mut project = tests::manufactured();
    let before = project.snapshot();
    let context = context_hash(&before).unwrap();
    let mut value = case(&project);
    put(&mut project, value.clone());
    let revision = project.info().revision;
    assert_eq!(project.info().analysis.cases[0].edited_revision, revision);
    value.loads[0].force_n[1] = -20.0;
    put(&mut project, value.clone());
    let after = project.snapshot();
    assert_eq!(context_hash(&after).unwrap(), context);
    assert!(Arc::ptr_eq(
        &before.sources.values().next().unwrap().bytes,
        &after.sources.values().next().unwrap().bytes
    ));
    assert!(Arc::ptr_eq(
        before.manufacturing_intent.as_ref().unwrap(),
        after.manufacturing_intent.as_ref().unwrap()
    ));
    assert!(Arc::ptr_eq(
        before.manufacturing_artifact.as_ref().unwrap(),
        after.manufacturing_artifact.as_ref().unwrap()
    ));
    project.undo().unwrap();
    assert_eq!(
        project.info().analysis.cases[0].case.loads[0].force_n[1],
        -10.0
    );
    assert!(project.info().revision > revision);
    project.redo().unwrap();
    assert_eq!(project.info().analysis.cases[0].case, value);
}
#[test]
fn analysis_prepared_edits_reject_stale_commit_and_source_rebinding() {
    let mut project = tests::imported();
    let value = case(&project);
    let stale = project
        .prepare(Edit::Analysis {
            edit: AnalysisEdit::Put {
                case: value.clone(),
            },
        })
        .unwrap();
    put(&mut project, value.clone());
    assert_eq!(
        project.commit(stale).unwrap_err().code,
        ProjectErrorCode::StaleRevision
    );
    let original = project.info();
    let mut wrong = value;
    wrong.source_hash = SourceHash::from_digest(&[99; 32]);
    assert!(
        project
            .prepare(Edit::Analysis {
                edit: AnalysisEdit::Put { case: wrong }
            })
            .is_err()
    );
    assert_eq!(project.info(), original);
}
#[test]
fn analysis_refuses_oversized_incoming_allocations_before_mutation() {
    let project = tests::imported();
    let initial = project.info();
    let mut value = case(&project);
    value.name = String::with_capacity(MAX_ANALYSIS_DOCUMENT_BYTES + 1);
    value.name.push_str("Hook load");
    assert_eq!(
        project
            .prepare(Edit::Analysis {
                edit: AnalysisEdit::Put { case: value }
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::ResourceLimit
    );
    assert_eq!(project.info(), initial);
}
#[test]
fn analysis_identical_put_keeps_semantic_cleanliness_and_last_actual_edit_revision() {
    let mut project = tests::imported();
    let value = case(&project);
    put(&mut project, value.clone());
    let saved = project.capture().manifest;
    project.saved = Some((saved.revision, Content::from_manifest(&saved)));
    assert!(!project.info().dirty);
    put(&mut project, value);
    assert!(!project.info().dirty);
    assert!(project.info().revision > saved.revision);
    assert_eq!(project.info().analysis, saved.analysis);
}
#[test]
fn analysis_removal_cascades_and_undo_restores_the_case() {
    let mut project = tests::imported();
    let value = case(&project);
    put(&mut project, value.clone());
    let prepared = project
        .prepare(Edit::Remove {
            occurrence_id: value.occurrence_id,
        })
        .unwrap();
    project.commit(prepared).unwrap();
    assert!(project.info().analysis.cases.is_empty());
    project.undo().unwrap();
    assert_eq!(project.info().analysis.cases[0].case, value);
}
#[test]
fn analysis_case_transfer_tracks_exclusive_layout_membership_without_copying_cases() {
    let mut project = tests::imported();
    let value = case(&project);
    put(&mut project, value.clone());
    let prepared = project
        .prepare(Edit::PrintLayout {
            edit: PrintLayoutEdit::Create {
                name: "Other plate".into(),
                copy_setup_from: None,
            },
        })
        .unwrap();
    project.commit(prepared).unwrap();
    let target = project
        .info()
        .layouts
        .iter()
        .find(|layout| layout.layout_id != value.layout_id)
        .unwrap()
        .layout_id
        .clone();
    let prepared = project
        .prepare(Edit::PrintLayout {
            edit: PrintLayoutEdit::Transfer {
                source: value.layout_id.clone(),
                target: target.clone(),
                occurrences: vec![value.occurrence_id],
                copy: false,
            },
        })
        .unwrap();
    project.commit(prepared).unwrap();
    let info = project.info();
    assert_eq!(info.analysis.cases.len(), 1);
    assert_eq!(info.analysis.cases[0].case.layout_id, target);
    assert_eq!(info.analysis.cases[0].edited_revision, info.revision);
    project.undo().unwrap();
    assert_eq!(project.info().analysis.cases[0].case, value);
}
