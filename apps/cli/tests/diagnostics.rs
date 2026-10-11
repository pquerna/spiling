// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Actual-child diagnostic acceptance, sharing the native client and durable ledger.
use serde_json::Value;
use std::{
    path::PathBuf,
    process::{Command, Output},
};

fn cli() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_spiling-cli"));
    command.arg("--engine").arg(
        std::env::var_os("SPILING_TEST_ENGINE")
            .expect("set SPILING_TEST_ENGINE to the built native engine"),
    );
    command
}
fn report(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
#[cfg(target_os = "linux")]
fn reaped(pid: u64) {
    assert!(
        !PathBuf::from(format!("/proc/{pid}")).exists(),
        "child was not reaped before final output"
    );
}

#[test]
#[ignore = "actual diagnostic CLI regression; requires SPILING_TEST_ENGINE"]
fn diagnostic_readiness_and_triangle_finish_after_child_reap() {
    let diagnostic = report(cli().arg("diagnose").output().unwrap());
    assert_eq!(diagnostic["ping"], "pong");
    assert_eq!(diagnostic["hello"]["kernel"], "monstertruck");
    assert!(diagnostic["hello"]["session_id"].as_str().is_some());
    let root =
        std::env::temp_dir().join(format!("spiling-diagnostic-cli-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let output = root.join("triangle.splt");
    let triangle = report(
        cli()
            .arg("triangle")
            .arg("--output")
            .arg(&output)
            .output()
            .unwrap(),
    );
    let bytes = std::fs::read(&output).unwrap();
    assert_eq!(bytes.len(), 64);
    assert_eq!(&bytes[..4], b"SPLT");
    assert_eq!(triangle["synthetic"], true);
    #[cfg(target_os = "linux")]
    {
        reaped(diagnostic["hello"]["pid"].as_u64().unwrap());
        reaped(triangle["pid"].as_u64().unwrap());
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "actual retained diagnostic CLI regression; requires SPILING_TEST_ENGINE"]
fn same_request_retains_google_operation_across_fresh_processes_and_rejects_conflict() {
    let root = std::env::temp_dir().join(format!("spiling-operation-cli-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let store = root.join("store");
    let request_id = uuid::Uuid::new_v4().to_string();
    let invoke = |chunks: &str| {
        cli()
            .arg("job")
            .arg("--store")
            .arg(&store)
            .arg("--request-id")
            .arg(&request_id)
            .args([
                "--chunks",
                chunks,
                "--delay-ms",
                "150",
                "--chunk-bytes",
                "64",
                "--input-revision",
                "retained-input",
            ])
            .output()
            .unwrap()
    };
    let first = report(invoke("4"));
    assert_eq!(first["operation"]["state"], "succeeded");
    assert_eq!(first["operation"]["done"], true);
    assert_eq!(first["bytes"], 256);
    assert!(first["partial_updates"].as_u64().unwrap() > 0);
    let name = first["operation"]["name"].as_str().unwrap();
    assert!(name.starts_with("diagnostics/default/operations/"));
    let second = report(invoke("4"));
    assert_eq!(second["operation"]["name"], name);
    assert_eq!(
        second["operation"]["outputs"],
        first["operation"]["outputs"]
    );
    assert_eq!(second["bytes"], 256);
    let conflict = invoke("3");
    assert!(!conflict.status.success());
    assert!(conflict.stdout.is_empty());
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("cli_failure"));
    std::fs::remove_dir_all(root).unwrap();
}
