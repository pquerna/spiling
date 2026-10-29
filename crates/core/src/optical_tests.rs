// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::tests::{commit_appearance, manufactured};

pub(crate) fn optical_palette() -> Arc<PaletteAsset> {
    let mut document = crate::tests::palette_asset("palette.json")
        .document()
        .clone();
    document.rows.push(PaletteRow {
        key: "blue/catalog".into(),
        material_id: "blue".into(),
        evidence_id: "catalog".into(),
        name: "Blue".into(),
        evidence: AppearanceEvidence::CatalogEstimate {
            srgb: [0.0, 0.0, 1.0],
        },
    });
    let original = serde_json::to_vec_pretty(&document).unwrap();
    let canonical = document.canonical_bytes().unwrap();
    let record =
        PaletteRecord::new(&document, &original, &canonical, "palette.json".into()).unwrap();
    Arc::new(PaletteAsset::from_document(record, original, canonical, document).unwrap())
}

pub(crate) fn optical_request() -> OpticalRequest {
    OpticalRequest {
        model: OpticalModel::PrusaV7_007bb922,
        materials: vec!["red/catalog".into(), "blue/catalog".into()],
        target: OpticalTarget {
            color: AppearanceColor::D65Lab {
                channels: [50.0, 0.0, 0.0],
            },
            delta_e00_tolerance: 2.0,
            lightness_tolerance: None,
        },
        observation: OpticalObservation {
            illuminant: AppearanceIlluminant::D65,
            observer: AppearanceObserver::Cie1931TwoDegree,
            view: OpticalView::NormalToPatch,
        },
        patch: OpticalPatch {
            origin_mm: [0.0; 3],
            normal_xy: [1.0, 0.0],
            width_mm: 2.0,
            height_mm: 2.0,
        },
        recipe: LayerCycle {
            slots: vec![
                OpticalSlot {
                    material: 0,
                    height_mm: 0.5,
                },
                OpticalSlot {
                    material: 1,
                    height_mm: 0.5,
                },
            ],
            phase_slot: 0,
            origin_z_mm: 0.0,
        },
        search: OpticalSearchSpace {
            patterns: vec![OpticalPattern {
                materials: vec![0, 1],
            }],
            slot_heights_mm: vec![0.5],
            phase_slots: vec![0],
            origin_z_mm: 0.0,
        },
        limits: SearchLimits {
            max_visited: 1,
            max_alternatives: 1,
        },
    }
}

// Core admits a checked report and input closure, not a second optical evaluator.
// These synthetic finite numbers isolate lifecycle validation from model math.
pub(crate) fn optical_report(
    asset: &PaletteAsset,
    request: Arc<OpticalRequest>,
) -> Arc<OpticalReport> {
    let assessment = OpticalAssessment::Estimate {
        exposure: CycleExposure {
            period_mm: 1.0,
            phase_at_patch_mm: 0.0,
            complete_periods: 2,
            exposed_height_mm: [1.0, 1.0, 0.0],
            fractions: [0.5, 0.5, 0.0],
        },
        predicted_lab: [50.0, 0.0, 0.0],
        display_srgb8: [119; 3],
        delta_e00: 0.0,
        delta_lightness: 0.0,
        within_tolerance: true,
    };
    let material_evidence_hashes = request
        .materials
        .iter()
        .map(|key| {
            let index = asset
                .document()
                .rows
                .binary_search_by(|row| row.key.cmp(key))
                .unwrap();
            asset.prepared().evidence_hashes()[index].clone()
        })
        .collect();
    let report = Arc::new(OpticalReport {
        producer: OPTICAL_PRODUCER.into(),
        input_hash: optical_input_hash(&asset.record().canonical_hash, &request).unwrap(),
        palette_hash: asset.record().canonical_hash.clone(),
        request,
        material_evidence_hashes,
        evidence: OpticalEvidence::ReferenceEstimate,
        execution: OpticalExecution::StudyOnlyNotRouted,
        requested: assessment.clone(),
        total_candidates: 1,
        visits: vec![OpticalVisit {
            index: 0,
            assessment,
        }],
        alternatives: vec![0],
        alternatives_truncated: false,
        status: AppearanceStatus::Complete {
            outcome: AppearanceOutcome::AssessedMatch,
        },
    });
    report.validate().unwrap();
    report
}

pub(crate) fn optical_project() -> (Project, Arc<PaletteAsset>, Arc<OpticalReport>) {
    let mut project = manufactured();
    let asset = optical_palette();
    commit_appearance(
        &mut project,
        Edit::ImportAppearancePalette {
            asset: asset.clone(),
        },
    );
    commit_appearance(
        &mut project,
        Edit::SetOpticalStudy {
            request: optical_request(),
        },
    );
    let request = project
        .appearance_study()
        .optical
        .as_ref()
        .unwrap()
        .request
        .clone();
    let report = optical_report(&asset, request);
    commit_appearance(
        &mut project,
        Edit::AcceptOpticalReport {
            report: report.clone(),
            selection: Some(OpticalSelection::Requested),
        },
    );
    (project, asset, report)
}

#[test]
fn optical_acceptance_requires_exact_request_producer_evidence_and_complete_estimate_selection() {
    let (project, _, report) = optical_project();
    let mut foreign = (*report).clone();
    foreign.material_evidence_hashes[0] = SourceHash::from_bytes(b"foreign evidence");
    assert_eq!(
        project
            .prepare(Edit::AcceptOpticalReport {
                report: Arc::new(foreign),
                selection: None
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::CorruptAsset
    );

    let mut foreign = (*report).clone();
    let mut request = (*foreign.request).clone();
    request.patch.width_mm = 3.0;
    foreign.request = Arc::new(request);
    foreign.input_hash = optical_input_hash(&foreign.palette_hash, &foreign.request).unwrap();
    assert_eq!(
        project
            .prepare(Edit::AcceptOpticalReport {
                report: Arc::new(foreign),
                selection: None
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::InvalidProject
    );

    let mut foreign = (*report).clone();
    foreign.producer = "other-model".into();
    assert_eq!(
        project
            .prepare(Edit::AcceptOpticalReport {
                report: Arc::new(foreign),
                selection: None
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::InvalidProject
    );

    let mut interrupted = (*report).clone();
    interrupted.visits.clear();
    interrupted.alternatives.clear();
    interrupted.status = AppearanceStatus::Cancelled;
    interrupted.validate().unwrap();
    assert_eq!(
        project
            .prepare(Edit::AcceptOpticalReport {
                report: Arc::new(interrupted),
                selection: None
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::InvalidProject
    );
    assert_eq!(
        project
            .prepare(Edit::AcceptOpticalReport {
                report: report.clone(),
                selection: Some(OpticalSelection::Candidate { index: 1 })
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::InvalidProject
    );
    assert_eq!(
        project
            .appearance_study()
            .optical
            .as_ref()
            .unwrap()
            .report
            .as_ref()
            .unwrap()
            .as_ref(),
        report.as_ref()
    );
}

#[test]
fn optical_history_and_c1a_target_edits_preserve_independent_report_and_manufacturing() {
    let (mut project, _, report) = optical_project();
    let accepted = project.snapshot();
    let artifact = accepted.manufacturing_artifact.as_ref().unwrap();
    commit_appearance(
        &mut project,
        Edit::SetAppearanceTarget {
            target: crate::tests::appearance_target(),
            limits: SearchLimits::default(),
        },
    );
    assert!(Arc::ptr_eq(
        project
            .appearance_study()
            .optical
            .as_ref()
            .unwrap()
            .report
            .as_ref()
            .unwrap(),
        &report
    ));
    assert!(Arc::ptr_eq(
        project.snapshot().manufacturing_artifact.as_ref().unwrap(),
        artifact
    ));
    let mut request = optical_request();
    request.patch.width_mm = 4.0;
    commit_appearance(&mut project, Edit::SetOpticalStudy { request });
    assert!(
        project
            .appearance_study()
            .optical
            .as_ref()
            .unwrap()
            .report
            .is_none()
    );
    assert_eq!(
        project.appearance_study().target,
        Some(crate::tests::appearance_target())
    );
    assert!(Arc::ptr_eq(
        project.snapshot().manufacturing_artifact.as_ref().unwrap(),
        artifact
    ));
    project.undo().unwrap();
    assert!(Arc::ptr_eq(
        project
            .appearance_study()
            .optical
            .as_ref()
            .unwrap()
            .report
            .as_ref()
            .unwrap(),
        &report
    ));
    project.redo().unwrap();
    assert!(
        project
            .appearance_study()
            .optical
            .as_ref()
            .unwrap()
            .report
            .is_none()
    );
}

#[test]
fn optical_palette_replacement_clears_authority_and_drops_incompatible_request() {
    let (mut project, asset, _) = optical_project();
    commit_appearance(&mut project, Edit::ImportAppearancePalette { asset });
    let optical = project.appearance_study().optical.as_ref().unwrap();
    assert!(
        optical.report.is_none() && optical.report_record.is_none() && optical.selection.is_none()
    );
    commit_appearance(
        &mut project,
        Edit::ImportAppearancePalette {
            asset: crate::tests::palette_asset("only-red.json"),
        },
    );
    assert!(project.appearance_study().optical.is_none());
    project.undo().unwrap();
    assert!(project.appearance_study().optical.is_some());
    project.undo().unwrap();
    assert!(
        project
            .appearance_study()
            .optical
            .as_ref()
            .unwrap()
            .report
            .is_some()
    );
}

#[test]
fn optical_retention_charges_unique_requests_and_reports_across_live_history_and_prepared() {
    let (mut project, _, report) = optical_project();
    let a = project.snapshot();
    let mut b = (*a).clone();
    b.appearance_study = Arc::new((*a.appearance_study).clone());
    let one = appearance_bytes(std::iter::once(a.as_ref())).unwrap();
    let both = appearance_bytes([a.as_ref(), &b].into_iter()).unwrap();
    assert_eq!(both - one, b.appearance_study.allocated_bytes());
    let mut c = b.clone();
    let study = Arc::make_mut(&mut c.appearance_study);
    study.optical.as_mut().unwrap().report = Some(Arc::new((*report).clone()));
    assert_eq!(
        appearance_bytes([a.as_ref(), &c].into_iter()).unwrap() - one,
        c.appearance_study.allocated_bytes() + report.allocated_bytes()
    );
    let study = Arc::make_mut(&mut c.appearance_study);
    let separate_report = Arc::make_mut(study.optical.as_mut().unwrap().report.as_mut().unwrap());
    separate_report.request = Arc::new((*report.request).clone());
    let separate_request_bytes = separate_report.request.allocated_bytes();
    assert_eq!(
        appearance_bytes([a.as_ref(), &c].into_iter()).unwrap() - one,
        c.appearance_study.allocated_bytes() + report.allocated_bytes() + separate_request_bytes
    );

    let before = project.retained_appearance_bytes().unwrap();
    let prepared = project
        .prepare(Edit::SetAppearanceTarget {
            target: crate::tests::appearance_target(),
            limits: SearchLimits::default(),
        })
        .unwrap();
    let prepared_bytes = prepared.snapshot().appearance_study.allocated_bytes();
    assert_eq!(
        project.retained_appearance_bytes().unwrap(),
        before + prepared_bytes
    );
    project.undo().unwrap();
    assert_eq!(
        project.retained_appearance_bytes().unwrap(),
        before + prepared_bytes
    );
    assert_eq!(
        project.commit(prepared).unwrap_err().code,
        ProjectErrorCode::StaleRevision
    );
    assert_eq!(project.retained_appearance_bytes().unwrap(), before);
}

#[test]
fn optical_retained_capacity_cannot_hide_behind_small_encoded_request() {
    let (project, _, _) = optical_project();
    let mut request = optical_request();
    request.materials[0].reserve_exact(MAX_APPEARANCE_RETAINED_BYTES);
    request.validate().unwrap();
    assert_eq!(
        project
            .prepare(Edit::SetOpticalStudy { request })
            .unwrap_err()
            .code,
        ProjectErrorCode::ResourceLimit
    );
}

#[test]
fn optical_nullable_fields_are_required_not_compatibility_defaults() {
    let mut value = serde_json::to_value(AppearanceStudy::default()).unwrap();
    value.as_object_mut().unwrap().remove("optical");
    assert!(serde_json::from_value::<AppearanceStudy>(value).is_err());
    let mut value = serde_json::to_value(AppearanceStudy::default().info()).unwrap();
    value.as_object_mut().unwrap().remove("optical");
    assert!(serde_json::from_value::<AppearanceStudyInfo>(value).is_err());
}
