// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Native admission and the sole frontend owner of EngineClient. No engine domain logic lives here.
use crate::manufacture;
use crate::scene::{Scene, Section};
use spiling_contracts::manufacturing::{ManufacturingCommand, ManufacturingResponse};
use spiling_contracts::{
    Hello, NativeOperationView,
    geometry::{
        GeometryCommand, GeometryResponse, JobError, JobResult, JobStatus, NativePath, RigidPoseMm,
        SceneSummary,
    },
    project::{ProjectCommand, ProjectId, ProjectInfo, ProjectResponse, ProjectRevision},
};
use spiling_engine_client::{ClientError, EngineClient};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    runtime::Runtime,
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};

const QUEUE_SLOTS: usize = 8;
const HISTORY_ROWS: usize = 8;
const PROBLEM_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Starting,
    Running,
    Interrupted,
    Closed,
}

#[derive(Debug, Clone)]
pub struct Confirmation {
    pub id: u64,
    pub action: &'static str,
}

#[derive(Debug, Clone)]
pub struct JobView {
    pub name: String,
    pub action: &'static str,
    pub stage: String,
    pub status: JobStatus,
    pub completed: u32,
    pub total: u32,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub generation: u64,
    pub serial: u64,
    pub phase: Phase,
    pub hello: Option<Arc<Hello>>,
    pub project: Option<Arc<ProjectInfo>>,
    pub scene: Option<SceneSummary>,
    pub display: Option<Arc<Scene>>,
    pub section: Option<Arc<Section>>,
    pub section_loading: bool,
    pub display_loading: bool,
    pub display_paused: bool,
    pub inspection: Option<Arc<spiling_contracts::geometry::FaceInspection>>,
    pub manufacturing: Arc<manufacture::State>,
    pub bed_proposal: Option<Arc<spiling_contracts::geometry::BedPosePreparation>>,
    pub busy: bool,
    pub active_operation: bool,
    pub picker_open: bool,
    pub confirmation: Option<Confirmation>,
    pub jobs: VecDeque<JobView>,
    pub problems: VecDeque<String>,
    pub message: String,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            generation: 0,
            serial: 0,
            phase: Phase::Starting,
            hello: None,
            project: None,
            scene: None,
            display: None,
            section: None,
            section_loading: false,
            display_loading: false,
            display_paused: false,
            inspection: None,
            manufacturing: Arc::default(),
            bed_proposal: None,
            busy: true,
            active_operation: false,
            picker_open: false,
            confirmation: None,
            jobs: VecDeque::new(),
            problems: VecDeque::new(),
            message: "Connecting to the native engine…".into(),
        }
    }
}

pub struct Config {
    pub engine: PathBuf,
    pub store: PathBuf,
}
impl Config {
    pub fn installed() -> Result<Self, String> {
        let engine = engine_path()?;
        let dirs = directories::ProjectDirs::from("org", "Spiling", "Spiling")
            .ok_or("Cannot locate application-local storage")?;
        Ok(Self {
            engine,
            store: dirs.data_local_dir().join("engine-operations"),
        })
    }
}
fn engine_path() -> Result<PathBuf, String> {
    if cfg!(debug_assertions)
        && let Some(path) = std::env::var_os("SPILING_ENGINE_PATH")
    {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err("SPILING_ENGINE_PATH must be absolute".into());
        }
        return Ok(path);
    }
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let name = if cfg!(windows) {
        "spiling-engine.exe"
    } else {
        "spiling-engine"
    };
    Ok(executable
        .parent()
        .ok_or("Executable has no parent")?
        .join(name))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    New,
    Save,
    Undo,
    Redo,
    Close,
    Restart,
    Cancel,
    Resume,
    DismissProblems,
    PauseDisplay,
    ResumeDisplay,
    CancelSection,
    CancelReview,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Picker {
    Open,
    OpenReadOnly,
    Recover,
    Save,
    Import,
    Intent,
    Export,
}
impl Picker {
    fn title(self) -> &'static str {
        match self {
            Self::Open => "Open Spiling project directory",
            Self::OpenReadOnly => "Open project read-only",
            Self::Recover => "Recover previous committed checkpoint",
            Self::Save => "Choose a new project directory name",
            Self::Import => "Import a native STEP part",
            Self::Intent => "Load inert manufacturing intent JSON",
            Self::Export => "Choose a new manufacturing export directory name",
        }
    }
}

/// Paths stay inside host actions. They are never turned into a lossy persistent string.
#[derive(Debug)]
enum Action {
    New,
    Open {
        path: NativePath,
        read_only: bool,
        recover_previous: bool,
    },
    Save {
        target: Option<NativePath>,
    },
    Import(NativePath),
    LoadIntent(NativePath),
    Export(NativePath),
    Undo,
    Redo,
    Close,
    Restart,
}
impl Action {
    fn label(&self) -> &'static str {
        match self {
            Self::New => "Create a new project",
            Self::Open { .. } => "Open another project",
            Self::Save { .. } => "Save project",
            Self::Import(_) => "Import part",
            Self::LoadIntent(_) => "Load manufacturing intent",
            Self::Export(_) => "Export verified program",
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::Close => "Close Spiling",
            Self::Restart => "Restart engine",
        }
    }
    fn replaces(&self) -> bool {
        matches!(
            self,
            Self::New | Self::Open { .. } | Self::Close | Self::Restart
        )
    }
}
enum Request {
    Geometry(Box<GeometryCommand>),
    Manufacturing(Box<ManufacturingCommand>),
    Project(Box<ProjectCommand>),
    Command(Command),
    #[cfg(test)]
    Action(Action),
    Confirm {
        id: u64,
        discard: bool,
    },
    PickerStarted(u64),
    PickerFinished {
        id: u64,
        generation: u64,
        action: Result<Option<Action>, String>,
    },
}

/// One bounded command queue and one coalesced immutable snapshot, with no progress backlog.
/// Each path is validated against the contract limit before admission; queued controls contain
/// at most one native path. Snapshots retain only eight job/problem rows and one checked project.
pub struct Host {
    runtime: Option<Runtime>,
    worker: Option<JoinHandle<()>>,
    sender: mpsc::Sender<Request>,
    receiver: watch::Receiver<Snapshot>,
    stop: Option<oneshot::Sender<()>>,
    next_picker: u64,
}
impl Host {
    pub fn new(config: Config, wake: impl Fn() + Send + Sync + 'static) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let (sender, requests) = mpsc::channel(QUEUE_SLOTS);
        let (updates, receiver) = watch::channel(Snapshot::default());
        let (stop, stopped) = oneshot::channel();
        let actor = Actor {
            config,
            updates,
            wake: Arc::new(wake),
            state: Snapshot::default(),
            client: None,
            pending: None,
            confirmation: None,
            route: None,
            deferred: None,
            dialog: None,
            nonce: 0,
            executing: false,
            section_task: None,
            section_cancel: Arc::new(AtomicBool::new(false)),
            review_task: None,
            export_task: None,
            display_task: None,
            display_cancel: Arc::new(AtomicBool::new(false)),
        };
        let worker = runtime.spawn(actor.run(requests, stopped));
        Ok(Self {
            runtime: Some(runtime),
            worker: Some(worker),
            sender,
            receiver,
            stop: Some(stop),
            next_picker: 0,
        })
    }
    pub fn snapshot(&self) -> Snapshot {
        self.receiver.borrow().clone()
    }
    fn send(&self, request: Request) -> Result<(), String> {
        self.sender
            .try_send(request)
            .map_err(|e| format!("Native host admission: {e}"))
    }
    pub fn geometry(&self, command: GeometryCommand) -> Result<(), String> {
        self.send(Request::Geometry(Box::new(command)))
    }
    pub fn manufacturing(&self, command: ManufacturingCommand) -> Result<(), String> {
        // The existing client checks the strict encoded intent limit before RPC admission.
        if let ManufacturingCommand::SetIntent { intent, .. } = &command {
            intent.validate().map_err(|e| e.to_string())?;
            if serde_json::to_vec(intent).map_err(|e| e.to_string())?.len()
                > spiling_contracts::manufacturing::MAX_PROFILE_JSON_BYTES as usize
            {
                return Err("Manufacturing intent exceeds the control limit".into());
            }
        }
        self.send(Request::Manufacturing(Box::new(command)))
    }
    pub fn project(&self, command: ProjectCommand) -> Result<(), String> {
        // Control slots have an additional byte bound; domain validity remains native authority.
        if serde_json::to_vec(&command)
            .map_err(|e| e.to_string())?
            .len()
            > 65536
        {
            return Err("Project edit exceeds the control limit".into());
        }
        self.send(Request::Project(Box::new(command)))
    }
    pub fn command(&self, command: Command) -> Result<(), String> {
        self.send(Request::Command(command))
    }
    pub fn confirm(&self, id: u64, discard: bool) -> Result<(), String> {
        self.send(Request::Confirm { id, discard })
    }

    /// Create the parented dialog on the window thread; await it on the existing host runtime.
    pub fn choose(
        &mut self,
        parent: &eframe::Frame,
        ctx: &egui::Context,
        picker: Picker,
    ) -> Result<(), String> {
        self.next_picker += 1;
        let id = self.next_picker;
        let generation = self.snapshot().generation;
        let dialog = rfd::AsyncFileDialog::new()
            .set_parent(parent)
            .set_title(picker.title());
        self.send(Request::PickerStarted(id))?;
        let sender = self.sender.clone();
        let wake = ctx.clone();
        let future: std::pin::Pin<
            Box<dyn std::future::Future<Output = Option<rfd::FileHandle>> + Send>,
        > = if matches!(picker, Picker::Import | Picker::Intent) {
            // Both branches return different opaque future types, so erase only this OS boundary.
            Box::pin(
                dialog
                    .add_filter(
                        if picker == Picker::Import {
                            "STEP"
                        } else {
                            "Manufacturing intent JSON"
                        },
                        if picker == Picker::Import {
                            &["step", "stp", "STEP", "STP"][..]
                        } else {
                            &["json"][..]
                        },
                    )
                    .pick_file(),
            )
                as std::pin::Pin<
                    Box<dyn std::future::Future<Output = Option<rfd::FileHandle>> + Send>,
                >
        } else if matches!(picker, Picker::Save | Picker::Export) {
            Box::pin(
                dialog
                    .set_file_name(if picker == Picker::Export {
                        "Spiling Export"
                    } else {
                        "Spiling Project"
                    })
                    .save_file(),
            )
        } else {
            Box::pin(dialog.pick_folder())
        };
        self.runtime
            .as_ref()
            .expect("host runtime")
            .spawn(async move {
                let action = future
                    .await
                    .map(|file| selected_action(picker, file.path()))
                    .transpose();
                let _ = sender
                    .send(Request::PickerFinished {
                        id,
                        generation,
                        action,
                    })
                    .await;
                wake.request_repaint();
            });
        Ok(())
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(runtime) = self.runtime.take() {
            // Normal window exit already awaited cleanup. Abnormal destruction still reaps.
            if let Some(worker) = self.worker.take() {
                let _ = runtime.block_on(worker);
            }
            runtime.shutdown_timeout(Duration::from_secs(1));
        }
    }
}
fn selected_action(picker: Picker, path: &std::path::Path) -> Result<Action, String> {
    let path = NativePath::from_os_str(path.as_os_str()).map_err(|e| e.to_string())?;
    Ok(match picker {
        Picker::Save => Action::Save { target: Some(path) },
        Picker::Import => Action::Import(path),
        Picker::Intent => Action::LoadIntent(path),
        Picker::Export => Action::Export(path),
        _ => Action::Open {
            path,
            read_only: picker == Picker::OpenReadOnly,
            recover_previous: picker == Picker::Recover,
        },
    })
}

#[derive(Clone)]
struct Route {
    path: NativePath,
    read_only: bool,
}
enum Receipt {
    Open(Route),
    Save(Option<Route>),
    Import,
    Geometry,
    Manufacturing,
    Project,
}
struct Pending {
    name: String,
    label: &'static str,
    receipt: Receipt,
    paused: bool,
}
struct GuardedAction {
    id: u64,
    project: ProjectId,
    revision: ProjectRevision,
    action: Action,
}
struct Actor {
    config: Config,
    updates: watch::Sender<Snapshot>,
    wake: Arc<dyn Fn() + Send + Sync>,
    state: Snapshot,
    client: Option<EngineClient>,
    pending: Option<Pending>,
    confirmation: Option<GuardedAction>,
    route: Option<Route>,
    deferred: Option<Action>,
    dialog: Option<u64>,
    nonce: u64,
    executing: bool,
    review_task: Option<JoinHandle<Result<Arc<manufacture::Review>, ClientError>>>,
    export_task: Option<JoinHandle<Result<manufacture::StagedExport, String>>>,
    section_task: Option<JoinHandle<Result<Arc<Section>, ClientError>>>,
    section_cancel: Arc<AtomicBool>,
    display_task: Option<JoinHandle<Result<Arc<Scene>, ClientError>>>,
    display_cancel: Arc<AtomicBool>,
}
impl Actor {
    fn publish(&mut self) {
        self.state.section_loading = self.section_task.is_some();
        self.state.display_loading = self.display_task.is_some();
        Arc::make_mut(&mut self.state.manufacturing).loading = self.review_task.is_some();
        Arc::make_mut(&mut self.state.manufacturing).export_busy = self.export_task.is_some();
        self.state.active_operation = self.pending.is_some();
        self.state.busy = self.executing
            || self.pending.is_some()
            || self.state.phase == Phase::Starting
            || self.deferred.is_some()
            || self.export_task.is_some();
        self.state.picker_open = self.dialog.is_some();
        self.updates.send_replace(self.state.clone());
        (self.wake)();
    }
    fn problem(&mut self, message: impl Into<String>) {
        let mut message = message.into();
        let end = message
            .char_indices()
            .map(|(i, _)| i)
            .take_while(|i| *i <= PROBLEM_BYTES)
            .last()
            .unwrap_or(0);
        if message.len() > PROBLEM_BYTES {
            message.truncate(end);
        }
        self.state.message = message.clone();
        if self.state.problems.back() != Some(&message) {
            self.state.problems.push_back(message);
        }
        while self.state.problems.len() > HISTORY_ROWS {
            self.state.problems.pop_front();
        }
    }
    async fn run(mut self, mut requests: mpsc::Receiver<Request>, mut stop: oneshot::Receiver<()>) {
        self.start().await;
        self.publish();
        let mut timer = tokio::time::interval(Duration::from_millis(200));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut observation: Option<JoinHandle<Result<NativeOperationView, ClientError>>> = None;
        loop {
            tokio::select! {
                biased;
                _ = &mut stop => break,
                request = requests.recv() => match request {
                    Some(request) => { self.executing = true; self.publish(); self.request(request).await; self.state.serial += 1; self.executing = false; },
                    None => break,
                },
                result = async { match &mut observation { Some(task) => task.await, None => std::future::pending().await } } => {
                    observation = None;
                    match result {
                        Ok(Ok(job)) => self.observe(job).await,
                        Ok(Err(error)) => { if let Some(pending) = &mut self.pending { pending.paused = true; } self.failure(error).await; }
                        Err(error) => self.problem(format!("Operation observer failed: {error}")),
                    }
                },
                result = async {match &mut self.review_task {Some(task)=>task.await,None=>std::future::pending().await}} => {
                    self.review_task=None;
                    match result {
                        Ok(Ok(review)) if self.capture_current(&review.capture) => {Arc::make_mut(&mut self.state.manufacturing).review=Some(review);},
                        Ok(Ok(_))=>{},Ok(Err(error))=>self.failure(error).await,Err(error)=>self.problem(format!("Review transfer failed: {error}")),
                    }
                },
                result = async {match &mut self.export_task {Some(task)=>task.await,None=>std::future::pending().await}} => {
                    self.export_task=None;
                    match result {
                        Ok(Ok(mut staged)) if self.capture_current(&staged.capture) => {
                            if let Some(client)=&self.client {
                                match manufacture::check_current(&client.rpc(),&staged.capture).await {
                                    Ok(())=>{staged.output.commit();Arc::make_mut(&mut self.state.manufacturing).last_export=Some(staged.label);self.state.message="Exported plan.json, program.gcode, verification.json and provenance.json".into();},
                                    Err(error)=>self.failure(error).await,
                                }
                            }
                        },
                        Ok(Ok(_))=>self.problem("Discarded stale export before commit"),Ok(Err(error))=>self.problem(error),Err(error)=>self.problem(format!("Export worker failed: {error}")),
                    }
                    if let Some(action)=self.deferred.take(){self.admit(action).await;}
                },
                result = async { match &mut self.display_task { Some(task)=> task.await,None=>std::future::pending().await } } => {
                    self.display_task=None;
                    match result {
                        Ok(Ok(scene)) if self.state.scene.as_ref().is_some_and(|s|scene.same_revision(s)) => { self.state.display=Some(scene);self.state.display_paused=false; },
                        Ok(Ok(_))=>{},
                        Ok(Err(error))=>{self.state.display_paused=true;self.failure(error).await;},
                        Err(error)=>{self.state.display_paused=true;self.problem(format!("Display transfer failed: {error}"));},
                    }
                },
                result=async {match &mut self.section_task {Some(task)=>task.await,None=>std::future::pending().await}}=>{
                    self.section_task=None;
                    match result {
                        Ok(Ok(section)) if self.state.scene.as_ref().is_some_and(|s|s.session_id==section.summary.session_id && s.revision==section.summary.revision)=>self.state.section=Some(section),
                        Ok(Ok(_))=>{},Ok(Err(error))=>self.failure(error).await,Err(error)=>self.problem(format!("Section transfer failed: {error}")),
                    }
                },
                _ = timer.tick() => {
                    self.check_exit().await;
                    if observation.is_none() && let (Some(pending), Some(client)) = (&self.pending, &self.client) && !pending.paused {
                        let rpc = client.rpc(); let name = pending.name.clone();
                        observation = Some(tokio::spawn(async move { rpc.native_operation(name).await }));
                    }
                    // Idle liveness checks do not repaint or re-download project/scene state.
                    continue;
                }
            }
            self.publish();
            if self.state.phase == Phase::Closed {
                break;
            }
        }
        if let Some(task) = observation {
            task.abort();
        }
        self.section_cancel.store(true, Ordering::Relaxed);
        self.cancel_display();
        self.cleanup().await;
    }
    fn cancel_display(&mut self) {
        self.display_cancel.store(true, Ordering::Relaxed);
        if let Some(task) = self.display_task.take() {
            task.abort();
        }
    }
    fn schedule_display(&mut self) {
        if self.display_task.is_some() || self.state.display_paused {
            return;
        }
        let (Some(client), Some(summary)) = (&self.client, &self.state.scene) else {
            return;
        };
        if self
            .state
            .display
            .as_ref()
            .is_some_and(|s| s.same_revision(summary))
        {
            return;
        }
        self.display_cancel = Arc::new(AtomicBool::new(false));
        self.display_task = Some(tokio::spawn(crate::scene::load(
            client.rpc(),
            summary.clone(),
            self.state.display.clone(),
            self.display_cancel.clone(),
            self.state
                .section
                .as_ref()
                .map_or(0, |s| s.summary.total_bytes as usize * 6)
                + self.state.manufacturing.display_bytes()
                + if self.section_task.is_some() {
                    spiling_contracts::geometry::MAX_SECTION_BYTES as usize * 6
                } else {
                    0
                },
        )));
    }
    async fn start(&mut self) {
        self.cancel_display();
        self.section_cancel.store(true, Ordering::Relaxed);
        self.state.section = None;
        self.state.display = None;
        self.state.inspection = None;
        self.state.display_paused = false;
        self.clear_review();
        self.state.manufacturing = Arc::default();
        self.state.bed_proposal = None;
        self.state.phase = Phase::Starting;
        self.state.generation += 1;
        self.state.hello = None;
        self.state.project = None;
        self.state.scene = None;
        match EngineClient::spawn_in(&self.config.engine, &self.config.store).await {
            Ok(client) => {
                eprintln!(
                    "Spiling native host: connected pid={} build={} session={:?}",
                    client.hello().pid,
                    client.hello().engine_build,
                    client.hello().session_id
                );
                self.state.hello = Some(Arc::new(client.hello().clone()));
                self.client = Some(client);
                self.state.phase = Phase::Running;
                self.state.message = "Native engine connected".into();
                if let Err(error) = self.refresh().await {
                    self.failure(error).await;
                }
            }
            Err(error) => {
                self.state.phase = Phase::Interrupted;
                self.problem(error.to_string());
            }
        }
    }
    async fn check_exit(&mut self) {
        let Some(client) = &mut self.client else {
            return;
        };
        match client.status().await {
            Ok(true) => {}
            Ok(false) => {
                let message = if self.route.is_some() {
                    "Native engine exited; unsaved changes are unavailable. Restart reopens the attached saved checkpoint."
                } else {
                    "Native engine exited; unsaved changes are unavailable. Restart starts a new project."
                };
                self.disconnect(message.into()).await;
                self.publish();
            }
            Err(error) => {
                self.failure(error).await;
                self.publish();
            }
        }
    }
    async fn cleanup(&mut self) {
        self.clear_review();
        // A staged transaction must finish and drop before the engine/session is released.
        if let Some(task) = self.export_task.take() {
            let _ = task.await;
        }

        if let Some(mut client) = self.client.take() {
            let pid = client.hello().pid;
            if let Err(error) = client.shutdown().await {
                self.problem(format!("Clean engine shutdown failed: {error}"));
                if let Err(error) = client.terminate().await {
                    self.problem(format!("Engine reap failed: {error}"));
                    return;
                }
            }
            eprintln!("Spiling native host: child {pid} exit awaited");
        }
    }
    async fn disconnect(&mut self, reason: String) {
        self.cancel_display();
        self.clear_review();
        self.state.bed_proposal = None;
        if let Some(mut client) = self.client.take() {
            let _ = client.terminate().await;
        }
        self.pending = None;
        self.confirmation = None;
        self.state.confirmation = None;
        self.deferred = None;
        self.state.generation += 1;
        self.state.phase = Phase::Interrupted;
        self.state.hello = None;
        self.problem(reason);
    }
    async fn failure(&mut self, error: ClientError) {
        if error.is_fatal() {
            self.disconnect(error.to_string()).await;
        } else {
            self.problem(error.to_string());
        }
    }
    async fn refresh(&mut self) -> Result<(), ClientError> {
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| ClientError::InvalidRequest("Engine is not connected".into()))?;
        let session_id = client.hello().session_id.clone();
        let rpc = client.rpc();
        let project = match rpc
            .project(ProjectCommand::Get {
                session_id: session_id.clone(),
            })
            .await?
        {
            ProjectResponse::Status { info } => info,
            _ => {
                return Err(ClientError::Protocol(
                    "GetProject returned an unexpected reply".into(),
                ));
            }
        };
        let scene = match rpc
            .geometry(GeometryCommand::GetScene {
                session_id: session_id.clone(),
            })
            .await?
        {
            GeometryResponse::Scene { summary } => summary,
            _ => {
                return Err(ClientError::Protocol(
                    "GetScene returned an unexpected reply".into(),
                ));
            }
        };
        if self.state.scene.as_ref().is_none_or(|s| s != &scene) {
            self.cancel_display();
            self.section_cancel.store(true, Ordering::Relaxed);
            self.state.section = None;
            self.state.inspection = None;
            self.state.display_paused = false;
        }
        let status = rpc
            .manufacturing(ManufacturingCommand::Get { session_id })
            .await?;
        let ManufacturingResponse::Status {
            info,
            intent,
            artifact,
            resource,
        } = status
        else {
            return Err(ClientError::Protocol(
                "Unexpected manufacturing status".into(),
            ));
        };
        if info.project_id != project.project_id || info.revision != project.revision {
            return Err(ClientError::InvalidRequest(
                "Project changed during status retrieval".into(),
            ));
        }
        let changed = self
            .state
            .manufacturing
            .revision
            .as_ref()
            .is_none_or(|(id, r)| *id != project.project_id || *r != project.revision)
            || self.state.manufacturing.artifact != artifact
            || self.state.manufacturing.resource != resource;
        if changed {
            self.clear_review();
            self.state.bed_proposal = None;
            Arc::make_mut(&mut self.state.manufacturing).preflight = None;
        }
        let m = Arc::make_mut(&mut self.state.manufacturing);
        if m.revision
            .as_ref()
            .is_some_and(|(id, _)| *id != project.project_id)
        {
            m.loaded = None;
        }
        m.revision = Some((project.project_id.clone(), project.revision));
        m.intent = intent;
        m.artifact = artifact;
        m.resource = resource;
        self.state.project = Some(Arc::new(project));
        self.state.scene = Some(scene);
        self.schedule_display();
        Ok(())
    }
    async fn request(&mut self, request: Request) {
        match request {
            Request::Manufacturing(command) => self.manufacturing(*command).await,
            Request::Project(command) => self.project_edit(*command).await,
            Request::Geometry(command) => {
                if matches!(*command, GeometryCommand::StartSection { .. })
                    && self.section_task.is_some()
                {
                    self.problem("Finish or cancel the current section transfer");
                    return;
                }
                if self.pending.is_some()
                    || self.export_task.is_some()
                    || self.dialog.is_some()
                    || self.confirmation.is_some()
                {
                    self.problem("Finish the active native action first");
                    return;
                }
                let Some(client) = &self.client else {
                    self.problem("Engine is disconnected");
                    return;
                };
                match client.geometry(*command).await {
                    Ok(GeometryResponse::FaceInspection { inspection }) => {
                        self.state.inspection = Some(Arc::new(inspection))
                    }
                    Ok(GeometryResponse::SceneChanged { .. }) => self.refresh_after_failure().await,
                    Ok(GeometryResponse::OperationAccepted { operation }) => {
                        self.accept(operation, Receipt::Geometry, "Native geometry")
                            .await
                    }
                    Ok(GeometryResponse::Error { error }) => self.problem(error.to_string()),
                    Ok(GeometryResponse::ProjectError { error }) => self.problem(error.to_string()),
                    Ok(_) => self.problem("Unexpected geometry control reply"),
                    Err(error) => self.failure(error).await,
                }
            }
            Request::Command(Command::CancelReview) => {
                self.clear_review();
                self.state.message = "Review transfer cancelled; verify again to reload".into();
            }
            Request::Command(Command::CancelSection) => {
                self.section_cancel.store(true, Ordering::Relaxed);
            }
            Request::Command(Command::PauseDisplay) => {
                self.display_cancel.store(true, Ordering::Relaxed);
                self.state.display_paused = true;
            }
            Request::Command(Command::ResumeDisplay) => {
                if self.display_task.is_none() {
                    self.state.display_paused = false;
                    self.schedule_display();
                }
            }
            Request::Command(Command::DismissProblems) => self.state.problems.clear(),
            Request::Command(Command::Cancel) => self.cancel().await,
            Request::Command(Command::Resume) => {
                if let Some(pending) = &mut self.pending {
                    pending.paused = false;
                    self.state.message = "Resuming operation observation".into();
                }
            }
            Request::Command(command) => {
                let action = match command {
                    Command::New => Action::New,
                    Command::Save => Action::Save { target: None },
                    Command::Undo => Action::Undo,
                    Command::Redo => Action::Redo,
                    Command::Close => Action::Close,
                    Command::Restart => Action::Restart,
                    _ => unreachable!(),
                };
                self.admit(action).await;
            }
            #[cfg(test)]
            Request::Action(action) => self.admit(action).await,
            Request::Confirm { id, discard } => {
                if self.confirmation.as_ref().is_some_and(|a| a.id == id) {
                    let guarded = self.confirmation.take().expect("checked confirmation");
                    self.state.confirmation = None;
                    if discard {
                        match self.refresh().await {
                            Ok(())
                                if self.state.project.as_ref().is_some_and(|p| {
                                    p.project_id == guarded.project
                                        && p.revision == guarded.revision
                                }) =>
                            {
                                self.execute(guarded.action, true).await
                            }
                            Ok(()) => self.admit(guarded.action).await,
                            Err(error) => self.failure(error).await,
                        }
                    } else {
                        self.state.message = "Kept the current project".into();
                    }
                }
            }
            Request::PickerStarted(id) => {
                if self.pending.is_none()
                    && self.export_task.is_none()
                    && self.dialog.is_none()
                    && self.confirmation.is_none()
                {
                    self.dialog = Some(id);
                } else {
                    self.problem("Cannot open a picker while native work is active");
                }
            }
            Request::PickerFinished {
                id,
                generation,
                action,
            } => {
                if self.dialog != Some(id) {
                    return;
                }
                self.dialog = None;
                if let Some(action) = self.deferred.take() {
                    self.admit(action).await;
                    return;
                }
                if self.state.generation != generation {
                    self.problem("Ignored a selection from a previous engine session");
                    return;
                }
                match action {
                    Ok(Some(action)) => self.admit(action).await,
                    Ok(None) => {
                        self.state.message = "Selection cancelled; project preserved".into()
                    }
                    Err(error) => self.problem(error),
                }
            }
        }
    }
    async fn cancel(&mut self) {
        if let (Some(pending), Some(client)) = (&mut self.pending, &self.client) {
            pending.paused = false;
            match client.rpc().cancel_operation(&pending.name).await {
                Ok(()) => {
                    self.state.message =
                        "Cancellation requested; awaiting native terminal outcome".into()
                }
                Err(error) => self.failure(error).await,
            }
        }
    }
    async fn admit(&mut self, action: Action) {
        if self.pending.is_some() || self.export_task.is_some() || self.dialog.is_some() {
            if matches!(action, Action::Close | Action::Restart) {
                self.deferred = Some(action);
                self.cancel().await;
            } else {
                self.problem(
                    "Wait for the current operation or picker before another project action",
                );
            }
            return;
        }
        if self.confirmation.is_some() {
            self.problem("Resolve the pending discard confirmation first");
            return;
        }
        if self.client.is_none() {
            match action {
                Action::Close => self.state.phase = Phase::Closed,
                Action::Restart => {
                    self.start().await;
                    self.reopen().await;
                }
                _ => self.problem("Native engine is not connected"),
            }
            return;
        }
        if let Err(error) = self.refresh().await {
            self.failure(error).await;
            return;
        }
        if action.replaces()
            && let Some(project) = &self.state.project
            && (project.dirty || project.save_uncertain)
        {
            self.nonce += 1;
            self.state.confirmation = Some(Confirmation {
                id: self.nonce,
                action: action.label(),
            });
            self.confirmation = Some(GuardedAction {
                id: self.nonce,
                project: project.project_id.clone(),
                revision: project.revision,
                action,
            });
            return;
        }
        self.execute(action, false).await;
    }
    async fn execute(&mut self, action: Action, discard: bool) {
        if matches!(action, Action::LoadIntent(_) | Action::Export(_)) {
            self.file_action(action).await;
            return;
        }
        if matches!(action, Action::Close | Action::Restart) {
            self.cleanup().await;
            if matches!(action, Action::Close) {
                self.state.phase = Phase::Closed;
            } else {
                self.start().await;
                self.reopen().await;
            }
            return;
        }
        let Some(client) = &self.client else {
            self.problem("Native engine is not connected");
            return;
        };
        let session_id = client.hello().session_id.clone();
        let Some(scene) = &self.state.scene else {
            self.problem("Current native scene is unavailable");
            return;
        };
        let base_revision = scene.revision;
        let label = action.label();
        let mut receipt = None;
        let command = match action {
            Action::New => ProjectCommand::New {
                session_id,
                base_revision,
                discard_changes: discard,
            },
            Action::Open {
                path,
                read_only,
                recover_previous,
            } => {
                receipt = Some(Receipt::Open(Route {
                    path: path.clone(),
                    read_only,
                }));
                ProjectCommand::Open {
                    session_id,
                    base_revision,
                    path,
                    read_only,
                    recover_previous,
                    discard_changes: discard,
                }
            }
            Action::Save { target } => {
                receipt = Some(Receipt::Save(
                    target
                        .clone()
                        .map(|path| Route {
                            path,
                            read_only: false,
                        })
                        .or_else(|| self.route.clone()),
                ));
                ProjectCommand::Save {
                    session_id,
                    base_revision,
                    target,
                }
            }
            Action::Undo => ProjectCommand::Undo {
                session_id,
                base_revision,
            },
            Action::Redo => ProjectCommand::Redo {
                session_id,
                base_revision,
            },
            Action::Import(source) => {
                let result = client
                    .geometry(GeometryCommand::ImportPart {
                        session_id,
                        base_revision,
                        source,
                        initial_pose: RigidPoseMm::IDENTITY,
                    })
                    .await;
                match result {
                    Ok(GeometryResponse::OperationAccepted { operation }) => {
                        self.accept(operation, Receipt::Import, label).await
                    }
                    Ok(_) => self.problem("Import returned an unexpected native reply"),
                    Err(error) => {
                        self.failure(error).await;
                        self.refresh_after_failure().await;
                    }
                }
                return;
            }
            Action::Close | Action::Restart | Action::LoadIntent(_) | Action::Export(_) => {
                unreachable!()
            }
        };
        let new = matches!(command, ProjectCommand::New { .. });
        match client.project(command).await {
            Ok(ProjectResponse::SceneChanged { info, .. } | ProjectResponse::Status { info }) => {
                if new {
                    self.route = None;
                }
                self.state.project = Some(Arc::new(info));
                self.state.message = format!("{label} completed");
                self.refresh_after_failure().await;
            }
            Ok(ProjectResponse::OperationAccepted { operation }) => {
                if let Some(receipt) = receipt {
                    self.accept(operation, receipt, label).await;
                } else {
                    self.disconnect("Unexpected accepted project action".into())
                        .await;
                }
            }
            Ok(ProjectResponse::Error { error }) => {
                self.problem(error.to_string());
                self.refresh_after_failure().await;
            }
            Err(error) => {
                self.failure(error).await;
                self.refresh_after_failure().await;
            }
        }
    }
    async fn reopen(&mut self) {
        if self.client.is_some()
            && let Some(route) = self.route.clone()
        {
            Box::pin(self.admit(Action::Open {
                path: route.path,
                read_only: route.read_only,
                recover_previous: false,
            }))
            .await;
        }
    }
    async fn refresh_after_failure(&mut self) {
        if self.client.is_some()
            && let Err(error) = self.refresh().await
        {
            self.failure(error).await;
        }
    }
    fn job(&mut self, job: &NativeOperationView, label: &'static str) {
        let view = JobView {
            name: job.name.clone(),
            action: label,
            stage: job.stage.clone(),
            status: job.status,
            completed: job.completed_units,
            total: job.total_units,
        };
        if let Some(row) = self.state.jobs.iter_mut().find(|row| row.name == job.name) {
            *row = view;
        } else {
            self.state.jobs.push_back(view);
        }
        while self.state.jobs.len() > HISTORY_ROWS {
            self.state.jobs.pop_front();
        }
    }
    async fn accept(
        &mut self,
        operation: NativeOperationView,
        receipt: Receipt,
        label: &'static str,
    ) {
        self.pending = Some(Pending {
            name: operation.name.clone(),
            receipt,
            label,
            paused: false,
        });
        self.state.message = format!("{label}: native operation accepted");
        self.observe(operation).await;
    }
    async fn observe(&mut self, operation: NativeOperationView) {
        let Some(pending) = &self.pending else {
            return;
        };
        if pending.name != operation.name
            || self
                .state
                .hello
                .as_ref()
                .is_none_or(|hello| hello.session_id != operation.session_id)
        {
            return;
        }
        let label = pending.label;
        self.job(&operation, label);
        if !operation.done {
            return;
        }
        let pending = self.pending.take().expect("terminal pending operation");
        let completed = operation.status == JobStatus::Completed;
        match (&pending.receipt, &operation.result) {
            (Receipt::Open(route), Some(JobResult::Scene { summary })) if completed => {
                self.route = Some(route.clone());
                let _ = summary;
            }
            (Receipt::Save(route), Some(JobResult::ProjectSaved { info }))
                if completed || info.save_uncertain =>
            {
                if let Some(route) = route {
                    self.route = Some(route.clone());
                }
                self.state.project = Some(Arc::new(info.clone()));
            }
            (Receipt::Manufacturing, Some(JobResult::ManufacturingPreflight { report }))
                if completed =>
            {
                Arc::make_mut(&mut self.state.manufacturing).preflight =
                    Some(Arc::new(report.clone()));
            }
            (Receipt::Manufacturing, Some(JobResult::ManufacturingCompiled { .. }))
                if completed => {}
            (
                Receipt::Manufacturing,
                Some(JobResult::ManufacturingVerified {
                    record,
                    report,
                    resource,
                    provenance,
                    replay_display,
                }),
            ) if completed => {
                self.verified(
                    record.clone(),
                    report.clone(),
                    resource.clone(),
                    provenance.clone(),
                    replay_display.clone(),
                )
                .await;
            }
            (Receipt::Geometry, Some(JobResult::BedPosePrepared { preparation })) if completed => {
                self.state.bed_proposal = Some(Arc::new(preparation.clone()));
            }
            (Receipt::Project, Some(JobResult::Scene { .. })) if completed => {}
            (Receipt::Geometry, Some(JobResult::Section { artifact_id })) if completed => {
                if self.section_task.is_some() {
                    self.problem("Finish the current section transfer first");
                } else if let (Some(client), Some(scene)) = (&self.client, &self.state.scene) {
                    self.section_cancel = Arc::new(AtomicBool::new(false));
                    self.section_task = Some(tokio::spawn(crate::scene::load_section(
                        client.rpc(),
                        scene.clone(),
                        *artifact_id,
                        self.section_cancel.clone(),
                        self.state.display.as_ref().map_or(0, |s| s.raw_bytes * 2)
                            + self.state.manufacturing.display_bytes()
                            + self
                                .state
                                .section
                                .as_ref()
                                .map_or(0, |s| s.summary.total_bytes as usize * 6)
                            + if self.review_task.is_some() {
                                80 * 1024 * 1024
                            } else {
                                0
                            },
                    )));
                }
            }
            (Receipt::Import | Receipt::Geometry, Some(JobResult::Scene { .. })) if completed => {}
            _ if completed => {
                self.disconnect(
                    "Native completion did not contain the required project receipt".into(),
                )
                .await;
                return;
            }
            _ => {}
        }
        if completed {
            self.state.message = format!("{label} completed");
        } else {
            let detail = operation
                .error
                .as_ref()
                .map(|error| match error {
                    JobError::Geometry { error } => error.to_string(),
                    JobError::Project { error } => error.to_string(),
                    JobError::Manufacturing { error } => error.to_string(),
                })
                .unwrap_or_else(|| "No native error detail was returned".into());
            self.problem(format!("{label} · {:?}: {detail}", operation.status));
        }
        self.refresh_after_failure().await;
        if let Some(action) = self.deferred.take() {
            Box::pin(self.admit(action)).await;
        }
    }
}

#[cfg(test)]
mod tests;

mod services;
