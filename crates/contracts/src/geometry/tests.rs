// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;

#[test]
fn identities_are_canonical_and_content_scoped() {
    let digest: [u8; 32] = Sha256::digest(b"abc").into();
    assert_eq!(
        SourceHash::from_bytes(b"abc").as_str(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        DefinitionId::from_source_sha256(&digest).as_str(),
        "b30b34eba89b65be95070ca70366465f6dadd3fee7a7c51b2f28e72d146e6133"
    );
    assert_ne!(
        DefinitionId::from_source_sha256(&digest).as_str(),
        SourceHash::from_digest(&digest).as_str()
    );
    for text in ["", "a", &"A".repeat(64), &"g".repeat(64)] {
        assert!(DefinitionId::parse(text).is_err());
    }
    let face = FaceId::from_step_entity(9_007_199_254_740_993).unwrap();
    assert_eq!(
        serde_json::to_string(&face).unwrap(),
        "\"step:9007199254740993\""
    );
    assert_eq!(face.step_entity(), 9_007_199_254_740_993);
    for text in [
        "step:0",
        "step:01",
        "step:+1",
        "step:18446744073709551616",
        "1",
        "step:1.0",
    ] {
        assert!(FaceId::parse(text).is_err());
    }
    assert!(SessionId::parse("00000000-0000-0000-0000-000000000000").is_err());
    assert!(SessionId::parse("550E8400-E29B-41D4-A716-446655440000").is_err());
    let session = SessionId::new();
    assert_eq!(SessionId::parse(session.as_str()).unwrap(), session);
    assert_ne!(session, SessionId::new());
    assert!(serde_json::from_str::<ArtifactId>("0").is_err());
    assert!(OccurrenceId::new(0).is_err());
    assert_eq!(
        ArtifactId::new(u32::MAX).unwrap().next().unwrap_err().code,
        GeometryErrorCode::ResourceLimit
    );
    assert_eq!(
        SceneRevision(u32::MAX).next().unwrap_err().code,
        GeometryErrorCode::ResourceLimit
    );
    assert_eq!(SceneRevision::ZERO.next().unwrap(), SceneRevision(1));
}

#[test]
fn finite_bounds_pose_and_provenance_are_checked_at_json_boundary() {
    assert!(AabbMm::new([f64::NAN, 0.0, 0.0], [1.0; 3]).is_err());
    assert!(AabbMm::new([2.0; 3], [1.0; 3]).is_err());
    assert!(serde_json::from_str::<AabbMm>(r#"{"min":[2,0,0],"max":[1,1,1]}"#).is_err());
    assert!(
        serde_json::from_str::<RigidPoseMm>(
            r#"{"translation_mm":[0,0,0],"rotation_xyzw":[0,0,0,0]}"#
        )
        .is_err()
    );
    assert!(
        serde_json::from_str::<RigidPoseMm>(
            r#"{"translation_mm":[0,0,0],"rotation_xyzw":[0,0,0,1],"scale":2}"#
        )
        .is_err()
    );
    let mut pose = RigidPoseMm::IDENTITY;
    pose.rotation_xyzw[3] += POSE_NORM_TOLERANCE * 0.5;
    assert!(pose.validate().is_ok());
    pose.rotation_xyzw[3] += POSE_NORM_TOLERANCE * 2.0;
    assert_eq!(
        pose.validate().unwrap_err().code,
        GeometryErrorCode::InvalidPose
    );
    pose = RigidPoseMm::IDENTITY;
    pose.translation_mm[0] = f64::INFINITY;
    assert!(pose.validate().is_err());
    let mut provenance = SourceProvenance {
        source_hash: SourceHash::from_bytes(b"part"),
        source_name: "part.step".into(),
        source_unit: SourceUnit::Inch,
        uncertainty_mm: Some(0.01),
    };
    assert_eq!(SourceUnit::Inch.scale_to_mm(), 25.4);
    assert_eq!(SourceUnit::Metre.scale_to_mm(), 1000.0);
    assert!(provenance.validate().is_ok());
    provenance.uncertainty_mm = Some(-1.0);
    assert!(
        serde_json::from_value::<SourceProvenance>(serde_json::to_value(&provenance).unwrap())
            .is_err()
    );
    let text = "é".repeat(800);
    let error = GeometryError::new(GeometryErrorCode::KernelFailure, &text);
    assert_eq!(error.message.len(), MAX_ERROR_MESSAGE_BYTES as usize);
    assert!(
        serde_json::from_value::<GeometryError>(
            serde_json::json!({"code":"kernel_failure","message":text})
        )
        .is_err()
    );
}

#[test]
fn planes_normalize_and_frames_have_deterministic_axis_ties() {
    let plane = PlaneMm {
        origin_mm: [1.0, 2.0, 3.0],
        normal: [0.0, 0.0, 10.0],
    };
    assert_eq!(plane.normalized().unwrap().normal, [0.0, 0.0, 1.0]);
    let frame = PlaneFrameMm::from_plane(plane).unwrap();
    assert_eq!(frame.x_axis, [0.0, -1.0, 0.0]);
    assert_eq!(frame.y_axis, [1.0, 0.0, 0.0]);
    assert_eq!(frame.origin_mm, plane.origin_mm);
    assert!(
        PlaneMm {
            origin_mm: [0.0; 3],
            normal: [0.0; 3]
        }
        .normalized()
        .is_err()
    );
    assert!(
        PlaneMm {
            origin_mm: [0.0; 3],
            normal: [f64::INFINITY, 0.0, 0.0]
        }
        .normalized()
        .is_err()
    );
    for magnitude in [f64::MAX, f64::from_bits(1)] {
        let normalized = PlaneMm {
            origin_mm: [0.0; 3],
            normal: [magnitude; 3],
        }
        .normalized()
        .unwrap();
        for component in normalized.normal {
            assert!((component - 1.0 / 3.0_f64.sqrt()).abs() < 1e-15);
        }
        assert!((norm3(normalized.normal) - 1.0).abs() < POSE_NORM_TOLERANCE);
    }
}

#[test]
fn face_refs_reject_every_stale_or_cross_identity() {
    let session = SessionId::new();
    let definition = DefinitionId::from_source_sha256(&[1; 32]);
    let bounds = AabbMm::new([0.0; 3], [1.0; 3]).unwrap();
    let scene = SceneSummary {
        session_id: session.clone(),
        revision: SceneRevision(1),
        definition_count: 1,
        occurrence_count: 1,
        bounds_mm: Some(bounds),
        unique_mesh_bytes: 152,
    };
    let record = DefinitionRecord {
        definition_id: definition.clone(),
        provenance: SourceProvenance {
            source_hash: SourceHash::from_bytes(b"part"),
            source_name: "part".into(),
            source_unit: SourceUnit::Millimetre,
            uncertainty_mm: None,
        },
        face_count: 1,
        bounds_mm: bounds,
        mesh_artifact_id: ArtifactId::new(1).unwrap(),
    };
    let occurrence = OccurrenceRecord {
        occurrence_id: OccurrenceId::new(1).unwrap(),
        definition_id: definition.clone(),
        pose: RigidPoseMm::IDENTITY,
    };
    let faces = [FaceIndexRow {
        ordinal: 0,
        face_id: FaceId::from_step_entity(123).unwrap(),
    }];
    let reference = FaceRef {
        session_id: session,
        scene_revision: scene.revision,
        occurrence_id: occurrence.occurrence_id,
        definition_id: definition,
        face_id: faces[0].face_id.clone(),
    };
    reference
        .validate_current(&scene, &occurrence, &record, &faces)
        .unwrap();
    let mut stale = reference.clone();
    stale.session_id = SessionId::new();
    assert!(
        stale
            .validate_current(&scene, &occurrence, &record, &faces)
            .is_err()
    );
    stale = reference.clone();
    stale.scene_revision = SceneRevision::ZERO;
    assert!(
        stale
            .validate_current(&scene, &occurrence, &record, &faces)
            .is_err()
    );
    stale = reference.clone();
    stale.occurrence_id = OccurrenceId::new(2).unwrap();
    assert!(
        stale
            .validate_current(&scene, &occurrence, &record, &faces)
            .is_err()
    );
    stale = reference.clone();
    stale.definition_id = DefinitionId::from_source_sha256(&[2; 32]);
    assert!(
        stale
            .validate_current(&scene, &occurrence, &record, &faces)
            .is_err()
    );
    stale = reference;
    stale.face_id = FaceId::from_step_entity(124).unwrap();
    assert!(
        stale
            .validate_current(&scene, &occurrence, &record, &faces)
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn native_paths_preserve_non_utf8_and_reject_oversized_or_wrong_host() {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let bytes = b"/tmp/part-\xff.step";
    let encoded = NativePath::from_os_str(std::ffi::OsStr::from_bytes(bytes)).unwrap();
    assert_eq!(encoded.to_os_string().unwrap().into_vec(), bytes);
    for hex in ["00", "0", "zz", "ff00ff", ""] {
        assert!(
            NativePath::UnixBytes { hex: hex.into() }
                .validate()
                .is_err()
        );
    }
    assert!(
        NativePath::WindowsWide { units: vec![65] }
            .validate()
            .is_err()
    );
    assert_eq!(
        NativePath::UnixBytes {
            hex: "61".repeat(MAX_NATIVE_PATH_UNITS as usize + 1)
        }
        .validate()
        .unwrap_err()
        .code,
        GeometryErrorCode::ResourceLimit
    );
    assert!(
        NativePath::UnixBytes {
            hex: "61".repeat(MAX_NATIVE_PATH_UNITS as usize)
        }
        .validate()
        .is_ok()
    );
}
