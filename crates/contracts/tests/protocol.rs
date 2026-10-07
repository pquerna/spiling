// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use spiling_contracts::*;

#[test]
fn triangle_matches_cross_language_golden() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/protocol/triangle.json")).unwrap();
    assert_eq!(fixture["encoding"], "hex");
    let hex = fixture["data"].as_str().unwrap();
    let decoded: Vec<_> = (0..hex.len())
        .step_by(2)
        .map(|offset| u8::from_str_radix(&hex[offset..offset + 2], 16).unwrap())
        .collect();
    assert_eq!(decoded, synthetic_triangle());
    assert_eq!(decoded.len(), 64);
}

#[test]
fn shell_views_reject_inconsistent_terminal_snapshots_and_preserve_integer_widths() {
    use google::longrunning::{Operation, operation::Result as Outcome};
    use prost::Message;
    use rpc::{DiagnosticMetadata, DiagnosticResult, DiagnosticState};
    let meta = DiagnosticMetadata {
        state: DiagnosticState::Running as i32,
        state_version: u64::MAX,
        phase: "generating".into(),
        total_units: 1,
        ..Default::default()
    };
    let mut op = Operation {
        name: "diagnostics/test/operations/example".into(),
        metadata: Some(prost_types::Any {
            type_url: METADATA_TYPE.into(),
            value: meta.encode_to_vec(),
        }),
        done: false,
        result: None,
    };
    let view = operation_view(&op).unwrap();
    assert_eq!(view.state_version, u64::MAX.to_string());
    op.done = true;
    assert!(operation_view(&op).is_err());
    op.result = Some(Outcome::Response(prost_types::Any {
        type_url: RESULT_TYPE.into(),
        value: DiagnosticResult::default().encode_to_vec(),
    }));
    assert!(
        operation_view(&op).is_err(),
        "running metadata cannot report terminal success"
    );
    let mut finished = meta;
    finished.state = DiagnosticState::Succeeded as i32;
    finished.completed_units = 1;
    op.metadata = Some(prost_types::Any {
        type_url: METADATA_TYPE.into(),
        value: finished.encode_to_vec(),
    });
    assert!(
        operation_view(&op).is_err(),
        "missing output cannot count as complete"
    );
}
