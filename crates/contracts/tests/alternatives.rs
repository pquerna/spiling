// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use spiling_contracts::{geometry::*, manufacturing::*, native, project::*, rpc};
use std::sync::Arc;

fn intent() -> ManufacturingIntent {
    serde_json::from_str(include_str!(
        "../../../fixtures/manufacturing/complete-sparse.intent.json"
    ))
    .unwrap()
}

fn option(name: &str) -> ManufacturingOption {
    ManufacturingOption::new(
        name.into(),
        SourceHash::from_bytes(b"original captured source and setup"),
        Arc::new(intent()),
    )
    .unwrap()
}

#[test]
fn option_identity_binds_name_context_and_complete_intent() {
    let original = option("Reference");
    assert_eq!(original, option("Reference"));
    let mut changed = original.clone();
    changed.name = "Candidate".into();
    assert!(changed.validate().is_err());
    changed = original.clone();
    changed.context_hash = SourceHash::from_bytes(b"another source");
    assert!(changed.validate().is_err());
    changed = original.clone();
    Arc::make_mut(&mut changed.intent).recipe.seam_policy = SeamPolicy::Distributed;
    assert!(changed.validate().is_err());
    assert_ne!(option("Candidate").id, original.id);
}

#[test]
fn option_json_has_no_missing_field_or_unknown_field_defaults() {
    let original = serde_json::to_value(option("Reference")).unwrap();
    for field in ["id", "name", "context_hash", "intent"] {
        let mut missing = original.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<ManufacturingOption>(missing).is_err());
    }
    let mut extra = original;
    extra["active"] = true.into();
    assert!(serde_json::from_value::<ManufacturingOption>(extra).is_err());
}

#[test]
fn option_collection_rejects_duplicates_count_and_invalid_names() {
    let original = option("Reference");
    assert!(validate_manufacturing_options(&[original.clone(), original]).is_err());
    let options = (0..=MAX_MANUFACTURING_OPTIONS)
        .map(|index| option(&format!("Option {index}")))
        .collect::<Vec<_>>();
    assert!(validate_manufacturing_options(&options).is_err());
    for name in ["", " ", " leading", "trailing ", "line\nbreak"] {
        assert!(
            ManufacturingOption::new(
                name.into(),
                SourceHash::from_bytes(b"context"),
                Arc::new(intent()),
            )
            .is_err()
        );
    }
    assert!(
        ManufacturingOption::new(
            "x".repeat(MAX_MANUFACTURING_OPTION_NAME_BYTES + 1),
            SourceHash::from_bytes(b"context"),
            Arc::new(intent()),
        )
        .is_err()
    );
}

#[test]
fn options_use_typed_revision_bound_commands_not_activation_on_read() {
    let session_id = SessionId::parse("550e8400-e29b-41d4-a716-446655440000").unwrap();
    let option = option("Reference");
    let commands = [
        ProjectCommand::RetainManufacturingOption {
            layout_id: PrintLayoutId::parse("1ccf1bc1-590b-4fe2-88b5-7e8de3da1fcb").unwrap(),
            session_id: session_id.clone(),
            base_revision: ProjectRevision(12),
            option: option.clone(),
        },
        ProjectCommand::GetManufacturingOption {
            layout_id: PrintLayoutId::parse("1ccf1bc1-590b-4fe2-88b5-7e8de3da1fcb").unwrap(),
            session_id: session_id.clone(),
            base_revision: ProjectRevision(12),
            id: option.id.clone(),
        },
        ProjectCommand::RemoveManufacturingOption {
            layout_id: PrintLayoutId::parse("1ccf1bc1-590b-4fe2-88b5-7e8de3da1fcb").unwrap(),
            session_id: session_id.clone(),
            base_revision: ProjectRevision(12),
            id: option.id.clone(),
        },
        ProjectCommand::ApplyManufacturingOption {
            layout_id: PrintLayoutId::parse("1ccf1bc1-590b-4fe2-88b5-7e8de3da1fcb").unwrap(),
            session_id,
            base_revision: ProjectRevision(12),
            id: option.id,
        },
    ];
    for command in commands {
        let native::ProjectCall::Execute(wire) = native::project_call(command.clone(), "").unwrap()
        else {
            panic!("expected typed project execution");
        };
        assert_eq!(ProjectCommand::try_from(*wire).unwrap(), command);
    }
}

#[test]
fn option_native_roundtrip_rejects_rebound_content() {
    let option = option("Reference");
    let mut wire: rpc::ManufacturingOption = option.clone().try_into().unwrap();
    assert_eq!(ManufacturingOption::try_from(wire.clone()).unwrap(), option);
    wire.name = "Rebound".into();
    assert!(ManufacturingOption::try_from(wire).is_err());
}

#[test]
fn comparison_diff_reports_exact_changes_without_conferring_authority() {
    let before = intent();
    let mut after = before.clone();
    after.recipe.seam_policy = SeamPolicy::Distributed;
    let (changes, complete) = manufacturing_input_changes(Some(&before), &after).unwrap();
    assert!(complete);
    assert!(!changes.is_empty());
    assert!(changes.iter().all(|change| {
        change.group == ManufacturingInputGroup::Recipe
            && change.path.contains("seam_policy")
            && change.before != change.after
    }));
    let (unchanged, complete) = manufacturing_input_changes(Some(&after), &after).unwrap();
    assert!(complete);
    assert!(unchanged.is_empty());
}

#[test]
fn evaluation_native_request_requires_original_context_and_complete_intent() {
    let command = ManufacturingCommand::EvaluateAlternative {
        layout_id: PrintLayoutId::parse("1ccf1bc1-590b-4fe2-88b5-7e8de3da1fcb").unwrap(),
        session_id: SessionId::parse("550e8400-e29b-41d4-a716-446655440000").unwrap(),
        base_revision: ProjectRevision(12),
        expected_context_hash: SourceHash::from_bytes(b"original context"),
        intent: Arc::new(intent()),
    };
    let wire: rpc::EvaluateManufacturingAlternativeRequest = command.clone().try_into().unwrap();
    assert_eq!(
        ManufacturingCommand::try_from(wire.clone()).unwrap(),
        command
    );
    let mut missing = wire.clone();
    missing.expected_context_hash.clear();
    assert!(ManufacturingCommand::try_from(missing).is_err());
    let mut malformed = wire.clone();
    malformed.expected_context_hash.make_ascii_uppercase();
    assert!(ManufacturingCommand::try_from(malformed).is_err());
    let mut missing = wire;
    missing.construction = None;
    assert!(ManufacturingCommand::try_from(missing).is_err());
}

#[test]
fn retained_status_summaries_do_not_duplicate_complete_profiles() {
    use prost::Message;
    let mut total = 0;
    for index in 0..MAX_MANUFACTURING_OPTIONS {
        let original = option(&format!("Option {index}"));
        let summary = original.info();
        let wire: rpc::ManufacturingOptionInfo = summary.clone().try_into().unwrap();
        total += wire.encoded_len();
        assert_eq!(ManufacturingOptionInfo::try_from(wire).unwrap(), summary);
        assert!(
            serde_json::to_value(summary)
                .unwrap()
                .get("intent")
                .is_none()
        );
    }
    assert!(total < spiling_contracts::MAX_CONTROL_BYTES as usize / 8);
}
