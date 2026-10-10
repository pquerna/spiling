// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

use serde::Serialize;
use spiling_contracts::{
    Hello, PROTOCOL_VERSION,
    geometry::{
        ArtifactId, EngineJob, GeometryCommand, GeometryError, GeometryErrorCode, GeometryResponse,
        JobResult, JobStatus, MAX_DISPLAY_LABEL_BYTES, NativePath, RigidPoseMm, SceneRevision,
        SelectedSource, SessionId, bounded_text,
    },
    project::{
        ProjectCommand, ProjectError, ProjectErrorCode, ProjectPathIntent, ProjectResponse,
        SelectedProjectPath,
    },
};
use spiling_engine_client::{ClientError, EngineClient};
use tauri::{Manager, State};
use tokio::sync::{Mutex, MutexGuard};

pub struct EngineState {
    pub inner: Mutex<Supervisor>,
    pub closing: AtomicBool,
    pub exit_ready: AtomicBool,
    fixture_root: Option<PathBuf>,
}

impl EngineState {
    pub fn geometry_debug_enabled(&self) -> bool {
        self.fixture_root.is_some()
    }
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

impl Default for EngineState {
    fn default() -> Self {
        // Read once at startup, never a caller-controlled filesystem root.
        let fixture_root = std::env::var("SPILING_CEF_DEBUG_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .filter(|port| *port >= 1024)
            .and_then(|_| std::env::var_os("SPILING_GEOMETRY_FIXTURE_ROOT"))
            .and_then(|path| PathBuf::from(path).canonicalize().ok())
            .filter(|path| path.is_dir());
        Self {
            inner: Mutex::default(),
            closing: AtomicBool::new(false),
            exit_ready: AtomicBool::new(false),
            fixture_root,
        }
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
    sources: Vec<SelectedSourceEntry>,
    project_paths: Vec<SelectedProjectEntry>,
    attached_project: Option<AttachedProject>,
    pending_project: Option<(spiling_contracts::geometry::JobId, AttachedProject, bool)>,
}

// A shared job's completion alone is not a project open/save receipt.
fn project_job_attaches(job: &EngineJob, replacing: bool) -> bool {
    match &job.result {
        Some(JobResult::Scene { .. }) => replacing && job.status == JobStatus::Completed,
        Some(JobResult::ProjectSaved { info }) => {
            !replacing
                && (job.status == JobStatus::Completed
                    || (matches!(job.status, JobStatus::Failed | JobStatus::Cancelled)
                        && info.save_uncertain))
        }
        Some(JobResult::Section { .. })
        | Some(JobResult::ManufacturingCompiled { .. })
        | Some(JobResult::ManufacturingVerified { .. })
        | None => false,
    }
}

impl Supervisor {
    async fn geometry(&mut self, command: GeometryCommand) -> Result<GeometryResponse, String> {
        self.refresh().await;
        let client = self.client.as_mut().ok_or("engine is not running")?;
        match client.geometry(command).await {
            Ok(response) => {
                if let GeometryResponse::Job { job } = &response
                    && self
                        .pending_project
                        .as_ref()
                        .is_some_and(|(id, _, _)| *id == job.job_id)
                {
                    let replacing = self.pending_project.as_ref().unwrap().2;
                    if job.status.is_terminal()
                        && let Some((_, attached, _)) = self.pending_project.take()
                        && project_job_attaches(job, replacing)
                    {
                        self.attached_project = Some(attached);
                        if replacing {
                            self.sources.clear();
                            self.project_paths.clear();
                        }
                    }
                }
                Ok(response)
            }
            Err(ClientError::Project(error)) => Ok(GeometryResponse::ProjectError { error }),
            Err(ClientError::Manufacturing(error)) => Err(error.to_string()),
            Err(error) if !error.is_fatal() => Err(error.to_string()),
            Err(error) => Err(self
                .disconnect(format!("geometry exchange failed: {error}"))
                .await),
        }
    }
    async fn project(&mut self, command: ProjectCommand) -> Result<ProjectResponse, String> {
        self.refresh().await;
        let attachment = match &command {
            ProjectCommand::Open {
                path,
                read_only,
                recover_previous,
                ..
            } => Some(AttachedProject {
                path: path.clone(),
                read_only: *read_only,
                recover_previous: *recover_previous,
            }),
            ProjectCommand::Save { target, .. } => target
                .clone()
                .map(|path| AttachedProject {
                    path,
                    read_only: false,
                    recover_previous: false,
                })
                .or_else(|| {
                    self.attached_project.clone().map(|mut attached| {
                        attached.recover_previous = false;
                        attached
                    })
                }),
            _ => None,
        };
        let is_new = matches!(command, ProjectCommand::New { .. });
        let replacing = matches!(command, ProjectCommand::Open { .. });
        let client = self.client.as_mut().ok_or("engine is not running")?;
        match client.project(command).await {
            Ok(response) => {
                if let ProjectResponse::JobAccepted { job_id } = &response {
                    if let Some(attachment) = attachment {
                        self.pending_project = Some((*job_id, attachment, replacing));
                    }
                } else if is_new && matches!(response, ProjectResponse::SceneChanged { .. }) {
                    self.attached_project = None;
                    self.sources.clear();
                    self.project_paths.clear();
                }
                Ok(response)
            }
            Err(ClientError::Project(error)) => Ok(ProjectResponse::Error { error }),
            Err(ClientError::Manufacturing(error)) => Err(error.to_string()),
            Err(error) if !error.is_fatal() => Err(error.to_string()),
            Err(error) => Err(self
                .disconnect(format!("project exchange failed: {error}"))
                .await),
        }
    }
    pub async fn dirty_for_close(&mut self) -> bool {
        self.refresh().await;
        let Some(client) = self.client.as_ref() else {
            return false;
        };
        let session_id = client.hello().session_id.clone();
        match self.project(ProjectCommand::Get { session_id }).await {
            Ok(ProjectResponse::Status { info }) => info.dirty,
            // Unknown status in a healthy session is not permission to lose edits.
            _ => self.client.is_some(),
        }
    }
    async fn refresh(&mut self) {
        let Some(client) = self.client.as_mut() else {
            return;
        };
        match client.status().await {
            Ok(true) => {}
            Ok(false) => {
                self.sources.clear();
                self.project_paths.clear();
                self.pending_project = None;
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
        self.sources.clear();
        self.project_paths.clear();
        self.pending_project = None;
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
        self.sources.clear();
        self.project_paths.clear();
        self.pending_project = None;
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
pub async fn engine_restart(
    state: State<'_, EngineState>,
    discard_changes: bool,
) -> Result<Hello, String> {
    let mut supervisor = state.active().await?;
    supervisor.refresh().await;
    if let Some(client) = supervisor.client.as_ref() {
        let session_id = client.hello().session_id.clone();
        if let ProjectResponse::Status { info } = supervisor
            .project(ProjectCommand::Get { session_id })
            .await?
            && info.dirty
            && !discard_changes
        {
            return Err("dirty_project: confirm discarding unsaved changes before restart".into());
        }
    }
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
    supervisor.sources.clear();
    supervisor.project_paths.clear();
    supervisor.pending_project = None;
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

struct SelectedSourceEntry {
    selection: SelectedSource,
    path: NativePath,
}

fn domain(code: GeometryErrorCode, message: &str) -> GeometryResponse {
    GeometryResponse::Error {
        error: GeometryError::new(code, message),
    }
}

fn selected_sources(
    paths: Vec<PathBuf>,
    session: SessionId,
) -> Result<Vec<SelectedSourceEntry>, String> {
    if paths.len() > 32 {
        return Err("resource_limit: select at most 32 files".into());
    }
    paths
        .into_iter()
        .map(|path| {
            let native =
                NativePath::from_os_str(path.as_os_str()).map_err(|error| error.to_string())?;
            let label = bounded_text(
                &path
                    .file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy(),
                MAX_DISPLAY_LABEL_BYTES as usize,
            );
            Ok(SelectedSourceEntry {
                selection: SelectedSource {
                    token: uuid::Uuid::new_v4().to_string(),
                    session_id: session.clone(),
                    label,
                },
                path: native,
            })
        })
        .collect()
}

async fn selection_session(state: &EngineState) -> Result<SessionId, String> {
    let mut supervisor = state.active().await?;
    supervisor.refresh().await;
    Ok(supervisor
        .client
        .as_ref()
        .ok_or("engine is not running")?
        .hello()
        .session_id
        .clone())
}

async fn install_selection(
    state: &EngineState,
    session: SessionId,
    paths: Vec<PathBuf>,
) -> Result<Vec<SelectedSource>, String> {
    let sources = selected_sources(paths, session.clone())?;
    let mut supervisor = state.active().await?;
    supervisor.refresh().await;
    if supervisor
        .client
        .as_ref()
        .map(|client| &client.hello().session_id)
        != Some(&session)
    {
        return Err("stale_revision: engine changed while selecting files".into());
    }
    let selections = sources
        .iter()
        .map(|source| source.selection.clone())
        .collect();
    supervisor.sources = sources;
    Ok(selections)
}

#[tauri::command]
pub async fn geometry_select_sources(
    app: tauri::AppHandle,
    state: State<'_, EngineState>,
) -> Result<Vec<SelectedSource>, String> {
    let session = selection_session(&state).await?;
    let (send, receive) = tokio::sync::oneshot::channel();
    // Native portal/dialog work is scheduled on the application thread, with no
    // engine mutex held. Stop/restart may proceed while the picker is open.
    app.run_on_main_thread(move || {
        let paths = rfd::FileDialog::new()
            .add_filter("STEP parts", &["step", "stp"])
            .pick_files();
        let _ = send.send(paths);
    })
    .map_err(|error| error.to_string())?;
    match receive
        .await
        .map_err(|_| "source picker closed without a result")?
    {
        None => Ok(Vec::new()), // Cancel preserves the previous unconsumed tokens.
        Some(paths) => install_selection(&state, session, paths).await,
    }
}

#[tauri::command]
pub async fn geometry_import_source(
    state: State<'_, EngineState>,
    token: String,
    session_id: SessionId,
    base_revision: SceneRevision,
    initial_pose: RigidPoseMm,
) -> Result<GeometryResponse, String> {
    let mut supervisor = state.active().await?;
    supervisor.refresh().await;
    let Some(index) = supervisor.sources.iter().position(|source| {
        source.selection.token == token && source.selection.session_id == session_id
    }) else {
        return Ok(domain(
            GeometryErrorCode::UnknownHandle,
            "unknown selected source; select the file again",
        ));
    };
    if supervisor
        .client
        .as_ref()
        .map(|client| &client.hello().session_id)
        != Some(&session_id)
    {
        return Ok(domain(
            GeometryErrorCode::StaleRevision,
            "selected source belongs to an old engine session",
        ));
    }
    let source = supervisor.sources[index].path.clone();
    let response = supervisor
        .geometry(GeometryCommand::ImportPart {
            session_id,
            base_revision,
            source,
            initial_pose,
        })
        .await?;
    if matches!(response, GeometryResponse::JobAccepted { .. }) {
        supervisor.sources.remove(index);
    }
    Ok(response)
}

#[tauri::command]
pub async fn geometry_control(
    state: State<'_, EngineState>,
    command: GeometryCommand,
) -> Result<GeometryResponse, String> {
    if matches!(
        command,
        GeometryCommand::ImportPart { .. } | GeometryCommand::ReadArtifactChunk { .. }
    ) {
        return Ok(domain(
            GeometryErrorCode::UnsupportedGeometry,
            "use token admission or the binary chunk bridge",
        ));
    }
    state.active().await?.geometry(command).await
}

#[tauri::command]
pub async fn geometry_chunk(
    state: State<'_, EngineState>,
    session_id: SessionId,
    artifact_id: ArtifactId,
    chunk_index: u32,
) -> Result<tauri::ipc::Response, String> {
    let mut supervisor = state.active().await?;
    supervisor.refresh().await;
    let client = supervisor.client.as_mut().ok_or("engine is not running")?;
    match client
        .read_geometry_chunk(&session_id, artifact_id, chunk_index)
        .await
    {
        Ok(bytes) => Ok(tauri::ipc::Response::new(bytes)),
        Err(error) if !error.is_fatal() => Err(error.to_string()),
        Err(error) => Err(supervisor
            .disconnect(format!("geometry chunk failed: {error}"))
            .await),
    }
}

#[tauri::command]
pub async fn geometry_debug_select_sources(
    state: State<'_, EngineState>,
    relative_paths: Vec<String>,
) -> Result<Vec<SelectedSource>, String> {
    let root = state
        .fixture_root
        .as_ref()
        .ok_or("geometry fixture selection is disabled")?;
    let session = selection_session(&state).await?;
    let paths = resolve_fixture_sources(root, relative_paths)?;
    install_selection(&state, session, paths).await
}

fn resolve_fixture_sources(
    root: &std::path::Path,
    relative_paths: Vec<String>,
) -> Result<Vec<PathBuf>, String> {
    if relative_paths.len() > 32 {
        return Err("resource_limit: select at most 32 files".into());
    }
    relative_paths
        .into_iter()
        .map(|relative| {
            let relative = std::path::Path::new(&relative);
            if relative.is_absolute()
                || relative.components().any(|part| {
                    !matches!(
                        part,
                        std::path::Component::Normal(_) | std::path::Component::CurDir
                    )
                })
            {
                return Err(
                    "fixture source must be relative and cannot escape the fixture root".to_owned(),
                );
            }
            let path = root
                .join(relative)
                .canonicalize()
                .map_err(|error| error.to_string())?;
            if !path.starts_with(root) || !path.is_file() {
                return Err(
                    "fixture source must be an existing regular file beneath the fixture root"
                        .into(),
                );
            }
            Ok(path)
        })
        .collect()
}

#[derive(Clone)]
struct AttachedProject {
    path: NativePath,
    read_only: bool,
    recover_previous: bool,
}

struct SelectedProjectEntry {
    selection: SelectedProjectPath,
    path: NativePath,
}

fn project_error(code: ProjectErrorCode, message: &str) -> ProjectResponse {
    ProjectResponse::Error {
        error: ProjectError::new(code, message),
    }
}

async fn install_project_path(
    state: &EngineState,
    session: SessionId,
    path: PathBuf,
    intent: ProjectPathIntent,
) -> Result<SelectedProjectPath, String> {
    let native = NativePath::from_os_str(path.as_os_str()).map_err(|error| error.to_string())?;
    let selection = SelectedProjectPath {
        token: uuid::Uuid::new_v4().to_string(),
        session_id: session.clone(),
        label: bounded_text(
            &path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy(),
            MAX_DISPLAY_LABEL_BYTES as usize,
        ),
        intent,
    };
    let mut supervisor = state.active().await?;
    supervisor.refresh().await;
    if supervisor
        .client
        .as_ref()
        .map(|client| &client.hello().session_id)
        != Some(&session)
    {
        return Err("stale_revision: engine changed during project selection".into());
    }
    // At most one unconsumed selection per intent; cancellation preserves it.
    supervisor
        .project_paths
        .retain(|entry| entry.selection.intent != intent);
    supervisor.project_paths.push(SelectedProjectEntry {
        selection: selection.clone(),
        path: native,
    });
    Ok(selection)
}

#[tauri::command]
pub async fn project_select_path(
    app: tauri::AppHandle,
    state: State<'_, EngineState>,
    intent: ProjectPathIntent,
) -> Result<Option<SelectedProjectPath>, String> {
    let session = selection_session(&state).await?;
    let (send, receive) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let dialog = rfd::FileDialog::new();
        let path = match intent {
            ProjectPathIntent::Open => dialog.set_title("Open project directory").pick_folder(),
            // Admit a new directory name; core creates it, never the shell.
            ProjectPathIntent::Save => dialog
                .set_title("Choose a new project directory name")
                .save_file(),
        };
        let _ = send.send(path);
    })
    .map_err(|error| error.to_string())?;
    match receive
        .await
        .map_err(|_| "project picker closed without a result")?
    {
        None => Ok(None),
        Some(path) => install_project_path(&state, session, path, intent)
            .await
            .map(Some),
    }
}

#[tauri::command]
pub async fn project_control(
    state: State<'_, EngineState>,
    command: ProjectCommand,
) -> Result<ProjectResponse, String> {
    if matches!(
        command,
        ProjectCommand::Open { .. } | ProjectCommand::Save { .. }
    ) {
        return Ok(project_error(
            ProjectErrorCode::InvalidProject,
            "use the token-only project open/save bridge",
        ));
    }
    state.active().await?.project(command).await
}

#[tauri::command]
pub async fn project_open(
    state: State<'_, EngineState>,
    token: String,
    session_id: SessionId,
    base_revision: SceneRevision,
    read_only: bool,
    recover_previous: bool,
    discard_changes: bool,
) -> Result<ProjectResponse, String> {
    let mut supervisor = state.active().await?;
    supervisor.refresh().await;
    let Some(index) = supervisor.project_paths.iter().position(|entry| {
        entry.selection.token == token
            && entry.selection.session_id == session_id
            && entry.selection.intent == ProjectPathIntent::Open
    }) else {
        return Ok(project_error(
            ProjectErrorCode::InvalidProject,
            "unknown or wrong-intent project token; select again",
        ));
    };
    let path = supervisor.project_paths[index].path.clone();
    let response = supervisor
        .project(ProjectCommand::Open {
            session_id,
            base_revision,
            path,
            read_only,
            recover_previous,
            discard_changes,
        })
        .await?;
    if matches!(response, ProjectResponse::JobAccepted { .. }) {
        supervisor.project_paths.remove(index);
    }
    Ok(response)
}

#[tauri::command]
pub async fn project_save(
    state: State<'_, EngineState>,
    token: Option<String>,
    session_id: SessionId,
    base_revision: SceneRevision,
) -> Result<ProjectResponse, String> {
    let mut supervisor = state.active().await?;
    supervisor.refresh().await;
    let index = if let Some(token) = token {
        let Some(index) = supervisor.project_paths.iter().position(|entry| {
            entry.selection.token == token
                && entry.selection.session_id == session_id
                && entry.selection.intent == ProjectPathIntent::Save
        }) else {
            return Ok(project_error(
                ProjectErrorCode::InvalidProject,
                "unknown or wrong-intent project token; select again",
            ));
        };
        Some(index)
    } else {
        None
    };
    let target = index.map(|index| supervisor.project_paths[index].path.clone());
    let response = supervisor
        .project(ProjectCommand::Save {
            session_id,
            base_revision,
            target,
        })
        .await?;
    if matches!(response, ProjectResponse::JobAccepted { .. })
        && let Some(index) = index
    {
        supervisor.project_paths.remove(index);
    }
    Ok(response)
}

#[tauri::command]
pub async fn project_reopen(
    state: State<'_, EngineState>,
    session_id: SessionId,
    base_revision: SceneRevision,
) -> Result<ProjectResponse, String> {
    let mut supervisor = state.active().await?;
    let Some(attached) = supervisor.attached_project.clone() else {
        return Ok(project_error(
            ProjectErrorCode::NoSavedPath,
            "no attached saved project to reopen",
        ));
    };
    supervisor
        .project(ProjectCommand::Open {
            session_id,
            base_revision,
            path: attached.path,
            read_only: attached.read_only,
            recover_previous: attached.recover_previous,
            discard_changes: false,
        })
        .await
}

#[tauri::command]
pub async fn project_debug_select_path(
    state: State<'_, EngineState>,
    relative_path: String,
    intent: ProjectPathIntent,
) -> Result<SelectedProjectPath, String> {
    let root = state
        .fixture_root
        .as_ref()
        .ok_or("project fixture selection is disabled")?;
    let path = resolve_fixture_project(root, &relative_path, intent)?;
    let session = selection_session(&state).await?;
    install_project_path(&state, session, path, intent).await
}

fn resolve_fixture_project(
    root: &std::path::Path,
    relative_path: &str,
    intent: ProjectPathIntent,
) -> Result<PathBuf, String> {
    let relative = std::path::Path::new(relative_path);
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err("project fixture path must be confined and relative".into());
    }
    let candidate = root.join(relative);
    let path = match intent {
        ProjectPathIntent::Open => candidate
            .canonicalize()
            .map_err(|error| error.to_string())?,
        ProjectPathIntent::Save => {
            match std::fs::symlink_metadata(&candidate) {
                Ok(_) => return Err("first save requires a new directory".into()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.to_string()),
            }
            let parent = candidate
                .parent()
                .ok_or("project path has no parent")?
                .canonicalize()
                .map_err(|error| error.to_string())?;
            if !parent.starts_with(root) {
                return Err("project path escapes fixture root".into());
            }
            parent.join(candidate.file_name().ok_or("project path has no name")?)
        }
    };
    if !path.starts_with(root) || (intent == ProjectPathIntent::Open && !path.is_dir()) {
        return Err("project fixture must resolve beneath the fixed root".into());
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn project_info(save_uncertain: bool) -> spiling_contracts::project::ProjectInfo {
        spiling_contracts::project::ProjectInfo {
            project_id: spiling_contracts::project::ProjectId::new(),
            revision: spiling_contracts::project::ProjectRevision(1),
            saved_revision: Some(spiling_contracts::project::ProjectRevision::ZERO),
            path_label: Some("checkpoint".into()),
            dirty: save_uncertain,
            save_uncertain,
            read_only: false,
            recovered_previous: false,
            can_undo: true,
            can_redo: false,
        }
    }

    fn job(status: JobStatus, result: Option<JobResult>) -> EngineJob {
        EngineJob {
            job_id: spiling_contracts::geometry::JobId::new(1).unwrap(),
            status,
            stage: "test receipt".into(),
            result,
            error: None,
        }
    }

    #[test]
    fn project_routes_require_the_current_jobs_open_or_save_receipt() {
        let scene = job(
            JobStatus::Completed,
            Some(JobResult::Scene {
                summary: spiling_contracts::geometry::SceneSummary {
                    session_id: SessionId::new(),
                    revision: SceneRevision::ZERO,
                    definition_count: 0,
                    occurrence_count: 0,
                    bounds_mm: None,
                    unique_mesh_bytes: 0,
                },
            }),
        );
        assert!(project_job_attaches(&scene, true));
        assert!(!project_job_attaches(&scene, false));

        let mut save = job(
            JobStatus::Completed,
            Some(JobResult::ProjectSaved {
                info: project_info(false),
            }),
        );
        assert!(project_job_attaches(&save, false));
        assert!(!project_job_attaches(&save, true));
        save.status = JobStatus::Failed;
        assert!(!project_job_attaches(&save, false));
        save.result = Some(JobResult::ProjectSaved {
            info: project_info(true),
        });
        assert!(project_job_attaches(&save, false));
        assert!(!project_job_attaches(&save, true));
        save.status = JobStatus::Cancelled;
        assert!(project_job_attaches(&save, false));
        save.status = JobStatus::Running;
        assert!(!project_job_attaches(&save, false));
        save.status = JobStatus::Failed;
        save.result = None;
        // Prior ProjectInfo uncertainty cannot replace this job's missing receipt.
        assert!(!project_job_attaches(&save, false));
    }

    #[test]
    fn common_manufacturing_jobs_cannot_attach_checkpoint_routes() {
        use spiling_contracts::{
            geometry::{JobError, SourceHash},
            manufacturing::{
                ManufacturingArtifactRecord, ManufacturingError, ManufacturingErrorCode,
                ManufacturingSummary, VerificationReport,
            },
        };

        let record = ManufacturingArtifactRecord {
            hash: SourceHash::from_bytes(b"{}"),
            input_hash: SourceHash::from_bytes(b"input"),
            byte_count: 2,
            summary: ManufacturingSummary {
                layers: 1,
                paths: 1,
                deposition_segments: 1,
                deposited_volume_mm3: 1.0,
                filament_length_mm: 1.0,
                software_only: true,
            },
        };
        let results = [
            JobResult::ManufacturingCompiled {
                info: project_info(false),
                record: record.clone(),
            },
            JobResult::ManufacturingVerified {
                record,
                report: VerificationReport {
                    verified: true,
                    coverage: vec!["software replay".into()],
                    limitations: vec!["no physical certification".into()],
                    deposition_segments: 1,
                    travel_segments: 1,
                    deposited_volume_mm3: 1.0,
                    filament_length_mm: 1.0,
                    max_position_error_mm: 0.0,
                    max_extrusion_error_mm: 0.0,
                },
            },
            JobResult::Section {
                artifact_id: ArtifactId::new(1).unwrap(),
            },
        ];
        for result in results {
            for status in [
                JobStatus::Completed,
                JobStatus::Failed,
                JobStatus::Cancelled,
            ] {
                let completed = job(status, Some(result.clone()));
                assert!(!project_job_attaches(&completed, false));
                assert!(!project_job_attaches(&completed, true));
            }
        }
        let mut failed = job(JobStatus::Failed, None);
        failed.error = Some(JobError::Manufacturing {
            error: ManufacturingError::new(
                ManufacturingErrorCode::VerificationFailed,
                "program replay failed",
            ),
        });
        assert!(!project_job_attaches(&failed, false));
        assert!(!project_job_attaches(&failed, true));
    }

    #[test]
    fn project_fixture_admission_confines_open_and_new_save_targets() {
        let directory =
            std::env::temp_dir().join(format!("spiling-project-gate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let root = directory.canonicalize().unwrap();
        std::fs::create_dir(root.join("existing")).unwrap();
        std::fs::write(root.join("file"), b"not a project directory").unwrap();
        assert_eq!(
            resolve_fixture_project(&root, "existing", ProjectPathIntent::Open).unwrap(),
            root.join("existing")
        );
        assert_eq!(
            resolve_fixture_project(&root, "new", ProjectPathIntent::Save).unwrap(),
            root.join("new")
        );
        assert!(!root.join("new").exists()); // admission never creates storage
        assert!(resolve_fixture_project(&root, "existing", ProjectPathIntent::Save).is_err());
        assert!(resolve_fixture_project(&root, "file", ProjectPathIntent::Open).is_err());
        assert!(resolve_fixture_project(&root, "missing", ProjectPathIntent::Open).is_err());
        assert!(resolve_fixture_project(&root, "missing/new", ProjectPathIntent::Save).is_err());
        for invalid in ["", ".", "../escape", "/absolute"] {
            for intent in [ProjectPathIntent::Open, ProjectPathIntent::Save] {
                assert!(resolve_fixture_project(&root, invalid, intent).is_err());
            }
        }
        #[cfg(unix)]
        {
            let outside = std::env::temp_dir()
                .join(format!("spiling-project-outside-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&outside).unwrap();
            std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();
            assert!(resolve_fixture_project(&root, "escape", ProjectPathIntent::Open).is_err());
            assert!(resolve_fixture_project(&root, "escape/new", ProjectPathIntent::Save).is_err());
            std::os::unix::fs::symlink(outside.join("missing"), root.join("dangling")).unwrap();
            assert!(resolve_fixture_project(&root, "dangling", ProjectPathIntent::Save).is_err());
            std::fs::remove_dir(outside).unwrap();
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn selection_is_bounded_and_tokens_bind_the_session() {
        let session = SessionId::new();
        let paths = vec![PathBuf::from("part.step"); 32];
        let selected = selected_sources(paths, session.clone()).unwrap();
        let mut tokens = std::collections::HashSet::new();
        for source in &selected {
            assert_eq!(source.selection.session_id, session);
            assert!(tokens.insert(source.selection.token.clone()));
            assert!(uuid::Uuid::parse_str(&source.selection.token).is_ok());
        }
        assert!(selected_sources(vec![PathBuf::from("part.step"); 33], session).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn source_tokens_preserve_native_bytes_not_lossy_labels() {
        use std::os::unix::ffi::OsStringExt;
        let path = PathBuf::from(std::ffi::OsString::from_vec(b"part-\xff.step".to_vec()));
        let selected = selected_sources(vec![path.clone()], SessionId::new()).unwrap();
        assert_eq!(
            selected[0].path,
            NativePath::from_os_str(path.as_os_str()).unwrap()
        );
        assert!(selected[0].selection.label.contains('\u{fffd}'));
        let long = PathBuf::from("é".repeat(150));
        let selected = selected_sources(vec![long], SessionId::new()).unwrap();
        assert!(selected[0].selection.label.len() <= MAX_DISPLAY_LABEL_BYTES as usize);
        assert_eq!(selected[0].selection.label.chars().count(), 128);
    }

    #[test]
    fn fixture_sources_require_canonical_regular_descendants() {
        let directory =
            std::env::temp_dir().join(format!("spiling-source-gate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let root = directory.canonicalize().unwrap();
        std::fs::write(root.join("part.step"), b"original test source").unwrap();
        let result = resolve_fixture_sources(&root, vec!["part.step".into()]).unwrap();
        assert_eq!(result, vec![root.join("part.step")]);
        assert!(resolve_fixture_sources(&root, vec!["../part.step".into()]).is_err());
        assert!(
            resolve_fixture_sources(
                &root,
                vec![root.join("part.step").to_string_lossy().into_owned()]
            )
            .is_err()
        );
        assert!(resolve_fixture_sources(&root, vec![".".into()]).is_err());
        assert!(resolve_fixture_sources(&root, vec!["missing.step".into()]).is_err());
        assert!(resolve_fixture_sources(&root, vec!["part.step".into(); 33]).is_err());
        #[cfg(unix)]
        {
            let outside =
                std::env::temp_dir().join(format!("spiling-outside-{}.step", uuid::Uuid::new_v4()));
            std::fs::write(&outside, b"outside").unwrap();
            std::os::unix::fs::symlink(&outside, root.join("escape.step")).unwrap();
            assert!(resolve_fixture_sources(&root, vec!["escape.step".into()]).is_err());
            std::fs::remove_file(outside).unwrap();
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
