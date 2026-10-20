// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use spiling_appearance::*;
use spiling_contracts::appearance::*;
use spiling_contracts::geometry::SourceHash;
use spiling_contracts::project::ProjectErrorCode;
use std::{cell::Cell, sync::Arc};

const PALETTE: &[u8] = include_bytes!("../../../fixtures/appearance/palette.json");
fn palette() -> PaletteDocument {
    parse_palette(PALETTE).unwrap()
}
fn prepare(document: &PaletteDocument) -> PreparedPalette {
    PreparedPalette::new(document.clone(), &canonical_palette(document).unwrap()).unwrap()
}
fn target() -> AppearanceTarget {
    serde_json::from_slice(include_bytes!(
        "../../../fixtures/appearance/target-orange.json"
    ))
    .unwrap()
}
fn near(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{actual:.17} != {expected:.17}"
    );
}
fn measured_conditions() -> AppearanceConditions {
    AppearanceConditions {
        geometry: Some("45:0".into()),
        backing: Some("white".into()),
        thickness_mm: Some(1.0),
        cohort: Some("own-original-cohort".into()),
        instrument: Some("synthetic-test-instrument".into()),
        calibration: Some("synthetic-test-calibration".into()),
        aperture_mm: Some(4.0),
        specular: Some("excluded".into()),
        orientation: Some("normal".into()),
        process: Some("declared-process".into()),
        construction: Some("declared-construction".into()),
        ..AppearanceConditions::default()
    }
}
fn row(id: &str, evidence: AppearanceEvidence) -> PaletteRow {
    PaletteRow {
        key: format!("{id}/e"),
        material_id: id.into(),
        evidence_id: "e".into(),
        name: id.into(),
        evidence,
    }
}

#[test]
fn independently_observed_own_vectors() {
    let vectors: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../fixtures/appearance/reference-vectors.json"
    ))
    .unwrap();
    let array = |v: &serde_json::Value| std::array::from_fn(|i| v[i].as_f64().unwrap());
    for v in vectors["delta_e00"].as_array().unwrap() {
        let a = array(&v["lab1"]);
        let b = array(&v["lab2"]);
        let expected = v["expected"].as_f64().unwrap();
        near(ciede2000(a, b).unwrap(), expected, 2e-12);
        near(ciede2000(b, a).unwrap(), expected, 2e-12);
        near(ciede2000(a, a).unwrap(), 0.0, 1e-14);
    }
    for v in vectors["srgb_to_d65_lab"].as_array().unwrap() {
        let actual = color_to_d65_lab(AppearanceColor::Srgb {
            channels: array(&v["srgb"]),
        })
        .unwrap();
        let expected = array(&v["expected"]);
        for i in 0..3 {
            near(actual[i], expected[i], 3e-12);
        }
    }
}
#[test]
fn explicit_scales_piecewise_transfer_and_unclamped_roundtrip() {
    for srgb in [
        [0.0; 3],
        [1.0; 3],
        [0.04045, 0.0031308, 0.5],
        [1.0, 0.0, 0.0],
        [0.2, 0.8, 0.1],
    ] {
        let xyz = srgb_to_xyz_y1(srgb).unwrap();
        let lab = color_to_d65_lab(AppearanceColor::XyzY1 { channels: xyz }).unwrap();
        let lab100 = color_to_d65_lab(AppearanceColor::XyzY100 {
            channels: xyz.map(|v| 100.0 * v),
        })
        .unwrap();
        let xyz_again = d65_lab_to_xyz_y1(lab).unwrap();
        let rgb_again = xyz_y1_to_srgb(xyz_again).unwrap();
        for i in 0..3 {
            near(lab[i], lab100[i], 2e-12);
            near(xyz[i], xyz_again[i], 2e-14);
            near(srgb[i], rgb_again[i], 4e-8);
        }
    }
    let lab = [50.0, 150.0, -150.0];
    AppearanceColor::D65Lab { channels: lab }
        .validate()
        .unwrap();
    let rgb = xyz_y1_to_srgb(d65_lab_to_xyz_y1(lab).unwrap()).unwrap();
    assert!(rgb.iter().any(|v| !(0.0..=1.0).contains(v)));
    let xyz = [2.0, 1.5, 3.0];
    AppearanceColor::XyzY1 { channels: xyz }.validate().unwrap();
    assert!(color_to_d65_lab(AppearanceColor::XyzY1 { channels: xyz }).unwrap()[0] > 100.0);
    assert!(
        color_to_d65_lab(AppearanceColor::D65Lab {
            channels: [50.0, 300.0, -300.0]
        })
        .is_ok()
    );
    assert_eq!(
        color_to_d65_lab(AppearanceColor::D65Lab {
            channels: [150.0, 300.0, -300.0]
        })
        .unwrap(),
        [150.0, 300.0, -300.0]
    );
    AppearanceColor::D65Lab {
        channels: [-10.0, 0.0, 0.0],
    }
    .validate()
    .unwrap();
    assert!(
        color_to_d65_lab(AppearanceColor::Srgb {
            channels: [f64::NAN, 0.0, 0.0]
        })
        .is_err()
    );
    assert!(ciede2000([50.0, f64::MAX, f64::MAX], [50.0, -f64::MAX, f64::MAX]).is_err());
    AppearanceColor::XyzY100 {
        channels: [-1.0, 0.0, 0.0],
    }
    .validate()
    .unwrap();
    assert!(
        color_to_d65_lab(AppearanceColor::XyzY100 {
            channels: [-1.0, 0.0, 0.0]
        })
        .is_ok()
    );
}
#[test]
fn complete_catalog_match_and_genuine_mismatch() {
    let document = palette();
    let limits = SearchLimits::default();
    let report = select_single(&prepare(&document), &target(), limits, &|| false).unwrap();
    assert_eq!(report.visits.len(), 6);
    assert_eq!(
        report.status,
        AppearanceStatus::Complete {
            outcome: AppearanceOutcome::AssessedMatch
        }
    );
    assert_eq!(
        report.alternatives[0].key,
        "261ae7e7-20d9-5969-9ed5-dd82eea29bcf/catalog-rgb"
    );
    assert_eq!(report.alternatives[0].name, "PLA Prusa Orange");
    assert!(report.visits.iter().all(|v| matches!(
        v.assessment,
        RowAssessment::Assessed {
            evidence: AssessedEvidence::CatalogEstimate,
            ..
        }
    )));
    let red: AppearanceTarget = serde_json::from_slice(include_bytes!(
        "../../../fixtures/appearance/target-red.json"
    ))
    .unwrap();
    let mismatch = select_single(&prepare(&document), &red, limits, &|| false).unwrap();
    assert_eq!(
        mismatch.status,
        AppearanceStatus::Complete {
            outcome: AppearanceOutcome::AssessedMismatch
        }
    );
    assert!(!mismatch.alternatives.is_empty()); // Outside tolerance still has real nearest alternatives.
}
#[test]
fn every_row_charged_and_stops_distinct_from_alternative_truncation() {
    let mut document = palette();
    document.rows[0].evidence = AppearanceEvidence::Unknown {
        reason: "no admissible evidence".into(),
    };
    let complete = select_single(
        &prepare(&document),
        &target(),
        SearchLimits {
            max_visited: 4096,
            max_alternatives: 1,
        },
        &|| false,
    )
    .unwrap();
    assert!(complete.is_complete());
    assert!(complete.alternatives_truncated);
    assert_eq!(complete.visits.len(), 6);
    assert_eq!(complete.alternatives.len(), 1);
    let limited = select_single(
        &prepare(&document),
        &target(),
        SearchLimits {
            max_visited: 1,
            max_alternatives: 1,
        },
        &|| false,
    )
    .unwrap();
    assert_eq!(limited.status, AppearanceStatus::WorkLimit);
    assert_eq!(limited.visits.len(), 1);
    assert_eq!(limited.visits[0].assessment, RowAssessment::UnknownEvidence);
    let calls = Cell::new(0);
    let cancelled = select_single(
        &prepare(&document),
        &target(),
        SearchLimits::default(),
        &|| {
            calls.set(calls.get() + 1);
            calls.get() > 2
        },
    )
    .unwrap();
    assert_eq!(cancelled.status, AppearanceStatus::Cancelled);
    assert_eq!(cancelled.visits.len(), 2);
    assert!(!cancelled.is_complete());
    let immediate = select_single(
        &prepare(&document),
        &target(),
        SearchLimits::default(),
        &|| true,
    )
    .unwrap();
    assert_eq!(immediate.visits.len(), 0);
    assert_eq!(immediate.status, AppearanceStatus::Cancelled);
}
#[test]
fn deterministic_sorted_identity_and_tie_breaking() {
    let mut document = palette();
    let t = target();
    let limits = SearchLimits::default();
    let first = select_single(&prepare(&document), &t, limits, &|| false).unwrap();
    let canonical = canonical_palette(&document).unwrap();
    assert_eq!(
        document.canonical_hash().unwrap(),
        SourceHash::from_bytes(&canonical)
    );
    document.rows.reverse();
    assert_eq!(canonical_palette(&document).unwrap(), canonical);
    assert_eq!(
        select_single(&prepare(&document), &t, limits, &|| false).unwrap(),
        first
    );
    document.rows = vec![
        row(
            "b",
            AppearanceEvidence::CatalogEstimate {
                srgb: [1.0, 0.0, 0.0],
            },
        ),
        row(
            "a",
            AppearanceEvidence::CatalogEstimate {
                srgb: [1.0, 0.0, 0.0],
            },
        ),
    ];
    let tied = select_single(&prepare(&document), &t, limits, &|| false).unwrap();
    assert_eq!(
        tied.alternatives
            .iter()
            .map(|v| v.key.as_str())
            .collect::<Vec<_>>(),
        vec!["a/e", "b/e"]
    );
    assert!(tied.visits.windows(2).all(|v| v[0].key < v[1].key));
    let evidence = document.rows[0].evidence_hash().unwrap();
    document.rows[0].name = "renamed display label".into();
    assert_eq!(document.rows[0].evidence_hash().unwrap(), evidence);
}
#[test]
fn declared_measured_readings_require_matching_complete_conditions() {
    let mut document = palette();
    let mut t = target();
    t.evidence_scope = EvidenceScope::Measured;
    t.conditions = measured_conditions();
    let color = t.color;
    let mut incomplete = t.conditions.clone();
    incomplete.calibration = None;
    let mut incompatible = t.conditions.clone();
    incompatible.illuminant = AppearanceIlluminant::D50;
    let mut other_cohort = t.conditions.clone();
    other_cohort.cohort = Some("different".into());
    document.rows = vec![
        row(
            "a",
            AppearanceEvidence::CatalogEstimate {
                srgb: [1.0, 0.0, 0.0],
            },
        ),
        row(
            "b",
            AppearanceEvidence::Measured {
                color,
                conditions: incomplete,
                material_lot: Some("lot-b".into()),
            },
        ),
        row(
            "c",
            AppearanceEvidence::Measured {
                color,
                conditions: incompatible,
                material_lot: Some("lot-c".into()),
            },
        ),
        row(
            "d",
            AppearanceEvidence::Measured {
                color,
                conditions: other_cohort,
                material_lot: Some("lot-d".into()),
            },
        ),
        row(
            "e",
            AppearanceEvidence::Measured {
                color,
                conditions: t.conditions.clone(),
                material_lot: Some("lot-e".into()),
            },
        ),
    ];
    let report =
        select_single(&prepare(&document), &t, SearchLimits::default(), &|| false).unwrap();
    assert_eq!(report.visits[0].assessment, RowAssessment::FilteredEvidence);
    assert_eq!(report.visits[1].assessment, RowAssessment::UnknownEvidence);
    assert_eq!(
        report.visits[2].assessment,
        RowAssessment::ConditionsMismatch
    );
    assert_eq!(
        report.visits[3].assessment,
        RowAssessment::ConditionsMismatch
    );
    assert_eq!(
        report
            .alternatives
            .iter()
            .map(|v| v.key.as_str())
            .collect::<Vec<_>>(),
        vec!["e/e"]
    );
    document.rows.pop();
    let none = select_single(&prepare(&document), &t, SearchLimits::default(), &|| false).unwrap();
    assert_eq!(
        none.status,
        AppearanceStatus::Complete {
            outcome: AppearanceOutcome::NoAssessedMatch
        }
    );
    assert!(none.alternatives.is_empty());
    t.conditions.observer = AppearanceObserver::Cie1964TenDegree;
    assert!(
        select_single(&prepare(&document), &t, SearchLimits::default(), &|| false)
            .unwrap()
            .alternatives
            .is_empty()
    );
}
#[test]
fn separate_lightness_gate_and_input_identity() {
    let mut document = palette();
    let mut t = target();
    t.color = AppearanceColor::D65Lab {
        channels: [50.0, 0.0, 0.0],
    };
    t.conditions = measured_conditions();
    t.evidence_scope = EvidenceScope::Measured;
    t.delta_e00_tolerance = 100.0;
    t.lightness_tolerance = Some(0.1);
    document.rows = vec![row(
        "a",
        AppearanceEvidence::Measured {
            color: AppearanceColor::D65Lab {
                channels: [51.0, 0.0, 0.0],
            },
            conditions: t.conditions.clone(),
            material_lot: Some("declared-lot".into()),
        },
    )];
    let limited =
        select_single(&prepare(&document), &t, SearchLimits::default(), &|| false).unwrap();
    assert_eq!(
        limited.status,
        AppearanceStatus::Complete {
            outcome: AppearanceOutcome::AssessedMismatch
        }
    );
    t.lightness_tolerance = None;
    let broad = select_single(&prepare(&document), &t, SearchLimits::default(), &|| false).unwrap();
    assert_ne!(limited.input_hash, broad.input_hash);
    assert_eq!(
        broad.status,
        AppearanceStatus::Complete {
            outcome: AppearanceOutcome::AssessedMatch
        }
    );
}
#[test]
fn palette_and_report_records_bind_all_bytes_and_metadata() {
    let document = palette();
    let canonical = canonical_palette(&document).unwrap();
    let record = PaletteRecord::new(&document, PALETTE, &canonical, "test.json".into()).unwrap();
    record.validate_bytes(PALETTE, &canonical).unwrap();
    let mut corrupt = canonical.clone();
    corrupt[0] = b'[';
    assert_eq!(
        record.validate_bytes(PALETTE, &corrupt).unwrap_err().code,
        ProjectErrorCode::CorruptAsset
    );
    let mut wrong = record.clone();
    wrong.source_revision = "wrong".into();
    assert!(wrong.validate_bytes(PALETTE, &canonical).is_err());
    let report = select_single(
        &prepare(&document),
        &target(),
        SearchLimits::default(),
        &|| false,
    )
    .unwrap();
    let bytes = report.encode().unwrap();
    let descriptor = report.record(&bytes).unwrap();
    assert_eq!(descriptor, report.canonical_record().unwrap());
    assert_eq!(descriptor.validate_bytes(&bytes).unwrap(), report);
    let mut wrong = descriptor.clone();
    wrong.complete = false;
    assert!(wrong.validate_bytes(&bytes).is_err());
    let mut wrong = descriptor.clone();
    wrong.input_hash = SourceHash::from_bytes(b"wrong");
    assert!(wrong.validate_bytes(&bytes).is_err());
    let mut wrong = report.clone();
    wrong.alternatives.reverse();
    assert!(wrong.validate().is_err());
    let mut wrong = report.clone();
    wrong.producer = "other".into();
    assert!(wrong.validate().is_err());
    let study = AppearanceStudy {
        palette: Some(record),
        target: Some(target()),
        limits: SearchLimits::default(),
        report: Some(Arc::new(report.clone())),
        report_record: Some(descriptor.clone()),
        selection: Some(report.alternatives[0].key.clone()),
    };
    study.validate().unwrap();
    assert_eq!(study.info().report, Some(descriptor));
    study.info().validate().unwrap();
    let mut wrong = study.clone();
    wrong.selection = Some("absent/e".into());
    assert!(wrong.validate().is_err());
    let mut wrong = study;
    wrong.report_record = None;
    assert!(wrong.validate().is_err());
}
#[test]
fn hostile_imports_are_bounded_before_growth_and_strict() {
    let document = palette();
    let canonical = canonical_palette(&document).unwrap();
    let bound = PaletteDocument::allocation_bound(&canonical).unwrap();
    assert!(document.allocated_bytes() <= bound);
    assert!(PaletteDocument::decode_bounded(&canonical, bound - 1).is_err());
    PaletteDocument::decode_bounded(&canonical, bound).unwrap();
    assert!(
        document
            .canonical_bytes_bounded(canonical.len() - 1)
            .is_err()
    );
    assert_eq!(
        document.canonical_bytes_bounded(canonical.len()).unwrap(),
        canonical
    );
    let mut unknown: serde_json::Value = serde_json::from_slice(PALETTE).unwrap();
    unknown["future"] = true.into();
    assert!(parse_palette(&serde_json::to_vec(&unknown).unwrap()).is_err());
    let mut missing: serde_json::Value = serde_json::to_value(target()).unwrap();
    missing["conditions"]
        .as_object_mut()
        .unwrap()
        .remove("calibration");
    assert!(serde_json::from_value::<AppearanceTarget>(missing).is_err());
    let too_many = format!(
        "{{\"name\":\"x\",\"source_url\":\"https://example.org\",\"source_revision\":\"own\",\"license\":\"OSL-3.0\",\"conversion\":\"{PALETTE_CONVERSION}\",\"rows\":[{}]}}",
        std::iter::repeat_n(
            serde_json::to_string(&document.rows[0]).unwrap(),
            MAX_PALETTE_ROWS + 1
        )
        .collect::<Vec<_>>()
        .join(",")
    );
    assert!(parse_palette(too_many.as_bytes()).is_err());
    let huge_string = format!("{{\"name\":\"{}\"}}", "x".repeat(4097));
    assert_eq!(
        parse_palette(huge_string.as_bytes()).unwrap_err().code,
        ProjectErrorCode::ResourceLimit
    );
    let oversized = vec![b' '; MAX_PALETTE_BYTES + 1];
    assert_eq!(
        parse_palette(&oversized).unwrap_err().code,
        ProjectErrorCode::ResourceLimit
    );
    let mut duplicate = document;
    duplicate.rows.push(duplicate.rows[0].clone());
    assert!(duplicate.validate().is_err());
    assert!(serde_json::from_str::<AppearanceStudy>("{}").is_err());
}
#[test]
fn complete_maximum_rows_fit_one_mebibyte_without_losing_visits() {
    let mut document = palette();
    let mut t = target();
    t.color = AppearanceColor::Srgb {
        channels: [0.123456789012345, 0.234567890123456, 0.345678901234567],
    };
    document.rows = (0..MAX_PALETTE_ROWS)
        .map(|i| {
            let id = format!("{i:05}{}", "x".repeat(57)); // 62 + slash + e = maximal 64-byte key.
            row(
                &id,
                AppearanceEvidence::CatalogEstimate {
                    srgb: [0.987654321098765, 0.876543210987654, 0.765432109876543],
                },
            )
        })
        .collect();
    let report =
        select_single(&prepare(&document), &t, SearchLimits::default(), &|| false).unwrap();
    assert_eq!(report.visits.len(), MAX_PALETTE_ROWS);
    assert!(report.is_complete());
    assert!(report.alternatives_truncated);
    let bytes = report.encode().unwrap();
    assert!(bytes.len() <= MAX_APPEARANCE_REPORT_BYTES);
    assert_eq!(AppearanceReport::decode(&bytes).unwrap(), report);
}

#[test]
fn unresolved_relevant_evidence_prevents_an_all_palette_mismatch_claim() {
    let mut document = palette();
    let mut t = target();
    t.color = AppearanceColor::Srgb {
        channels: [1.0, 0.0, 0.0],
    };
    t.delta_e00_tolerance = 0.01;
    document.rows = vec![
        row(
            "a",
            AppearanceEvidence::CatalogEstimate {
                srgb: [0.0, 1.0, 0.0],
            },
        ),
        row(
            "b",
            AppearanceEvidence::Unknown {
                reason: "no color evidence".into(),
            },
        ),
    ];
    let unknown =
        select_single(&prepare(&document), &t, SearchLimits::default(), &|| false).unwrap();
    assert_eq!(
        unknown.status,
        AppearanceStatus::Complete {
            outcome: AppearanceOutcome::NoAssessedMatch
        }
    );
    assert_eq!(unknown.alternatives.len(), 1);
    let mut invalid = unknown.clone();
    invalid.status = AppearanceStatus::Complete {
        outcome: AppearanceOutcome::AssessedMismatch,
    };
    assert!(invalid.validate().is_err());
    document.rows[1].evidence = AppearanceEvidence::Measured {
        color: t.color,
        conditions: measured_conditions(),
        material_lot: Some("declared-lot".into()),
    };
    let filtered =
        select_single(&prepare(&document), &t, SearchLimits::default(), &|| false).unwrap();
    assert_eq!(
        filtered.status,
        AppearanceStatus::Complete {
            outcome: AppearanceOutcome::AssessedMismatch
        }
    );
    t.evidence_scope = EvidenceScope::Measured;
    t.conditions = measured_conditions();
    document.rows[0].evidence = AppearanceEvidence::Measured {
        color: AppearanceColor::D65Lab {
            channels: [10.0, 0.0, 0.0],
        },
        conditions: t.conditions.clone(),
        material_lot: Some("declared-lot".into()),
    };
    if let AppearanceEvidence::Measured { conditions, .. } = &mut document.rows[1].evidence {
        conditions.observer = AppearanceObserver::Unknown;
    }
    let incompatible =
        select_single(&prepare(&document), &t, SearchLimits::default(), &|| false).unwrap();
    assert_eq!(
        incompatible.status,
        AppearanceStatus::Complete {
            outcome: AppearanceOutcome::NoAssessedMatch
        }
    );
}

#[test]
fn prepared_palette_is_immutable_sorted_and_precharged() {
    let mut document = palette();
    document.rows.reverse();
    let canonical = canonical_palette(&document).unwrap();
    let original = serde_json::to_vec(&document).unwrap();
    let bound = PreparedPalette::allocation_bound(&original).unwrap();
    assert!(PreparedPalette::decode_bounded(&original, &canonical, bound - 1).is_err());
    let prepared = PreparedPalette::decode_bounded(&original, &canonical, bound).unwrap();
    assert!(prepared.allocated_bytes() <= bound);
    assert!(
        prepared
            .document()
            .rows
            .windows(2)
            .all(|rows| rows[0].key < rows[1].key)
    );
    assert_eq!(
        prepared.canonical_hash(),
        &SourceHash::from_bytes(&canonical)
    );
    for (row, hash) in prepared
        .document()
        .rows
        .iter()
        .zip(prepared.evidence_hashes())
    {
        assert_eq!(&row.evidence_hash().unwrap(), hash);
    }
    let one = select_single(&prepared, &target(), SearchLimits::default(), &|| false).unwrap();
    assert_eq!(
        select_single(&prepared, &target(), SearchLimits::default(), &|| false).unwrap(),
        one
    );
}

#[test]
fn declared_process_construction_refs_bind_readings() {
    let mut document = palette();
    let mut t = target();
    t.evidence_scope = EvidenceScope::Measured;
    t.conditions = measured_conditions();
    for field in ["process", "construction"] {
        let mut missing = t.conditions.clone();
        let missing_ref = match field {
            "process" => &mut missing.process,
            _ => &mut missing.construction,
        };
        *missing_ref = None;
        document.rows = vec![row(
            "a",
            AppearanceEvidence::Measured {
                color: t.color,
                conditions: missing,
                material_lot: Some("declared-lot".into()),
            },
        )];
        let report =
            select_single(&prepare(&document), &t, SearchLimits::default(), &|| false).unwrap();
        assert_eq!(report.visits[0].assessment, RowAssessment::UnknownEvidence);
        let mut mismatched = t.conditions.clone();
        let different_ref = match field {
            "process" => &mut mismatched.process,
            _ => &mut mismatched.construction,
        };
        *different_ref = Some("different-declared-reference".into());
        document.rows[0].evidence = AppearanceEvidence::Measured {
            color: t.color,
            conditions: mismatched,
            material_lot: Some("declared-lot".into()),
        };
        let report =
            select_single(&prepare(&document), &t, SearchLimits::default(), &|| false).unwrap();
        assert_eq!(
            report.visits[0].assessment,
            RowAssessment::ConditionsMismatch
        );
        assert_eq!(
            report.status,
            AppearanceStatus::Complete {
                outcome: AppearanceOutcome::NoAssessedMatch
            }
        );
        document.rows[0].evidence = AppearanceEvidence::Measured {
            color: t.color,
            conditions: t.conditions.clone(),
            material_lot: Some("declared-lot".into()),
        };
        let matched =
            select_single(&prepare(&document), &t, SearchLimits::default(), &|| false).unwrap();
        assert_eq!(
            matched.status,
            AppearanceStatus::Complete {
                outcome: AppearanceOutcome::AssessedMatch
            }
        );
        assert_ne!(matched.input_hash, report.input_hash); // target is unchanged: row evidence binds palette identity.
    }
}

#[test]
fn candidate_lots_are_required_evidence_not_a_shared_target_filter() {
    let mut document = palette();
    let mut t = target();
    t.evidence_scope = EvidenceScope::Measured;
    t.conditions = measured_conditions();
    document.rows = vec![
        row(
            "a",
            AppearanceEvidence::Measured {
                color: t.color,
                conditions: t.conditions.clone(),
                material_lot: Some("candidate-lot-a".into()),
            },
        ),
        row(
            "b",
            AppearanceEvidence::Measured {
                color: t.color,
                conditions: t.conditions.clone(),
                material_lot: Some("candidate-lot-b".into()),
            },
        ),
        row(
            "c",
            AppearanceEvidence::Measured {
                color: t.color,
                conditions: t.conditions.clone(),
                material_lot: None,
            },
        ),
    ];
    let prepared = prepare(&document);
    let report = select_single(&prepared, &t, SearchLimits::default(), &|| false).unwrap();
    assert!(matches!(
        report.visits[0].assessment,
        RowAssessment::Assessed {
            evidence: AssessedEvidence::Measured,
            within_tolerance: true,
            ..
        }
    ));
    assert!(matches!(
        report.visits[1].assessment,
        RowAssessment::Assessed {
            evidence: AssessedEvidence::Measured,
            within_tolerance: true,
            ..
        }
    ));
    assert_eq!(report.visits[2].assessment, RowAssessment::UnknownEvidence);
    assert_eq!(
        report
            .alternatives
            .iter()
            .map(|v| v.key.as_str())
            .collect::<Vec<_>>(),
        vec!["a/e", "b/e"]
    );
    let old_hash = document.rows[0].evidence_hash().unwrap();
    if let AppearanceEvidence::Measured { material_lot, .. } = &mut document.rows[0].evidence {
        *material_lot = Some("changed-candidate-lot".into());
    }
    assert_ne!(document.rows[0].evidence_hash().unwrap(), old_hash);
    let changed =
        select_single(&prepare(&document), &t, SearchLimits::default(), &|| false).unwrap();
    assert_ne!(report.input_hash, changed.input_hash);
    let mut strict: serde_json::Value = serde_json::to_value(&document).unwrap();
    strict["rows"][0]["evidence"]
        .as_object_mut()
        .unwrap()
        .remove("material_lot");
    assert!(parse_palette(&serde_json::to_vec(&strict).unwrap()).is_err());
}
