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
#[ignore = "actual preflight CLI regression; requires SPILING_TEST_ENGINE"]
fn refused_preflight_is_read_only_and_never_supplies_machine_permissions() {
    let root = root();
    let project = root.join("project");
    let created = report(
        cli("create", &project)
            .arg("--import")
            .arg(fixture("box-mm.step"))
            .output()
            .unwrap(),
    );
    let rejected_preflight = terminal_error(
        cli("inspect", &project)
            .arg("--preflight")
            .output()
            .unwrap(),
    );
    assert_eq!(rejected_preflight["domain"], "manufacturing");
    assert_eq!(rejected_preflight["error"]["code"], "no_intent");
    assert_eq!(
        rejected_preflight["project"]["revision"],
        created["project"]["revision"]
    );
    assert!(rejected_preflight["manufacturing"]["intent"].is_null());
    let rejected = cli("open", &project)
        .args(["--center-on-bed", "1"])
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    let reopened = report(cli("inspect", &project).output().unwrap());
    assert_eq!(
        reopened["project"]["revision"],
        created["project"]["revision"]
    );
    assert!(reopened["manufacturing"]["intent"].is_null());
    let invalid = report(
        cli("edit", &project)
            .arg("--intent")
            .arg(manufacturing_intent_fixture())
            .args([
                "--pose",
                "1",
                r#"{"translation_mm":[0,0,20],"rotation_xyzw":[0,0,0,1]}"#,
            ])
            .output()
            .unwrap(),
    );
    let inspected = report(
        cli("inspect", &project)
            .arg("--preflight")
            .output()
            .unwrap(),
    );
    assert_eq!(
        inspected["manufacturing"]["preflight"]["disposition"],
        "refused"
    );
    assert_eq!(
        inspected["manufacturing"]["preflight"]["software_only"],
        true
    );
    assert!(
        inspected["manufacturing"]["preflight"]["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| {
                check["severity"] == "error" && check["code"] == "floating_without_support"
            })
    );
    assert_eq!(
        inspected["project"]["revision"],
        invalid["project"]["revision"]
    );
    assert_eq!(inspected["project"]["dirty"], false);
    assert_eq!(inspected["project"]["read_only"], true);
    assert!(inspected["manufacturing"]["artifact"].is_null());
    assert_eq!(inspected["manufacturing"]["not_machine_ready"], true);
    assert_eq!(inspected["manufacturing"]["preflight_freshness"], "current");
    assert_eq!(
        inspected["manufacturing"]["current_revision"],
        inspected["project"]["revision"]
    );
    let historical = report(
        cli("edit", &project)
            .args([
                "--preflight",
                "--pose",
                "1",
                r#"{"translation_mm":[0,0,0],"rotation_xyzw":[0,0,0,1]}"#,
            ])
            .output()
            .unwrap(),
    );
    assert_eq!(
        historical["manufacturing"]["preflight"]["disposition"], "refused",
        "A historical refusal must not be rewritten as a fresh success after placement"
    );
    assert_eq!(historical["manufacturing"]["preflight_freshness"], "stale");
    assert_eq!(
        historical["manufacturing"]["current_revision"],
        historical["project"]["revision"]
    );
    let rows = historical["operations"].as_array().unwrap();
    assert_eq!(
        rows.iter()
            .find(|row| row["operation"] == "preflight")
            .unwrap()["preflight_freshness"],
        "current"
    );
    assert_eq!(
        rows.iter().find(|row| row["operation"] == "pose").unwrap()["preflight_freshness"],
        "stale"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "actual software manufacturing CLI regression; requires SPILING_TEST_ENGINE"]
fn manufacturing_fresh_process_replay_export_invalidation_and_undo() {
    let root = root();
    let project = root.join("project");
    let source = root.join("source.step");
    std::fs::copy(fixture("box-mm.step"), &source).unwrap();
    let first_export = root.join("first-export");
    let first_replay = root.join("first.splr");
    let created = report(
        cli("create", &project)
            .arg("--import")
            .arg(&source)
            .arg("--intent")
            .arg(manufacturing_intent_fixture())
            .args([
                "--place-on-bed",
                "1",
                "--center-on-bed",
                "1",
                "--preflight",
                "--compile",
                "--verify",
                "--manufacturing-out",
            ])
            .arg(&first_export)
            .arg("--replay-out")
            .arg(&first_replay)
            .output()
            .unwrap(),
    );
    let artifact = created["manufacturing"]["artifact"].clone();
    assert!(artifact["hash"].as_str().is_some());
    assert_eq!(artifact["summary"]["software_only"], true);
    let native_coverage = &artifact["summary"]["native_geometry"];
    assert_eq!(native_coverage["evidence"], "certified");
    let features = &native_coverage["source_feature_coverage"];
    assert_eq!(
        features["disjoint_source_layer_count"],
        native_coverage["certified_layer_count"]
    );
    assert_eq!(features["invariant_source_union_layer_count"], 0);
    assert_eq!(features["native_interface_complex_layer_count"], 0);
    assert_eq!(features["owned_critical_slab_count"], 0);
    assert!(
        native_coverage["requested_shape_error_mm"]
            .as_f64()
            .unwrap()
            > 0.0
    );
    assert!(
        native_coverage["achieved_shape_error_mm"].as_f64().unwrap()
            <= native_coverage["requested_shape_error_mm"]
                .as_f64()
                .unwrap()
    );
    assert_eq!(created["manufacturing"]["not_machine_ready"], true);
    assert_eq!(created["manufacturing"]["verification"]["verified"], true);
    assert_eq!(
        created["manufacturing"]["preflight"]["disposition"],
        "admitted"
    );
    assert_eq!(created["manufacturing"]["preflight"]["software_only"], true);
    assert!(
        !created["manufacturing"]["preflight"]["checks"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    for check in created["manufacturing"]["preflight"]["checks"]
        .as_array()
        .unwrap()
    {
        assert!(check["required"].is_boolean());
        assert!(matches!(
            check["evidence"].as_str(),
            Some("not_evaluated" | "certified" | "unsupported" | "indeterminate" | "failed")
        ));
        if check["required"] == true {
            assert_eq!(check["evidence"], "certified");
        }
    }
    let declared_intent: Value =
        serde_json::from_slice(&std::fs::read(manufacturing_intent_fixture()).unwrap()).unwrap();
    assert_eq!(created["manufacturing"]["intent"], declared_intent);
    let operations = created["operations"].as_array().unwrap();
    let preflight = operations
        .iter()
        .find(|row| row["operation"] == "preflight")
        .unwrap();
    let centered = operations
        .iter()
        .find(|row| row["operation"] == "center_on_bed")
        .unwrap();
    assert_eq!(
        preflight["project"]["revision"],
        centered["project"]["revision"]
    );
    assert_eq!(
        preflight["preflight"], created["manufacturing"]["preflight"],
        "operator JSON must retain the whole captured report, including optional analysis evidence"
    );
    assert_eq!(created["project"]["dirty"], false);
    assert_eq!(created["project"]["can_undo"], true);
    std::fs::remove_file(source).unwrap();
    let reopened_export = root.join("reopened-export");
    let reopened_replay = root.join("reopened.splr");
    let reopened = report(
        cli("inspect", &project)
            .args(["--preflight", "--verify", "--manufacturing-out"])
            .arg(&reopened_export)
            .arg("--replay-out")
            .arg(&reopened_replay)
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
        reopened["manufacturing"]["preflight"]["disposition"],
        "admitted"
    );
    assert_eq!(
        reopened["project"]["revision"],
        created["project"]["revision"]
    );
    assert_eq!(
        reopened["manufacturing"]["intent"],
        created["manufacturing"]["intent"]
    );
    assert_eq!(reopened["manufacturing_export"]["artifact"], artifact);
    let display: spiling_contracts::manufacturing::ReplayDisplayDescriptor =
        serde_json::from_value(reopened["manufacturing_replay"]["replay_display"].clone()).unwrap();
    let record: spiling_contracts::manufacturing::ManufacturingArtifactRecord =
        serde_json::from_value(artifact.clone()).unwrap();
    let actual = std::fs::read(&reopened_replay).unwrap();
    display.validate_for_record(&record).unwrap();
    spiling_contracts::manufacturing::validate_replay_display(&actual, &display).unwrap();
    assert_eq!(actual, std::fs::read(&first_replay).unwrap());
    assert_eq!(
        created["manufacturing_replay"]["replay_display"],
        reopened["manufacturing_replay"]["replay_display"]
    );
    let occupied = terminal_error(
        cli("inspect", &project)
            .arg("--replay-out")
            .arg(&reopened_replay)
            .output()
            .unwrap(),
    );
    assert_eq!(occupied["domain"], "input_or_output");
    assert_eq!(std::fs::read(&reopened_replay).unwrap(), actual);
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
    let corrupt_replay = root.join("corrupt.splr");
    terminal_error(
        cli("inspect", &project)
            .arg("--manufacturing-out")
            .arg(&corrupt_output)
            .arg("--replay-out")
            .arg(&corrupt_replay)
            .output()
            .unwrap(),
    );
    assert!(!corrupt_output.exists());
    assert!(!corrupt_replay.exists());
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

#[test]
#[ignore = "actual native CLI regression; requires SPILING_TEST_ENGINE"]
fn ordered_surface_ribs_compile_save_fresh_read_only_export_and_metadata_reveal() {
    surface_operator_workflow("box-mm.step", false);
}

#[test]
#[ignore = "actual native CLI regression; requires SPILING_TEST_ENGINE"]
fn ordered_cylindrical_ribs_compile_history_save_fresh_read_only_motion_and_reveal() {
    surface_operator_workflow("independent-exporters/occt-cylinder-mm.step", true);
}

fn surface_operator_workflow(source_fixture: &str, cylindrical: bool) {
    let root = root();
    let project = root.join("surface-project");
    let source = root.join("owned-source.step");
    std::fs::copy(fixture(source_fixture), &source).unwrap();
    let intent = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/manufacturing/solid-fill.intent.json");
    let baseline_export = root.join("baseline-export");
    let baseline_replay = root.join("baseline.splr");
    let created = report(
        cli("create", &project)
            .arg("--import")
            .arg(&source)
            .arg("--intent")
            .arg(&intent)
            .args([
                "--center-on-bed",
                "1",
                "--preflight",
                "--compile",
                "--verify",
            ])
            .arg("--manufacturing-out")
            .arg(&baseline_export)
            .arg("--replay-out")
            .arg(&baseline_replay)
            .output()
            .unwrap(),
    );
    let original_hash = created["manufacturing"]["artifact"]["input_hash"].clone();
    let occurrence = created["occurrences"][0]["occurrence_id"]
        .as_u64()
        .unwrap()
        .to_string();
    let mut query = cli("inspect", &project);
    for face in created["definitions"][0]["faces"].as_array().unwrap() {
        query
            .arg("--inspect-face")
            .arg(&occurrence)
            .arg(face["face_id"].as_str().unwrap());
    }
    let inspected = report(query.output().unwrap());
    let face = inspected["face_inspections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|inspection| {
            let carrier = &inspection["face"]["geometry"]["analytic"];
            if cylindrical {
                carrier["kind"] == "cylinder"
            } else {
                carrier["kind"] == "plane"
                    && carrier["normal"][2]
                        .as_f64()
                        .is_some_and(|z| z.abs() < 1e-10)
            }
        })
        .expect("fixture must expose the actual native carrier for the requested finish");
    let face_id = face["reference"]["face_id"].as_str().unwrap();
    if cylindrical {
        assert_eq!(face["face"]["geometry"]["analytic"]["radius_mm"], 5.0);
    }
    let finish = if cylindrical {
        r#"{"kind":"cylindrical_ribs","depth_mm":0.1,"rib_count":12,"phase_fraction":0.25}"#
    } else {
        r#"{"kind":"vertical_ribs","depth_mm":0.1,"pitch_mm":2,"edge_margin_mm":1,"phase_mm":0}"#
    };
    let export = root.join("rib-export");
    let finished = report(
        cli("edit", &project)
            .args([
                "--surface-finish",
                &occurrence,
                face_id,
                "definition",
                finish,
                "--preflight",
                "--compile",
                "--verify",
            ])
            .arg("--manufacturing-out")
            .arg(&export)
            .args(["--undo", "--redo", "--save"])
            .output()
            .unwrap(),
    );
    let overlays = finished["project"]["surface_overlays"].clone();
    assert_eq!(overlays["schema_version"], 3);
    assert_eq!(
        overlays["layers"][0]["finish"],
        serde_json::to_value(
            serde_json::from_str::<spiling_contracts::surface::SurfaceFinish>(finish).unwrap()
        )
        .unwrap()
    );
    assert_eq!(overlays["layers"][0]["scope"]["kind"], "definition");
    assert_eq!(overlays["layers"][0]["face_id"], face_id);
    assert_ne!(
        finished["manufacturing"]["artifact"]["input_hash"],
        original_hash
    );
    assert!(
        finished["manufacturing"]["verified_artifact_hash"].is_null(),
        "history must not restore fresh replay authorization"
    );
    let operations = finished["operations"].as_array().unwrap();
    let undo = operations
        .iter()
        .position(|row| row["operation"] == "undo")
        .unwrap();
    assert!(operations[undo]["manufacturing"]["artifact"].is_null());
    assert_eq!(operations[undo + 1]["operation"], "redo");
    assert_eq!(
        operations[undo + 1]["manufacturing"]["artifact"],
        finished["manufacturing"]["artifact"]
    );
    assert_eq!(finished["project"]["dirty"], false);
    let provenance: Value =
        serde_json::from_slice(&std::fs::read(export.join("provenance.json")).unwrap()).unwrap();
    assert_eq!(provenance["surface_overlays"], overlays);
    assert_eq!(provenance["planner_version"], "8");
    let baseline_provenance: Value =
        serde_json::from_slice(&std::fs::read(baseline_export.join("provenance.json")).unwrap())
            .unwrap();
    assert_eq!(
        provenance["definitions"],
        baseline_provenance["definitions"]
    );
    assert_eq!(
        provenance["occurrences"],
        baseline_provenance["occurrences"]
    );
    assert_ne!(
        std::fs::read(baseline_export.join("program.gcode")).unwrap(),
        std::fs::read(export.join("program.gcode")).unwrap(),
        "finish must change actual emitted machine motion"
    );
    let plan: Value =
        serde_json::from_slice(&std::fs::read(export.join("plan.json")).unwrap()).unwrap();
    let mut added_area = 0.0;
    for layer in plan["layers"].as_array().unwrap() {
        let coverage = &layer["region_coverage"];
        let added = coverage["surface_added_area_mm2"].as_f64().unwrap();
        let source = coverage["source_section_area_mm2"].as_f64().unwrap();
        let realized = coverage["section_area_mm2"].as_f64().unwrap();
        assert!((realized - source - added).abs() < 1e-6);
        assert!(added > 0.0);
        if cylindrical {
            assert!(
                (source - 25.0 * std::f64::consts::PI).abs() < 0.2,
                "unchanged OCCT cylinder source radius is five millimetres"
            );
            assert!(layer["z_mm"].as_f64().unwrap() <= 8.0);
        }
        added_area += added;
    }
    if cylindrical {
        assert_eq!(
            plan["layers"].as_array().unwrap().last().unwrap()["z_mm"],
            8.0
        );
    }
    assert!(
        added_area > 0.0,
        "rib construction must add actual realized material"
    );
    std::fs::remove_file(source).unwrap();
    let reopen_export = root.join("reopen-export");
    let replay = root.join("reopen.splr");
    let reopened = report(
        cli("inspect", &project)
            .args(["--preflight", "--verify"])
            .arg("--manufacturing-out")
            .arg(&reopen_export)
            .arg("--replay-out")
            .arg(&replay)
            .output()
            .unwrap(),
    );
    assert_eq!(reopened["project"]["surface_overlays"], overlays);
    for file in ["plan.json", "program.gcode", "provenance.json"] {
        assert_eq!(
            std::fs::read(export.join(file)).unwrap(),
            std::fs::read(reopen_export.join(file)).unwrap()
        );
    }
    assert_eq!(
        reopened["manufacturing_replay"]["provenance"]["surface_overlays"],
        overlays
    );
    let display: spiling_contracts::manufacturing::ReplayDisplayDescriptor =
        serde_json::from_value(reopened["manufacturing_replay"]["replay_display"].clone()).unwrap();
    let record: spiling_contracts::manufacturing::ManufacturingArtifactRecord =
        serde_json::from_value(reopened["manufacturing"]["artifact"].clone()).unwrap();
    display.validate_for_record(&record).unwrap();
    let actual_motion = std::fs::read(&replay).unwrap();
    spiling_contracts::manufacturing::validate_replay_display(&actual_motion, &display).unwrap();
    assert_ne!(
        &actual_motion[spiling_contracts::manufacturing::REPLAY_HEADER_BYTES..],
        &std::fs::read(&baseline_replay).unwrap()
            [spiling_contracts::manufacturing::REPLAY_HEADER_BYTES..],
        "actual decoded motion records, not only provenance headers, must change"
    );
    let fresh_verification: Value =
        serde_json::from_slice(&std::fs::read(reopen_export.join("verification.json")).unwrap())
            .unwrap();
    assert_eq!(
        fresh_verification,
        reopened["manufacturing"]["verification"]
    );
    assert_eq!(
        reopened["project"]["revision"],
        finished["project"]["revision"]
    );
    let revealed = report(
        cli("edit", &project)
            .args([
                "--surface-finish",
                &occurrence,
                face_id,
                "occurrence",
                r#"{"kind":"smooth"}"#,
                "--surface-finish-enabled",
                "2",
                "false",
                "--surface-finish-enabled",
                "2",
                "true",
                "--remove-surface-finish",
                "2",
            ])
            .output()
            .unwrap(),
    );
    assert_eq!(revealed["project"]["surface_overlays"], overlays);
    assert!(revealed["manufacturing"]["artifact"].is_null());
    let reveal_operations = revealed["operations"].as_array().unwrap();
    assert_eq!(
        reveal_operations[1]["project"]["surface_overlays"]["layers"][1]["finish"]["kind"],
        "smooth"
    );
    assert_eq!(
        reveal_operations[2]["project"]["surface_overlays"]["layers"][1]["enabled"],
        false
    );
    assert_eq!(
        reveal_operations[3]["project"]["surface_overlays"]["layers"][1]["enabled"],
        true
    );
    for flags in [
        vec![
            "--surface-finish",
            &occurrence,
            face_id,
            "occurrence",
            finish,
        ],
        vec!["--surface-finish-enabled", "1", "false"],
        vec!["--remove-surface-finish", "1"],
        vec!["--compile"],
        vec!["--save"],
    ] {
        let failure = Command::new(env!("CARGO_BIN_EXE_spiling-cli"))
            .arg("--engine")
            .arg(root.join("nonexistent-engine"))
            .args(["project", "inspect"])
            .arg(&project)
            .args(flags)
            .output()
            .unwrap();
        assert!(!failure.status.success());
        assert!(String::from_utf8_lossy(&failure.stderr).contains("read-only"));
    }
    let after_rejected = report(cli("inspect", &project).output().unwrap());
    assert_eq!(
        after_rejected["project"]["revision"],
        revealed["project"]["revision"]
    );
    assert_eq!(after_rejected["project"]["surface_overlays"], overlays);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "actual native CLI regression; requires SPILING_TEST_ENGINE"]
fn cylindrical_finish_refuses_split_native_face_without_smooth_fallback() {
    let root = root();
    let project = root.join("split-cylinder-project");
    let source = root.join("owned-split-cylinder.step");
    std::fs::copy(fixture("cylinder.step"), &source).unwrap();
    let intent = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/manufacturing/solid-fill.intent.json");
    let created = report(
        cli("create", &project)
            .arg("--import")
            .arg(&source)
            .arg("--intent")
            .arg(intent)
            .args(["--center-on-bed", "1", "--compile"])
            .output()
            .unwrap(),
    );
    let occurrence = created["occurrences"][0]["occurrence_id"]
        .as_u64()
        .unwrap()
        .to_string();
    let mut query = cli("inspect", &project);
    for face in created["definitions"][0]["faces"].as_array().unwrap() {
        query
            .arg("--inspect-face")
            .arg(&occurrence)
            .arg(face["face_id"].as_str().unwrap());
    }
    let inspected = report(query.output().unwrap());
    let faces: Vec<_> = inspected["face_inspections"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|face| face["face"]["geometry"]["analytic"]["kind"] == "cylinder")
        .collect();
    assert_eq!(
        faces.len(),
        2,
        "original fixture has split lateral native faces"
    );
    for face in faces {
        let failure = terminal_error(cli("edit", &project)
            .args(["--surface-finish", &occurrence, face["reference"]["face_id"].as_str().unwrap(),
                "definition", r#"{"kind":"cylindrical_ribs","depth_mm":0.1,"rib_count":12,"phase_fraction":0.25}"#,
                "--compile"])
            .output().unwrap());
        assert_eq!(failure["domain"], "manufacturing");
        assert_eq!(failure["error"]["code"], "unsupported_geometry");
        assert!(
            failure["manufacturing"]["artifact"].is_null(),
            "unsupported split faces cannot publish an untextured fallback"
        );
    }
    let reopened = report(cli("inspect", &project).output().unwrap());
    assert!(
        reopened["project"]["surface_overlays"]["layers"]
            .as_array()
            .unwrap()
            .is_empty(),
        "failed edit workflow must not checkpoint metadata"
    );
    assert_eq!(
        reopened["manufacturing"]["artifact"],
        created["manufacturing"]["artifact"]
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "actual native image CLI regression; requires SPILING_TEST_ENGINE"]
fn planar_image_relief_survives_source_deletion_and_fresh_readonly_retrieval() {
    image_operator_workflow("box-mm.step", false);
}

#[test]
#[ignore = "actual native image CLI regression; requires SPILING_TEST_ENGINE"]
fn cylindrical_image_relief_survives_source_deletion_and_fresh_readonly_retrieval() {
    image_operator_workflow("independent-exporters/occt-cylinder-mm.step", true);
}

// A real PNG with asymmetric U/Z nodes, unequal wrap borders and a black row.
// Stored DEFLATE avoids introducing an image decoder into the CLI test consumer.
fn image_png() -> Vec<u8> {
    fn crc(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb88320 & (0u32.wrapping_sub(crc & 1)));
            }
        }
        !crc
    }
    fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], bytes: &[u8]) {
        png.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        let start = png.len();
        png.extend_from_slice(kind);
        png.extend_from_slice(bytes);
        let checksum = crc(&png[start..]);
        png.extend_from_slice(&checksum.to_be_bytes());
    }
    let pixels: Vec<u8> = (0..32)
        .flat_map(|row| {
            if row == 7 {
                [0, 0, 0, 0]
            } else if row == 19 {
                [250, 10, 255, 160]
            } else {
                [0, 40 + row * 3, 70, 20]
            }
        })
        .collect();
    let mut raw = Vec::new();
    for row in pixels.chunks_exact(4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut zlib = vec![0x78, 0x01, 0x01];
    zlib.extend_from_slice(&(raw.len() as u16).to_le_bytes());
    zlib.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
    zlib.extend_from_slice(&raw);
    let (mut a, mut b) = (1u32, 0u32);
    for byte in raw {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut png, b"IHDR", &[0, 0, 0, 4, 0, 0, 0, 32, 8, 0, 0, 0, 0]);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    png
}

fn image_operator_workflow(source_fixture: &str, cylindrical: bool) {
    let root = root();
    let project = root.join("image-project");
    let source = root.join("original.step");
    let png = root.join("original.png");
    std::fs::copy(fixture(source_fixture), &source).unwrap();
    std::fs::write(&png, image_png()).unwrap();
    let created = report(
        cli("create", &project)
            .arg("--import")
            .arg(&source)
            .arg("--intent")
            .arg(manufacturing_intent_fixture())
            .args(["--center-on-bed", "1", "--compile"])
            .output()
            .unwrap(),
    );
    let mut query = cli("inspect", &project);
    for face in created["definitions"][0]["faces"].as_array().unwrap() {
        query.args(["--inspect-face", "1", face["face_id"].as_str().unwrap()]);
    }
    let inspected = report(query.output().unwrap());
    let face = inspected["face_inspections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|face| {
            let carrier = &face["face"]["geometry"]["analytic"];
            if cylindrical {
                carrier["kind"] == "cylinder"
            } else {
                carrier["kind"] == "plane"
                    && carrier["normal"][2]
                        .as_f64()
                        .is_some_and(|z| z.abs() < 1e-10)
            }
        })
        .unwrap()["reference"]["face_id"]
        .as_str()
        .unwrap();
    let parameters = serde_json::json!({
        "depth_mm":0.08, "u_repeats":2, "u_phase_fraction":0.137,
        "z_period_mm":3.7, "z_phase_mm":0.213,
        "edge_margin_mm":if cylindrical {0.0} else {1.0}, "vertical_error_mm":0.01
    })
    .to_string();
    let export = root.join("image-export");
    let authored = report(
        cli("edit", &project)
            .args(["--surface-image", "1", face, "definition"])
            .arg(&png)
            .arg(&parameters)
            .args(["--undo", "--redo", "--compile", "--verify"])
            .arg("--manufacturing-out")
            .arg(&export)
            .output()
            .unwrap(),
    );
    fn persistent_native_identity(definitions: &Value) -> Vec<[&Value; 5]> {
        definitions
            .as_array()
            .unwrap()
            .iter()
            .map(|definition| {
                let record = &definition["record"];
                [
                    &record["definition_id"],
                    &record["bounds_mm"],
                    &record["face_count"],
                    &record["provenance"],
                    &definition["faces"],
                ]
            })
            .collect()
    }
    assert_eq!(
        persistent_native_identity(&authored["definitions"]),
        persistent_native_identity(&created["definitions"]),
        "image authoring must preserve stored source records and native face identities"
    );
    assert_eq!(
        authored["occurrences"], created["occurrences"],
        "image authoring must not modify native placements"
    );
    let overlays = &authored["project"]["surface_overlays"];
    let hash = overlays["images"][0]["image_hash"].as_str().unwrap();
    let provenance: Value =
        serde_json::from_slice(&std::fs::read(export.join("provenance.json")).unwrap()).unwrap();
    assert_eq!(provenance["surface_overlays"], *overlays);
    assert_eq!(provenance["surface_images"].as_array().unwrap().len(), 1);
    let plan: Value =
        serde_json::from_slice(&std::fs::read(export.join("plan.json")).unwrap()).unwrap();
    let layers = plan["layers"].as_array().unwrap();
    let added: Vec<_> = layers
        .iter()
        .map(|layer| {
            let coverage = &layer["region_coverage"];
            let source = coverage["source_section_area_mm2"].as_f64().unwrap();
            let realized = coverage["section_area_mm2"].as_f64().unwrap();
            let added = coverage["surface_added_area_mm2"].as_f64().unwrap();
            assert!((realized - source - added).abs() < 1e-6);
            added
        })
        .collect();
    assert!(added.iter().any(|a| *a > 0.0));
    assert!(
        added.windows(2).any(|a| (a[0] - a[1]).abs() > 1e-5),
        "actual material must vary across physical Z, not merely the provenance hash"
    );
    let exteriors: Vec<Vec<[f64; 2]>> = layers
        .iter()
        .map(|layer| {
            layer["paths"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|path| path["kind"] == "perimeter")
                .flat_map(|path| path["points_mm"].as_array().unwrap())
                .map(|point| {
                    assert!(
                        (point[2].as_f64().unwrap() - layer["z_mm"].as_f64().unwrap()).abs() < 1e-8
                    );
                    [point[0].as_f64().unwrap(), point[1].as_f64().unwrap()]
                })
                .collect()
        })
        .collect();
    assert!(exteriors.iter().all(|points| !points.is_empty()));
    assert!(
        exteriors.windows(2).any(|pair| pair[0] != pair[1]),
        "actual attributed wall feature locations must vary with Z"
    );
    let program = std::fs::read(export.join("program.gcode")).unwrap();
    assert!(
        program.windows(2).any(|bytes| bytes == b" Z"),
        "actual emitted path must carry physical Z"
    );
    let history = authored["operations"].as_array().unwrap();
    let undo = history
        .iter()
        .find(|row| row["operation"] == "undo")
        .unwrap();
    assert!(
        undo["project"]["surface_overlays"]["images"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    std::fs::remove_file(&source).unwrap();
    std::fs::remove_file(&png).unwrap();
    let pgm = root.join("fresh.pgm");
    let motion = root.join("fresh.splr");
    let fresh_export = root.join("fresh-export");
    let reopened = report(
        cli("inspect", &project)
            .args(["--surface-image-out", hash])
            .arg(&pgm)
            .arg("--manufacturing-out")
            .arg(&fresh_export)
            .arg("--replay-out")
            .arg(&motion)
            .output()
            .unwrap(),
    );
    assert_eq!(
        reopened["project"]["revision"],
        authored["project"]["revision"]
    );
    assert_eq!(reopened["project"]["surface_overlays"], *overlays);
    let record: spiling_contracts::surface::SurfaceImageRecord =
        serde_json::from_value(overlays["images"][0].clone()).unwrap();
    let canonical = std::fs::read(pgm).unwrap();
    let view = record.validate_content(&canonical).unwrap();
    assert_eq!((view.width, view.height), (4, 32));
    assert_eq!(&view.pixels[7 * 4..8 * 4], &[0, 0, 0, 0]);
    assert_eq!(&view.pixels[19 * 4..20 * 4], &[250, 10, 255, 160]);
    assert_eq!(
        std::fs::read(fresh_export.join("program.gcode")).unwrap(),
        program
    );
    let descriptor =
        serde_json::from_value(reopened["manufacturing_replay"]["replay_display"].clone()).unwrap();
    spiling_contracts::manufacturing::validate_replay_display(
        &std::fs::read(motion).unwrap(),
        &descriptor,
    )
    .unwrap();
    let failure = Command::new(env!("CARGO_BIN_EXE_spiling-cli"))
        .arg("--engine")
        .arg(root.join("missing-engine"))
        .args(["project", "inspect"])
        .arg(&project)
        .args(["--surface-image", "1", face, "definition"])
        .arg(&png)
        .arg(parameters)
        .output()
        .unwrap();
    assert!(!failure.status.success());
    assert!(String::from_utf8_lossy(&failure.stderr).contains("read-only"));
    std::fs::remove_dir_all(root).unwrap();
}
