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
        "engine_native_operation",
        "geometry_select_sources",
        "geometry_import_source",
        "geometry_control",
        "geometry_chunk",
        "geometry_debug_select_sources",
        "project_select_path",
        "project_debug_select_path",
        "project_control",
        "project_open",
        "project_save",
        "project_reopen",
        "surface_image_select",
        "surface_image_debug_select",
        "surface_image_import",
        "surface_image_bytes",
        "manufacturing_control",
        "manufacturing_select_intent",
        "manufacturing_debug_select_intent",
        "manufacturing_load_intent",
        "manufacturing_layers",
        "manufacturing_prepare_bed",
        "manufacturing_replay",
        "manufacturing_select_export",
        "manufacturing_debug_select_export",
        "manufacturing_export",
        "manufacturing_diagnostic_attach",
    ]);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("failed to prepare the Spiling CEF application");
}
