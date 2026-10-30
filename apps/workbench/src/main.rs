// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> eframe::Result {
    let open = match spiling_workbench::launch::parse(std::env::args_os().skip(1)) {
        Ok(open) => open,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    eframe::run_native(
        "Spiling",
        spiling_workbench::native_options(),
        Box::new(move |cc| {
            Ok(Box::new(spiling_workbench::app::Workbench::with_open(
                cc, open,
            )))
        }),
    )
}
