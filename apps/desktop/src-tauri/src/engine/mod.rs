// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

use serde::Serialize;
use spiling_contracts::{Hello, OperationView, operation_view, rpc::RunDiagnosticRequest};
use spiling_engine_client::{EngineClient, EngineRpc};
use tauri::{AppHandle, Manager, State};
use tokio::sync::{Mutex, MutexGuard};

#[derive(Default)]
pub struct EngineState {
    pub inner: Mutex<Supervisor>,
    pub closing: AtomicBool,
    pub exit_ready: AtomicBool,
}

impl EngineState {
    async fn active(&self) -> Result<MutexGuard<'_, Supervisor>, String> {
        if self.closing.load(Ordering::Acquire) {
            return Err("desktop is closing; engine commands are no longer accepted".into());
        }
        let guard = self.inner.lock().await;
        if self.closing.load(Ordering::Acquire) {
            return Err("desktop is closing; engine commands are no longer accepted".into());
        }
        Ok(guard)
    }
}

#[derive(Clone, Copy, Default, Serialize)]
#[serde(rename_all = "snake_case")]
enum Lifecycle {
    #[default]
    Stopped,
    Running,
    Interrupted,
}

#[derive(Serialize)]
pub struct EngineStatus {
    state: Lifecycle,
    message: Option<String>,
    hello: Option<Hello>,
}

#[derive(Default)]
pub struct Supervisor {
    client: Option<EngineClient>,
    state: Lifecycle,
    message: Option<String>,
}

impl Supervisor {
    async fn refresh(&mut self) {
        let Some(client) = self.client.as_mut() else {
            return;
        };
        match client.status().await {
            Ok(true) => {}
            Ok(false) => {
                self.client = None;
                self.state = Lifecycle::Interrupted;
                self.message = Some("engine exited unexpectedly; restart to recover".into());
            }
            Err(error) => {
                self.disconnect(format!("engine status failed: {error}"))
                    .await;
            }
        }
    }

    async fn disconnect(&mut self, mut reason: String) -> String {
        if let Some(mut client) = self.client.take()
            && let Err(error) = client.terminate().await
        {
            reason.push_str(&format!("; engine cleanup failed: {error}"));
        }
        self.state = Lifecycle::Interrupted;
        self.message = Some(reason.clone());
        reason
    }

    async fn start(&mut self, store: &std::path::Path) -> Result<Hello, String> {
        self.refresh().await;
        if let Some(client) = &self.client {
            return Ok(client.hello().clone());
        }
        let result = async {
            let path = engine_path()?;
            EngineClient::spawn_in(&path, store)
                .await
                .map_err(|error| format!("could not start engine at {}: {error}", path.display()))
        }
        .await;
        match result {
            Ok(client) => {
                let hello = client.hello().clone();
                self.client = Some(client);
                self.state = Lifecycle::Running;
                self.message = None;
                Ok(hello)
            }
            Err(error) => {
                self.state = Lifecycle::Interrupted;
                self.message = Some(error.clone());
                Err(error)
            }
        }
    }

    pub async fn stop(&mut self) -> Result<(), String> {
        let mut note = None;
        if let Some(mut client) = self.client.take()
            && let Err(error) = client.shutdown().await
        {
            let reason = format!("graceful engine shutdown failed: {error}");
            if let Err(cleanup) = client.terminate().await {
                let reason = format!("{reason}; forced cleanup failed: {cleanup}");
                self.state = Lifecycle::Interrupted;
                self.message = Some(reason.clone());
                return Err(reason);
            }
            eprintln!("Spiling: {reason}; engine forcibly stopped");
            note = Some(format!("{reason}; engine forcibly stopped"));
        }
        self.state = Lifecycle::Stopped;
        self.message = note;
        Ok(())
    }

    fn snapshot(&self) -> EngineStatus {
        EngineStatus {
            state: self.state,
            message: self.message.clone(),
            hello: self.client.as_ref().map(|client| client.hello().clone()),
        }
    }
}

fn engine_path() -> Result<PathBuf, String> {
    if cfg!(debug_assertions)
        && let Some(path) = std::env::var_os("SPILING_ENGINE_PATH")
    {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err("SPILING_ENGINE_PATH must name an absolute development engine path".into());
        }
        return Ok(path);
    }
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot resolve installed desktop executable: {error}"))?;
    let directory = executable
        .parent()
        .ok_or("installed desktop executable has no parent directory")?;
    let name = if cfg!(windows) {
        "spiling-engine.exe"
    } else {
        "spiling-engine"
    };
    let adjacent = directory.join(name);
    // CEF's Debian bundle relocates the desktop to share/Spiling while the
    // external binary remains in bin under the same installation prefix.
    if cfg!(target_os = "linux")
        && !adjacent.is_file()
        && directory.file_name() == Some(std::ffi::OsStr::new("Spiling"))
        && let Some(share) = directory.parent()
        && share.file_name() == Some(std::ffi::OsStr::new("share"))
        && let Some(prefix) = share.parent()
    {
        return Ok(prefix.join("bin").join(name));
    }
    Ok(adjacent)
}

#[tauri::command]
pub async fn engine_start(app: AppHandle, state: State<'_, EngineState>) -> Result<Hello, String> {
    let store = app
        .path()
        .app_local_data_dir()
        .map_err(|e| e.to_string())?
        .join("engine-operations");
    state.active().await?.start(&store).await
}

#[tauri::command]
pub async fn engine_status(state: State<'_, EngineState>) -> Result<EngineStatus, String> {
    let mut supervisor = state.active().await?;
    supervisor.refresh().await;
    Ok(supervisor.snapshot())
}

#[tauri::command]
pub async fn engine_stop(state: State<'_, EngineState>) -> Result<(), String> {
    state.active().await?.stop().await
}

#[tauri::command]
pub async fn engine_restart(
    app: AppHandle,
    state: State<'_, EngineState>,
) -> Result<Hello, String> {
    let mut supervisor = state.active().await?;
    supervisor.stop().await?;
    let store = app
        .path()
        .app_local_data_dir()
        .map_err(|e| e.to_string())?
        .join("engine-operations");
    supervisor.start(&store).await
}

async fn rpc(state: &EngineState) -> Result<EngineRpc, String> {
    let mut supervisor = state.active().await?;
    supervisor.refresh().await;
    supervisor
        .client
        .as_ref()
        .map(EngineClient::rpc)
        .ok_or_else(|| "engine is not running".into())
}

#[tauri::command]
pub async fn engine_triangle(
    state: State<'_, EngineState>,
) -> Result<tauri::ipc::Response, String> {
    let bytes = rpc(&state)
        .await?
        .triangle()
        .await
        .map_err(|e| e.to_string())?;
    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command]
pub async fn engine_run_diagnostic(
    state: State<'_, EngineState>,
    request_id: String,
) -> Result<OperationView, String> {
    let op = rpc(&state)
        .await?
        .run_diagnostic(RunDiagnosticRequest {
            parent: "diagnostics/desktop".into(),
            request_id,
            chunk_count: 4,
            delay_ms: 250,
            chunk_bytes: 64,
            input_revision: "diagnostic".into(),
        })
        .await
        .map_err(|e| e.to_string())?;
    operation_view(&op)
}

#[tauri::command]
pub async fn engine_get_operation(
    state: State<'_, EngineState>,
    name: String,
) -> Result<OperationView, String> {
    operation_view(
        &rpc(&state)
            .await?
            .get_operation(name)
            .await
            .map_err(|e| e.to_string())?,
    )
}

#[tauri::command]
pub async fn engine_cancel_operation(
    state: State<'_, EngineState>,
    name: String,
) -> Result<(), String> {
    rpc(&state)
        .await?
        .cancel_operation(name)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn engine_read_artifact(
    state: State<'_, EngineState>,
    name: String,
) -> Result<tauri::ipc::Response, String> {
    let rpc = rpc(&state).await?;
    let artifact = rpc.get_artifact(name).await.map_err(|e| e.to_string())?;
    let bytes = rpc
        .read_artifact(&artifact)
        .await
        .map_err(|e| e.to_string())?;
    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command]
pub async fn engine_interrupt(state: State<'_, EngineState>) -> Result<(), String> {
    let mut supervisor = state.active().await?;
    supervisor.refresh().await;
    let mut client = supervisor.client.take().ok_or("engine is not running")?;
    supervisor.state = Lifecycle::Interrupted;
    supervisor.message =
        Some("engine interrupted by the diagnostic action; restart to recover".into());
    if let Err(error) = client.terminate().await {
        let reason = format!("diagnostic engine interrupt failed: {error}");
        supervisor.message = Some(reason.clone());
        return Err(reason);
    }
    Ok(())
}
