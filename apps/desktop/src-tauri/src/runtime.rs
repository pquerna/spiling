// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use serde::Serialize;
use tauri_runtime_cef::{CEF_API_VERSION_LAST, Cef, RemoteDebugging, SandboxPolicy};

pub fn configuration() -> Result<Cef, String> {
    let mut cef = Cef::default().remote_debugging(RemoteDebugging::Disabled);
    if let Some(value) = std::env::var_os("SPILING_CEF_DEBUG_PORT") {
        let port = value
            .to_str()
            .and_then(|value| value.parse::<u16>().ok())
            .filter(|port| *port >= 1024)
            .ok_or("SPILING_CEF_DEBUG_PORT must be an integer between 1024 and 65535")?;
        cef = cef.remote_debugging(RemoteDebugging::Port {
            port,
            allowed_origins: Vec::new(),
        });
        eprintln!("Spiling diagnostic: unauthenticated CEF debugging enabled on port {port}");
    }
    if std::env::var_os("SPILING_CEF_UNSANDBOXED").as_deref() == Some(std::ffi::OsStr::new("1")) {
        eprintln!("Spiling diagnostic: CEF sandbox explicitly disabled for this run");
        cef = cef.sandbox(SandboxPolicy::Disabled);
    }
    if std::env::var_os("SPILING_CEF_SOFTWARE_GPU").as_deref() == Some(std::ffi::OsStr::new("1")) {
        eprintln!("Spiling diagnostic: SwiftShader software GPU explicitly enabled for this run");
        cef = cef.command_line_args([
            ("use-angle", Some("swiftshader")),
            ("use-vulkan", Some("swiftshader")),
            ("enable-unsafe-swiftshader", None),
            ("use-webgpu-adapter", Some("swiftshader")),
            ("enable-unsafe-webgpu", None),
        ]);
    }
    Ok(cef)
}

#[derive(Serialize)]
pub struct RuntimeInfo {
    app_build: &'static str,
    runtime: &'static str,
    cef_api_version: i32,
}

#[tauri::command]
pub fn runtime_info() -> RuntimeInfo {
    RuntimeInfo {
        app_build: env!("CARGO_PKG_VERSION"),
        runtime: "cef",
        cef_api_version: CEF_API_VERSION_LAST,
    }
}
