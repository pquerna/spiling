// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

fn main() {
    let manifest = tauri_build::AppManifest::new().commands(&[
        "engine_start",
        "engine_status",
        "engine_stop",
        "engine_restart",
        "engine_triangle",
        "engine_run_diagnostic",
        "engine_get_operation",
        "engine_cancel_operation",
        "engine_read_artifact",
        "engine_interrupt",
        "runtime_info",
    ]);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("failed to prepare the Spiling CEF application");
}
