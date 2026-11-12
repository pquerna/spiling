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
            footprint: crate::analysis::load::ForceFootprint::WholeFace {},
            face_id: FaceId::from_step_entity(10).unwrap(),
            force_n: [0.0, -12.0, 0.0],
            moment_n_mm: [20.0, 0.0, 0.0],
        }],
        fasteners: vec![],
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
    for field in ["loads", "restraints", "fasteners", "mode", "source_hash"] {
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
    let json = serde_json::json!({"cases": vec![record; MAX_ANALYSIS_CASES + 1], "results": []});
    assert!(serde_json::from_value::<AnalysisDocument>(json).is_err());
}

#[test]
fn ideal_fastener_contracts_keep_native_ownership_and_motion_explicit() {
    use crate::analysis::fastener::*;
    let mut fastener = AnalysisFastener {
        id: 1,
        name: "Mount screw".into(),
        hole: AnalysisHole {
            bore_faces: vec![
                FaceId::from_step_entity(57).unwrap(),
                FaceId::from_step_entity(64).unwrap(),
            ],
            cap_faces: [
                FaceId::from_step_entity(100).unwrap(),
                FaceId::from_step_entity(101).unwrap(),
            ],
            center_mm: [10.0, 10.0, 0.0],
            radius_mm: 3.0,
            depth_mm: 8.0,
        },
        attachment: FastenerAttachment::Clamp {
            end: HoleEnd::End,
            washer_diameter_mm: 10.0,
        },
    };
    for attachment in [
        FastenerAttachment::Clamp {
            end: HoleEnd::Start,
            washer_diameter_mm: 10.0,
        },
        FastenerAttachment::Pin {
            allow_axial_slide: true,
            allow_spin: true,
        },
        FastenerAttachment::BondedEngagement {
            end: HoleEnd::End,
            engagement_mm: 3.0,
        },
    ] {
        fastener.attachment = attachment;
        let wire: rpc::AnalysisFastener = fastener.clone().try_into().unwrap();
        assert_eq!(AnalysisFastener::try_from(wire).unwrap(), fastener);
        assert_eq!(
            serde_json::from_value::<AnalysisFastener>(serde_json::to_value(&fastener).unwrap())
                .unwrap(),
            fastener
        );
    }
    let mut value = case();
    value.fasteners.push(fastener.clone());
    value.validate().unwrap();
    value.fasteners.push(fastener.clone());
    assert!(value.validate().is_err());
    fastener.attachment = FastenerAttachment::Pin {
        allow_axial_slide: true,
        allow_spin: true,
    };
    assert_eq!(
        fastener.constrained_motions(),
        [true, true, false, true, true, false]
    );
    for attachment in [
        FastenerAttachment::Clamp {
            end: HoleEnd::Start,
            washer_diameter_mm: 6.0,
        },
        FastenerAttachment::Clamp {
            end: HoleEnd::Start,
            washer_diameter_mm: f64::NAN,
        },
        FastenerAttachment::BondedEngagement {
            end: HoleEnd::End,
            engagement_mm: 8.1,
        },
        FastenerAttachment::BondedEngagement {
            end: HoleEnd::End,
            engagement_mm: 0.0,
        },
    ] {
        fastener.attachment = attachment;
        assert!(fastener.validate().is_err());
    }
    fastener.attachment = FastenerAttachment::Pin {
        allow_axial_slide: false,
        allow_spin: false,
    };
    let mut missing = serde_json::to_value(&fastener).unwrap();
    missing["attachment"]
        .as_object_mut()
        .unwrap()
        .remove("allow_spin");
    assert!(serde_json::from_value::<AnalysisFastener>(missing).is_err());
    let mut wire: rpc::AnalysisFastener = fastener.clone().try_into().unwrap();
    wire.attachment = None;
    assert!(AnalysisFastener::try_from(wire).is_err());
    fastener
        .hole
        .bore_faces
        .push(fastener.hole.cap_faces[0].clone());
    assert!(fastener.validate().is_err());
}
#[test]
fn retained_analysis_descriptors_are_required_bounded_and_case_bound() {
    let value = case();
    let result = AnalysisResultRecord {
        case_id: value.id.clone(),
        hash: SourceHash::from_bytes(b"field descriptor, not numerical evidence"),
        byte_count: 100,
        input_hash: SourceHash::from_bytes(b"captured inputs"),
        captured_revision: ProjectRevision(3),
    };
    let document = AnalysisDocument {
        cases: vec![AnalysisCaseRecord {
            case: value,
            edited_revision: ProjectRevision(1),
        }],
        results: vec![result.clone()],
    };
    document.validate().unwrap();
    let json = serde_json::to_value(&document).unwrap();
    let mut missing = json.clone();
    missing.as_object_mut().unwrap().remove("results");
    assert!(serde_json::from_value::<AnalysisDocument>(missing).is_err());
    let mut oversized = json;
    oversized["results"] = serde_json::json!(vec![result; MAX_ANALYSIS_CASES + 1]);
    assert!(serde_json::from_value::<AnalysisDocument>(oversized).is_err());
    for variant in 0..4 {
        let mut invalid = document.clone();
        match variant {
            0 => invalid.results.push(invalid.results[0].clone()),
            1 => invalid.results[0].case_id = AnalysisCaseId::new(),
            2 => invalid.results[0].byte_count = 0,
            _ => invalid.results[0].byte_count = mechanics::MAX_MECHANICAL_FIELD_BYTES as u32 + 1,
        }
        assert!(invalid.validate().is_err());
        let wire = rpc::AnalysisDocument::try_from(invalid);
        assert!(wire.is_err());
    }
    let wire: rpc::AnalysisDocument = document.clone().try_into().unwrap();
    assert_eq!(AnalysisDocument::try_from(wire).unwrap(), document);
}
#[test]
fn analysis_document_binds_revision_source_hash_and_layout_membership() {
    let value = case();
    let document = AnalysisDocument {
        results: vec![],
        cases: vec![AnalysisCaseRecord {
            case: value.clone(),
            edited_revision: ProjectRevision(3),
        }],
    };
    let definitions = vec![StoredDefinition {
        definition_id: value.definition_id.clone(),
        provenance: SourceProvenance {
            normalized: None,
            source_hash: value.source_hash.clone(),
            source_name: "part.step".into(),
            source_unit: SourceUnit::Millimetre,
            uncertainty_mm: None,
            source_solid: None,
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
