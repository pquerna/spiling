// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;

fn result(report: &Value) -> &Value {
    &report["operations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|operation| operation["result"]["summary"].is_object())
        .expect("actual checked analysis fields must be returned")["result"]
}

#[test]
#[ignore = "actual fastener CLI and supervised engine; requires SPILING_TEST_ENGINE"]
fn ordered_native_fasteners_and_portable_saved_fields_through_separate_cli_processes() {
    let root = root();
    let project = root.join("fastener.spil");
    let source = root.join("plate.step");
    let store = root.join("operations");
    std::fs::copy(
        fixture("independent-exporters/occt-through-hole-mm.step"),
        &source,
    )
    .unwrap();
    let retained = |mode: &str| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_spiling-cli"));
        command
            .arg("--engine")
            .arg(engine())
            .arg("--store")
            .arg(&store)
            .arg("project")
            .arg(mode)
            .arg(&project);
        command
    };
    let created = report(
        retained("create")
            .arg("--import")
            .arg(&source)
            .args(["--analysis-case", "Native clamp", "1", "basic"])
            .output()
            .unwrap(),
    );
    let mut query = retained("inspect");
    for face in created["definitions"][0]["faces"].as_array().unwrap() {
        query.args(["--inspect-face", "1", face["face_id"].as_str().unwrap()]);
    }
    let inspected = report(query.output().unwrap());
    let inspections = inspected["face_inspections"].as_array().unwrap();
    let bore = inspections
        .iter()
        .find(|face| {
            let analytic = &face["face"]["geometry"]["analytic"];
            analytic["kind"] == "cylinder"
                && analytic["radius_mm"]
                    .as_f64()
                    .is_some_and(|radius| (radius - 3.0).abs() < 1e-7)
        })
        .expect("actual native cylindrical bore")["reference"]["face_id"]
        .as_str()
        .unwrap();
    let loaded = inspections
        .iter()
        .find(|face| {
            let analytic = &face["face"]["geometry"]["analytic"];
            analytic["kind"] == "plane"
                && analytic["normal"][0]
                    .as_f64()
                    .is_some_and(|normal| normal.abs() > 0.99)
                && analytic["origin_mm"][0]
                    .as_f64()
                    .is_some_and(|coordinate| (coordinate - 20.0).abs() < 1e-7)
        })
        .expect("actual native x=20 load face")["reference"]["face_id"]
        .as_str()
        .unwrap();
    let edited = report(
        retained("edit")
            .args(["--analysis-clamp", "Native clamp", bore, "start", "8"])
            .args(["--analysis-pin", "Native clamp", bore, "free", "fixed"])
            .args(["--analysis-engagement", "Native clamp", bore, "end", "3.1"])
            .args(["--analysis-unfasten", "Native clamp", "1"])
            .args(["--analysis-clamp", "Native clamp", bore, "end", "8"])
            .args([
                "--analysis-load",
                "Native clamp",
                loaded,
                "0",
                "0",
                "-10",
                "-100",
                "200",
                "0",
            ])
            .output()
            .unwrap(),
    );
    let edits = edited["operations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|operation| operation["operation"] == "analysis_edit")
        .map(|operation| &operation["project"]["analysis"]["cases"][0]["case"])
        .collect::<Vec<_>>();
    assert_eq!(edits.len(), 6);
    let attachment = |index: usize| &edits[index]["fasteners"][0]["attachment"];
    assert_eq!(
        attachment(0),
        &serde_json::json!({
            "kind": "clamp", "end": "start", "washer_diameter_mm": 8.0
        })
    );
    assert_eq!(
        attachment(1),
        &serde_json::json!({
            "kind": "pin", "allow_axial_slide": true, "allow_spin": false
        })
    );
    assert_eq!(
        attachment(2),
        &serde_json::json!({
            "kind": "bonded_engagement", "end": "end", "engagement_mm": 3.1
        })
    );
    assert!(edits[3]["fasteners"].as_array().unwrap().is_empty());
    assert_eq!(
        attachment(4),
        &serde_json::json!({
            "kind": "clamp", "end": "end", "washer_diameter_mm": 8.0
        })
    );
    for case in &edits {
        assert!(case["restraints"].as_array().unwrap().is_empty());
        if let Some(fastener) = case["fasteners"].as_array().unwrap().first() {
            assert_eq!(fastener["id"], 1);
            assert_eq!(
                fastener["hole"]["center_mm"],
                serde_json::json!([10.0, 10.0, 0.0])
            );
            assert_eq!(fastener["hole"]["radius_mm"], 3.0);
            assert_eq!(fastener["hole"]["depth_mm"], 8.0);
        }
    }
    assert_eq!(edited["project"]["dirty"], false);
    let assumptions = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/analysis/hook-assumptions.json");
    let settings: Value = serde_json::from_slice(&std::fs::read(&assumptions).unwrap()).unwrap();
    assert_eq!(settings["maximum_span_mm"], 12);
    let mechanism = terminal_error(
        retained("edit")
            .args(["--analysis-pin", "Native clamp", bore, "fixed", "free"])
            .args(["--analysis-run", "Native clamp"])
            .arg(&assumptions)
            .output()
            .unwrap(),
    );
    assert!(
        mechanism.to_string().contains("DisconnectedOrMechanism"),
        "{mechanism}"
    );
    std::fs::remove_file(&source).unwrap();
    let solved = report(
        retained("edit")
            .args(["--analysis-run-and-attach", "Native clamp"])
            .arg(&assumptions)
            .output()
            .unwrap(),
    );
    let saved = result(&solved);
    assert_eq!(
        saved["summary"]["case"]["case"],
        edited["project"]["analysis"]["cases"][0]["case"]
    );
    assert_eq!(
        saved["summary"]["mesh_geometry"]["kind"],
        "rational_bored_plate"
    );
    assert_eq!(saved["summary"]["elements"], 144);
    assert_eq!(saved["summary"]["dofs"], 4704);
    assert_eq!(saved["summary"]["accuracy_evidence"], "unqualified");
    assert!(
        saved["summary"]["maximum_displacement_mm"]
            .as_f64()
            .unwrap()
            > 0.0
    );
    assert_eq!(saved["field_samples"].as_array().unwrap().len(), 16);
    let response = &saved["summary"]["fastener_responses"][0];
    assert!(
        (response["area_mm2"].as_f64().unwrap() / (std::f64::consts::PI * 7.0) - 1.0).abs() < 1e-9
    );
    for (axis, expected) in [0.0, 0.0, 10.0].into_iter().enumerate() {
        assert!((response["reaction_force_n"][axis].as_f64().unwrap() - expected).abs() < 1e-6);
    }
    for (axis, expected) in [100.0, -200.0, 0.0].into_iter().enumerate() {
        assert!((response["reaction_moment_n_mm"][axis].as_f64().unwrap() - expected).abs() < 1e-5);
    }
    assert_eq!(
        solved["project"]["analysis"]["results"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(solved["project"]["dirty"], false);
    let portable_directory = root.join("portable");
    std::fs::create_dir(&portable_directory).unwrap();
    let copied = portable_directory.join("only.spil");
    std::fs::copy(&project, &copied).unwrap();
    std::fs::remove_file(&project).unwrap();
    std::fs::remove_dir_all(&store).unwrap();
    assert_eq!(std::fs::read_dir(&portable_directory).unwrap().count(), 1);
    let restored = report(
        cli("inspect", &copied)
            .args(["--analysis-saved", "Native clamp"])
            .output()
            .unwrap(),
    );
    assert_ne!(
        restored["hello"]["session_id"],
        solved["hello"]["session_id"]
    );
    assert_eq!(restored["project"]["read_only"], true);
    assert_eq!(restored["project"]["dirty"], false);
    assert_eq!(
        restored["project"]["analysis"],
        solved["project"]["analysis"]
    );
    let returned = result(&restored);
    assert_eq!(returned["summary"], saved["summary"]);
    assert_eq!(returned["field_samples"], saved["field_samples"]);
    assert_eq!(returned["resource"]["sha256"], saved["resource"]["sha256"]);
    std::fs::remove_dir_all(root).unwrap();
}
