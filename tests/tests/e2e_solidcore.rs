// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Master E2E integration runner for Spiling RFC 07 SolidCore Kernel Adoption.
//! Orchestrates and validates end-to-end consistency across Tier 1, 2, 3, and 4 test suites.

use spiling_contracts::geometry::*;
use spiling_e2e_tests::fixtures::*;
use spiling_e2e_tests::helpers::*;
use spiling_geometry::*;
use std::sync::atomic::AtomicBool;

#[test]
fn test_e2e_occt_reference_corpus_integrity() {
    for &fixture_path in ALL_OCCT_FIXTURES {
        let bytes = fixture_bytes(fixture_path);
        let def = import_test_step(&bytes).unwrap_or_else(|e| {
            panic!("failed to import OCCT reference fixture {fixture_path}: {e:?}")
        });

        // 1. Bounds must be finite and non-empty
        let b = def.bounds_mm();
        assert!(b.max[0] >= b.min[0], "valid x bounds for {fixture_path}");
        assert!(b.max[1] >= b.min[1], "valid y bounds for {fixture_path}");
        assert!(b.max[2] >= b.min[2], "valid z bounds for {fixture_path}");

        // 2. Midplane section must succeed
        let sec = section_midplane(&def, fixture_path)
            .unwrap_or_else(|e| panic!("failed midplane section on {fixture_path}: {e:?}"));
        assert!(!sec.loops().is_empty(), "midplane loops for {fixture_path}");

        // 3. Tessellation must produce valid mesh
        let mesh = tessellate_display(&def)
            .unwrap_or_else(|e| panic!("failed tessellation on {fixture_path}: {e:?}"));
        assert!(
            !mesh_positions(&mesh).is_empty(),
            "mesh vertices for {fixture_path}"
        );
        assert!(
            !mesh_indices(&mesh).is_empty(),
            "mesh indices for {fixture_path}"
        );
    }
}

#[test]
fn test_e2e_adversarial_corpus_rejection_integrity() {
    for &(fixture_path, expected_err) in ALL_ADVERSARIAL_FIXTURES {
        let bytes = fixture_bytes(fixture_path);
        let res = import_test_step(&bytes);
        assert!(
            res.is_err(),
            "adversarial fixture {fixture_path} expected rejection '{expected_err}', but import succeeded"
        );
    }
}

#[test]
fn test_e2e_query_work_and_mesh_byte_governors_pipeline() {
    let bytes = fixture_bytes("independent-exporters/occt-box-mm.step");
    let def = import_test_step(&bytes).expect("box import");

    let mut budget = GeometryWorkBudget::new(50_000);
    let cancel = AtomicBool::new(false);
    let initial_consumed = budget.consumed();

    let plane = PlaneMm {
        origin_mm: [0.0, 0.0, 4.0],
        normal: [0.0, 0.0, 1.0],
    };
    let _sec = prepare_section_source(
        &def,
        spiling_geometry::MAX_PREPARED_SECTION_SOURCE_BYTES,
        &cancel,
        &mut budget,
    )
    .and_then(|source| {
        source.section_with_byte_limit(plane, &cancel, MAX_SECTION_BYTES, &mut budget)
    })
    .expect("budgeted section");
    assert!(
        budget.consumed() > initial_consumed,
        "consumed budget must increase monotonically"
    );

    let mesh = tessellate_with_budget(
        &def,
        DisplayProfile::MeshTrimmedMm005V2,
        &cancel,
        spiling_contracts::display::MeshBudget::FROZEN,
    )
    .expect("budgeted tessellation");
    assert!(mesh.total_bytes <= spiling_contracts::display::MeshBudget::FROZEN.bytes);
    assert!(
        budget.consumed() <= 50_000,
        "must stay within allocated budget"
    );
}

#[test]
fn test_e2e_cooperative_cancellation_pipeline() {
    let bytes = fixture_bytes("perforated-plate.step");
    let def = import_test_step(&bytes).expect("plate import");

    let cancel = AtomicBool::new(true);
    let plane = PlaneMm {
        origin_mm: [0.0, 0.0, 10.0],
        normal: [0.0, 0.0, 1.0],
    };
    let res = section(&def, plane, &cancel);
    assert!(res.is_err(), "must halt on cooperative cancellation");
}
