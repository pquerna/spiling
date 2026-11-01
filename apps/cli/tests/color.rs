// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
//! Actual CLI -> authenticated engine naming, independent of geometry or project state.
use spiling_contracts::color_naming::*;
use std::{path::PathBuf, process::Command};
fn command() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_spiling-cli"));
    cmd.arg("--engine")
        .arg(std::env::var_os("SPILING_TEST_ENGINE").expect("set SPILING_TEST_ENGINE"))
        .arg("color");
    cmd
}
#[test]
#[ignore = "actual child regression; requires SPILING_TEST_ENGINE"]
fn color_queries_preserve_numeric_samples_and_return_mode_owned_provenance() {
    for mode in ["exact", "nearest", "descriptive"] {
        let out = command()
            .args(["#CF524E", "123456", "000000", "FFFFFF", "--mode", mode])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let reply: ColorNamingResponse = serde_json::from_value(json["naming"].clone()).unwrap();
        reply.validate().unwrap();
        assert_eq!(
            reply.colors.iter().map(|v| v.srgb8).collect::<Vec<_>>(),
            vec![[207, 82, 78], [18, 52, 86], [0; 3], [255; 3]]
        );
        assert_eq!(reply.colors[0].name.as_ref().unwrap().label, "Dark coral");
        if mode == "exact" {
            assert!(reply.colors[1].name.is_none());
        } else {
            assert!(reply.colors.iter().all(|c| c.name.is_some()));
        }
        #[cfg(target_os = "linux")]
        assert!(
            !PathBuf::from(format!("/proc/{}", json["hello"]["pid"].as_u64().unwrap())).exists(),
            "CLI must reap its child"
        );
    }
    for input in ["#fff", "#12345678", "not-a-color"] {
        let out = command().arg(input).output().unwrap();
        assert!(!out.status.success());
        assert!(out.stdout.is_empty());
    }
}
