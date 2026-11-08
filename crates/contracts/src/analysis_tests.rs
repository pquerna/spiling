// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::*;
use crate::rpc;

fn case() -> AnalysisCase {
    AnalysisCase {
        id: AnalysisCaseId::new(),
        name: "Hook load".into(),
        layout_id: PrintLayoutId::new(),
        occurrence_id: OccurrenceId::new(1).unwrap(),
        definition_id: DefinitionId::from_source_sha256(&[42; 32]),
        source_hash: SourceHash::from_digest(&[42; 32]),
        mode: AnalysisMode::Basic,
        loads: vec![AnalysisFaceLoad {
            face_id: FaceId::from_step_entity(10).unwrap(),
            force_n: [0.0, -12.0, 0.0],
            moment_n_mm: [20.0, 0.0, 0.0],
        }],
        restraints: vec![AnalysisFaceRestraint {
            face_id: FaceId::from_step_entity(11).unwrap(),
            fixed_translation: [true; 3],
        }],
    }
}
#[test]
fn analysis_wrenches_are_explicit_finite_and_whole_face_patches_do_not_overlap() {
    let mut value = case();
    value.validate().unwrap();
    value.loads[0].force_n = [0.0; 3];
    value.validate().unwrap();
    value.loads[0].moment_n_mm = [0.0; 3];
    assert!(value.validate().is_err());
    value.loads[0].force_n[0] = f64::NAN;
    assert!(value.validate().is_err());
    value = case();
    value.loads.push(value.loads[0].clone());
    assert!(value.validate().is_err());
    value = case();
    value.restraints[0].face_id = value.loads[0].face_id.clone();
    assert!(value.validate().is_err());
    value = case();
    value.restraints[0].fixed_translation = [false; 3];
    assert!(value.validate().is_err());
    value.loads.clear();
    value.restraints.clear();
    value.validate().unwrap();
}
#[test]
fn analysis_native_adapters_reject_missing_components_and_unknown_modes() {
    let value = case();
    let wire: rpc::AnalysisCase = value.clone().try_into().unwrap();
    assert_eq!(AnalysisCase::try_from(wire.clone()).unwrap(), value);
    let mut invalid = wire.clone();
    invalid.mode = 0;
    assert!(AnalysisCase::try_from(invalid).is_err());
    let mut invalid = wire.clone();
    invalid.loads[0].moment_n_mm.clear();
    assert!(AnalysisCase::try_from(invalid).is_err());
    let mut invalid = wire;
    invalid.restraints[0].fixed_translation.push(true);
    assert!(AnalysisCase::try_from(invalid).is_err());
    let edit = AnalysisEdit::Put { case: value };
    assert_eq!(
        AnalysisEdit::try_from(rpc::AnalysisEdit::try_from(edit.clone()).unwrap()).unwrap(),
        edit
    );
}
#[test]
fn analysis_json_is_strict_and_bounded_without_compatibility_defaults() {
    let value = case();
    let mut json = serde_json::to_value(&value).unwrap();
    for field in ["loads", "restraints", "mode", "source_hash"] {
        let mut missing = json.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<AnalysisCase>(missing).is_err());
    }
    for mode in ["scout", "balanced", "reference"] {
        json["mode"] = mode.into();
        assert!(serde_json::from_value::<AnalysisCase>(json.clone()).is_err());
    }
    for field in ["result", "safety_factor", "convergence"] {
        let mut invented = serde_json::to_value(&value).unwrap();
        invented[field] = serde_json::json!({"qualified": true});
        assert!(serde_json::from_value::<AnalysisCase>(invented).is_err());
    }
    json = serde_json::to_value(&value).unwrap();
    json["loads"] =
        serde_json::Value::Array(vec![json["loads"][0].clone(); MAX_ANALYSIS_PATCHES + 1]);
    assert!(serde_json::from_value::<AnalysisCase>(json).is_err());
    let record = AnalysisCaseRecord {
        case: value,
        edited_revision: ProjectRevision(1),
    };
    let json = serde_json::json!({"cases": vec![record; MAX_ANALYSIS_CASES + 1]});
    assert!(serde_json::from_value::<AnalysisDocument>(json).is_err());
}
#[test]
fn analysis_document_binds_revision_source_hash_and_layout_membership() {
    let value = case();
    let document = AnalysisDocument {
        cases: vec![AnalysisCaseRecord {
            case: value.clone(),
            edited_revision: ProjectRevision(3),
        }],
    };
    let definitions = vec![StoredDefinition {
        definition_id: value.definition_id.clone(),
        provenance: SourceProvenance {
            source_hash: value.source_hash.clone(),
            source_name: "part.step".into(),
            source_unit: SourceUnit::Millimetre,
            uncertainty_mm: None,
        },
    }];
    let mut layouts = vec![PrintLayoutRecord {
        layout_id: value.layout_id.clone(),
        name: "Plate 1".into(),
        occurrences: vec![OccurrenceRecord {
            occurrence_id: value.occurrence_id,
            definition_id: value.definition_id.clone(),
            pose: RigidPoseMm::IDENTITY,
        }],
        hardware: None,
        manufacturing_intent: None,
        manufacturing_options: vec![],
        manufacturing_artifact: None,
    }];
    document
        .validate_subjects(&definitions, &layouts, ProjectRevision(3))
        .unwrap();
    assert!(
        document
            .validate_subjects(&definitions, &layouts, ProjectRevision(2))
            .is_err()
    );
    assert!(
        document
            .validate_subjects(&[], &layouts, ProjectRevision(3))
            .is_err()
    );
    layouts[0].occurrences.clear();
    assert!(
        document
            .validate_subjects(&definitions, &layouts, ProjectRevision(3))
            .is_err()
    );
}
