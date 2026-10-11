// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Opt-in operator acceptance through separate actual CLI/engine invocations.
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};
fn engine() -> PathBuf {
    std::env::var_os("SPILING_TEST_ENGINE")
        .map(PathBuf::from)
        .expect("set SPILING_TEST_ENGINE to a built native engine")
}
fn cli(mode: &str, path: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_spiling-cli"));
    command
        .arg("--engine")
        .arg(engine())
        .arg("project")
        .arg(mode)
        .arg(path);
    command
}
fn root() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "spiling-project-cli-{}",
        spiling_contracts::geometry::SessionId::new().as_str()
    ));
    std::fs::create_dir(&root).unwrap();
    root
}
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/geometry")
        .join(name)
}
fn report(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    #[cfg(target_os = "linux")]
    assert!(!PathBuf::from(format!("/proc/{}", value["hello"]["pid"].as_u64().unwrap())).exists());
    value
}
fn terminal_error(output: Output) -> Value {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error: Value = std::str::from_utf8(&output.stderr)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|value| value["event"] == "cli_failure")
        .expect("CLI must emit its classified terminal error after engine diagnostics");
    error
}
fn failure(output: Output, code: &str) {
    let error = terminal_error(output);
    assert_eq!(error["domain"], "project");
    assert_eq!(error["error"]["code"], code);
}
const POSE: &str =
    r#"{"translation_mm":[40,0,0],"rotation_xyzw":[0,0,0.7071067811865476,0.7071067811865476]}"#;
const MOVED: &str = r#"{"translation_mm":[60,0,0],"rotation_xyzw":[0,0,0,1]}"#;

#[test]
#[ignore = "actual native CLI regression; requires SPILING_TEST_ENGINE"]
fn durable_operator_roundtrip_undo_checkpoints_source_removal_and_native_sections() {
    let root = root();
    let project = root.join("project");
    let box_source = root.join("box.step");
    let cylinder_source = root.join("cylinder.step");
    std::fs::copy(fixture("box-mm.step"), &box_source).unwrap();
    std::fs::copy(fixture("cylinder.step"), &cylinder_source).unwrap();
    let created = report(
        cli("create", &project)
            .arg("--import")
            .arg(&box_source)
            .arg("--import")
            .arg(&cylinder_source)
            .output()
            .unwrap(),
    );
    assert_eq!(created["scene"]["occurrence_count"], 2);
    assert_eq!(created["project"]["dirty"], false);
    assert_eq!(created["project"]["can_undo"], true);
    let definition = created["occurrences"][0]["definition_id"].as_str().unwrap();
    let edited = report(
        cli("edit", &project)
            .arg("--add")
            .arg(definition)
            .arg(POSE)
            .args([
                "--pose", "3", MOVED, "--undo", "--redo", "--save", "--pose", "3", POSE, "--undo",
                "--save", "--undo", "--redo",
            ])
            .args(["--section", "0,0,4:0,0,1"])
            .output()
            .unwrap(),
    );
    assert_eq!(edited["scene"]["occurrence_count"], 3);
    assert_eq!(edited["project"]["dirty"], false);
    let ops = edited["operations"].as_array().unwrap();
    let checkpoint = ops
        .iter()
        .position(|row| row["operation"] == "save")
        .unwrap();
    assert_eq!(ops[checkpoint]["project"]["can_undo"], true);
    assert_eq!(ops[checkpoint + 1]["project"]["dirty"], true);
    assert_eq!(ops[checkpoint + 2]["operation"], "undo");
    assert_eq!(ops[checkpoint + 2]["project"]["dirty"], false);
    assert_eq!(ops[checkpoint + 4]["operation"], "undo");
    assert_eq!(ops[checkpoint + 4]["project"]["dirty"], true);
    for row in edited["section"]["occurrences"].as_array().unwrap() {
        assert_eq!(row["loop_count"], 1);
        let area = row["area_mm2"].as_f64().unwrap();
        assert!((area - 200.0).abs() < 1e-6 || (area - 25.0 * std::f64::consts::PI).abs() < 0.16);
    }
    std::fs::remove_file(box_source).unwrap();
    std::fs::remove_file(cylinder_source).unwrap();
    let reopened = report(
        cli("inspect", &project)
            .args(["--section", "0,0,4:0,0,1"])
            .output()
            .unwrap(),
    );
    assert_ne!(
        edited["hello"]["session_id"],
        reopened["hello"]["session_id"]
    );
    assert_eq!(
        edited["project"]["project_id"],
        reopened["project"]["project_id"]
    );
    assert_eq!(edited["occurrences"], reopened["occurrences"]);
    for (before, after) in edited["section"]["occurrences"]
        .as_array()
        .unwrap()
        .iter()
        .zip(reopened["section"]["occurrences"].as_array().unwrap())
    {
        assert_eq!(before["occurrence_id"], after["occurrence_id"]);
        assert_eq!(before["definition_id"], after["definition_id"]);
        assert_eq!(before["loop_count"], after["loop_count"]);
        assert!(
            (before["area_mm2"].as_f64().unwrap() - after["area_mm2"].as_f64().unwrap()).abs()
                < 1e-6
        );
    }
    assert_eq!(edited["unique_mesh_bytes"], reopened["unique_mesh_bytes"]);
    assert_eq!(reopened["project"]["read_only"], true);
    assert_eq!(reopened["project"]["can_undo"], false);
    for (before, after) in edited["definitions"]
        .as_array()
        .unwrap()
        .iter()
        .zip(reopened["definitions"].as_array().unwrap())
    {
        assert_eq!(
            before["record"]["definition_id"],
            after["record"]["definition_id"]
        );
        assert_eq!(before["faces"], after["faces"]);
    }
    assert_eq!(
        edited["definitions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| &row["record"]["provenance"])
            .collect::<Vec<_>>(),
        reopened["definitions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| &row["record"]["provenance"])
            .collect::<Vec<_>>(),
        "native source labels and immutable provenance must survive deletion/reopen"
    );
    let occurrence = &reopened["occurrences"][0];
    let definition = reopened["definitions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["record"]["definition_id"] == occurrence["definition_id"])
        .unwrap();
    let face_id = definition["faces"][0]["face_id"].as_str().unwrap();
    let inspected = report(
        cli("inspect", &project)
            .arg("--inspect-face")
            .arg(occurrence["occurrence_id"].as_u64().unwrap().to_string())
            .arg(face_id)
            .output()
            .unwrap(),
    );
    assert_eq!(
        inspected["face_inspections"][0]["reference"]["face_id"],
        face_id
    );
    assert_eq!(inspected["face_inspections"][0]["pose"], occurrence["pose"]);
    let removed = report(
        cli("open", &project)
            .args(["--remove", "3", "--undo", "--redo"])
            .output()
            .unwrap(),
    );
    assert_eq!(removed["scene"]["occurrence_count"], 2);
    assert_eq!(removed["project"]["dirty"], true);
    let unchanged = report(cli("open", &project).output().unwrap());
    assert_eq!(unchanged["occurrences"], reopened["occurrences"]);
    // Save is a usable standalone command and keeps persistent references.
    assert_eq!(
        report(cli("save", &project).output().unwrap())["occurrences"],
        reopened["occurrences"]
    );
    failure(cli("open", &root.join("missing")).output().unwrap(), "io");
    let hash = reopened["definitions"][0]["record"]["provenance"]["source_hash"]
        .as_str()
        .unwrap();
    let asset = project.join("sources").join(format!("{hash}.step"));
    let source_bytes = std::fs::read(&asset).unwrap();
    std::fs::remove_file(&asset).unwrap();
    failure(cli("open", &project).output().unwrap(), "missing_asset");
    std::fs::write(&asset, b"not the original STEP bytes").unwrap();
    failure(cli("open", &project).output().unwrap(), "corrupt_asset");
    std::fs::write(&asset, source_bytes).unwrap();
    let manifest = project.join("manifest.json");
    let manifest_bytes = std::fs::read(&manifest).unwrap();
    std::fs::write(&manifest, b"{invalid").unwrap();
    failure(cli("open", &project).output().unwrap(), "invalid_project");
    let recovered = report(cli("recover", &project).output().unwrap());
    assert_eq!(recovered["project"]["recovered_previous"], true);
    assert_eq!(recovered["project"]["dirty"], true);
    assert_eq!(
        recovered["project"]["project_id"],
        reopened["project"]["project_id"]
    );
    assert_eq!(recovered["occurrences"], reopened["occurrences"]);
    // Explicit recovery does not silently repair or overwrite the broken current manifest.
    failure(cli("open", &project).output().unwrap(), "invalid_project");
    std::fs::write(&manifest, manifest_bytes).unwrap();
    // This exclusive test tree contains only names created by this test/native service.
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
#[ignore = "actual native CLI regression; requires SPILING_TEST_ENGINE"]
fn native_non_utf8_project_and_source_paths_roundtrip() {
    use std::os::unix::ffi::OsStringExt;
    let root = root();
    let project = root.join(std::ffi::OsString::from_vec(b"project-\xff".to_vec()));
    let source = root.join(std::ffi::OsString::from_vec(b"source-\xfe.step".to_vec()));
    std::fs::copy(fixture("box-mm.step"), &source).unwrap();
    let created = report(
        cli("create", &project)
            .arg("--import")
            .arg(&source)
            .output()
            .unwrap(),
    );
    std::fs::remove_file(source).unwrap();
    let reopened = report(cli("open", &project).output().unwrap());
    assert_eq!(created["occurrences"], reopened["occurrences"]);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "actual native CLI regression; requires SPILING_TEST_ENGINE"]
fn rejected_checkpoint_reports_observed_project_state_without_overwriting_foreign_output() {
    let root = root();
    let occupied = root.join("occupied");
    std::fs::create_dir(&occupied).unwrap();
    let foreign = occupied.join("foreign");
    std::fs::write(&foreign, b"operator-owned bytes").unwrap();
    let output_directory = root.join("inspection");
    let output = cli("create", &occupied)
        .arg("--import")
        .arg(fixture("box-mm.step"))
        .arg("--out")
        .arg(&output_directory)
        .output()
        .unwrap();
    let error = terminal_error(output);
    assert_eq!(error["operation"], "save");
    assert_eq!(error["project"]["dirty"], true);
    assert!(error["project"]["saved_revision"].is_null());
    assert!(error["project"]["path_label"].is_null());
    assert_eq!(std::fs::read(foreign).unwrap(), b"operator-owned bytes");
    assert!(!output_directory.exists());
    std::fs::remove_dir_all(root).unwrap();
}

fn manufacturing_intent_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/manufacturing/solid-fill.intent.json")
}

#[test]
#[ignore = "actual software manufacturing CLI regression; requires SPILING_TEST_ENGINE"]
fn manufacturing_fresh_process_replay_export_invalidation_and_undo() {
    let root = root();
    let project = root.join("project");
    let source = root.join("source.step");
    std::fs::copy(fixture("box-mm.step"), &source).unwrap();
    let first_export = root.join("first-export");
    let created = report(
        cli("create", &project)
            .arg("--import")
            .arg(&source)
            .arg("--intent")
            .arg(manufacturing_intent_fixture())
            .args(["--compile", "--verify", "--manufacturing-out"])
            .arg(&first_export)
            .output()
            .unwrap(),
    );
    let artifact = created["manufacturing"]["artifact"].clone();
    assert!(artifact["hash"].as_str().is_some());
    assert_eq!(artifact["summary"]["software_only"], true);
    assert_eq!(created["manufacturing"]["not_machine_ready"], true);
    assert_eq!(created["manufacturing"]["verification"]["verified"], true);
    assert_eq!(created["project"]["dirty"], false);
    assert_eq!(created["project"]["can_undo"], true);
    std::fs::remove_file(source).unwrap();
    let reopened_export = root.join("reopened-export");
    let reopened = report(
        cli("inspect", &project)
            .args(["--verify", "--manufacturing-out"])
            .arg(&reopened_export)
            .output()
            .unwrap(),
    );
    assert_ne!(
        created["hello"]["session_id"],
        reopened["hello"]["session_id"]
    );
    assert_eq!(reopened["project"]["read_only"], true);
    assert_eq!(reopened["project"]["dirty"], false);
    assert_eq!(reopened["manufacturing"]["artifact"], artifact);
    assert_eq!(
        reopened["manufacturing"]["intent"],
        created["manufacturing"]["intent"]
    );
    assert_eq!(reopened["manufacturing_export"]["artifact"], artifact);
    for name in [
        "plan.json",
        "program.gcode",
        "verification.json",
        "provenance.json",
    ] {
        assert_eq!(
            std::fs::read(first_export.join(name)).unwrap(),
            std::fs::read(reopened_export.join(name)).unwrap(),
            "{name}"
        );
    }
    assert_eq!(
        serde_json::from_slice::<Value>(
            &std::fs::read(reopened_export.join("verification.json")).unwrap()
        )
        .unwrap(),
        reopened["manufacturing_export"]["verification"],
        "export must contain the exact fresh verification, not the stored bundle flag"
    );
    let mut intent: Value =
        serde_json::from_slice(&std::fs::read(manufacturing_intent_fixture()).unwrap()).unwrap();
    intent["recipe"]["print_speed_mm_s"] = serde_json::json!(36.0);
    let changed_intent = root.join("changed-intent.json");
    std::fs::write(&changed_intent, serde_json::to_vec(&intent).unwrap()).unwrap();
    let edited = report(
        cli("edit", &project)
            .arg("--intent")
            .arg(&changed_intent)
            .args([
                "--undo", "--save", "--redo", "--undo", "--pose", "1", MOVED, "--undo", "--verify",
            ])
            .output()
            .unwrap(),
    );
    let operations = edited["operations"].as_array().unwrap();
    assert!(operations[1]["manufacturing"]["artifact"].is_null());
    assert_eq!(operations[2]["manufacturing"]["artifact"], artifact);
    // Save preserves the redo branch: replaying the actual edit invalidates
    // its old artifact, and undo restores the original coherent input/bundle.
    assert!(operations[4]["manufacturing"]["artifact"].is_null());
    assert_eq!(operations[5]["manufacturing"]["artifact"], artifact);
    assert!(operations[6]["manufacturing"]["artifact"].is_null());
    assert_eq!(operations[7]["manufacturing"]["artifact"], artifact);
    assert_eq!(edited["manufacturing"]["artifact"], artifact);
    assert_eq!(
        edited["manufacturing"]["verified_artifact_hash"],
        artifact["hash"]
    );
    assert_eq!(
        edited["manufacturing"]["intent"],
        created["manufacturing"]["intent"]
    );
    let fresh = report(cli("inspect", &project).arg("--verify").output().unwrap());
    assert_eq!(fresh["manufacturing"]["artifact"], artifact);
    let changed_export = root.join("changed-export");
    let compiled_edit = report(
        cli("edit", &project)
            .arg("--intent")
            .arg(&changed_intent)
            .args(["--compile", "--verify", "--manufacturing-out"])
            .arg(&changed_export)
            .output()
            .unwrap(),
    );
    assert_eq!(compiled_edit["project"]["dirty"], false);
    assert_ne!(
        compiled_edit["manufacturing"]["artifact"]["input_hash"],
        artifact["input_hash"]
    );
    let changed_reopen_export = root.join("changed-reopen-export");
    let changed_reopen = report(
        cli("inspect", &project)
            .arg("--manufacturing-out")
            .arg(&changed_reopen_export)
            .output()
            .unwrap(),
    );
    assert_eq!(
        compiled_edit["manufacturing"]["artifact"],
        changed_reopen["manufacturing"]["artifact"]
    );
    for name in [
        "plan.json",
        "program.gcode",
        "verification.json",
        "provenance.json",
    ] {
        assert_eq!(
            std::fs::read(changed_export.join(name)).unwrap(),
            std::fs::read(changed_reopen_export.join(name)).unwrap(),
            "{name}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "actual software manufacturing failure regression; requires SPILING_TEST_ENGINE"]
fn manufacturing_errors_leave_no_export_or_success_and_preserve_checkpoint() {
    let root = root();
    let project = root.join("project");
    let created = report(
        cli("create", &project)
            .arg("--import")
            .arg(fixture("box-mm.step"))
            .arg("--intent")
            .arg(manufacturing_intent_fixture())
            .arg("--compile")
            .output()
            .unwrap(),
    );
    let artifact = created["manufacturing"]["artifact"].clone();
    let failed_output = root.join("failed-export");
    let failure = terminal_error(
        cli("open", &project)
            .args(["--pose", "1", MOVED, "--verify", "--manufacturing-out"])
            .arg(&failed_output)
            .output()
            .unwrap(),
    );
    assert_eq!(failure["domain"], "manufacturing");
    assert_eq!(failure["project"]["dirty"], true);
    assert!(failure["manufacturing"]["artifact"].is_null());
    assert!(!failed_output.exists());
    assert_eq!(
        report(cli("inspect", &project).arg("--verify").output().unwrap())["manufacturing"]["artifact"],
        artifact
    );
    let mut too_many_layers: Value =
        serde_json::from_slice(&std::fs::read(manufacturing_intent_fixture()).unwrap()).unwrap();
    too_many_layers["recipe"]["layer_height_mm"] = serde_json::json!(0.001);
    let limited_intent = root.join("limited-intent.json");
    std::fs::write(
        &limited_intent,
        serde_json::to_vec(&too_many_layers).unwrap(),
    )
    .unwrap();
    let limited_output = root.join("limited-export");
    let resource_failure = terminal_error(
        cli("open", &project)
            .arg("--intent")
            .arg(&limited_intent)
            .arg("--compile")
            .arg("--manufacturing-out")
            .arg(&limited_output)
            .output()
            .unwrap(),
    );
    assert_eq!(resource_failure["domain"], "manufacturing");
    assert_eq!(resource_failure["error"]["code"], "resource_limit");
    assert!(resource_failure["manufacturing"]["artifact"].is_null());
    assert!(!limited_output.exists());
    assert_eq!(
        report(cli("inspect", &project).arg("--verify").output().unwrap())["manufacturing"]["artifact"],
        artifact
    );
    let occupied = root.join("occupied-export");
    std::fs::create_dir(&occupied).unwrap();
    std::fs::write(occupied.join("foreign"), b"operator-owned").unwrap();
    let failure = terminal_error(
        cli("inspect", &project)
            .arg("--manufacturing-out")
            .arg(&occupied)
            .output()
            .unwrap(),
    );
    assert_eq!(failure["domain"], "input_or_output");
    assert_eq!(
        std::fs::read(occupied.join("foreign")).unwrap(),
        b"operator-owned"
    );
    let read_only_failure =
        terminal_error(cli("inspect", &project).arg("--compile").output().unwrap());
    assert_eq!(read_only_failure["domain"], "input_or_output");
    let asset = project
        .join("manufacturing")
        .join(format!("{}.json", artifact["hash"].as_str().unwrap()));
    let original = std::fs::read(&asset).unwrap();
    let mut corrupt: Value = serde_json::from_slice(&original).unwrap();
    corrupt["program"] = serde_json::json!("G21\nG90\nM82\nG92 E0\nG1 X999 Y999 Z999 E999 F999\n");
    std::fs::write(&asset, serde_json::to_vec(&corrupt).unwrap()).unwrap();
    let corrupt_output = root.join("corrupt-export");
    terminal_error(
        cli("inspect", &project)
            .arg("--manufacturing-out")
            .arg(&corrupt_output)
            .output()
            .unwrap(),
    );
    assert!(!corrupt_output.exists());
    std::fs::write(&asset, original).unwrap();
    assert_eq!(
        report(cli("inspect", &project).arg("--verify").output().unwrap())["manufacturing"]["artifact"],
        artifact
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
#[ignore = "actual native manufacturing path regression; requires SPILING_TEST_ENGINE"]
fn manufacturing_intent_and_export_keep_non_utf8_native_paths() {
    use std::os::unix::ffi::OsStringExt;
    let root = root();
    let project = root.join("project");
    let intent = root.join(std::ffi::OsString::from_vec(b"intent-\xff.json".to_vec()));
    let export = root.join(std::ffi::OsString::from_vec(b"export-\xfe".to_vec()));
    std::fs::copy(manufacturing_intent_fixture(), &intent).unwrap();
    let created = report(
        cli("create", &project)
            .arg("--import")
            .arg(fixture("box-mm.step"))
            .arg("--intent")
            .arg(intent)
            .arg("--compile")
            .arg("--manufacturing-out")
            .arg(&export)
            .output()
            .unwrap(),
    );
    assert_eq!(
        created["manufacturing_export"]["verification"]["verified"],
        true
    );
    assert!(export.join("program.gcode").is_file());
    std::fs::remove_dir_all(root).unwrap();
}
