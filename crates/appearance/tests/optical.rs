// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use serde::Deserialize;
use spiling_appearance::optical::{compare, resolve_cycle};
use spiling_appearance::{ciede2000, color_to_d65_lab, srgb_to_xyz_y1};
use spiling_contracts::appearance::*;
use spiling_contracts::geometry::SourceHash;
use spiling_contracts::optical::*;
use std::{cell::Cell, sync::Arc};

fn near(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{actual:.17} != {expected:.17}"
    );
}
fn document(colors: &[[u8; 3]]) -> PaletteDocument {
    PaletteDocument {
        name: "Synthetic optical controls".into(),
        source_url: "https://example.com/synthetic-optical".into(),
        source_revision: "own-synthetic-1".into(),
        license: "OSL-3.0".into(),
        conversion: PALETTE_CONVERSION.into(),
        rows: colors
            .iter()
            .enumerate()
            .map(|(i, color)| PaletteRow {
                key: format!("m{i}/e"),
                material_id: format!("m{i}"),
                evidence_id: "e".into(),
                name: format!("synthetic {i}"),
                evidence: AppearanceEvidence::CatalogEstimate {
                    srgb: color.map(|v| v as f64 / 255.0),
                },
            })
            .collect(),
    }
}
fn prepared(document: &PaletteDocument) -> PreparedPalette {
    PreparedPalette::new(document.clone(), &document.canonical_bytes().unwrap()).unwrap()
}
fn record(document: &PaletteDocument) -> PaletteRecord {
    let bytes = document.canonical_bytes().unwrap();
    PaletteRecord::new(document, &bytes, &bytes, "synthetic.json".into()).unwrap()
}
fn patch(z: f64, height: f64) -> OpticalPatch {
    OpticalPatch {
        origin_mm: [0.0, 0.0, z],
        normal_xy: [1.0, 0.0],
        width_mm: 4.0,
        height_mm: height,
    }
}
fn request(count: usize) -> OpticalRequest {
    OpticalRequest {
        model: OpticalModel::PrusaV7_007bb922,
        materials: (0..count).map(|i| format!("m{i}/e")).collect(),
        target: OpticalTarget {
            color: AppearanceColor::Srgb { channels: [0.5; 3] },
            delta_e00_tolerance: 2.0,
            lightness_tolerance: None,
        },
        observation: OpticalObservation {
            illuminant: AppearanceIlluminant::D65,
            observer: AppearanceObserver::Cie1931TwoDegree,
            view: OpticalView::NormalToPatch,
        },
        patch: patch(0.0, 2.0),
        recipe: LayerCycle {
            slots: vec![
                OpticalSlot {
                    material: 0,
                    height_mm: 1.0,
                },
                OpticalSlot {
                    material: 1,
                    height_mm: 1.0,
                },
            ],
            phase_slot: 0,
            origin_z_mm: 0.0,
        },
        search: OpticalSearchSpace {
            patterns: vec![OpticalPattern {
                materials: vec![0, 1],
            }],
            slot_heights_mm: vec![1.0],
            phase_slots: vec![0, 1],
            origin_z_mm: 0.0,
        },
        limits: SearchLimits {
            max_visited: 1024,
            max_alternatives: 256,
        },
    }
}
fn estimate(assessment: &OpticalAssessment) -> ([f64; 3], [u8; 3], f64) {
    let OpticalAssessment::Estimate {
        predicted_lab,
        display_srgb8,
        delta_e00,
        ..
    } = assessment
    else {
        panic!("expected a numeric reference estimate: {assessment:?}")
    };
    (*predicted_lab, *display_srgb8, *delta_e00)
}
#[derive(Deserialize)]
struct Oracle {
    cases: Vec<OracleCase>,
}
#[derive(Deserialize)]
struct OracleCase {
    name: String,
    rgb: Vec<[u8; 3]>,
    ratios: Vec<f64>,
    lab: [f64; 3],
    display: [u8; 3],
}

#[test]
fn pinned_upstream_synthetic_oracle_through_public_compare() {
    let oracle: Oracle =
        serde_json::from_slice(include_bytes!("fixtures/prusa-v7-synthetic.json")).unwrap();
    for case in oracle.cases {
        let mut colors = case.rgb.clone();
        if colors.len() == 1 {
            colors.push([123, 45, 67]);
        }
        let palette = prepared(&document(&colors));
        let mut query = request(colors.len());
        query.recipe.slots = case
            .ratios
            .iter()
            .enumerate()
            .map(|(i, &height_mm)| OpticalSlot {
                material: i as u8,
                height_mm,
            })
            .collect();
        query.patch.height_mm = case.ratios.iter().sum();
        let request = Arc::new(query);
        let report = compare(&palette, request.clone(), &|| false).unwrap();
        assert!(Arc::ptr_eq(&request, &report.request));
        let (lab, display, error) = estimate(&report.requested);
        for i in 0..3 {
            near(lab[i], case.lab[i], 2e-10);
        }
        assert_eq!(display, case.display, "{}", case.name);
        near(
            error,
            ciede2000(lab, color_to_d65_lab(request.target.color).unwrap()).unwrap(),
            2e-12,
        );
        // Exact raw reference Lab, not Lab recomputed from its clipped display.
        if case.name == "black_white_equal" {
            let displayed = color_to_d65_lab(AppearanceColor::Srgb {
                channels: display.map(|v| v as f64 / 255.0),
            })
            .unwrap();
            assert!((lab[0] - displayed[0]).abs() > 0.1);
        }
    }
}

#[test]
fn unequal_rotated_negative_origin_and_partial_cycle_exposure() {
    let cycle = LayerCycle {
        slots: vec![
            OpticalSlot {
                material: 0,
                height_mm: 1.0,
            },
            OpticalSlot {
                material: 1,
                height_mm: 2.0,
            },
            OpticalSlot {
                material: 2,
                height_mm: 1.0,
            },
        ],
        phase_slot: 1,
        origin_z_mm: -2.0,
    };
    // Rotated order at -2: material 1 for 2mm, material 2 for 1mm,
    // material 0 for 1mm. Window [-3.5, 3) exposes [2, 3, 1.5].
    let exposure = resolve_cycle(&cycle, &patch(-3.5, 6.5)).unwrap();
    near(exposure.period_mm, 4.0, 0.0);
    near(exposure.phase_at_patch_mm, 2.5, 0.0);
    assert_eq!(exposure.complete_periods, 1);
    for (i, expected) in [2.0, 3.0, 1.5].into_iter().enumerate() {
        near(exposure.exposed_height_mm[i], expected, 1e-14);
        near(exposure.fractions[i], expected / 6.5, 1e-14);
    }
    let partial = resolve_cycle(&cycle, &patch(-1.5, 0.25)).unwrap();
    assert_eq!(partial.exposed_height_mm, [0.0, 0.25, 0.0]);
    assert_eq!(partial.fractions, [0.0, 1.0, 0.0]);
    let mut translated = cycle.clone();
    translated.origin_z_mm += 8.0;
    assert_eq!(
        resolve_cycle(&translated, &patch(4.5, 6.5)).unwrap(),
        exposure
    );
    let mut wider = patch(-3.5, 6.5);
    wider.width_mm = 400.0;
    assert_eq!(resolve_cycle(&cycle, &wider).unwrap(), exposure);
}

#[test]
fn analytic_exposure_agrees_with_independent_short_interval_integral() {
    let mut cycle = LayerCycle {
        slots: vec![
            OpticalSlot {
                material: 0,
                height_mm: 0.25,
            },
            OpticalSlot {
                material: 1,
                height_mm: 0.75,
            },
            OpticalSlot {
                material: 0,
                height_mm: 0.5,
            },
        ],
        phase_slot: 0,
        origin_z_mm: -1.25,
    };
    for phase in 0..3 {
        cycle.phase_slot = phase;
        for (start, height) in [(-3.0, 0.125), (-1.75, 2.875), (0.0, 4.25), (0.375, 0.875)] {
            let p = patch(start, height);
            let actual = resolve_cycle(&cycle, &p).unwrap();
            let mut expected = [0.0; 3];
            // Independent explicit interval clipping for a deliberately small
            // test window; production must not enumerate these physical layers.
            for repeat in -8..9 {
                let mut z = cycle.origin_z_mm + repeat as f64 * 1.5;
                for offset in 0..3 {
                    let slot = cycle.slots[(phase as usize + offset) % 3];
                    expected[slot.material as usize] +=
                        (z + slot.height_mm).min(start + height).max(start)
                            - z.max(start).min(start + height);
                    z += slot.height_mm;
                }
            }
            for i in 0..3 {
                near(actual.exposed_height_mm[i], expected[i], 2e-14);
            }
        }
    }
}

#[test]
fn huge_finite_cycles_and_representability_refusals() {
    let cycle = LayerCycle {
        slots: vec![
            OpticalSlot {
                material: 0,
                height_mm: 0.25,
            },
            OpticalSlot {
                material: 1,
                height_mm: 0.25,
            },
        ],
        phase_slot: 0,
        origin_z_mm: 0.0,
    };
    let actual =
        resolve_cycle(&cycle, &patch(-1_000_000_000_000.0, 2_000_000_000_000.125)).unwrap();
    assert_eq!(actual.complete_periods, 4_000_000_000_000);
    near(actual.exposed_height_mm[0], 1_000_000_000_000.125, 0.0);
    near(actual.exposed_height_mm[1], 1_000_000_000_000.0, 0.0);
    assert!(resolve_cycle(&cycle, &patch(0.0, 3_000_000_000_000_000.0)).is_err());
    assert!(resolve_cycle(&cycle, &patch(1e30, 1.0)).is_err());
    let mut lost_slot = cycle.clone();
    lost_slot.slots[1].height_mm = f64::MIN_POSITIVE;
    assert!(resolve_cycle(&cycle, &patch(-1e-20, 1e-21)).is_err());
    assert!(resolve_cycle(&lost_slot, &patch(0.0, 1.0)).is_err());
    let decimal = LayerCycle {
        slots: vec![OpticalSlot {
            material: 0,
            height_mm: 0.1,
        }],
        phase_slot: 0,
        origin_z_mm: 0.0,
    };
    let decimal = resolve_cycle(&decimal, &patch(0.0, 1.0)).unwrap();
    // f64(0.1) is slightly above 1/10: the real binary-input quotient is 9,
    // not a rounded h/period=10 plus an additional nearly-full remainder.
    assert_eq!(decimal.complete_periods, 9);
    near(decimal.exposed_height_mm[0], 1.0, 2e-15);
}

#[test]
fn equal_fractions_do_not_fabricate_sequence_or_phase_color_effects() {
    let palette = prepared(&document(&[[255, 0, 0], [0, 255, 0]]));
    let mut query = request(2);
    query.search.patterns = vec![
        OpticalPattern {
            materials: vec![0, 1],
        },
        OpticalPattern {
            materials: vec![1, 0],
        },
        OpticalPattern {
            materials: vec![0, 0, 1, 1],
        },
    ];
    query.search.slot_heights_mm = vec![0.5];
    query.search.phase_slots = vec![0, 1, 2, 3];
    let report = compare(&palette, Arc::new(query), &|| false).unwrap();
    let reference = estimate(&report.requested);
    for visit in &report.visits {
        if !matches!(visit.assessment, OpticalAssessment::FilteredPhase) {
            assert_eq!(estimate(&visit.assessment), reference);
        }
    }
    assert_eq!(report.total_candidates, 12);
    assert_eq!(report.alternatives, vec![0, 1, 4, 5, 8, 9, 10, 11]);
}

#[test]
fn grid_order_filtered_visits_work_cancel_and_output_cap_are_independent() {
    let palette = prepared(&document(&[[0, 0, 0], [255, 255, 255]]));
    let mut query = request(2);
    query.search.patterns = vec![
        OpticalPattern { materials: vec![0] },
        OpticalPattern {
            materials: vec![0, 1],
        },
    ];
    query.search.slot_heights_mm = vec![0.5, 1.0];
    query.search.phase_slots = vec![1, 0];
    query.limits.max_alternatives = 1;
    for (index, (materials, height, phase)) in [
        (vec![0], 0.5, 1),
        (vec![0], 0.5, 0),
        (vec![0], 1.0, 1),
        (vec![0], 1.0, 0),
        (vec![0, 1], 0.5, 1),
        (vec![0, 1], 0.5, 0),
        (vec![0, 1], 1.0, 1),
        (vec![0, 1], 1.0, 0),
    ]
    .into_iter()
    .enumerate()
    {
        let cycle = query.candidate(index as u32).unwrap();
        assert_eq!(
            cycle.slots.iter().map(|s| s.material).collect::<Vec<_>>(),
            materials
        );
        assert!(cycle.slots.iter().all(|s| s.height_mm == height));
        assert_eq!(cycle.phase_slot, phase);
    }
    let full = compare(&palette, Arc::new(query.clone()), &|| false).unwrap();
    assert!(full.is_complete());
    assert_eq!(full.visits.len(), 8);
    assert!(full.alternatives_truncated);
    assert!(matches!(
        full.visits[0].assessment,
        OpticalAssessment::FilteredPhase
    ));
    query.limits.max_visited = 3;
    let stopped = compare(&palette, Arc::new(query.clone()), &|| false).unwrap();
    assert_eq!(stopped.status, AppearanceStatus::WorkLimit);
    assert_eq!(stopped.visits.len(), 3);
    assert!(!stopped.is_complete());
    assert_eq!(stopped.alternatives.len(), 1);
    query.limits.max_visited = 1024;
    let calls = Cell::new(0);
    let cancelled = compare(&palette, Arc::new(query.clone()), &|| {
        let old = calls.get();
        calls.set(old + 1);
        old == 3
    })
    .unwrap();
    assert_eq!(cancelled.status, AppearanceStatus::Cancelled);
    assert_eq!(cancelled.visits.len(), 3);
    let immediate = compare(&palette, Arc::new(query), &|| true).unwrap();
    assert!(immediate.visits.is_empty());
    assert_eq!(immediate.status, AppearanceStatus::Cancelled);
    assert!(matches!(
        immediate.requested,
        OpticalAssessment::Estimate { .. }
    ));
}

fn measured_conditions() -> AppearanceConditions {
    AppearanceConditions {
        geometry: Some("normal".into()),
        backing: Some("synthetic white".into()),
        thickness_mm: Some(1.0),
        cohort: Some("synthetic".into()),
        instrument: Some("synthetic".into()),
        calibration: Some("synthetic".into()),
        aperture_mm: Some(1.0),
        specular: Some("excluded".into()),
        orientation: Some("normal".into()),
        process: Some("synthetic".into()),
        construction: Some("synthetic".into()),
        ..AppearanceConditions::default()
    }
}
#[test]
fn evidence_conditions_domain_and_only_positive_exposure_authority() {
    let mut doc = document(&[[51, 85, 119], [255, 255, 255], [0, 0, 0]]);
    let mut query = request(3);
    doc.rows[1].evidence = AppearanceEvidence::Unknown {
        reason: "no reading".into(),
    };
    query.recipe.slots = vec![OpticalSlot {
        material: 0,
        height_mm: 2.0,
    }];
    let unused_unknown = compare(&prepared(&doc), Arc::new(query.clone()), &|| false).unwrap();
    assert!(matches!(
        unused_unknown.requested,
        OpticalAssessment::Estimate { .. }
    ));
    assert!(matches!(
        unused_unknown.visits[0].assessment,
        OpticalAssessment::Unassessed {
            reason: OpticalUnknownReason::MissingEvidence,
            ..
        }
    ));
    doc.rows[0].evidence = AppearanceEvidence::Measured {
        color: AppearanceColor::XyzY1 {
            channels: srgb_to_xyz_y1([0.2, 1.0 / 3.0, 119.0 / 255.0]).unwrap(),
        },
        conditions: measured_conditions(),
        material_lot: Some("own-synthetic-lot".into()),
    };
    let supported = compare(&prepared(&doc), Arc::new(query.clone()), &|| false).unwrap();
    assert_eq!(
        estimate(&supported.requested),
        estimate(&unused_unknown.requested)
    );
    if let AppearanceEvidence::Measured { color, .. } = &mut doc.rows[0].evidence {
        *color = AppearanceColor::D65Lab {
            channels: [50.0, 150.0, -150.0],
        };
    }
    let domain = compare(&prepared(&doc), Arc::new(query.clone()), &|| false).unwrap();
    assert!(matches!(
        domain.requested,
        OpticalAssessment::Unassessed {
            reason: OpticalUnknownReason::UnsupportedColorDomain,
            ..
        }
    ));
    if let AppearanceEvidence::Measured { conditions, .. } = &mut doc.rows[0].evidence {
        conditions.illuminant = AppearanceIlluminant::D50;
    }
    let unsupported = compare(&prepared(&doc), Arc::new(query.clone()), &|| false).unwrap();
    assert!(matches!(
        unsupported.requested,
        OpticalAssessment::Unassessed {
            reason: OpticalUnknownReason::UnsupportedConditions,
            ..
        }
    ));
    query.observation.observer = AppearanceObserver::Unknown;
    let unknown = compare(&prepared(&doc), Arc::new(query), &|| false).unwrap();
    assert_eq!(
        unknown.status,
        AppearanceStatus::Complete {
            outcome: AppearanceOutcome::NoAssessedMatch
        }
    );
    assert!(unknown.alternatives.is_empty());
    assert!(!unknown.has_selection(&OpticalSelection::Requested));
}

#[test]
fn identity_binds_order_phase_width_conditions_and_source_evidence() {
    let doc = document(&[[0, 0, 0], [255, 255, 255]]);
    let palette = prepared(&doc);
    let original = request(2);
    let hash = optical_input_hash(palette.canonical_hash(), &original).unwrap();
    let mut changes = Vec::new();
    let mut q = original.clone();
    q.patch.width_mm *= 2.0;
    changes.push(q);
    let mut q = original.clone();
    q.recipe.phase_slot = 1;
    changes.push(q);
    let mut q = original.clone();
    q.search.phase_slots.reverse();
    changes.push(q);
    let mut q = original.clone();
    q.materials.reverse();
    changes.push(q);
    let mut q = original.clone();
    q.observation.illuminant = AppearanceIlluminant::Unknown;
    changes.push(q);
    let mut q = original.clone();
    q.patch.origin_mm[0] = 10.0;
    changes.push(q);
    for change in changes {
        assert_ne!(
            optical_input_hash(palette.canonical_hash(), &change).unwrap(),
            hash
        );
    }
    assert_ne!(
        optical_input_hash(&SourceHash::from_bytes(b"other source"), &original).unwrap(),
        hash
    );
    let report = compare(&palette, Arc::new(original), &|| false).unwrap();
    assert_eq!(report.material_evidence_hashes, palette.evidence_hashes());
    let mut other = doc.clone();
    other.rows[0].name = "renamed source label".into();
    assert_ne!(prepared(&other).canonical_hash(), palette.canonical_hash());
    assert_eq!(
        prepared(&other).evidence_hashes(),
        palette.evidence_hashes()
    );
}

#[test]
fn strict_request_admission_rejects_bad_cardinality_numbers_and_json() {
    let original = request(2);
    let mut bad = Vec::new();
    let mut q = original.clone();
    q.recipe.phase_slot = 2;
    bad.push(q);
    let mut q = original.clone();
    q.recipe.slots[0].material = 2;
    bad.push(q);
    let mut q = original.clone();
    q.patch.normal_xy = [0.0, 0.0];
    bad.push(q);
    let mut q = original.clone();
    q.patch.height_mm = f64::INFINITY;
    bad.push(q);
    let mut q = original.clone();
    q.patch.origin_mm[0] = f64::NAN;
    bad.push(q);
    let mut q = original.clone();
    q.recipe.slots[0].height_mm = 0.0;
    bad.push(q);
    let mut q = original.clone();
    q.materials[1] = q.materials[0].clone();
    bad.push(q);
    let mut q = original.clone();
    q.search.patterns.push(q.search.patterns[0].clone());
    bad.push(q);
    let mut q = original.clone();
    q.search.slot_heights_mm = vec![1.0, 1.0];
    bad.push(q);
    let mut q = original.clone();
    q.search.phase_slots = vec![0, 0];
    bad.push(q);
    let mut q = original.clone();
    q.search.phase_slots = vec![64];
    bad.push(q);
    let mut q = original.clone();
    q.limits.max_visited = 1025;
    bad.push(q);
    let mut q = original.clone();
    q.search.patterns.clear();
    bad.push(q);
    for query in bad {
        assert!(query.validate().is_err());
    }
    let mut json = serde_json::to_value(&original).unwrap();
    json.as_object_mut()
        .unwrap()
        .insert("unrecognized".into(), true.into());
    assert!(OpticalRequest::decode(&serde_json::to_vec(&json).unwrap()).is_err());
    let mut json = serde_json::to_value(&original).unwrap();
    json["target"]
        .as_object_mut()
        .unwrap()
        .remove("lightness_tolerance");
    assert!(OpticalRequest::decode(&serde_json::to_vec(&json).unwrap()).is_err());
    let mut json = serde_json::to_value(&original).unwrap();
    json["materials"] = serde_json::json!(["m0/e", "m1/e", "m2/e", "m3/e"]);
    assert!(OpticalRequest::decode(&serde_json::to_vec(&json).unwrap()).is_err());
    assert!(OpticalRequest::decode(&vec![b' '; MAX_OPTICAL_REQUEST_BYTES + 1]).is_err());
    let mut doc = document(&[[0, 0, 0], [255, 255, 255]]);
    doc.rows[1].material_id = "m0".into();
    doc.rows[1].evidence_id = "f".into();
    doc.rows[1].key = "m0/f".into();
    let mut query = original;
    query.materials[1] = "m0/f".into();
    assert!(query.validate_palette(&prepared(&doc)).is_err());
}

#[test]
fn report_validation_and_acceptance_bind_real_estimates_and_exact_bytes() {
    let doc = document(&[[0, 0, 0], [255, 255, 255]]);
    let palette_record = record(&doc);
    let mut query = request(2);
    query.limits.max_alternatives = 1;
    let report = compare(&prepared(&doc), Arc::new(query.clone()), &|| false).unwrap();
    let bytes = report.encode().unwrap();
    let descriptor = report.canonical_record().unwrap();
    assert_eq!(descriptor.validate_bytes(&bytes).unwrap(), report);
    let mut damaged = bytes.clone();
    damaged[0] = b'[';
    assert!(descriptor.validate_bytes(&damaged).is_err());
    let mut invalid = report.clone();
    invalid.visits[0].index = 1;
    assert!(invalid.validate().is_err());
    let mut invalid = report.clone();
    invalid.alternatives = vec![1];
    assert!(invalid.validate().is_err());
    let mut invalid = report.clone();
    invalid.alternatives_truncated = false;
    assert!(invalid.validate().is_err());
    let mut invalid = report.clone();
    invalid.status = AppearanceStatus::WorkLimit;
    assert!(invalid.validate().is_err());
    let mut invalid = report.clone();
    invalid.total_candidates += 1;
    assert!(invalid.validate().is_err());
    let mut invalid = report.clone();
    invalid.material_evidence_hashes.clear();
    assert!(invalid.validate().is_err());
    let mut invalid = report.clone();
    invalid.input_hash = SourceHash::from_bytes(b"wrong");
    assert!(invalid.validate().is_err());
    let mut invalid = report.clone();
    if let OpticalAssessment::Estimate { exposure, .. } = &mut invalid.requested {
        exposure.fractions[0] = 0.0;
    }
    assert!(invalid.validate().is_err());
    let mut invalid = report.clone();
    if let OpticalAssessment::Estimate {
        within_tolerance, ..
    } = &mut invalid.requested
    {
        *within_tolerance = !*within_tolerance;
    }
    assert!(invalid.validate().is_err());
    let mut invalid = report.clone();
    if let OpticalAssessment::Estimate { predicted_lab, .. } = &mut invalid.requested {
        predicted_lab[0] = f64::NAN;
    }
    assert!(invalid.validate().is_err());
    let mut study = OpticalStudy::new(query);
    study.report = Some(Arc::new(report.clone()));
    study.report_record = Some(descriptor);
    study.selection = Some(OpticalSelection::Requested);
    study.validate(&palette_record).unwrap();
    study.selection = Some(OpticalSelection::Candidate { index: 1 });
    assert!(study.validate(&palette_record).is_err());
    study.selection = None;
    let mut stopped = report;
    stopped.visits.pop();
    stopped.alternatives = vec![0];
    stopped.alternatives_truncated = false;
    stopped.status = AppearanceStatus::Cancelled;
    stopped.validate().unwrap();
    study.report_record = Some(stopped.canonical_record().unwrap());
    study.report = Some(Arc::new(stopped));
    assert!(study.validate(&palette_record).is_err());
}

#[test]
fn maximum_grid_is_complete_and_not_stopped_by_top_k_retention() {
    let palette = prepared(&document(&[[0, 0, 0], [255, 255, 255]]));
    let mut query = request(2);
    query.search.patterns = (0..64)
        .map(|bits| OpticalPattern {
            materials: (0..6).map(|bit| ((bits >> bit) & 1) as u8).collect(),
        })
        .collect();
    query.search.slot_heights_mm = (1..=16).map(|height| height as f64 / 8.0).collect();
    query.search.phase_slots = vec![0];
    assert_eq!(query.candidate_count().unwrap(), 1024);
    let report = compare(&palette, Arc::new(query.clone()), &|| false).unwrap();
    assert!(report.is_complete());
    assert_eq!(report.visits.len(), 1024);
    assert_eq!(report.alternatives.len(), 256);
    assert!(report.alternatives_truncated);
    assert!(report.encode().unwrap().len() <= MAX_APPEARANCE_REPORT_BYTES);
    query.search.phase_slots.push(1);
    assert!(query.candidate_count().is_err());
    assert!(query.validate().is_err());
}

#[test]
fn target_is_unquantized_and_unknown_reports_cannot_select_an_estimate() {
    let doc = document(&[[0, 0, 0], [255, 255, 255]]);
    let palette = prepared(&doc);
    let mut query = request(2);
    let first = compare(&palette, Arc::new(query.clone()), &|| false).unwrap();
    query.target.color = AppearanceColor::Srgb {
        channels: [0.5001; 3],
    };
    let second = compare(&palette, Arc::new(query.clone()), &|| false).unwrap();
    assert_eq!(estimate(&first.requested).0, estimate(&second.requested).0);
    assert_eq!(estimate(&first.requested).1, estimate(&second.requested).1);
    assert!((estimate(&first.requested).2 - estimate(&second.requested).2).abs() > 1e-6);
    assert_ne!(first.input_hash, second.input_hash);
    query.observation.illuminant = AppearanceIlluminant::Unknown;
    let report = compare(&palette, Arc::new(query.clone()), &|| false).unwrap();
    let mut study = OpticalStudy::new(query);
    study.report_record = Some(report.canonical_record().unwrap());
    study.report = Some(Arc::new(report));
    study.validate(&record(&doc)).unwrap();
    study.selection = Some(OpticalSelection::Requested);
    assert!(study.validate(&record(&doc)).is_err());
}

#[test]
fn malformed_report_and_bounded_nested_sequences_are_rejected() {
    let report = compare(
        &prepared(&document(&[[0, 0, 0], [255, 255, 255]])),
        Arc::new(request(2)),
        &|| false,
    )
    .unwrap();
    let mut value = serde_json::to_value(&report).unwrap();
    value["input_hash"] = serde_json::json!("A".repeat(64));
    assert!(OpticalReport::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    let mut value = serde_json::to_value(&report).unwrap();
    value["material_evidence_hashes"] = serde_json::json!([
        report.input_hash,
        report.palette_hash,
        report.input_hash,
        report.palette_hash
    ]);
    assert!(OpticalReport::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    let mut value = serde_json::to_value(request(2)).unwrap();
    value["recipe"]["slots"] =
        serde_json::json!(vec![serde_json::json!({"material":0,"height_mm":1.0}); 65]);
    assert!(OpticalRequest::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    let mut value = serde_json::to_value(request(2)).unwrap();
    value["search"]["slot_heights_mm"] = serde_json::json!((1..=17).collect::<Vec<_>>());
    assert!(OpticalRequest::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    let mut value = serde_json::to_value(request(2)).unwrap();
    value["materials"][0] = serde_json::json!("x".repeat(65));
    assert!(OpticalRequest::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    assert!(OpticalReport::decode(&vec![b' '; MAX_APPEARANCE_REPORT_BYTES + 1]).is_err());
}

#[test]
fn requested_forward_match_does_not_claim_a_grid_match() {
    let doc = document(&[[0, 0, 0], [255, 255, 255]]);
    let mut query = request(2);
    query.recipe.slots = vec![OpticalSlot {
        material: 0,
        height_mm: 1.0,
    }];
    query.target.color = AppearanceColor::D65Lab { channels: [0.0; 3] };
    query.target.delta_e00_tolerance = 0.0;
    query.search.patterns = vec![OpticalPattern { materials: vec![1] }];
    query.search.phase_slots = vec![0];
    let report = compare(&prepared(&doc), Arc::new(query), &|| false).unwrap();
    assert_eq!(estimate(&report.requested).2, 0.0);
    assert_eq!(
        report.status,
        AppearanceStatus::Complete {
            outcome: AppearanceOutcome::AssessedMismatch
        }
    );
    assert!(report.has_selection(&OpticalSelection::Requested));
}

#[test]
fn catalog_basis_quantization_is_explicit_but_evidence_identity_is_preserved() {
    let mut doc = document(&[[128, 128, 128], [255, 255, 255]]);
    let mut query = request(2);
    query.recipe.slots = vec![OpticalSlot {
        material: 0,
        height_mm: 1.0,
    }];
    let byte_basis = compare(&prepared(&doc), Arc::new(query.clone()), &|| false).unwrap();
    doc.rows[0].evidence = AppearanceEvidence::CatalogEstimate { srgb: [0.5; 3] };
    let half_basis = compare(&prepared(&doc), Arc::new(query), &|| false).unwrap();
    assert_eq!(
        estimate(&byte_basis.requested),
        estimate(&half_basis.requested)
    );
    assert_eq!(estimate(&half_basis.requested).1, [128; 3]);
    assert_ne!(
        byte_basis.material_evidence_hashes,
        half_basis.material_evidence_hashes
    );
    assert_ne!(byte_basis.input_hash, half_basis.input_hash);
}

#[test]
fn retained_allocation_accounting_excludes_shared_request_and_report_storage() {
    let doc = document(&[[0, 0, 0], [255, 255, 255]]);
    let mut report = compare(&prepared(&doc), Arc::new(request(2)), &|| false).unwrap();
    let report_charge = report.allocated_bytes();
    let old_request_charge = report.request.allocated_bytes();
    Arc::make_mut(&mut report.request).materials[0].reserve(256);
    assert!(report.request.allocated_bytes() > old_request_charge);
    assert_eq!(report.allocated_bytes(), report_charge);
    let mut study = OpticalStudy::new(report.request.as_ref().clone());
    study.request = report.request.clone();
    study.report_record = Some(report.canonical_record().unwrap());
    study.report = Some(Arc::new(report));
    study.validate(&record(&doc)).unwrap();
    let study_charge = study.allocated_bytes();
    Arc::make_mut(&mut study.request).materials[0].reserve(512);
    assert_eq!(study.allocated_bytes(), study_charge);
    study.validate(&record(&doc)).unwrap();
}
