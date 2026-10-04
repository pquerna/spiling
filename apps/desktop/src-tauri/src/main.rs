// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod engine;
mod runtime;

use std::sync::atomic::Ordering;

use engine::EngineState;
use tauri::{AppHandle, Manager, RunEvent, WindowEvent};

// Upstream handles renderer/GPU/utility processes before shell state exists.
#[tauri_runtime_cef::cef_entry_point]
fn main() {
    let cef = runtime::configuration().expect("invalid Spiling CEF diagnostic configuration");
    tauri::Builder::default()
        .runtime(cef)
        .manage(EngineState::default())
        .invoke_handler(tauri::generate_handler![
            engine::engine_start,
            engine::engine_status,
            engine::engine_stop,
            engine::engine_restart,
            engine::engine_triangle,
            engine::engine_interrupt,
            runtime::runtime_info,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build the Spiling CEF desktop")
        .run(|app, event| match event {
            RunEvent::WindowEvent {
                label,
                event: WindowEvent::CloseRequested { api, .. },
                ..
            } if label == "main" => {
                if !app
                    .state::<EngineState>()
                    .exit_ready
                    .load(Ordering::Acquire)
                {
                    api.prevent_close();
                    close_after_engine_cleanup(app, 0);
                }
            }
            RunEvent::ExitRequested { api, code, .. }
                if !app
                    .state::<EngineState>()
                    .exit_ready
                    .load(Ordering::Acquire) =>
            {
                api.prevent_exit();
                close_after_engine_cleanup(app, code.unwrap_or(0));
            }
            _ => {}
        });
}

fn close_after_engine_cleanup(app: &AppHandle, code: i32) {
    let state = app.state::<EngineState>();
    if state.closing.swap(true, Ordering::AcqRel) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<EngineState>();
        {
            let mut supervisor = state.inner.lock().await;
            if let Err(error) = supervisor.stop().await {
                eprintln!("Spiling: engine cleanup during desktop close failed: {error}");
            }
        }
        state.exit_ready.store(true, Ordering::Release);
        app.exit(code);
    });
}
