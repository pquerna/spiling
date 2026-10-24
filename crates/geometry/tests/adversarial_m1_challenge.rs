// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Empirical Challenger Suite for Milestone M1
//! Covers:
//! 1. StepEntityId edge cases, string conversions, large ints, serde, FaceId conversions
//! 2. Definition arena borrow methods: face_surface, edge_curve, edgeuse_uv_trim_curve
//!    with valid and non-existent IDs (out-of-bounds, u32::MAX) -> zero panic
//! 3. retained_query_proof_bytes monotonicity, determinism, non-zero bounds, resource limits
//! 4. Complete 14 adversarial mutated fixtures from fixtures/geometry/manifest.json
//! 5. Baseline accepted models: box-mm, box-inch (scaling), cylinder, through-hole, perforated-plate
//! 6. Cancellation and resource limit enforcement
//! 7. Thread transfer (Send) and upstream Sync analysis

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use spiling_contracts::geometry::{
    FaceId, GeometryErrorCode, MAX_SOURCE_BYTES, SourceUnit, StepEntityId,
};
use spiling_geometry::{
    from_step_bytes, from_step_bytes_with_cancel, from_step_file,
};

const MAX_QUERY_PROOF_BYTES: usize = 32 * 1024 * 1024;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/geometry")
}

fn load_fixture(name: &str) -> Vec<u8> {
    std::fs::read(fixtures_dir().join(name))
        .unwrap_or_else(|e| panic!("failed to read fixture {name}: {e}"))
}

// =========================================================================
// Category 1: StepEntityId Empirical Challenge
// =========================================================================

#[test]
fn challenge_step_entity_id_zero() {
    let id0 = StepEntityId(0);
    assert_eq!(id0.0, 0);
    assert_eq!(id0.get(), 0);
    assert_eq!(id0.to_string(), "step:0");
    assert_eq!(format!("{id0}"), "step:0");
    assert_eq!(u64::from(id0), 0);
    assert_eq!(StepEntityId::from(0u64), id0);

    // JSON serialization
    let json = serde_json::to_string(&id0).expect("serialize id 0");
    assert_eq!(json, "0");
    let back: StepEntityId = serde_json::from_str(&json).expect("deserialize id 0");
    assert_eq!(back, id0);

    // Conversion to FaceId: must be rejected because entity 0 is invalid in STEP
    assert!(
        FaceId::try_from(id0).is_err(),
        "FaceId::try_from must reject StepEntityId(0)"
    );
    assert!(
        FaceId::from_step_entity_id(id0).is_err(),
        "FaceId::from_step_entity_id must reject StepEntityId(0)"
    );

    // Note invariant observation: FaceId::parse("step:0") returns Err (canonical nonzero decimal rule)
    let parse_res = FaceId::parse("step:0");
    assert!(
        parse_res.is_err(),
        "FaceId::parse rejects step:0 per contract rule"
    );
}

#[test]
fn challenge_step_entity_id_extremes_and_overflow() {
    let test_ids = [
        1u64,
        42u64,
        1_000_000_000u64,
        9_007_199_254_740_991u64, // JS MAX_SAFE_INTEGER
        9_007_199_254_740_993u64, // JS MAX_SAFE_INTEGER + 2
        u32::MAX as u64,
        u64::MAX - 1,
        u64::MAX,
    ];

    for &val in &test_ids {
        let id = StepEntityId::new(val);
        assert_eq!(id.get(), val);
        assert_eq!(id.0, val);
        assert_eq!(u64::from(id), val);
        assert_eq!(StepEntityId::from(val), id);

        let expected_str = format!("step:{val}");
        assert_eq!(id.to_string(), expected_str);

        // JSON serde
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, val.to_string());
        let deser: StepEntityId = serde_json::from_str(&json).unwrap();
        assert_eq!(deser, id);

        // FaceId conversions
        let face_id: FaceId = id.try_into().unwrap();
        assert_eq!(face_id.as_str(), expected_str);
        assert_eq!(face_id.step_entity(), val);

        let back1 = StepEntityId::try_from(&face_id).unwrap();
        assert_eq!(back1, id);
        let back2 = StepEntityId::try_from(face_id.clone()).unwrap();
        assert_eq!(back2, id);

        // FaceId::parse should succeed for positive canonical decimal
        let parsed = FaceId::parse(&expected_str).unwrap();
        assert_eq!(parsed, face_id);
    }
}

#[test]
fn challenge_face_id_invalid_strings() {
    let invalid_strings = [
        "",
        "step",
        "step:",
        "face:128",
        "step:-1",
        "step:0",
        "step:00",
        "step:01",
        "step: 128",
        "step:128 ",
        "step:128\n",
        "step:abc",
        "step:128.5",
        "step:18446744073709551616", // u64::MAX + 1 overflow
        "step:999999999999999999999999999999",
    ];

    for s in invalid_strings {
        assert!(
            FaceId::parse(s).is_err(),
            "FaceId::parse should reject invalid string: '{s}'"
        );
    }
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
    let handle = std::thread::spawn(move || {
        assert_eq!(def.faces().len(), 6);
        def.bounds_mm()
    });
    let bounds = handle.join().expect("thread completed");
    assert_eq!(bounds.max[0], 20.0);
}

// =========================================================================
// Category 3: retained_query_proof_bytes Monotonicity & Invariants
// =========================================================================

#[test]
fn challenge_retained_query_proof_bytes_monotonicity_and_bounds() {
    let box_mm = from_step_bytes(&load_fixture("box-mm.step")).expect("box-mm");
    let box_inch = from_step_bytes(&load_fixture("box-inch.step")).expect("box-inch");
    let cylinder = from_step_bytes(&load_fixture("cylinder.step")).expect("cylinder");
    let through_hole = from_step_bytes(&load_fixture("through-hole.step")).expect("through-hole");
    let plate = from_step_bytes(&load_fixture("perforated-plate.step")).expect("plate");

    let b_box_mm = box_mm.retained_query_proof_bytes();
    let b_box_inch = box_inch.retained_query_proof_bytes();
    let b_cylinder = cylinder.retained_query_proof_bytes();
    let b_through_hole = through_hole.retained_query_proof_bytes();
    let b_plate = plate.retained_query_proof_bytes();

    // 1. Non-zero byte bound
    assert!(b_box_mm > 0, "box_mm retained bytes must be > 0");
    assert!(b_box_inch > 0, "box_inch retained bytes must be > 0");
    assert!(b_cylinder > 0, "cylinder retained bytes must be > 0");
    assert!(b_through_hole > 0, "through-hole retained bytes must be > 0");
    assert!(b_plate > 0, "plate retained bytes must be > 0");

    // 2. Determinism: repeated calculation gives identical bytes
    assert_eq!(box_mm.retained_query_proof_bytes(), b_box_mm);
    assert_eq!(plate.retained_query_proof_bytes(), b_plate);

    // 3. Topology scaling: box-mm (6 planar faces) vs through-hole (8 faces, internal void)
    assert!(
        b_through_hole > b_box_mm,
        "through-hole ({b_through_hole}) must retain more query bytes than simple box ({b_box_mm})"
    );

    // 4. Perforated plate (512 holes, 1000+ faces) must retain orders of magnitude more bytes
    assert!(
        b_plate > b_through_hole * 10,
        "perforated plate ({b_plate}) must retain significantly more bytes than through-hole ({b_through_hole})"
    );

    // 5. Invariant limit check: all standard models fit within MAX_QUERY_PROOF_BYTES (32MB)
    assert!(
        b_box_mm <= MAX_QUERY_PROOF_BYTES,
        "box_mm must fit within MAX_QUERY_PROOF_BYTES"
    );
    assert!(
        b_plate <= MAX_QUERY_PROOF_BYTES,
        "plate must fit within MAX_QUERY_PROOF_BYTES ({b_plate} vs {MAX_QUERY_PROOF_BYTES})"
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
                err_code_val,
                expected_err_str,
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
fn challenge_baseline_accepted_fixtures_geometry_and_scaling() {
    // 1. Box mm
    let box_mm = from_step_bytes(&load_fixture("box-mm.step")).expect("box-mm");
    assert_eq!(box_mm.faces().len(), 6);
    assert_eq!(box_mm.provenance().source_unit, SourceUnit::Millimetre);
    let b_mm = box_mm.bounds_mm();
    assert!((b_mm.min[0] - 0.0).abs() < 1e-5);
    assert!((b_mm.min[1] - 0.0).abs() < 1e-5);
    assert!((b_mm.min[2] - 0.0).abs() < 1e-5);
    assert!((b_mm.max[0] - 20.0).abs() < 1e-5);
    assert!((b_mm.max[1] - 10.0).abs() < 1e-5);
    assert!((b_mm.max[2] - 8.0).abs() < 1e-5);

    // 1b. Box inch (must evaluate to 20x10x8 mm physical dimensions)
    let box_in = from_step_bytes(&load_fixture("box-inch.step")).expect("box-inch");
    assert_eq!(box_in.faces().len(), 6);
    assert_eq!(box_in.provenance().source_unit, SourceUnit::Inch);
    let b_in = box_in.bounds_mm();
    assert!((b_in.min[0] - 0.0).abs() < 1e-5);
    assert!((b_in.min[1] - 0.0).abs() < 1e-5);
    assert!((b_in.min[2] - 0.0).abs() < 1e-5);
    assert!((b_in.max[0] - 20.0).abs() < 1e-5);
    assert!((b_in.max[1] - 10.0).abs() < 1e-5);
    assert!((b_in.max[2] - 8.0).abs() < 1e-5);

    // 2. Cylinder
    let cyl = from_step_bytes(&load_fixture("cylinder.step")).expect("cylinder");
    assert_eq!(cyl.faces().len(), 4, "cylinder has 2 semi-cylinders + 2 planar caps = 4 faces");
    let b_cyl = cyl.bounds_mm();
    assert!((b_cyl.min[0] - (-5.0)).abs() < 1e-5);
    assert!((b_cyl.min[1] - (-5.0)).abs() < 1e-5);
    assert!((b_cyl.min[2] - 0.0).abs() < 1e-5);
    assert!((b_cyl.max[0] - 5.0).abs() < 1e-5);
    assert!((b_cyl.max[1] - 5.0).abs() < 1e-5);
    assert!((b_cyl.max[2] - 8.0).abs() < 1e-5);

    // 3. Through-hole
    let th = from_step_bytes(&load_fixture("through-hole.step")).expect("through-hole");
    let b_th = th.bounds_mm();
    assert!((b_th.min[0] - 0.0).abs() < 1e-5);
    assert!((b_th.max[0] - 20.0).abs() < 1e-5);
    assert!((b_th.max[1] - 20.0).abs() < 1e-5);
    assert!((b_th.max[2] - 8.0).abs() < 1e-5);

    // 4. Perforated plate
    let plate = from_step_bytes(&load_fixture("perforated-plate.step")).expect("plate");
    let b_plate = plate.bounds_mm();
    assert!((b_plate.min[0] - 0.0).abs() < 1e-4);
    assert!((b_plate.max[0] - 1640.0).abs() < 1e-4);
    assert!((b_plate.max[1] - 840.0).abs() < 1e-4);
    assert!((b_plate.max[2] - 20.0).abs() < 1e-4);
}

#[test]
fn challenge_reproduce_box_inch_bounds_double_scaling_bug() {
    let box_inch = from_step_bytes(&load_fixture("box-inch.step")).expect("box-inch");
    let b_in = box_inch.bounds_mm();

    // Verify double-scaling fix:
    // manifest.json and RFC 07 specify expected bounds max: [20.0, 10.0, 8.0] mm.
    println!("Empirical box-inch bounds: min={:?}, max={:?}", b_in.min, b_in.max);
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
