// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

use serde::Serialize;
use spiling_contracts::{Hello, PROTOCOL_VERSION};
use spiling_engine_client::EngineClient;
use tauri::State;
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

    async fn start(&mut self) -> Result<Hello, String> {
        self.refresh().await;
        if let Some(client) = &self.client {
            return Ok(client.hello().clone());
        }
        let result = async {
            let path = engine_path()?;
            let version = protocol_version()?;
            EngineClient::spawn(&path, version)
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
    Ok(directory.join(if cfg!(windows) {
        "spiling-engine.exe"
    } else {
        "spiling-engine"
    }))
}

fn protocol_version() -> Result<u16, String> {
    match std::env::var_os("SPILING_PROTOCOL_VERSION") {
        Some(value) => value
            .to_str()
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| "SPILING_PROTOCOL_VERSION must be an unsigned 16-bit integer".into()),
        None => Ok(PROTOCOL_VERSION),
    }
}

#[tauri::command]
pub async fn engine_start(state: State<'_, EngineState>) -> Result<Hello, String> {
    state.active().await?.start().await
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
pub async fn engine_restart(state: State<'_, EngineState>) -> Result<Hello, String> {
    let mut supervisor = state.active().await?;
    supervisor.stop().await?;
    supervisor.start().await
}

#[tauri::command]
pub async fn engine_triangle(
    state: State<'_, EngineState>,
) -> Result<tauri::ipc::Response, String> {
    let mut supervisor = state.active().await?;
    supervisor.refresh().await;
    let client = supervisor.client.as_mut().ok_or("engine is not running")?;
    match client.triangle().await {
        Ok(bytes) => Ok(tauri::ipc::Response::new(bytes)),
        Err(error) => Err(supervisor
            .disconnect(format!("triangle transfer failed: {error}"))
            .await),
    }
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
