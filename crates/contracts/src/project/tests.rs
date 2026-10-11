// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use sha2::{Digest, Sha256};

fn manifest() -> ProjectManifest {
    let digest: [u8; 32] = Sha256::digest(b"original immutable source").into();
    let definition_id = DefinitionId::from_source_sha256(&digest);
    ProjectManifest {
        format_version: PROJECT_FORMAT_VERSION,
        project_id: ProjectId::new(),
        revision: ProjectRevision(7),
        units: ProjectUnits::Millimetres,
        frame: ProjectFrame::RightHanded,
        next_occurrence: 2,
        definitions: vec![StoredDefinition {
            definition_id: definition_id.clone(),
            provenance: SourceProvenance {
                source_hash: SourceHash::from_digest(&digest),
                source_name: "original.step".into(),
                source_unit: SourceUnit::Millimetre,
                uncertainty_mm: Some(1e-6),
            },
        }],
        occurrences: vec![OccurrenceRecord {
            occurrence_id: OccurrenceId::new(1).unwrap(),
            definition_id,
            pose: RigidPoseMm::IDENTITY,
        }],
        manufacturing_intent: None,
        manufacturing_artifact: None,
    }
}

#[test]
fn manifest_rejects_inconsistent_sources_references_and_allocator_rewind() {
    let valid = manifest();
    valid.validate().unwrap();
    let mut changed = valid.clone();
    changed.definitions[0].provenance.source_hash = SourceHash::from_bytes(b"different source");
    assert_eq!(
        changed.validate().unwrap_err().code,
        ProjectErrorCode::InvalidProject
    );
    changed = valid.clone();
    changed.definitions.push(changed.definitions[0].clone());
    assert_eq!(
        changed.validate().unwrap_err().code,
        ProjectErrorCode::InvalidProject
    );
    changed = valid.clone();
    changed.occurrences.push(changed.occurrences[0].clone());
    assert_eq!(
        changed.validate().unwrap_err().code,
        ProjectErrorCode::InvalidProject
    );
    changed = valid.clone();
    changed.next_occurrence = 1;
    assert_eq!(
        changed.validate().unwrap_err().code,
        ProjectErrorCode::InvalidProject
    );
    changed = valid.clone();
    changed.occurrences.clear();
    assert_eq!(
        changed.validate().unwrap_err().code,
        ProjectErrorCode::InvalidProject
    );
    changed = valid;
    changed.definitions.clear();
    assert_eq!(
        changed.validate().unwrap_err().code,
        ProjectErrorCode::InvalidProject
    );
}

#[test]
fn manifest_does_not_admit_runtime_handles_or_incompatible_coordinate_contracts() {
    let valid = serde_json::to_value(manifest()).unwrap();
    for (field, value) in [
        ("session_id", serde_json::json!(SessionId::new())),
        ("units", serde_json::json!("inch")),
        ("frame", serde_json::json!("left_handed")),
    ] {
        let mut changed = valid.clone();
        changed[field] = value;
        assert!(serde_json::from_value::<ProjectManifest>(changed).is_err());
    }
    let mut changed = manifest();
    changed.format_version = PROJECT_FORMAT_VERSION + 1;
    assert_eq!(
        changed.validate().unwrap_err().code,
        ProjectErrorCode::UnsupportedFormat
    );
}

#[test]
fn manifest_requires_explicit_current_format_manufacturing_fields() {
    let valid = serde_json::to_value(manifest()).unwrap();
    for field in ["manufacturing_intent", "manufacturing_artifact"] {
        let mut changed = valid.clone();
        changed.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<ProjectManifest>(changed).is_err());
    }
    let mut old = manifest();
    old.format_version = 1;
    assert_eq!(
        old.validate().unwrap_err().code,
        ProjectErrorCode::UnsupportedFormat
    );
}

#[test]
fn persistent_revision_exhaustion_never_wraps() {
    assert_eq!(
        ProjectRevision(u32::MAX - 1).next().unwrap(),
        ProjectRevision(u32::MAX)
    );
    assert_eq!(
        ProjectRevision(u32::MAX).next().unwrap_err().code,
        ProjectErrorCode::ResourceLimit
    );
}

#[test]
fn project_errors_preserve_utf8_and_reject_unbounded_wire_messages() {
    let text = "界".repeat(MAX_PROJECT_ERROR_BYTES);
    let bounded = ProjectError::new(ProjectErrorCode::CorruptAsset, &text);
    assert!(bounded.message.len() <= MAX_PROJECT_ERROR_BYTES);
    assert!(text.starts_with(&bounded.message));
    assert!(
        serde_json::from_value::<ProjectError>(
            serde_json::json!({"code":"corrupt_asset","message":text})
        )
        .is_err()
    );
}
