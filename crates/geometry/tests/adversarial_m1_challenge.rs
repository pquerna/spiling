// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Native arena handles, rejected inputs, units and cancellation.
//! Source-ID wire/range behavior belongs to contracts; actual byte-limit
//! refusal lives in adversarial_m1. Oversized source/material coverage belongs
//! to the explicit geometry_stress target rather than memory-size ratios.

mod common;

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use spiling_contracts::geometry::{GeometryErrorCode, MAX_SOURCE_BYTES, SourceUnit};
use spiling_geometry::{from_step_bytes, from_step_bytes_with_cancel, from_step_file};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/geometry")
}

fn load_fixture(name: &str) -> Vec<u8> {
    std::fs::read(fixtures_dir().join(name))
        .unwrap_or_else(|e| panic!("failed to read fixture {name}: {e}"))
}

// =========================================================================
// Category 2: Definition Arena Borrow Methods Graceful Failure & Zero-Mutex
// =========================================================================

#[test]
fn challenge_arena_borrows_graceful_none_on_invalid_ids() {
    let def = from_step_bytes(&load_fixture("box-mm.step")).expect("load box-mm");
    let brep = def.brep();

    let num_faces = brep.faces().len() as u32;
    let num_edges = brep.edges().len() as u32;
    let num_edgeuses = brep.edgeuses().len() as u32;

    assert!(num_faces > 0, "box must have faces");
    assert!(num_edges > 0, "box must have edges");
    assert!(num_edgeuses > 0, "box must have edgeuses");

    // 1. Valid queries must succeed
    for i in 0..num_faces {
        let face_id = solidcore::topology::FaceId::from_index(i);
        let s = def.face_surface(face_id);
        assert!(s.is_some(), "valid face {i} should yield surface");
    }

    for i in 0..num_edges {
        let edge_id = solidcore::topology::EdgeId::from_index(i);
        let c = def.edge_curve(edge_id);
        assert!(c.is_some(), "valid edge {i} should yield curve");
    }

    // 2. Query non-existent FaceId
    let bad_face_indices = [
        num_faces,
        num_faces + 1,
        1_000,
        100_000,
        u32::MAX - 1,
        u32::MAX,
    ];
    for &bad_idx in &bad_face_indices {
        let bad_face_id = solidcore::topology::FaceId::from_index(bad_idx);
        let surface = def.face_surface(bad_face_id);
        assert!(
            surface.is_none(),
            "querying out-of-bounds FaceId {bad_idx} must return None gracefully without panicking"
        );
    }

    // 3. Query non-existent EdgeId
    let bad_edge_indices = [
        num_edges,
        num_edges + 1,
        1_000,
        100_000,
        u32::MAX - 1,
        u32::MAX,
    ];
    for &bad_idx in &bad_edge_indices {
        let bad_edge_id = solidcore::topology::EdgeId::from_index(bad_idx);
        let curve = def.edge_curve(bad_edge_id);
        assert!(
            curve.is_none(),
            "querying out-of-bounds EdgeId {bad_idx} must return None gracefully without panicking"
        );
    }

    // 4. Query non-existent EdgeuseId
    let bad_edgeuse_indices = [
        num_edgeuses,
        num_edgeuses + 1,
        1_000,
        100_000,
        u32::MAX - 1,
        u32::MAX,
    ];
    for &bad_idx in &bad_edgeuse_indices {
        let bad_edgeuse_id = solidcore::topology::EdgeuseId::from_index(bad_idx);
        let uv_curve = def.edgeuse_uv_trim_curve(bad_edgeuse_id);
        assert!(
            uv_curve.is_none(),
            "querying out-of-bounds EdgeuseId {bad_idx} must return None gracefully without panicking"
        );
    }
}

#[test]
fn challenge_arena_borrows_zero_mutex_simultaneous_references() {
    let def = from_step_bytes(&load_fixture("cylinder.step")).expect("load cylinder");

    let num_faces = def.brep().faces().len() as u32;
    let num_edges = def.brep().edges().len() as u32;

    // Simultaneous borrows across multiple arena elements: zero mutex contention
    let s0 = def.face_surface(solidcore::topology::FaceId::from_index(0 % num_faces));
    let s1 = def.face_surface(solidcore::topology::FaceId::from_index(1 % num_faces));
    let c0 = def.edge_curve(solidcore::topology::EdgeId::from_index(0 % num_edges));
    let c1 = def.edge_curve(solidcore::topology::EdgeId::from_index(1 % num_edges));

    assert!(s0.is_some());
    assert!(s1.is_some());
    assert!(c0.is_some());
    assert!(c1.is_some());
}

#[test]
fn challenge_definition_send_across_threads() {
    // Verify Definition implements Send so it can be moved between threads/actors
    let def = from_step_bytes(&load_fixture("box-mm.step")).expect("box-mm");
    let expected = def.bounds_mm();
    let handle = std::thread::spawn(move || {
        assert_eq!(def.faces().len(), 6);
        def.bounds_mm()
    });
    let bounds = handle.join().expect("thread completed");
    assert_eq!(
        bounds, expected,
        "thread transfer preserves the import enclosure"
    );
}

// =========================================================================
// Category 4: STEP Import Pipeline & Manifest Adversarial Mutations
// =========================================================================

#[test]
fn challenge_manifest_all_14_adversaries() {
    let manifest_bytes = load_fixture("manifest.json");
    let manifest: serde_json::Value =
        serde_json::from_slice(&manifest_bytes).expect("parse manifest.json");

    let fixtures = manifest["fixtures"]
        .as_array()
        .expect("fixtures array in manifest");

    let mut adversary_count = 0;
    for entry in fixtures {
        let path = entry["path"].as_str().expect("fixture path");
        let source_bytes = load_fixture(path);

        if let Some(expected_err_str) = entry["expected_error"].as_str() {
            adversary_count += 1;
            let result = from_step_bytes(&source_bytes);
            assert!(
                result.is_err(),
                "adversary fixture '{path}' MUST be rejected, but import returned Ok"
            );
            let err = result.unwrap_err();
            let err_code_val = serde_json::to_value(err.code).unwrap();
            assert_eq!(
                err_code_val, expected_err_str,
                "fixture '{path}' expected error '{expected_err_str}' but got '{err_code_val}' (msg: {})",
                err.message
            );
        } else if path == "unsupported-nurbs.step" {
            // Special mutation: replace_one_plane_with_original_nurbs_carrier -> expected accepted!
            let result = from_step_bytes(&source_bytes);
            assert!(
                result.is_ok(),
                "fixture '{path}' has expected 'accepted', must succeed"
            );
        }
    }

    assert_eq!(
        adversary_count, 13,
        "manifest should contain 13 expected_error adversaries + 1 accepted mutation (unsupported-nurbs.step)"
    );
}

#[test]
fn challenge_reproduce_box_inch_bounds_double_scaling_bug() {
    let box_inch = from_step_bytes(&load_fixture("box-inch.step")).expect("box-inch");
    let b_in = common::physical_bounds(&box_inch, 1e-4).expect("inch physical extents");

    // Verify double-scaling fix:
    // manifest.json and RFC 07 specify expected bounds max: [20.0, 10.0, 8.0] mm.
    println!(
        "Empirical box-inch bounds: min={:?}, max={:?}",
        b_in.min, b_in.max
    );
    assert_eq!(box_inch.provenance().source_unit, SourceUnit::Inch);
    assert!(
        (b_in.max[0] - 20.0).abs() < 1e-4,
        "box-inch physical max x must be 20.0 mm, got {}",
        b_in.max[0]
    );
    assert!(
        (b_in.max[1] - 10.0).abs() < 1e-4,
        "box-inch physical max y must be 10.0 mm, got {}",
        b_in.max[1]
    );
    assert!(
        (b_in.max[2] - 8.0).abs() < 1e-4,
        "box-inch physical max z must be 8.0 mm, got {}",
        b_in.max[2]
    );
}

// =========================================================================
// Category 5: Cancellation & Resource Limit Stress Tests
// =========================================================================

#[test]
fn challenge_cancellation_pre_aborts_cleanly() {
    let bytes = load_fixture("box-mm.step");
    let cancel = AtomicBool::new(true); // Pre-cancelled
    let res = from_step_bytes_with_cancel(&bytes, &cancel);
    assert!(res.is_err(), "must abort when cancelled");
    let err = res.unwrap_err();
    assert_eq!(
        err.code,
        GeometryErrorCode::Cancelled,
        "must report Cancelled"
    );
}

#[test]
fn challenge_max_source_bytes_exceeded() {
    let huge = vec![b' '; MAX_SOURCE_BYTES as usize + 1];
    let res = from_step_bytes(&huge);
    assert!(res.is_err(), "must reject oversized source");
    assert_eq!(
        res.unwrap_err().code,
        GeometryErrorCode::ResourceLimit,
        "must report ResourceLimit"
    );
}

#[test]
fn challenge_from_step_file_nonexistent() {
    let res = from_step_file(fixtures_dir().join("does-not-exist.step"));
    assert!(res.is_err());
    assert_eq!(
        res.unwrap_err().code,
        GeometryErrorCode::SourceIo,
        "must report SourceIo on missing file"
    );
}
