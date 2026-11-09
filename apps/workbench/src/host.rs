// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Native admission and the sole frontend owner of EngineClient. No engine domain logic lives here.
use crate::manufacture;
pub mod hardware;
pub mod library;
pub mod recipes;
pub mod stock;
use crate::scene::{Scene, Section};
pub use library::LibraryAction;
use spiling_contracts::manufacturing::{ManufacturingCommand, ManufacturingResponse};
use spiling_contracts::{
    Hello, NativeOperationView,
    geometry::{
        GeometryCommand, GeometryResponse, JobError, JobResult, JobStatus, NativePath, RigidPoseMm,
        SceneSummary,
    },
    project::{
        ProjectCommand, ProjectErrorCode, ProjectId, ProjectInfo, ProjectResponse, ProjectRevision,
    },
};
use spiling_engine_client::{ClientError, EngineClient, NativeOperationWatch};
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
pub struct PlateEditReceipt {
    pub nonce: u64,
    pub session: spiling_contracts::geometry::SessionId,
    pub project: spiling_contracts::project::ProjectId,
    pub base_revision: spiling_contracts::project::ProjectRevision,
    pub revision: spiling_contracts::project::ProjectRevision,
    pub edit: spiling_contracts::project::PrintLayoutEdit,
}
/// Observed synchronous Core process transaction, never a request acknowledgement.
#[derive(Debug, Clone)]
pub struct ProcessEditReceipt {
    pub nonce: u64,
    pub session: spiling_contracts::geometry::SessionId,
    pub project: ProjectId,
    pub base_revision: ProjectRevision,
    pub revision: ProjectRevision,
    pub edit: spiling_contracts::process::ProcessEdit,
}
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub analysis_model: Option<Arc<spiling_contracts::analysis::mechanics::AnalysisRunSettings>>,
    pub mechanics_previous: Option<Arc<crate::assets::MechanicsCapture>>,
    pub mechanics: Option<Arc<crate::assets::MechanicsCapture>>,
    pub library: Arc<library::State>,
    pub diagnostic: Option<Arc<spiling_contracts::OperationView>>,
    pub diagnostic_triangle: bool,
    pub generation: u64,
    pub serial: u64,
    pub phase: Phase,
    pub hello: Option<Arc<Hello>>,
    pub project: Option<Arc<ProjectInfo>>,
    pub scene: Option<SceneSummary>,
    pub layout_overview: Option<Arc<spiling_contracts::geometry::PrintLayoutOverview>>,
    pub display: Option<Arc<Scene>>,
    pub section: Option<Arc<Section>>,
    pub section_loading: bool,
    pub display_loading: bool,
    pub display_paused: bool,
    pub inspection: Option<Arc<spiling_contracts::geometry::FaceInspection>>,
    pub manufacturing: Arc<manufacture::State>,
    pub image_preview: Option<Arc<crate::assets::ImagePreview>>,
    pub image_loading: bool,
    pub optical_report: Option<Arc<crate::assets::OpticalCapture>>,
    pub optical_loading: bool,
    pub stock: stock::State,
    pub plate_edit: Option<Arc<PlateEditReceipt>>,
    pub process_edit: Option<Arc<ProcessEditReceipt>>,
    pub layout_proposal: Option<Arc<spiling_contracts::geometry::LayoutProposal>>,
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
            analysis_model: None,
            mechanics_previous: None,
            mechanics: None,
            library: Arc::default(),
            diagnostic: None,
            diagnostic_triangle: false,
            generation: 0,
            serial: 0,
            phase: Phase::Starting,
            hello: None,
            project: None,
            scene: None,
            layout_overview: None,
            display: None,
            section: None,
            section_loading: false,
            display_loading: false,
            display_paused: false,
            inspection: None,
            manufacturing: Arc::default(),
            image_preview: None,
            image_loading: false,
            optical_report: None,
            optical_loading: false,
            stock: stock::State::default(),
            bed_proposal: None,
            plate_edit: None,
            process_edit: None,
            layout_proposal: None,
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
    DiscardAlternative,
    PinAlternativeReference { nonce: u64 },
    Diagnostic,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Picker {
    AnalysisModel,
    Library,
    Open,
    OpenReadOnly,
    Recover,
    Save,
    Import,
    Intent,
    Material,
    MaterialCatalog,
    Export,
    Image,
}
impl Picker {
    fn title(self) -> &'static str {
        match self {
            Self::AnalysisModel => "Choose an explicit assumed-material analysis model",
            Self::Library => "Choose My library (.spil)",
            Self::Open => "Open Spiling project (.spil)",
            Self::OpenReadOnly => "Open Spiling project read-only (.spil)",
            Self::Recover => "Recover previous save from a Spiling project (.spil)",
            Self::Save => "Save Spiling project to a new .spil file",
            Self::Import => "Import a native STEP part",
            Self::Image => "Import height image PNG or canonical PGM",
            Self::Intent => "Load inert manufacturing intent JSON",
            Self::Material => "Select material and setup profile JSON",
            Self::MaterialCatalog => "Browse material catalog JSON",
            Self::Export => "Choose a new manufacturing export directory name",
        }
    }
}

/// Paths stay inside host actions. They are never turned into a lossy persistent string.
#[derive(Debug)]
enum Action {
    LoadAnalysisModel(NativePath),
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
    SetLibraryLocation(NativePath),
    LoadMaterial(NativePath),
    LoadMaterialCatalog(NativePath),
    ImportImage {
        source: NativePath,
        request: Box<crate::assets::ImageRequest>,
    },
    Export(NativePath),
    Undo,
    Redo,
    Close,
    Restart,
}
impl Action {
    fn label(&self) -> &'static str {
        match self {
            Self::LoadAnalysisModel(_) => "Load analysis model for review",
            Self::New => "Create a new project",
            Self::Open { .. } => "Open another project",
            Self::Save { .. } => "Save project",
            Self::Import(_) => "Import part",
            Self::ImportImage { .. } => "Import image and apply relief",
            Self::SetLibraryLocation(_) => "Choose My library",
            Self::LoadIntent(_) => "Load manufacturing intent",
            Self::LoadMaterial(_) => "Load material profile",
            Self::LoadMaterialCatalog(_) => "Load material catalog",
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
    ComputeSettings(spiling_contracts::compute::ComputeSettings),
    Library(Box<LibraryAction>),
    Geometry(Box<GeometryCommand>),
    StartupOpen(crate::launch::Open),
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
        Self::with_compute(config, Default::default(), wake)
    }

    pub fn with_compute(
        config: Config,
        compute_settings: spiling_contracts::compute::ComputeSettings,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            // Bounded debug-build frames for generated typed operations and nested restart/open.
            .thread_stack_size(8 * 1024 * 1024)
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let (sender, requests) = mpsc::channel(QUEUE_SLOTS);
        let (updates, receiver) = watch::channel(Snapshot::default());
        let (stop, stopped) = oneshot::channel();
        let actor = Actor {
            config,
            compute_settings,
            updates,
            wake: Arc::new(wake),
            state: Snapshot::default(),
            client: None,
            pending: None,
            confirmation: None,
            route: None,
            library_location: None,
            deferred: None,
            dialog: None,
            nonce: 0,
            executing: false,
            section_task: None,
            section_cancel: Arc::new(AtomicBool::new(false)),
            optical_task: None,
            mechanics_task: None,
            mechanics_restore_key: None,
            stock_task: None,
            diagnostic_task: None,
            diagnostic_paused: false,
            image_task: None,
            review_task: None,
            alternative_task: None,
            preserve_current_for_option_edit: false,
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
    pub fn open_from_launch(&self, open: crate::launch::Open) -> Result<(), String> {
        self.send(Request::StartupOpen(open))
    }
    pub fn geometry(&self, command: GeometryCommand) -> Result<(), String> {
        self.send(Request::Geometry(Box::new(command)))
    }
    pub fn manufacturing(&self, command: ManufacturingCommand) -> Result<(), String> {
        // Validate variable-sized controls before bounded host admission; the engine
        // independently validates them and owns mutation authority.
        if let ManufacturingCommand::SetIntent { intent, .. }
        | ManufacturingCommand::EvaluateAlternative { intent, .. } = &command
        {
            intent.validate().map_err(|e| e.to_string())?;
            if serde_json::to_vec(intent).map_err(|e| e.to_string())?.len()
                > spiling_contracts::manufacturing::MAX_PROFILE_JSON_BYTES as usize
            {
                return Err("Manufacturing intent exceeds the control limit".into());
            }
        }
        if let ManufacturingCommand::ApplyMaterialProfile { selection, .. } = &command {
            selection.validate().map_err(|e| e.to_string())?;
        }
        if let ManufacturingCommand::ReviewMaterials { request, .. } = &command {
            request.validate().map_err(|e| e.to_string())?;
        }
        if serde_json::to_vec(&command)
            .map_err(|e| e.to_string())?
            .len()
            > spiling_contracts::MAX_CONTROL_BYTES as usize
        {
            return Err("Manufacturing command exceeds the host control limit".into());
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
    pub fn compute_settings(
        &self,
        settings: spiling_contracts::compute::ComputeSettings,
    ) -> Result<(), String> {
        self.send(Request::ComputeSettings(settings))
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
        self.choose_native(parent, ctx, picker, None)
    }
    pub fn choose_image(
        &mut self,
        parent: &eframe::Frame,
        ctx: &egui::Context,
        request: crate::assets::ImageRequest,
    ) -> Result<(), String> {
        request.parameters.validate().map_err(|e| e.to_string())?;
        self.choose_native(parent, ctx, Picker::Image, Some(request))
    }
    fn choose_native(
        &mut self,
        parent: &eframe::Frame,
        ctx: &egui::Context,
        picker: Picker,
        image: Option<crate::assets::ImageRequest>,
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
        > = if matches!(
            picker,
            Picker::AnalysisModel
                | Picker::Import
                | Picker::Intent
                | Picker::Material
                | Picker::MaterialCatalog
                | Picker::Image
        ) {
            // Both branches return different opaque future types, so erase only this OS boundary.
            Box::pin(
                dialog
                    .add_filter(
                        if picker == Picker::AnalysisModel {
                            "Analysis model JSON"
                        } else if picker == Picker::Import {
                            "STEP"
                        } else if picker == Picker::Image {
                            "Height image PNG / PGM"
                        } else if picker == Picker::MaterialCatalog {
                            "Material catalog JSON"
                        } else if picker == Picker::Material {
                            "Material profile selection JSON"
                        } else {
                            "Manufacturing intent JSON"
                        },
                        if picker == Picker::Import {
                            &["step", "stp", "STEP", "STP"][..]
                        } else if picker == Picker::Image {
                            &["png", "pgm"][..]
                        } else {
                            &["json"][..]
                        },
                    )
                    .pick_file(),
            )
                as std::pin::Pin<
                    Box<dyn std::future::Future<Output = Option<rfd::FileHandle>> + Send>,
                >
        } else if picker == Picker::Save {
            Box::pin(
                dialog
                    .add_filter("Spiling project", &["spil"])
                    .set_file_name("Spiling Project.spil")
                    .save_file(),
            )
        } else if picker == Picker::Export {
            Box::pin(dialog.set_file_name("Spiling Export").save_file())
        } else {
            Box::pin(dialog.add_filter("Spiling project", &["spil"]).pick_file())
        };
        self.runtime
            .as_ref()
            .expect("host runtime")
            .spawn(async move {
                let action = future
                    .await
                    .map(|file| {
                        if let Some(request) = image {
                            NativePath::from_os_str(file.path().as_os_str())
                                .map(|source| Action::ImportImage {
                                    source,
                                    request: Box::new(request),
                                })
                                .map_err(|e| e.to_string())
                        } else {
                            selected_action(picker, file.path())
                        }
                    })
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
        Picker::AnalysisModel => Action::LoadAnalysisModel(path),
        Picker::Library => Action::SetLibraryLocation(path),
        Picker::Save => Action::Save { target: Some(path) },
        Picker::Import => Action::Import(path),
        Picker::Intent => Action::LoadIntent(path),
        Picker::Material => Action::LoadMaterial(path),
        Picker::MaterialCatalog => Action::LoadMaterialCatalog(path),
        Picker::Export => Action::Export(path),
        Picker::Image => {
            return Err("Height image selection requires captured mapping intent".into());
        }
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
    uncertain: bool,
}
enum Receipt {
    Open(Route),
    Save(Option<Route>),
    Import,
    Geometry,
    Manufacturing,
    Project,
}
type NativeObservation = (NativeOperationWatch, NativeOperationView);

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
    compute_settings: spiling_contracts::compute::ComputeSettings,
    updates: watch::Sender<Snapshot>,
    wake: Arc<dyn Fn() + Send + Sync>,
    state: Snapshot,
    client: Option<EngineClient>,
    pending: Option<Pending>,
    confirmation: Option<GuardedAction>,
    route: Option<Route>,
    library_location: Option<NativePath>,
    deferred: Option<Action>,
    dialog: Option<u64>,
    nonce: u64,
    executing: bool,
    optical_task: Option<JoinHandle<Result<Arc<crate::assets::OpticalCapture>, ClientError>>>,
    mechanics_task: Option<JoinHandle<Result<Arc<crate::assets::MechanicsCapture>, ClientError>>>,
    mechanics_restore_key: Option<(ProjectId, spiling_contracts::geometry::SourceHash)>,
    stock_task: Option<JoinHandle<Result<Arc<crate::assets::StockCapture>, ClientError>>>,
    diagnostic_task:
        Option<JoinHandle<Result<(spiling_contracts::OperationView, bool), ClientError>>>,
    diagnostic_paused: bool,
    image_task: Option<JoinHandle<Result<Arc<crate::assets::ImagePreview>, ClientError>>>,
    review_task: Option<JoinHandle<Result<Arc<manufacture::Review>, ClientError>>>,
    alternative_task: Option<JoinHandle<Result<Arc<manufacture::Review>, ClientError>>>,
    preserve_current_for_option_edit: bool,
    export_task: Option<JoinHandle<Result<manufacture::StagedExport, String>>>,
    section_task: Option<JoinHandle<Result<Arc<Section>, ClientError>>>,
    section_cancel: Arc<AtomicBool>,
    display_task: Option<JoinHandle<Result<Arc<Scene>, ClientError>>>,
    display_cancel: Arc<AtomicBool>,
}
impl Actor {
    fn publish(&mut self) {
        self.state.optical_loading = self.optical_task.is_some();
        self.state.stock.loading = self.stock_task.is_some();
        self.state.image_loading = self.image_task.is_some();
        self.state.section_loading = self.section_task.is_some();
        self.state.display_loading = self.display_task.is_some();
        Arc::make_mut(&mut self.state.manufacturing).loading = self.review_task.is_some();
        Arc::make_mut(&mut self.state.manufacturing).alternative_loading =
            self.alternative_task.is_some();
        Arc::make_mut(&mut self.state.manufacturing).export_busy = self.export_task.is_some();
        self.state.active_operation =
            self.pending.is_some() || self.state.diagnostic.as_ref().is_some_and(|d| !d.done);
        self.state.busy = self.executing
            || self.state.active_operation
            || self.mechanics_task.is_some()
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
        let mut observation_generation = self.state.generation;
        let mut watch: Option<NativeOperationWatch> = None;
        let mut observation: Option<JoinHandle<Result<NativeObservation, ClientError>>> = None;
        loop {
            tokio::select! {
                biased;
                _ = &mut stop => break,
                request = requests.recv() => match request {
                    Some(request) => { self.executing = true; self.publish(); Box::pin(self.request(request)).await; self.state.serial += 1; self.executing = false; },
                    None => break,
                },
                result = async { match &mut observation { Some(task) => task.await, None => std::future::pending().await } } => {
                    observation = None;
                    if observation_generation != self.state.generation {
                        watch = None;
                    } else {
                        match result {
                            Ok(Ok((next_watch, job))) => {
                                watch = Some(next_watch);
                                self.observe(job).await;
                            },
                            Ok(Err(error)) => { if let Some(pending) = &mut self.pending { pending.paused = true; } self.failure(error).await; }
                            Err(error) => { if let Some(pending) = &mut self.pending { pending.paused = true; } self.problem(format!("Operation observer failed: {error}")); },
                        }
                    }
                },
                result = async {
                    match &mut self.stock_task {
                        Some(task) => task.await,
                        None => std::future::pending().await,
                    }
                } => {
                    self.stock_task = None;
                    match result {
                        Ok(Ok(report)) if self.state.project.as_ref().is_some_and(|p|
                            p.project_id == report.project && p.revision == report.revision
                        ) => self.state.stock.capture = Some(report),
                        Ok(Ok(_)) => {},
                        Ok(Err(error)) => self.failure(error).await,
                        Err(error) => self.problem(error.to_string()),
                    }
                },
                result = async {
                    match &mut self.mechanics_task {
                        Some(task) => task.await,
                        None => std::future::pending().await,
                    }
                } => {
                    self.complete_mechanics(result).await;
                },
                result=async{match &mut self.optical_task{Some(task)=>task.await,None=>std::future::pending().await}}=>{
                    self.optical_task=None;
                    match result{Ok(Ok(report)) if self.state.project.as_ref().is_some_and(|p|p.project_id==report.project && p.revision==report.revision)=>self.state.optical_report=Some(report),Ok(Ok(_))=>{},Ok(Err(error))=>self.failure(error).await,Err(error)=>self.problem(error.to_string())}
                },
                result=async{match &mut self.diagnostic_task{Some(task)=>task.await,None=>std::future::pending().await}}=>{
                    self.diagnostic_task=None;
                    match result{
                        Ok(Ok((operation,triangle)))=>{let done=operation.done;self.state.diagnostic=Some(Arc::new(operation));self.state.diagnostic_triangle=triangle;if done && let Some(action)=self.deferred.take(){self.admit(action).await;}},
                        Ok(Err(error))=>{self.diagnostic_paused=true;self.failure(error).await;},Err(error)=>{self.diagnostic_paused=true;self.problem(error.to_string());},
                    }
                },
                result=async{match &mut self.image_task{Some(task)=>task.await,None=>std::future::pending().await}}=>{
                    self.image_task=None;
                    match result{
                        Ok(Ok(image)) if self.state.project.as_ref().is_some_and(|p|p.project_id==image.project && p.revision==image.revision) && self.state.hello.as_ref().is_some_and(|h|h.session_id==image.session)=>self.state.image_preview=Some(image),
                        Ok(Ok(_))=>{},Ok(Err(error))=>self.failure(error).await,Err(error)=>self.problem(error.to_string()),
                    }
                },
                result = async {match &mut self.review_task {Some(task)=>task.await,None=>std::future::pending().await}} => {
                    self.review_task=None;
                    match result {
                        Ok(Ok(review)) if self.capture_current(&review.capture) => {Arc::make_mut(&mut self.state.manufacturing).review=Some(review);},
                        Ok(Ok(_))=>{},Ok(Err(error))=>self.failure(error).await,Err(error)=>self.problem(format!("Review transfer failed: {error}")),
                    }
                },
                result = async {match &mut self.alternative_task {Some(task)=>task.await,None=>std::future::pending().await}} => {
                    self.alternative_task=None;
                    match result {
                        Ok(Ok(review)) if self.alternative_capture_matches(&review.capture) => {
                            Arc::make_mut(&mut self.state.manufacturing).alternative_review=Some(review);
                        },
                        Ok(Ok(_))=>{},Ok(Err(error))=>self.failure(error).await,
                        Err(error)=>self.problem(format!("Alternative transfer failed: {error}")),
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
                        Ok(Ok(scene)) if self.state.scene.as_ref().is_some_and(|s|scene.same_revision(s)) && self.state.layout_overview.as_ref().is_some_and(|o| scene.context.same_geometry(o)) => { self.state.display=Some(scene);self.state.display_paused=false; },
                        Ok(Ok(_))=>{},
                        Ok(Err(error))=>{self.state.display_paused=true;self.failure(error).await;},
                        Err(error)=>{self.state.display_paused=true;self.problem(format!("Display transfer failed: {error}"));},
                    }
                    self.schedule_display();
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
                    if self.diagnostic_task.is_none() && !self.diagnostic_paused && let (Some(operation),Some(client))=(&self.state.diagnostic,&self.client) && !operation.done{self.diagnostic_task=Some(tokio::spawn(crate::diagnostics::observe(client.rpc(),operation.name.clone())));}
                    // Idle liveness checks do not repaint or re-download project/scene state.
                    continue;
                }
            }
            self.publish();
            if self.state.phase == Phase::Closed {
                break;
            }
            if observation_generation != self.state.generation {
                if let Some(task) = observation.take() {
                    task.abort();
                    let _ = task.await;
                }
                watch = None;
            }
            if observation.is_none() {
                if let (Some(pending), Some(client)) = (&self.pending, &self.client)
                    && !pending.paused
                {
                    observation_generation = self.state.generation;
                    let rpc = client.rpc();
                    let name = pending.name.clone();
                    let previous = watch.take().filter(|w| w.name() == name);
                    // Observe immediately and keep the same bounded stream across progress.
                    // The timer is only a liveness fallback, never completion latency.
                    observation = Some(tokio::spawn(async move {
                        let mut watch = match previous {
                            Some(watch) => watch,
                            None => rpc.watch_native_operation(name).await?,
                        };
                        let job = watch.next().await?;
                        Ok((watch, job))
                    }));
                } else {
                    watch = None;
                }
            }
        }
        if let Some(task) = observation {
            task.abort();
            let _ = task.await;
        }
        drop(watch);
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
        let (Some(client), Some(summary), Some(overview)) =
            (&self.client, &self.state.scene, &self.state.layout_overview)
        else {
            return;
        };
        if self
            .state
            .display
            .as_ref()
            .is_some_and(|s| s.same_revision(summary) && s.context.same_geometry(overview))
        {
            return;
        }
        self.display_cancel = Arc::new(AtomicBool::new(false));
        self.display_task = Some(tokio::spawn(crate::scene::load(
            client.rpc(),
            summary.clone(),
            self.state.display.clone(),
            overview.clone(),
            self.display_cancel.clone(),
            self.state.section.as_ref().map_or(0, |s| {
                s.summary.total_bytes as usize * crate::scene::SECTION_DISPLAY_MULTIPLIER
            }) + self.state.manufacturing.display_bytes()
                + crate::scene::CONTEXT_DISPLAY_RESERVE_BYTES
                + if self.section_task.is_some() {
                    spiling_contracts::geometry::MAX_SECTION_BYTES as usize
                        * crate::scene::SECTION_DISPLAY_MULTIPLIER
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
        if let Some(task) = self.alternative_task.take() {
            task.abort();
            let _ = task.await;
        }
        let manufacturing = Arc::make_mut(&mut self.state.manufacturing);
        manufacturing.alternative = None;
        manufacturing.alternative_review = None;
        manufacturing.reference_review = None;
        manufacturing.inspected_option = None;
        self.state.image_preview = None;
        self.state.manufacturing = Arc::default();
        self.state.bed_proposal = None;
        self.state.layout_proposal = None;
        self.state.phase = Phase::Starting;
        self.state.generation += 1;
        self.state.hello = None;
        self.state.project = None;
        self.state.scene = None;
        // Restart must preserve the chosen private store even when it becomes unavailable.
        // Passing the exact location at launch avoids briefly selecting a different library.
        let startup = async {
            let mut command = tokio::process::Command::new(&self.config.engine);
            if let Some(location) = &self.library_location {
                command.arg("--library").arg(location.to_os_string()?);
            }
            EngineClient::spawn_command_with_compute(
                command,
                &self.config.store,
                self.compute_settings,
            )
            .await
        }
        .await;
        match startup {
            Ok(client) => {
                eprintln!(
                    "Spiling native host: connected pid={} build={} session={:?}",
                    client.hello().pid,
                    client.hello().engine_build,
                    client.hello().session_id
                );
                crate::performance::mark("engine_connected");
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
                let message = if self.route.as_ref().is_some_and(|route| route.uncertain)
                    || self
                        .pending
                        .as_ref()
                        .is_some_and(|pending| matches!(&pending.receipt, Receipt::Save(_)))
                {
                    "Native engine exited after an uncertain save. Restart starts a new session; explicitly inspect or reopen the attempted .spil target."
                } else if self.route.is_some() {
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
        self.mechanics_restore_key = None;
        if let Some(task) = self.mechanics_task.take() {
            task.abort();
            let _ = task.await;
        }
        if let Some(task) = self.optical_task.take() {
            task.abort();
            let _ = task.await;
        }
        if let Some(task) = self.stock_task.take() {
            task.abort();
            let _ = task.await;
        }
        self.state.stock = stock::State::default();
        self.state.optical_report = None;
        if let Some(task) = self.diagnostic_task.take() {
            task.abort();
            let _ = task.await;
        }
        self.state.diagnostic = None;
        self.state.diagnostic_triangle = false;
        if let Some(task) = self.image_task.take() {
            task.abort();
            let _ = task.await;
        }
        self.clear_review();
        // A staged transaction must finish and drop before the engine/session is released.
        if let Some(task) = self.alternative_task.take() {
            task.abort();
            let _ = task.await;
        }
        let manufacturing = Arc::make_mut(&mut self.state.manufacturing);
        manufacturing.alternative = None;
        manufacturing.alternative_review = None;
        manufacturing.reference_review = None;
        manufacturing.inspected_option = None;
        manufacturing.applied_settings = None;
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
        if let Some(pending) = self.pending.take() {
            self.retain_unresolved_save(pending.receipt);
        }
        self.cancel_display();
        self.clear_review();
        if let Some(task) = self.alternative_task.take() {
            task.abort();
            let _ = task.await;
        }
        self.state.bed_proposal = None;
        self.state.layout_proposal = None;
        if let Some(mut client) = self.client.take() {
            let _ = client.terminate().await;
        }
        if let Some(task) = self.diagnostic_task.take() {
            task.abort();
        }
        self.state.diagnostic = None;
        self.state.diagnostic_triangle = false;
        self.confirmation = None;
        self.state.confirmation = None;
        self.deferred = None;
        self.state.generation += 1;
        self.state.phase = Phase::Interrupted;
        self.state.hello = None;
        self.problem(reason);
    }
    fn retain_unresolved_save(&mut self, receipt: Receipt) {
        if let Receipt::Save(Some(mut route)) = receipt {
            route.uncertain = true;
            self.route = Some(route);
        }
    }
    async fn failure(&mut self, error: ClientError) {
        if error.is_fatal() {
            self.disconnect(error.to_string()).await;
        } else {
            self.problem(error.to_string());
        }
    }
    async fn refresh(&mut self) -> Result<(), ClientError> {
        let _opening = crate::performance::span("host_refresh");
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
        let GeometryResponse::LayoutOverview { overview } = rpc
            .geometry(GeometryCommand::GetLayoutOverview {
                session_id: session_id.clone(),
                scene_revision: scene.revision,
                project_revision: project.revision,
            })
            .await?
        else {
            return Err(ClientError::Protocol(
                "Unexpected plate overview reply".into(),
            ));
        };
        if overview.project_id != project.project_id
            || overview.session_id != session_id
            || overview.project_revision != project.revision
            || overview.scene_revision != scene.revision
            || overview.layouts.len() != project.layouts.len()
            || overview.layouts.iter().zip(&project.layouts).any(|(a, b)| {
                a.layout_id != b.layout_id || a.parts.len() != b.occurrence_count as usize
            })
        {
            return Err(ClientError::Protocol(
                "Plate overview does not match the captured project".into(),
            ));
        }
        if self.state.scene.as_ref().is_none_or(|s| s != &scene) {
            self.cancel_display();
            self.section_cancel.store(true, Ordering::Relaxed);
            self.state.section = None;
            self.state.inspection = None;
            self.state.display_paused = false;
        }
        if self
            .state
            .layout_overview
            .as_ref()
            .is_some_and(|old| !old.same_geometry(&overview))
        {
            self.cancel_display();
            self.state.display_paused = false;
        }
        let status = rpc
            .manufacturing(ManufacturingCommand::Get {
                session_id,
                layout_id: project.layout_id.clone(),
            })
            .await?;
        let ManufacturingResponse::Status {
            info,
            intent,
            artifact,
            resource,
            context_hash,
            interior_opportunity,
        } = status
        else {
            return Err(ClientError::Protocol(
                "Unexpected manufacturing status".into(),
            ));
        };
        if info.project_id != project.project_id
            || info.revision != project.revision
            || info.layout_id != project.layout_id
            || scene.layout_id != project.layout_id
        {
            return Err(ClientError::InvalidRequest(
                "Project changed during status retrieval".into(),
            ));
        }
        let layout_changed =
            self.state.manufacturing.layout_id.as_ref() != Some(&project.layout_id);
        let changed = layout_changed
            || self
                .state
                .manufacturing
                .revision
                .as_ref()
                .is_none_or(|(id, r)| *id != project.project_id || *r != project.revision)
            || self.state.manufacturing.artifact != artifact
            || self.state.manufacturing.resource != resource;
        if changed {
            let preserve = self.preserve_current_for_option_edit
                && self.state.manufacturing.context_hash.as_ref() == Some(&context_hash)
                && self.state.manufacturing.intent == intent
                && self.state.manufacturing.artifact == artifact
                && self.state.manufacturing.resource == resource;
            if preserve {
                let m = Arc::make_mut(&mut self.state.manufacturing);
                if let Some(capture) = &m.capture {
                    let mut capture = (**capture).clone();
                    capture.observed_revision = project.revision;
                    let capture = Arc::new(capture);
                    if let Some(review) = &m.review {
                        let mut review = (**review).clone();
                        review.capture = capture.clone();
                        m.review = Some(Arc::new(review));
                    }
                    m.capture = Some(capture);
                }
            } else {
                self.clear_review();
            }
            self.state.bed_proposal = None;
            self.state.layout_proposal = None;
            if self
                .state
                .optical_report
                .as_ref()
                .is_some_and(|report| !report.is_current(&project))
            {
                self.state.optical_report = None;
            }
            Arc::make_mut(&mut self.state.manufacturing).preflight = None;
        }
        let m = Arc::make_mut(&mut self.state.manufacturing);
        m.context_hash = Some(context_hash);
        m.interior_opportunity = interior_opportunity;
        if layout_changed
            || m.revision
                .as_ref()
                .is_some_and(|(id, _)| *id != project.project_id)
        {
            m.loaded = None;
            m.loaded_material = None;
            m.loaded_catalog = None;
            m.alternative = None;
            m.alternative_review = None;
            m.reference_review = None;
            m.inspected_option = None;
            m.applied_settings = None;
            if let Some(task) = self.alternative_task.take() {
                task.abort();
            }
        }
        m.layout_id = Some(project.layout_id.clone());
        m.revision = Some((project.project_id.clone(), project.revision));
        m.intent = intent;
        m.artifact = artifact;
        m.resource = resource;
        self.state.project = Some(Arc::new(project));
        self.state.scene = Some(scene);
        self.state.layout_overview = Some(Arc::new(overview));
        self.schedule_display();
        self.restore_mechanics();
        if self
            .state
            .project
            .as_ref()
            .is_some_and(|p| p.hardware.is_some())
            && let Err(e) = self.refresh_hardware().await
        {
            let state = Arc::make_mut(&mut self.state.library);
            state.hardware.error = Some(e.to_string());
            state.hardware.personal = None;
            state.hardware.local_selection = None;
        }
        Ok(())
    }
    fn restore_mechanics(&mut self) {
        if self.pending.is_none()
            && let (Some(client), Some(project)) = (&self.client, &self.state.project)
            && let Some(record) = project.analysis.results.iter().rev().find(|result| {
                project.analysis.cases.iter().any(|case| {
                    case.case.id == result.case_id && case.case.layout_id == project.layout_id
                })
            })
            && self.mechanics_task.is_none()
            && self.mechanics_restore_key.as_ref()
                != Some(&(project.project_id.clone(), record.hash.clone()))
            && self.state.mechanics.as_ref().is_none_or(|capture| {
                capture.project != project.project_id
                    || capture.summary.case.case.layout_id != project.layout_id
            })
        {
            self.mechanics_task = Some(tokio::spawn(crate::assets::restore_mechanics(
                client.rpc(),
                client.hello().session_id.clone(),
                project.project_id.clone(),
                project.revision,
                record.clone(),
            )));
        }
    }
    async fn complete_mechanics(
        &mut self,
        result: Result<
            Result<Arc<crate::assets::MechanicsCapture>, ClientError>,
            tokio::task::JoinError,
        >,
    ) {
        self.mechanics_task = None;
        self.mechanics_restore_key = None;
        let retry = match result {
            Ok(Ok(report))
                if self.state.project.as_ref().is_some_and(|project| {
                    project.project_id == report.project && project.revision == report.revision
                }) =>
            {
                self.mechanics_restore_key = self.state.project.as_ref().and_then(|project| {
                    project
                        .analysis
                        .results
                        .iter()
                        .find(|record| record.hash.as_str() == report.resource.sha256)
                        .map(|record| (project.project_id.clone(), record.hash.clone()))
                });
                self.state.mechanics_previous = self.state.mechanics.clone().filter(|previous| {
                    previous.project == report.project
                        && previous.summary.source_hash == report.summary.source_hash
                        && previous.summary.case.case.id == report.summary.case.case.id
                        && previous.summary.case.case.loads == report.summary.case.case.loads
                        && previous.summary.case.case.restraints
                            == report.summary.case.case.restraints
                        && (previous.summary.pose != report.summary.pose
                            || previous.summary.settings != report.summary.settings)
                });
                self.state.mechanics = Some(report);
                false
            }
            Ok(Ok(_)) => true,
            Ok(Err(ClientError::Project(error)))
                if error.code == ProjectErrorCode::StaleRevision =>
            {
                true
            }
            Ok(Err(ClientError::Geometry(error)))
                if error.code == spiling_contracts::geometry::GeometryErrorCode::StaleRevision =>
            {
                true
            }
            Ok(Err(error)) => {
                self.failure(error).await;
                false
            }
            Err(error) => {
                self.problem(error.to_string());
                false
            }
        };
        if retry {
            self.refresh_after_failure().await;
        }
        if let Some(action) = self.deferred.take() {
            self.admit(action).await;
        }
    }
    async fn request(&mut self, request: Request) {
        match request {
            Request::ComputeSettings(settings) => self.compute_settings = settings,
            Request::StartupOpen(open) => {
                self.admit(Action::Open {
                    path: open.path,
                    read_only: open.read_only,
                    recover_previous: false,
                })
                .await
            }
            Request::Manufacturing(command) => self.manufacturing(*command).await,
            Request::Library(action) => self.library_action(*action).await,
            Request::Project(command) => {
                self.project_edit(*command).await;
            }
            Request::Geometry(command) => {
                if matches!(*command, GeometryCommand::StartSection { .. })
                    && self.section_task.is_some()
                {
                    self.problem("Finish or cancel the current section transfer");
                    return;
                }
                if self.pending.is_some()
                    || self.mechanics_task.is_some()
                    || self.state.diagnostic.as_ref().is_some_and(|d| !d.done)
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
            Request::Command(Command::Diagnostic) => {
                if self.mechanics_task.is_some()
                    || self.export_task.is_some()
                    || self.dialog.is_some()
                    || self.confirmation.is_some()
                    || self.pending.is_some()
                    || self.state.diagnostic.as_ref().is_some_and(|d| !d.done)
                {
                    self.problem("Finish the active action before the diagnostic");
                    return;
                }
                if let Some(client) = &self.client {
                    match client
                        .rpc()
                        .run_diagnostic(spiling_contracts::rpc::RunDiagnosticRequest {
                            parent: "diagnostics/workbench".into(),
                            request_id: String::new(),
                            chunk_count: 4,
                            delay_ms: 250,
                            chunk_bytes: 64,
                            input_revision: "explicit-unitless-diagnostic".into(),
                        })
                        .await
                    {
                        Ok(operation) => match spiling_contracts::operation_view(&operation) {
                            Ok(view) => {
                                self.state.diagnostic = Some(Arc::new(view));
                                self.state.diagnostic_triangle = false;
                                self.diagnostic_paused = false;
                            }
                            Err(error) => self.problem(error),
                        },
                        Err(error) => self.failure(error).await,
                    }
                }
            }
            Request::Command(Command::CancelReview) => {
                self.clear_review();
                self.state.message = "Review transfer cancelled; verify again to reload".into();
            }
            Request::Command(Command::DiscardAlternative) => {
                if self.pending.is_some() {
                    self.problem(
                        "Resolve the alternative operation before discarding its inspection",
                    );
                } else {
                    if let Some(task) = self.alternative_task.take() {
                        task.abort();
                    }
                    let m = Arc::make_mut(&mut self.state.manufacturing);
                    m.retired_display_bytes = m.display_bytes();
                    m.alternative = None;
                    m.alternative_review = None;
                    m.reference_review = None;
                    self.state.message = "Alternative inspection closed".into();
                }
            }
            Request::Command(Command::CancelSection) => {
                self.section_cancel.store(true, Ordering::Relaxed);
            }
            Request::Command(Command::PinAlternativeReference { nonce }) => {
                let reference = self.state.manufacturing.review.clone().filter(|review| {
                    review.capture.nonce == nonce && self.capture_current(&review.capture)
                });
                if let Some(reference) = reference {
                    Arc::make_mut(&mut self.state.manufacturing).reference_review = Some(reference);
                } else {
                    self.problem("Current reference changed before reservation. End this trial, freshly verify Current and start again.");
                }
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
                self.diagnostic_paused = false;
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
        if self.pending.is_none()
            && let (Some(operation), Some(client)) = (&self.state.diagnostic, &self.client)
            && !operation.done
        {
            match client.rpc().cancel_operation(&operation.name).await {
                Ok(()) => {
                    self.diagnostic_paused = false;
                    self.state.message =
                        "Diagnostic cancellation requested; awaiting terminal receipt".into();
                }
                Err(error) => self.failure(error).await,
            }
            return;
        }
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
        if self.pending.is_some()
            || self.mechanics_task.is_some()
            || self.state.diagnostic.as_ref().is_some_and(|d| !d.done)
            || self.export_task.is_some()
            || self.dialog.is_some()
        {
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
        if let Action::ImportImage { source, request } = action {
            self.project_edit(ProjectCommand::ImportSurfaceImageFinish {
                session_id: request.session,
                base_revision: request.revision,
                reference: request.reference,
                scope: request.scope,
                source,
                parameters: request.parameters,
            })
            .await;
            return;
        }
        if matches!(
            action,
            Action::LoadAnalysisModel(_)
                | Action::LoadIntent(_)
                | Action::LoadMaterial(_)
                | Action::SetLibraryLocation(_)
                | Action::LoadMaterialCatalog(_)
                | Action::Export(_)
        ) {
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
                crate::performance::mark("open_requested");
                receipt = Some(Receipt::Open(Route {
                    path: path.clone(),
                    read_only,
                    uncertain: false,
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
                            uncertain: false,
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
            Action::Close
            | Action::Restart
            | Action::LoadAnalysisModel(_)
            | Action::LoadIntent(_)
            | Action::LoadMaterial(_)
            | Action::SetLibraryLocation(_)
            | Action::LoadMaterialCatalog(_)
            | Action::Export(_)
            | Action::ImportImage { .. } => {
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
            Ok(
                ProjectResponse::Filament { .. }
                | ProjectResponse::Filaments { .. }
                | ProjectResponse::Recipe { .. }
                | ProjectResponse::Recipes { .. }
                | ProjectResponse::ManufacturingOption { .. },
            ) => self.problem("Unexpected material response to project action"),
            Ok(ProjectResponse::Error { error }) => {
                self.problem(error.to_string());
                self.refresh_after_failure().await;
            }
            Err(error) => {
                if error.is_fatal()
                    && let Some(receipt) = receipt
                {
                    self.retain_unresolved_save(receipt);
                }
                self.failure(error).await;
                self.refresh_after_failure().await;
            }
        }
    }
    async fn reopen(&mut self) {
        if self.route.as_ref().is_some_and(|route| route.uncertain) {
            self.problem(
                "The previous save outcome is uncertain; explicitly inspect or reopen the attempted .spil target",
            );
            return;
        }
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
                self.state.analysis_model = None;
                self.mechanics_restore_key = None;
                self.state.mechanics = None;
                self.state.mechanics_previous = None;
                if let Some(task) = self.mechanics_task.take() {
                    task.abort();
                }
                crate::performance::mark("open_published");
                self.route = Some(route.clone());
                Arc::make_mut(&mut self.state.library)
                    .hardware
                    .local_selection = None;
                let _ = summary;
            }
            (Receipt::Save(route), Some(JobResult::ProjectSaved { info })) if completed => {
                if let Some(route) = route {
                    let mut route = route.clone();
                    route.uncertain = false;
                    self.route = Some(route);
                }
                self.state.project = Some(Arc::new(info.clone()));
                self.remember_hardware_selection().await;
            }
            (Receipt::Save(route), Some(JobResult::ProjectSaveUncertain { info }))
                if operation.status == JobStatus::Failed && info.save_uncertain =>
            {
                if let Some(route) = route {
                    let mut route = route.clone();
                    route.uncertain = true;
                    self.route = Some(route);
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
                Some(JobResult::ManufacturingAlternativeEvaluated { evaluation }),
            ) if completed => {
                self.alternative_evaluated(evaluation.clone()).await;
            }
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
            (Receipt::Geometry, Some(JobResult::LayoutPrepared { proposal })) if completed => {
                self.state.layout_proposal = Some(Arc::new(proposal.clone()));
            }
            (Receipt::Geometry, Some(JobResult::BedPosePrepared { preparation })) if completed => {
                self.state.bed_proposal = Some(Arc::new(preparation.clone()));
            }
            (
                Receipt::Project,
                Some(
                    JobResult::SurfaceImageImported { .. }
                    | JobResult::AppearancePaletteImported { .. }
                    | JobResult::OpticalConstructionApplied { .. },
                ),
            ) if completed => {}
            (
                Receipt::Project,
                Some(JobResult::AnalysisResultPrepared {
                    project_id,
                    read_revision,
                    record,
                    resource,
                }),
            ) if completed => {
                self.refresh_after_failure().await;
                if let Some(task) = self.mechanics_task.take() {
                    task.abort();
                }
                if let (Some(client), Some(project)) = (&self.client, &self.state.project)
                    && project.project_id == *project_id
                    && project.revision == *read_revision
                {
                    self.mechanics_task = Some(tokio::spawn(crate::assets::load_mechanics(
                        client.rpc(),
                        client.hello().session_id.clone(),
                        project_id.clone(),
                        *read_revision,
                        record.captured_revision,
                        record.input_hash.clone(),
                        resource.clone(),
                    )));
                }
            }
            (
                Receipt::Project,
                Some(JobResult::AnalysisCompleted {
                    project_id,
                    captured_revision,
                    input_hash,
                    resource,
                }),
            ) if completed => {
                self.refresh_after_failure().await;
                if let Some(task) = self.mechanics_task.take() {
                    task.abort();
                }
                if let (Some(client), Some(project)) = (&self.client, &self.state.project)
                    && project.project_id == *project_id
                {
                    self.mechanics_task = Some(tokio::spawn(crate::assets::load_mechanics(
                        client.rpc(),
                        client.hello().session_id.clone(),
                        project_id.clone(),
                        project.revision,
                        *captured_revision,
                        input_hash.clone(),
                        resource.clone(),
                    )));
                }
            }
            (
                Receipt::Project,
                Some(JobResult::StockMatched {
                    project_id,
                    captured_revision,
                    record,
                    resource,
                }),
            ) if completed => {
                self.refresh_after_failure().await;
                if let Some(task) = self.stock_task.take() {
                    task.abort();
                }
                if let (Some(client), Some(project)) = (&self.client, &self.state.project)
                    && project.project_id == *project_id
                    && project.revision == *captured_revision
                {
                    self.stock_task = Some(tokio::spawn(crate::assets::load_stock(
                        client.rpc(),
                        client.hello().session_id.clone(),
                        project_id.clone(),
                        *captured_revision,
                        record.clone(),
                        resource.clone(),
                    )));
                }
            }
            (
                Receipt::Project,
                Some(JobResult::OpticalCompared {
                    project_id,
                    captured_revision,
                    record,
                    resource,
                }),
            ) if completed => {
                self.refresh_after_failure().await;
                if let Some(task) = self.optical_task.take() {
                    task.abort();
                }
                if let (Some(client), Some(project)) = (&self.client, &self.state.project)
                    && project.project_id == *project_id
                    && project.revision == *captured_revision
                {
                    self.optical_task = Some(tokio::spawn(crate::assets::load_optical(
                        client.rpc(),
                        client.hello().session_id.clone(),
                        project_id.clone(),
                        project.revision,
                        record.clone(),
                        resource.clone(),
                    )));
                }
            }
            (
                Receipt::Project,
                Some(JobResult::SurfaceImagePrepared {
                    project_id,
                    captured_revision,
                    record,
                    resource,
                }),
            ) if completed => {
                if let Some(task) = self.image_task.take() {
                    task.abort();
                }
                if let Some(client) = &self.client {
                    self.image_task = Some(tokio::spawn(crate::assets::load_image(
                        client.rpc(),
                        client.hello().session_id.clone(),
                        project_id.clone(),
                        *captured_revision,
                        record.clone(),
                        resource.clone(),
                    )));
                }
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
                            + crate::scene::CONTEXT_DISPLAY_RESERVE_BYTES
                            + self.state.section.as_ref().map_or(0, |s| {
                                s.summary.total_bytes as usize
                                    * crate::scene::SECTION_DISPLAY_MULTIPLIER
                            })
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
                    JobError::Library { error } => error.to_string(),
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
