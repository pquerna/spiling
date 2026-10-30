// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> eframe::Result {
    eframe::run_native(
        "Spiling",
        spiling_workbench::native_options(),
        Box::new(|cc| Ok(Box::new(spiling_workbench::app::Workbench::new(cc)))),
    )
}
