// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Opt-in deep checks: actual CLI, shared client, native engine and frozen corpus.
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
fn engine() -> PathBuf {
    std::env::var_os("SPILING_TEST_ENGINE")
        .map(PathBuf::from)
        .expect("set SPILING_TEST_ENGINE to the built native engine")
}
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/geometry")
        .join(name)
}
fn cli() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_spiling-cli"));
    command.arg("--engine").arg(engine());
    command
}
fn root() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "spiling-cli-real-{}",
        spiling_contracts::geometry::SessionId::new().as_str()
    ));
    std::fs::create_dir(&root).unwrap();
    root
}
fn report(output: std::process::Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
#[cfg(target_os = "linux")]
fn assert_reaped(report: &serde_json::Value) {
    let pid = report["hello"]["pid"].as_u64().unwrap();
    assert!(
        !PathBuf::from(format!("/proc/{pid}")).exists(),
        "engine PID {pid} remains after successful CLI shutdown"
    );
}
#[test]
#[ignore = "native deep check; requires SPILING_TEST_ENGINE"]
fn two_parts_transfer_is_verified_even_without_output_and_sections_are_per_occurrence() {
    let root = root();
    let out = root.join("result");
    let recipe = fixture("scenes/two-parts.scene.json");
    let metadata = report(
        cli()
            .arg("geometry")
            .arg("--scene")
            .arg(&recipe)
            .arg("--section")
            .arg("0,0,4:0,0,1")
            .output()
            .unwrap(),
    );
    let persisted = report(
        cli()
            .arg("geometry")
            .arg("--scene")
            .arg(&recipe)
            .arg("--section")
            .arg("0,0,4:0,0,1")
            .arg("--out")
            .arg(&out)
            .output()
            .unwrap(),
    );
    assert_eq!(metadata["scene"]["definition_count"], 2);
    assert_eq!(metadata["scene"]["occurrence_count"], 3);
    assert_eq!(
        metadata["unique_mesh_bytes"],
        persisted["unique_mesh_bytes"]
    );
    assert_eq!(
        metadata["unique_mesh_chunks"],
        persisted["unique_mesh_chunks"]
    );
    let sources = metadata["definitions"].as_array().unwrap();
    let written = persisted["definitions"].as_array().unwrap();
    for (before, after) in sources.iter().zip(written) {
        assert_eq!(
            before["record"]["definition_id"],
            after["record"]["definition_id"]
        );
        assert_eq!(before["faces"], after["faces"]);
        for (a, b) in before["chunks"]
            .as_array()
            .unwrap()
            .iter()
            .zip(after["chunks"].as_array().unwrap())
        {
            assert_eq!(a["sha256"], b["sha256"]);
        }
    }
    let section = metadata["section"]["occurrences"].as_array().unwrap();
    assert_eq!(section.len(), 3);
    for row in section {
        assert_eq!(row["loop_count"], 1);
        let area = row["area_mm2"].as_f64().unwrap();
        assert!(
            (area - 200.0).abs() < 1e-6 || (area - 25.0 * std::f64::consts::PI).abs() < 0.16,
            "unexpected section area {area}"
        );
    }
    for row in persisted["definitions"].as_array().unwrap() {
        let artifact = row["record"]["mesh_artifact_id"].as_u64().unwrap();
        for chunk in row["chunks"].as_array().unwrap() {
            let index = chunk["chunk_index"].as_u64().unwrap();
            let bytes =
                std::fs::read(out.join(format!("mesh-{artifact:06}-{index:06}.splm"))).unwrap();
            assert_eq!(bytes.len() as u64, chunk["byte_count"].as_u64().unwrap());
            let hash: spiling_contracts::geometry::SourceHash =
                serde_json::from_value(chunk["sha256"].clone()).unwrap();
            assert!(hash.matches_bytes(&bytes));
        }
    }
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &std::fs::read(out.join("manifest.json")).unwrap()
        )
        .unwrap(),
        persisted
    );
    #[cfg(target_os = "linux")]
    {
        assert_reaped(&metadata);
        assert_reaped(&persisted);
    }
    // All names were produced by this test's exclusive directory.
    for entry in std::fs::read_dir(&out).unwrap() {
        std::fs::remove_file(entry.unwrap().path()).unwrap();
    }
    std::fs::remove_dir(out).unwrap();
    std::fs::remove_dir(root).unwrap();
}
#[test]
#[ignore = "native deep check; requires SPILING_TEST_ENGINE"]
fn repeated_instances_share_one_verified_mesh_and_large_origin_preserves_hashes() {
    let one = report(
        cli()
            .arg("geometry")
            .arg("--source")
            .arg(fixture("box-mm.step"))
            .output()
            .unwrap(),
    );
    let repeated = report(
        cli()
            .arg("geometry")
            .arg("--scene")
            .arg(fixture("scenes/repeated-128.scene.json"))
            .output()
            .unwrap(),
    );
    assert_eq!(repeated["scene"]["definition_count"], 1);
    assert_eq!(repeated["scene"]["occurrence_count"], 128);
    assert_eq!(one["unique_mesh_bytes"], repeated["unique_mesh_bytes"]);
    assert_eq!(one["unique_mesh_chunks"], repeated["unique_mesh_chunks"]);
    let ordinary = report(
        cli()
            .arg("geometry")
            .arg("--scene")
            .arg(fixture("scenes/two-parts.scene.json"))
            .output()
            .unwrap(),
    );
    let large = report(
        cli()
            .arg("geometry")
            .arg("--scene")
            .arg(fixture("scenes/large-origin.scene.json"))
            .arg("--section")
            .arg("1000000000,1000000000,1000000004:0,0,1")
            .output()
            .unwrap(),
    );
    assert_eq!(ordinary["unique_mesh_bytes"], large["unique_mesh_bytes"]);
    for (a, b) in ordinary["definitions"]
        .as_array()
        .unwrap()
        .iter()
        .zip(large["definitions"].as_array().unwrap())
    {
        assert_eq!(a["record"]["definition_id"], b["record"]["definition_id"]);
        for (a, b) in a["chunks"]
            .as_array()
            .unwrap()
            .iter()
            .zip(b["chunks"].as_array().unwrap())
        {
            assert_eq!(a["sha256"], b["sha256"]);
        }
    }
    #[cfg(target_os = "linux")]
    {
        assert_reaped(&one);
        assert_reaped(&repeated);
        assert_reaped(&ordinary);
        assert_reaped(&large);
    }
}
#[test]
#[ignore = "native deep check; requires SPILING_TEST_ENGINE"]
fn failure_rolls_back_new_output_without_deleting_existing_output() {
    let root = root();
    let out = root.join("new");
    let failure = cli()
        .arg("geometry")
        .arg("--source")
        .arg(fixture("truncated.step"))
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert!(!failure.status.success());
    assert!(failure.stdout.is_empty());
    assert!(!out.exists());
    std::fs::create_dir(&out).unwrap();
    std::fs::write(out.join("user-owned"), b"keep").unwrap();
    let refused = cli()
        .arg("geometry")
        .arg("--source")
        .arg(fixture("box-mm.step"))
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert_eq!(std::fs::read(out.join("user-owned")).unwrap(), b"keep");
    std::fs::remove_file(out.join("user-owned")).unwrap();
    std::fs::remove_dir(out).unwrap();
    std::fs::remove_dir(root).unwrap();
}
#[cfg(target_os = "linux")]
fn engine_child(cli_pid: u32) -> Option<u32> {
    std::fs::read_to_string(format!("/proc/{cli_pid}/task/{cli_pid}/children"))
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}
#[cfg(target_os = "linux")]
#[test]
#[ignore = "native deep check; requires SPILING_TEST_ENGINE"]
fn payload_output_failure_reaps_real_engine_and_retains_foreign_file() {
    use std::time::{Duration, Instant};
    let root = root();
    let out = root.join("result");
    let child = cli()
        .arg("geometry")
        .arg("--source")
        .arg(fixture("perforated-plate.step"))
        .arg("--out")
        .arg(&out)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !out.is_dir() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    // This foreign create_new file forces a real post-import write failure.
    let foreign = out.join("mesh-000001-000000.splm");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&foreign)
        .unwrap();
    drop(file);
    let engine_pid = loop {
        if let Some(pid) = engine_child(child.id()) {
            break pid;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    };
    let result = child.wait_with_output().unwrap();
    assert!(
        !result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
    assert!(!Path::new(&format!("/proc/{engine_pid}")).exists());
    assert!(foreign.is_file());
    assert!(!out.join("manifest.json").exists());
    std::fs::remove_file(foreign).unwrap();
    std::fs::remove_dir(out).unwrap();
    std::fs::remove_dir(root).unwrap();
}
#[cfg(target_os = "linux")]
#[test]
#[ignore = "native deep check; requires SPILING_TEST_ENGINE"]
fn closed_stdout_rolls_back_output_after_reaping() {
    use std::time::{Duration, Instant};
    let root = root();
    let out = root.join("result");
    let mut child = cli()
        .arg("geometry")
        .arg("--source")
        .arg(fixture("perforated-plate.step"))
        .arg("--out")
        .arg(&out)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let deadline = Instant::now() + Duration::from_secs(5);
    let engine_pid = loop {
        if let Some(pid) = engine_child(child.id()) {
            break pid;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    };
    let result = child.wait_with_output().unwrap();
    assert!(!result.status.success());
    assert!(!Path::new(&format!("/proc/{engine_pid}")).exists());
    assert!(!out.exists());
    std::fs::remove_dir(root).unwrap();
}
