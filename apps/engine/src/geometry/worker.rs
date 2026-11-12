// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{JobError, JobId, check_cancel, error, source};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use spiling_contracts::{
    display::{EncodedSection, SectionChunkBuilder},
    geometry::*,
    manufacturing::*,
    project::{ProjectError, ProjectErrorCode, StorageMaintenanceAction, StoredDefinition},
    surface::{SurfaceOverlays, SurfaceScope},
};
use spiling_core::{
    ManufacturingAsset, Project, SaveSnapshot, SourceAsset, SurfaceImageAsset,
    storage::{self, SaveReceipt, SaveStage, Storage},
};
use spiling_geometry::source::SourceDefinition as Definition;
use spiling_geometry::{
    GeometryWorkBudget, MAX_PREPARED_SECTION_BYTES, MAX_PREPARED_SECTION_SOURCE_BYTES,
    MeshArtifact, PreparedSectionSource,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::Duration,
};

pub use spiling_contracts::display::MeshBudget;
struct Resident {
    // Created and retained only inside run(). The compiler borrows cheap shared
    // handles on this same worker; concurrent native queries are not qualified.
    definition: Arc<Definition>,
    record: StoredDefinition,
    vertices: u32,
    triangles: u32,
    mesh_bytes: u32,
}
#[derive(Default)]
pub struct LiveSnapshot {
    ids: BTreeSet<DefinitionId>,
    epoch: u64,
}
impl LiveSnapshot {
    pub fn reserve(&mut self, id: DefinitionId) {
        if self.ids.insert(id) {
            self.epoch += 1;
        }
    }
    pub fn replace(&mut self, ids: BTreeSet<DefinitionId>) {
        if self.ids != ids {
            self.ids = ids;
            self.epoch += 1;
        }
    }
}
pub type Live = Arc<Mutex<LiveSnapshot>>;

pub struct ImportRequest {
    pub options: ModelImportOptions,
    pub source: NativePath,
    pub allowed_reuse: BTreeSet<DefinitionId>,
    pub budget: MeshBudget,
    pub retained_source_bytes: usize,
}

pub struct OpenRequest {
    pub path: NativePath,
    pub reuse_sources: Vec<Arc<SourceAsset>>,
    pub read_only: bool,
    pub recover_previous: bool,
    pub existing: Option<Arc<Storage>>,
    pub allowed_reuse: BTreeSet<DefinitionId>,
    pub retained_source_bytes: usize,
    pub retained_manufacturing_bytes: u64,
    pub budget: MeshBudget,
    pub retained_surface_image_bytes: usize,
    pub retained_appearance_bytes: usize,
    pub retained_material_bytes: usize,
    pub analysis_field_budget: usize,
}
struct Opened {
    project: Project,
    imports: Vec<Imported>,
    residents: BTreeMap<DefinitionId, Resident>,
}
#[derive(Clone)]
pub struct CompileRequest {
    pub project_id: spiling_contracts::project::ProjectId,
    pub revision: spiling_contracts::project::ProjectRevision,
    pub definition_ids: BTreeSet<DefinitionId>,
    pub stored_definitions: Vec<StoredDefinition>,
    pub occurrences: Vec<OccurrenceRecord>,
    pub intent: Arc<ManufacturingIntent>,
    pub surface_overlays: Arc<SurfaceOverlays>,
    pub process_document: Arc<spiling_contracts::process::ProcessDocument>,
    pub byte_budget: u32,
    pub surface_images: BTreeMap<SourceHash, Arc<spiling_surface_fields::HeightImage>>,
}
#[expect(
    clippy::large_enum_variant,
    reason = "One bounded worker slot retains the captured save snapshot inline without per-job boxing"
)]
pub enum Work {
    PrepareAnalysisFastener(super::analysis::FaceRequest),
    PrepareAnalysisFace(super::analysis::FaceRequest),
    RunAnalysis(spiling_analysis::run::RunCapture),
    GetAnalysisResult(Arc<spiling_core::AnalysisResultAsset>),
    StorageMaintenance {
        storage: Arc<Storage>,
        action: StorageMaintenanceAction,
        target: Option<NativePath>,
        commit: crate::session::CommitToken,
    },
    ApplyOpticalConstruction {
        snapshot: Arc<spiling_core::Snapshot>,
        base: spiling_contracts::project::ProjectRevision,
        application: spiling_contracts::manufacturing::ApplyOpticalConstruction,
    },
    Import(ImportRequest),
    ImportSurfaceImage {
        source: NativePath,
        canonical_budget: usize,
    },
    ImportAppearancePalette {
        source: NativePath,
        retained_budget: usize,
    },
    CompareAppearance {
        asset: Arc<spiling_core::PaletteAsset>,
        target: spiling_contracts::appearance::AppearanceTarget,
        limits: spiling_contracts::appearance::SearchLimits,
    },
    GetAppearanceReport {
        report: Arc<spiling_contracts::appearance::AppearanceReport>,
    },
    MatchStock {
        materials: Option<Arc<spiling_core::materials::MaterialAsset>>,
        library: Option<PathBuf>,
        query: spiling_contracts::stock::StockQuery,
    },
    CompareOptical {
        asset: Arc<spiling_core::PaletteAsset>,
        request: Arc<spiling_contracts::optical::OpticalRequest>,
    },
    GetOpticalReport {
        report: Arc<spiling_contracts::optical::OpticalReport>,
    },
    GetSurfaceImage {
        asset: Arc<SurfaceImageAsset>,
    },
    Open(OpenRequest),
    Save {
        snapshot: SaveSnapshot,
        retained_source_bytes: usize,
        existing: Option<Arc<Storage>>,
        target: Option<NativePath>,
        commit: crate::session::CommitToken,
        fault: Option<crate::ProjectFault>,
    },
    Compile(CompileRequest),
    EvaluateAlternative {
        interior_opportunity: Option<InteriorPolicyOpportunity>,
        request: CompileRequest,
        context_hash: SourceHash,
        changed_inputs: Vec<ManufacturingInputChange>,
        changed_inputs_complete: bool,
    },
    Preflight(CompileRequest),
    Verify {
        asset: Arc<ManufacturingAsset>,
        output: VerificationOutput,
    },
    Section {
        session: SessionId,
        artifact: ArtifactId,
        revision: SceneRevision,
        plane: PlaneMm,
        occurrences: Vec<OccurrenceRecord>,
    },
    PrepareBedPose(super::bed_pose::Request),
    PrepareLayout(super::layout::Request),
}
/// The single bounded control slot keeps owned work inline, without per-job boxing.
/// Private fields and constructors enforce that only Run carries work.
pub struct Command {
    kind: CommandKind,
    work: Option<Work>,
}
enum CommandKind {
    Run { job: JobId, cancel: Arc<AtomicBool> },
    Decide { job: JobId, promote: bool },
    Stop,
}
impl Command {
    pub fn run(job: JobId, cancel: Arc<AtomicBool>, work: Work) -> Self {
        Self {
            kind: CommandKind::Run { job, cancel },
            work: Some(work),
        }
    }
    pub fn decide(job: JobId, promote: bool) -> Self {
        Self {
            kind: CommandKind::Decide { job, promote },
            work: None,
        }
    }
    pub fn stop() -> Self {
        Self {
            kind: CommandKind::Stop,
            work: None,
        }
    }
}
pub enum ImportedDisplay {
    Reused,
    Mesh(MeshArtifact),
    Unavailable(GeometryError),
}
pub struct Imported {
    pub definition_id: DefinitionId,
    pub provenance: SourceProvenance,
    pub source: Arc<SourceAsset>,
    pub bounds: AabbMm,
    pub faces: Vec<FaceInfo>,
    pub display: ImportedDisplay,
}
#[expect(
    clippy::large_enum_variant,
    reason = "The single native worker returns bounded products without extra heap indirection"
)]
pub enum Product {
    AnalysisFastenerPrepared(spiling_contracts::analysis::fastener::AnalysisFastenerPreparation),
    AnalysisFacePrepared(spiling_contracts::analysis::load::AnalysisFacePreparation),
    AnalysisCompleted(Arc<spiling_analysis::run::RunProduct>),
    StorageMaintained(super::storage::MaintenanceProduct),
    OpticalConstruction(spiling_core::OpticalConstructionCandidate),
    /// Every placed solid of one source, each with its source placements.
    Import(Vec<(Imported, Vec<RigidPoseMm>)>),
    Open {
        project: Project,
        imports: Vec<Imported>,
    },
    Saved(SaveReceipt),
    SurfaceImage(Arc<SurfaceImageAsset>),
    AppearancePalette(Arc<spiling_core::PaletteAsset>),
    AppearanceReport(Arc<super::appearance::ReportPublication>),
    OpticalReport(Arc<super::optical::OpticalReportPublication>),
    StockReport(Arc<super::stock::StockPublication>),
    Section(EncodedSection),
    Compiled(Arc<ManufacturingAsset>),
    AlternativeEvaluated {
        evaluation: ManufacturingAlternativeEvaluation,
        asset: Arc<ManufacturingAsset>,
        replay: spiling_manufacturing::EncodedReplayDisplay,
    },
    Preflight(PreflightReport),
    BedPosePrepared(BedPosePreparation),
    LayoutPrepared(LayoutProposal),
    Verified {
        record: ManufacturingArtifactRecord,
        inspection: spiling_manufacturing::ManufacturingInspection,
    },
}
pub enum Transition {
    Staged,
    Acknowledged { promoted: bool },
}
/// One bounded result slot keeps its optional staged payload inline, without a
/// per-job box. ACKs carry no product; the dispatcher checks that invariant.
pub struct Event {
    pub job: JobId,
    pub transition: Transition,
    pub staged: Option<Result<Product, JobError>>,
}
pub struct Worker {
    pub command: SyncSender<Command>,
    pub result: Receiver<Event>,
    pub live: Live,
    pub progress: Arc<Mutex<Option<(JobId, String)>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Worker {
    pub fn new() -> Self {
        let (command, commands) = mpsc::sync_channel(1);
        let (results, result) = mpsc::sync_channel(1);
        let live = Arc::new(Mutex::new(LiveSnapshot::default()));
        let snapshot = live.clone();
        let progress = Arc::new(Mutex::new(None));
        let worker_progress = progress.clone();
        let thread = std::thread::spawn(move || {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(commands, results, snapshot, worker_progress)
            }))
            .is_err()
            {
                crate::diagnostic(
                    "error",
                    "geometry_worker_panic",
                    "native worker panicked; session interrupted",
                );
                std::process::exit(70);
            }
        });
        Self {
            command,
            result,
            live,
            progress,
            thread: Some(thread),
        }
    }
    pub fn stop(&mut self) -> Result<(), String> {
        self.command
            .send(Command::stop())
            .map_err(|_| "native worker disconnected".to_owned())?;
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| "native worker panicked".to_owned())?;
        }
        Ok(())
    }
}
fn reclaim(definitions: &mut BTreeMap<DefinitionId, Resident>, live: &Live, epoch: &mut u64) {
    let ids = {
        let snapshot = live.lock();
        if snapshot.epoch == *epoch {
            return;
        }
        *epoch = snapshot.epoch;
        snapshot.ids.clone()
    };
    // Destruction of kernel graphs can be expensive. Never hold the dispatcher's
    // snapshot mutex during it; unchanged snapshots allocate/copy nothing.
    definitions.retain(|id, _| ids.contains(id));
}
fn run(
    commands: Receiver<Command>,
    results: SyncSender<Event>,
    live: Live,
    progress: Arc<Mutex<Option<(JobId, String)>>>,
) {
    let mut definitions = BTreeMap::<DefinitionId, Resident>::new();
    let mut analysis_operator: Option<spiling_analysis::run::CachedOperator> = None;
    let mut epoch = 0;
    loop {
        reclaim(&mut definitions, &live, &mut epoch);
        if analysis_operator
            .as_ref()
            .is_some_and(|operator| !definitions.contains_key(operator.definition_id()))
        {
            analysis_operator = None;
        }
        let command = match commands.recv_timeout(Duration::from_millis(25)) {
            Ok(c) => c,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        let Command {
            kind: CommandKind::Run { job, cancel },
            work: Some(work),
        } = command
        else {
            if matches!(command.kind, CommandKind::Stop) {
                return;
            }
            panic!("unexpected native promotion command");
        };
        // The snapshot may have changed while the command slot was idle.
        reclaim(&mut definitions, &live, &mut epoch);
        let mut pending = BTreeMap::new();
        let mut pending_records = BTreeMap::new();
        let report = |stage: String| *progress.lock() = Some((job, stage));
        let mut display_work = GeometryWorkBudget::query();
        let result = match work {
            Work::PrepareAnalysisFastener(request) => definitions
                .get(&request.reference.definition_id)
                .ok_or_else(|| JobError::from(super::unknown()))
                .and_then(|resident| {
                    super::analysis::prepare_fastener(
                        resident.definition.native()?,
                        request,
                        &cancel,
                    )
                })
                .map(Product::AnalysisFastenerPrepared),
            Work::PrepareAnalysisFace(request) => definitions
                .get(&request.reference.definition_id)
                .ok_or_else(|| JobError::from(super::unknown()))
                .and_then(|resident| {
                    super::analysis::prepare_face(resident.definition.native()?, request, &cancel)
                })
                .map(Product::AnalysisFacePrepared),
            Work::GetAnalysisResult(asset) => Ok(Product::AnalysisCompleted(Arc::new(
                spiling_analysis::run::RunProduct {
                    summary: asset.summary.clone(),
                    bytes: (*asset.bytes).clone(),
                },
            ))),
            Work::RunAnalysis(capture) => definitions
                .get(&capture.case.case.definition_id)
                .ok_or_else(|| {
                    super::project_error(
                        ProjectErrorCode::MissingAsset,
                        "analysis native definition absent",
                    )
                    .into()
                })
                .and_then(|resident| {
                    spiling_analysis::run::execute(
                        resident.definition.native()?,
                        &capture,
                        &mut analysis_operator,
                        &cancel,
                    )
                    .map(|product| Product::AnalysisCompleted(Arc::new(product)))
                    .map_err(|message| super::analysis::mechanical_error(message, &cancel).into())
                }),
            Work::StorageMaintenance {
                storage,
                action,
                target,
                commit,
            } => super::storage::maintain(storage, action, target, commit, &cancel)
                .map(Product::StorageMaintained),
            Work::ApplyOpticalConstruction {
                snapshot,
                base,
                application,
            } => spiling_core::prepare_optical_construction(snapshot, base, application, &|| {
                cancel.load(Ordering::Acquire)
            })
            .map(Product::OpticalConstruction)
            .map_err(JobError::from),
            Work::ImportAppearancePalette {
                source,
                retained_budget,
            } => super::appearance::import_palette(source, retained_budget, &cancel)
                .map(Product::AppearancePalette)
                .map_err(JobError::from),
            Work::CompareAppearance {
                asset,
                target,
                limits,
            } => super::appearance::compare_palette(asset, target, limits, &cancel)
                .map(Product::AppearanceReport)
                .map_err(JobError::from),
            Work::GetAppearanceReport { report } => check_cancel(&cancel)
                .map_err(super::project_geometry_error)
                .and_then(|()| super::appearance::ReportPublication::new(report))
                .map(Arc::new)
                .map(Product::AppearanceReport)
                .map_err(JobError::from),
            Work::MatchStock {
                materials,
                library,
                query,
            } => super::stock::compare(materials, library, query, &cancel)
                .map(Arc::new)
                .map(Product::StockReport)
                .map_err(JobError::from),
            Work::CompareOptical { asset, request } => {
                super::optical::compare_palette(asset, request, &cancel)
                    .map(Product::OpticalReport)
                    .map_err(JobError::from)
            }
            Work::GetOpticalReport { report } => check_cancel(&cancel)
                .map_err(super::project_geometry_error)
                .and_then(|()| super::optical::OpticalReportPublication::new(report))
                .map(Arc::new)
                .map(Product::OpticalReport)
                .map_err(JobError::from),
            Work::ImportSurfaceImage {
                source,
                canonical_budget,
            } => import_surface_image(source, canonical_budget, &cancel).map(Product::SurfaceImage),
            Work::GetSurfaceImage { asset } => check_cancel(&cancel)
                .map_err(super::project_geometry_error)
                .and_then(|()| {
                    asset
                        .record()
                        .validate_content(asset.bytes().as_slice())
                        .map(|_| ())
                })
                .and_then(|()| check_cancel(&cancel).map_err(super::project_geometry_error))
                .map(|()| Product::SurfaceImage(asset))
                .map_err(JobError::from),
            Work::Import(request) => {
                import(request, &definitions, &cancel, &mut display_work, &report).map(
                    |(parts, residents)| {
                        for resident in residents {
                            pending.insert(resident.definition.id().clone(), resident);
                        }
                        Product::Import(parts)
                    },
                )
            }
            Work::Open(request) => open(request, &definitions, &cancel, &mut display_work, &report)
                .map(
                    |Opened {
                         project,
                         imports,
                         residents,
                     }| {
                        for imported in &imports {
                            if matches!(imported.display, ImportedDisplay::Reused) {
                                pending_records.insert(
                                    imported.definition_id.clone(),
                                    imported.source.record.clone(),
                                );
                            }
                        }
                        pending = residents;
                        Product::Open { project, imports }
                    },
                ),
            Work::Save {
                snapshot,
                retained_source_bytes,
                existing,
                target,
                commit,
                fault,
            } => source_retention(retained_source_bytes, &definitions)
                .map_err(JobError::from)
                .and_then(|retained| {
                    save(snapshot, existing, target, commit, fault, retained, &cancel)
                })
                .map(Product::Saved),
            Work::Compile(request) => {
                compile(request, &definitions, &cancel).map(Product::Compiled)
            }
            Work::EvaluateAlternative {
                interior_opportunity,
                request,
                context_hash,
                changed_inputs,
                changed_inputs_complete,
            } => {
                let project_id = request.project_id.clone();
                let captured_revision = request.revision;
                let intent = request.intent.clone();
                compile(request, &definitions, &cancel).and_then(|asset| {
                    let inspection =
                        inspect_asset(&asset, VerificationOutput::ReportAndMotion, &cancel)?;
                    let replay = inspection.replay.ok_or_else(|| {
                        ManufacturingError::new(
                            ManufacturingErrorCode::CorruptArtifact,
                            "candidate inspection omitted actual replay",
                        )
                    })?;
                    let interior_policy = interior_opportunity
                        .map(|opportunity| {
                            let bundle = asset.decode()?;
                            let selection =
                                bundle.plan.interior_selection.as_deref().ok_or_else(|| {
                                    ManufacturingError::new(
                                        ManufacturingErrorCode::VerificationFailed,
                                        "policy evaluation omitted construction assessment",
                                    )
                                })?;
                            InteriorPolicyAssessment::from_selection(opportunity, selection)
                                .map(Arc::new)
                        })
                        .transpose()?;
                    let evaluation = ManufacturingAlternativeEvaluation {
                        interior_policy,
                        project_id,
                        captured_revision,
                        context_hash,
                        intent,
                        changed_inputs,
                        changed_inputs_complete,
                        record: asset.record.clone(),
                        report: inspection.report,
                        provenance: inspection.provenance,
                        resource: spiling_contracts::ArtifactView {
                            name: format!("artifacts/{}", asset.record.hash.as_str()),
                            size_bytes: asset.record.byte_count.to_string(),
                            sha256: asset.record.hash.as_str().into(),
                            media_type: "application/x-spiling-manufacturing-bundle".into(),
                        },
                        replay_display: ReplayDisplayDescriptor {
                            info: replay.info.clone(),
                            pages: replay.pages.clone(),
                            resource: {
                                let hash = SourceHash::from_digest(replay.bytes.digest());
                                spiling_contracts::ArtifactView {
                                    name: format!("artifacts/{}", hash.as_str()),
                                    sha256: hash.as_str().into(),
                                    size_bytes: replay.bytes.len().to_string(),
                                    media_type: REPLAY_MEDIA_TYPE.into(),
                                }
                            },
                        },
                    };
                    let wire =
                        spiling_contracts::rpc::ManufacturingAlternativeEvaluation::try_from(
                            evaluation.clone(),
                        )
                        .map_err(|message| {
                            ManufacturingError::new(
                                ManufacturingErrorCode::CorruptArtifact,
                                message,
                            )
                        })?;
                    check_alternative_receipt_budget(prost::Message::encoded_len(&wire))?;
                    Ok(Product::AlternativeEvaluated {
                        evaluation,
                        asset,
                        replay,
                    })
                })
            }
            Work::Preflight(request) => {
                preflight(request, &definitions, &cancel).map(Product::Preflight)
            }
            Work::PrepareLayout(request) => {
                let natives = definitions
                    .iter()
                    .map(|(id, r)| (id.clone(), r.definition.clone()))
                    .collect();
                super::layout::prepare(request, &natives, &cancel, &report)
                    .map(Product::LayoutPrepared)
            }
            Work::PrepareBedPose(request) => definitions
                .get(&request.definition_id)
                .ok_or_else(|| {
                    error(
                        GeometryErrorCode::StaleRevision,
                        "native contact definition was reclaimed",
                    )
                })
                .and_then(|resident| {
                    super::bed_pose::prepare(&resident.definition, request, &cancel)
                })
                .map(Product::BedPosePrepared)
                .map_err(JobError::from),
            Work::Verify { asset, output } => {
                inspect_asset(&asset, output, &cancel).map(|inspection| Product::Verified {
                    record: asset.record.clone(),
                    inspection,
                })
            }
            Work::Section {
                session,
                artifact,
                revision,
                plane,
                occurrences,
            } => section(
                session,
                artifact,
                revision,
                plane,
                occurrences,
                &definitions,
                &cancel,
            )
            .map(Product::Section)
            .map_err(JobError::from),
        };
        let succeeded = result.is_ok();
        if results
            .send(Event {
                job,
                transition: Transition::Staged,
                staged: Some(result),
            })
            .is_err()
        {
            return;
        }
        let promote = match commands.recv() {
            Ok(Command {
                kind:
                    CommandKind::Decide {
                        job: decision_job,
                        promote,
                    },
                work: None,
            }) if decision_job == job => promote,
            Err(_)
            | Ok(Command {
                kind: CommandKind::Stop,
                work: None,
            }) => return,
            _ => panic!("native promotion correlation failure"),
        };
        if promote && !succeeded {
            panic!("cannot promote a failed native stage");
        }
        if promote {
            definitions.append(&mut pending);
            for (id, record) in pending_records {
                definitions
                    .get_mut(&id)
                    .expect("reused native definition")
                    .record = record;
            }
        }
        drop(pending);
        // The dispatcher reserves the native live pin before its commit decision.
        // Only the ACK publishes public scene metadata; no kernel object crosses it.
        if results
            .send(Event {
                job,
                transition: Transition::Acknowledged { promoted: promote },
                staged: None,
            })
            .is_err()
        {
            return;
        }
    }
}
/// Remaining capacity counts every old/history resident until the promotion ACK,
/// including an old copy of an ID that cannot use the dispatcher's cache.
struct Capacity {
    definitions: u32,
    source_bytes: usize,
    /// Count live allocations, not hashes: a recapture of identical bytes is
    /// another allocation, but several bodies of one capture share its Arc.
    charged_sources: BTreeSet<*const Vec<u8>>,
    faces: u32,
    mesh: MeshBudget,
}
impl Capacity {
    fn new(
        definitions: &BTreeMap<DefinitionId, Resident>,
        budget: MeshBudget,
        source_bytes: usize,
    ) -> Self {
        Self {
            definitions: MAX_DEFINITIONS - definitions.len() as u32,
            source_bytes,
            charged_sources: BTreeSet::new(),
            faces: MAX_NATIVE_FACES
                - definitions
                    .values()
                    .map(|r| r.definition.faces().len() as u32)
                    .sum::<u32>(),
            mesh: MeshBudget {
                vertices: budget.vertices.min(
                    MAX_DISPLAY_VERTICES - definitions.values().map(|r| r.vertices).sum::<u32>(),
                ),
                triangles: budget.triangles.min(
                    MAX_DISPLAY_TRIANGLES - definitions.values().map(|r| r.triangles).sum::<u32>(),
                ),
                bytes: budget.bytes.min(
                    MAX_SCENE_MESH_BYTES - definitions.values().map(|r| r.mesh_bytes).sum::<u32>(),
                ),
            },
        }
    }
    fn charge_source(&mut self, bytes: &Arc<Vec<u8>>) -> Result<(), GeometryError> {
        let pointer = Arc::as_ptr(bytes);
        if !self.charged_sources.contains(&pointer) {
            let remaining = self
                .source_bytes
                .checked_sub(bytes.capacity())
                .ok_or_else(|| {
                    error(
                        GeometryErrorCode::ResourceLimit,
                        "captured source exceeds current memory admission",
                    )
                })?;
            self.charged_sources.insert(pointer);
            self.source_bytes = remaining;
        }
        Ok(())
    }
    fn admit(
        &mut self,
        bytes: &[u8],
        solid: Option<&SourceSolidId>,
        cancel: &AtomicBool,
    ) -> Result<Definition, GeometryError> {
        check_cancel(cancel)?;
        if self.definitions == 0 {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                "resident/staged native definition cap exceeded",
            ));
        }
        let definition = spiling_geometry::from_step_solid_with_limits(
            bytes,
            solid,
            cancel,
            self.faces,
            self.source_bytes,
        )?;
        self.retain_definition(definition.into(), cancel)
    }
    fn retain_definition(
        &mut self,
        definition: Definition,
        cancel: &AtomicBool,
    ) -> Result<Definition, GeometryError> {
        if self.definitions == 0 {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                "resident/staged source definition cap exceeded",
            ));
        }
        let query_proof_bytes = definition.retained_query_proof_bytes();
        let remaining_bytes = self
            .source_bytes
            .checked_sub(query_proof_bytes)
            .ok_or_else(|| {
                error(
                    GeometryErrorCode::ResourceLimit,
                    "native query proof exceeds current memory admission",
                )
            })?;
        let faces = definition.faces().len() as u32;
        if faces > self.faces {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                "live/staged native face budget exceeded",
            ));
        }
        self.definitions -= 1;
        self.source_bytes = remaining_bytes;
        self.faces -= faces;
        check_cancel(cancel)?;
        Ok(definition)
    }
    fn tessellate(
        &mut self,
        definition: &Definition,
        cancel: &AtomicBool,
        work: &mut GeometryWorkBudget,
        progress: &dyn Fn(String),
    ) -> Result<MeshArtifact, GeometryError> {
        let mesh = definition.display(
            DisplayProfile::MeshTrimmedMm005V2,
            cancel,
            self.mesh,
            work,
            &mut |update| {
                use spiling_geometry::DisplayStage;
                let stage = match update.stage {
                    DisplayStage::PreparingEdges => "preparing_edges",
                    DisplayStage::TessellatingFace => "tessellating",
                    DisplayStage::AuditingFace => "auditing",
                    DisplayStage::FaceComplete => "face_complete",
                    DisplayStage::Complete => "complete",
                };
                progress(format!(
                    "display_{stage}_faces_{}_of_{}_attempt_{}",
                    update.completed_faces, update.total_faces, update.attempt,
                ));
            },
        )?;
        if mesh.vertex_count > self.mesh.vertices
            || mesh.triangle_count > self.mesh.triangles
            || mesh.total_bytes > self.mesh.bytes
        {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                "live/staged unique mesh budget exceeded",
            ));
        }
        self.mesh.vertices -= mesh.vertex_count;
        self.mesh.triangles -= mesh.triangle_count;
        self.mesh.bytes -= mesh.total_bytes;
        Ok(mesh)
    }
    fn display(
        &mut self,
        definition: &Definition,
        cancel: &AtomicBool,
        work: &mut GeometryWorkBudget,
        progress: &dyn Fn(String),
    ) -> Result<ImportedDisplay, GeometryError> {
        match self.tessellate(definition, cancel, work, progress) {
            Ok(mesh) => Ok(ImportedDisplay::Mesh(mesh)),
            Err(failure) => {
                if failure.code == GeometryErrorCode::Cancelled {
                    return Err(failure);
                }
                check_cancel(cancel)?;
                Ok(ImportedDisplay::Unavailable(failure))
            }
        }
    }
}
fn validate_provenance(definition: &Definition, source: &SourceAsset) -> Result<(), ProjectError> {
    let mut provenance = definition.provenance().clone();
    // Labels belong to the persistent source record, not a reopened path or the
    // STEP adapter's default label. All other provenance is derived again.
    provenance
        .source_name
        .clone_from(&source.record.provenance.source_name);
    if definition.id() != &source.record.definition_id || provenance != source.record.provenance {
        return Err(ProjectError::new(
            ProjectErrorCode::InvalidProject,
            "stored source identity/provenance disagrees with native admission",
        ));
    }
    Ok(())
}
fn imported(
    definition: &Definition,
    source: Arc<SourceAsset>,
    display: ImportedDisplay,
) -> Imported {
    Imported {
        definition_id: definition.id().clone(),
        provenance: source.record.provenance.clone(),
        source,
        bounds: definition.bounds_mm(),
        faces: definition.faces().to_vec(),
        display,
    }
}
fn resident(definition: Definition, source: &SourceAsset, display: &ImportedDisplay) -> Resident {
    let (vertices, triangles, mesh_bytes) = match display {
        ImportedDisplay::Mesh(mesh) => (mesh.vertex_count, mesh.triangle_count, mesh.total_bytes),
        ImportedDisplay::Unavailable(_) => (0, 0, 0),
        ImportedDisplay::Reused => {
            unreachable!("reused captures keep their existing native resident")
        }
    };
    Resident {
        definition: Arc::new(definition),
        record: source.record.clone(),
        vertices,
        triangles,
        mesh_bytes,
    }
}
type ImportedParts = (Vec<(Imported, Vec<RigidPoseMm>)>, Vec<Resident>);
fn source_retention(
    retained_source_bytes: usize,
    definitions: &BTreeMap<DefinitionId, Resident>,
) -> Result<usize, GeometryError> {
    definitions
        .values()
        .try_fold(retained_source_bytes, |bytes, resident| {
            bytes
                .checked_add(resident.definition.retained_query_proof_bytes())
                .ok_or_else(|| {
                    error(
                        GeometryErrorCode::ResourceLimit,
                        "retained native allocation accounting overflow",
                    )
                })
        })
}

fn import(
    request: ImportRequest,
    definitions: &BTreeMap<DefinitionId, Resident>,
    cancel: &AtomicBool,
    work: &mut GeometryWorkBudget,
    progress: &dyn Fn(String),
) -> Result<ImportedParts, JobError> {
    let ImportRequest {
        source,
        options,
        allowed_reuse: reuse,
        budget,
        retained_source_bytes,
    } = request;
    check_cancel(cancel)?;
    let growth =
        crate::resources::source_growth(source_retention(retained_source_bytes, definitions)?)?;
    let mut capacity = Capacity::new(definitions, budget, growth);
    let captured = source::capture(source, capacity.source_bytes, cancel)?;
    let digest: [u8; 32] = Sha256::digest(&captured.bytes).into();
    // A source that places one solid keeps its whole-source identity; each
    // solid of a multi-solid source is its own definition of the same bytes.
    if captured.mesh_format.is_some() {
        return import_mesh(
            captured,
            capacity,
            &options,
            (definitions, &reuse),
            cancel,
            work,
            progress,
        );
    }
    if options != ModelImportOptions::default() {
        return Err(error(
            GeometryErrorCode::UnsupportedGeometry,
            "STL units and source plates do not apply to STEP imports",
        )
        .into());
    }
    let bodies = spiling_geometry::source_bodies(&captured.bytes, cancel)?;
    if bodies.is_empty() {
        return Err(error(
            GeometryErrorCode::UnsupportedGeometry,
            "source places no MANIFOLD_SOLID_BREP; there is no solid to print",
        )
        .into());
    }
    let selects = bodies.len() > 1;
    let bytes = Arc::new(captured.bytes);
    capacity.charge_source(&bytes)?;
    let mut parts = Vec::with_capacity(bodies.len());
    let mut residents = Vec::new();
    for body in bodies {
        check_cancel(cancel)?;
        let solid = selects.then_some(body.solid);
        let id = DefinitionId::for_source(&digest, solid.as_ref());
        if reuse.contains(&id)
            && let Some(resident) = definitions.get(&id)
        {
            // Share the captured allocation, but preserve the first admitted
            // source's label and immutable record on an exact-byte reimport.
            let source = Arc::new(SourceAsset::shared(resident.record.clone(), bytes.clone())?);
            validate_provenance(&resident.definition, &source)?;
            parts.push((
                imported(&resident.definition, source, ImportedDisplay::Reused),
                body.placements,
            ));
            continue;
        }
        progress("preparing_native_geometry".into());
        let definition = capacity.admit(&bytes, solid.as_ref(), cancel)?;
        let mut provenance = definition.provenance().clone();
        provenance.source_name = captured.label.clone();
        let source = Arc::new(SourceAsset::shared(
            StoredDefinition {
                definition_id: definition.id().clone(),
                provenance,
            },
            bytes.clone(),
        )?);
        let display = capacity.display(&definition, cancel, work, progress)?;
        let resident = resident(definition, &source, &display);
        parts.push((
            imported(&resident.definition, source, display),
            body.placements,
        ));
        residents.push(resident);
    }
    check_cancel(cancel)?;
    Ok((parts, residents))
}
fn import_mesh(
    captured: source::Captured,
    mut capacity: Capacity,
    options: &ModelImportOptions,
    reuse: (&BTreeMap<DefinitionId, Resident>, &BTreeSet<DefinitionId>),
    cancel: &AtomicBool,
    work: &mut GeometryWorkBudget,
    progress: &dyn Fn(String),
) -> Result<ImportedParts, JobError> {
    use spiling_geometry::source::{HybridDefinition, SourceDefinition};
    progress("preparing_source_geometry".into());
    let decoded = spiling_geometry::source::decode_container(
        &captured.bytes,
        &captured.label,
        captured
            .mesh_format
            .expect("mesh dispatch carries a selected decoder"),
        capacity.source_bytes,
        options,
        cancel,
    )?;
    let bytes = Arc::new(captured.bytes);
    capacity.charge_source(&bytes)?;
    let source_hash = SourceHash::from_digest(&Sha256::digest(bytes.as_slice()).into());
    let mut parts = Vec::new();
    let mut residents = Vec::new();
    for mut decoded in decoded {
        decoded.notes.push(format!(
            "Analytic recovery deviation: {} mm.",
            decoded.complex.error_mm
        ));
        check_cancel(cancel)?;
        let normalized = Arc::new(decoded.complex.encode()?);
        capacity.charge_source(&normalized)?;
        let provenance = SourceProvenance {
            source_hash: source_hash.clone(),
            source_name: captured.label.clone(),
            source_unit: decoded.source_unit,
            uncertainty_mm: None,
            source_solid: None,
            normalized: Some(NormalizedSource {
                hash: SourceHash::from_digest(&Sha256::digest(normalized.as_slice()).into()),
                format: decoded.format,
                selector: decoded.selector,
                notes: decoded.notes,
            }),
        };
        let id = provenance.definition_id();
        if reuse.1.contains(&id)
            && let Some(resident) = reuse.0.get(&id)
        {
            let source = Arc::new(SourceAsset::with_normalized(
                resident.record.clone(),
                bytes.clone(),
                Some(normalized),
            )?);
            validate_provenance(&resident.definition, &source)?;
            parts.push((
                imported(&resident.definition, source, ImportedDisplay::Reused),
                decoded.placements,
            ));
            continue;
        }
        if capacity.definitions == 0 || decoded.complex.faces.len() > capacity.faces as usize {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                "source definition/region capacity exceeded",
            )
            .into());
        }
        let definition = SourceDefinition::Hybrid(HybridDefinition {
            id: provenance.definition_id(),
            provenance: provenance.clone(),
            bounds: decoded.complex.bounds()?,
            complex: decoded.complex,
        });
        let definition = capacity.retain_definition(definition, cancel)?;
        let source = Arc::new(SourceAsset::with_normalized(
            StoredDefinition {
                definition_id: definition.id().clone(),
                provenance,
            },
            bytes.clone(),
            Some(normalized),
        )?);
        let display = capacity.display(&definition, cancel, work, progress)?;
        let resident = resident(definition, &source, &display);
        parts.push((
            imported(&resident.definition, source, display),
            decoded.placements,
        ));
        residents.push(resident);
    }
    Ok((parts, residents))
}
fn import_surface_image(
    source: NativePath,
    canonical_budget: usize,
    cancel: &AtomicBool,
) -> Result<Arc<SurfaceImageAsset>, JobError> {
    let captured =
        source::capture(source, 1024 * 1024, cancel).map_err(super::project_geometry_error)?;
    let (record, bytes) = spiling_surface_fields::normalize(
        &captured.bytes,
        captured.label,
        canonical_budget,
        cancel,
    )?;
    check_cancel(cancel).map_err(super::project_geometry_error)?;
    SurfaceImageAsset::new(record, bytes)
        .map(Arc::new)
        .map_err(JobError::from)
}
pub(super) fn project_path(path: NativePath) -> Result<PathBuf, ProjectError> {
    path.to_os_string()
        .map(PathBuf::from)
        .map_err(|e| ProjectError::new(ProjectErrorCode::Io, e.message))
}
fn open(
    request: OpenRequest,
    definitions: &BTreeMap<DefinitionId, Resident>,
    cancel: &AtomicBool,
    work: &mut GeometryWorkBudget,
    progress: &dyn Fn(String),
) -> Result<Opened, JobError> {
    let _opening = crate::performance::span("open_prepare");
    crate::performance::mark("open_started");
    let OpenRequest {
        path,
        reuse_sources,
        read_only,
        recover_previous,
        existing,
        allowed_reuse,
        retained_source_bytes,
        retained_manufacturing_bytes,
        budget,
        retained_surface_image_bytes,
        retained_appearance_bytes,
        retained_material_bytes,
        analysis_field_budget,
    } = request;
    let path = project_path(path)?;
    // Old current/history allocations survive until ACK, even for identical IDs.
    // Enforce their remaining budget before the loader allocates incoming bytes.
    let source_budget =
        crate::resources::source_growth(source_retention(retained_source_bytes, definitions)?)?;
    let loading = crate::performance::span("open_storage");
    let loaded = storage::open_reusing(
        &path,
        read_only,
        recover_previous,
        cancel,
        existing,
        storage::LoadBudgets {
            analysis_bytes: analysis_field_budget,
            source_bytes: source_budget,
            material_bytes: spiling_core::materials::MAX_RETAINED_MATERIAL_BYTES
                .checked_sub(retained_material_bytes)
                .ok_or_else(|| {
                    ProjectError::new(
                        ProjectErrorCode::ResourceLimit,
                        "retained project material budget exceeded",
                    )
                })?,
            manufacturing_bytes: (MAX_MANUFACTURING_RETAINED_BYTES - retained_manufacturing_bytes)
                as usize,
            image_bytes: spiling_contracts::surface::MAX_SURFACE_IMAGE_RETAINED_BYTES
                .checked_sub(retained_surface_image_bytes)
                .ok_or_else(|| {
                    ProjectError::new(
                        ProjectErrorCode::ResourceLimit,
                        "retained surface image budget exceeded",
                    )
                })?,
            appearance_bytes: spiling_core::MAX_APPEARANCE_RETAINED_BYTES
                .checked_sub(retained_appearance_bytes)
                .ok_or_else(|| {
                    ProjectError::new(
                        ProjectErrorCode::ResourceLimit,
                        "retained appearance palette budget exceeded",
                    )
                })?,
        },
        &reuse_sources,
    )?;
    drop(loading);
    let mut capacity = Capacity::new(definitions, budget, source_budget);
    capacity
        .charged_sources
        .extend(reuse_sources.iter().flat_map(|source| {
            std::iter::once(Arc::as_ptr(&source.bytes))
                .chain(source.normalized.as_ref().map(Arc::as_ptr))
        }));
    for source in loaded.sources.values() {
        capacity.charge_source(&source.bytes)?;
        if let Some(bytes) = &source.normalized {
            capacity.charge_source(bytes)?;
        }
    }
    let mut pending = BTreeMap::new();
    let mut imports = Vec::with_capacity(loaded.sources.len());
    for (id, source) in &loaded.sources {
        check_cancel(cancel)?;
        if allowed_reuse.contains(id)
            && let Some(resident) = definitions.get(id)
        {
            validate_provenance(&resident.definition, source)?;
            validate_surface_targets(&resident.definition, &loaded.manifest.surface_overlays)?;
            validate_analysis_targets(&resident.definition, &loaded, cancel)?;
            imports.push(imported(
                &resident.definition,
                source.clone(),
                ImportedDisplay::Reused,
            ));
        } else {
            progress("preparing_native_geometry".into());
            let native = crate::performance::span("open_native");
            let definition = match &source.normalized {
                Some(bytes) => capacity.retain_definition(
                    Definition::restore(source.record.provenance.clone(), bytes, cancel)?,
                    cancel,
                )?,
                None => capacity.admit(
                    source.bytes.as_slice(),
                    source.record.provenance.source_solid.as_ref(),
                    cancel,
                )?,
            };
            validate_provenance(&definition, source)?;
            validate_surface_targets(&definition, &loaded.manifest.surface_overlays)?;
            validate_analysis_targets(&definition, &loaded, cancel)?;
            drop(native);
            let tessellation = crate::performance::span("open_tessellate");
            let display = capacity.display(&definition, cancel, work, progress)?;
            drop(tessellation);
            let resident = resident(definition, source, &display);
            imports.push(imported(&resident.definition, source.clone(), display));
            pending.insert(id.clone(), resident);
        }
    }
    check_cancel(cancel)?;
    for asset in loaded
        .layouts
        .iter()
        .filter_map(|l| l.manufacturing_artifact.as_ref())
    {
        let _replay = crate::performance::span("open_replay");
        inspect_asset(asset, VerificationOutput::ReportOnly, cancel)?;
    }
    let project = Project::from_loaded(loaded)?;
    check_cancel(cancel)?;
    crate::performance::mark("open_prepared");
    Ok(Opened {
        project,
        imports,
        residents: pending,
    })
}
fn validate_analysis_targets(
    definition: &Definition,
    loaded: &storage::LoadedProject,
    cancel: &AtomicBool,
) -> Result<(), ProjectError> {
    for record in loaded.manifest.analysis.cases.iter().chain(
        loaded
            .analysis_results
            .values()
            .map(|asset| &asset.summary.case),
    ) {
        if &record.case.definition_id == definition.id()
            && record.case.patch_faces().any(|face_id| {
                !definition
                    .faces()
                    .iter()
                    .any(|face| &face.face_id == face_id)
            })
        {
            return Err(ProjectError::new(
                ProjectErrorCode::InvalidProject,
                "stored analysis patch is absent from the authoritative native source face table",
            ));
        }
        if &record.case.definition_id == definition.id() {
            for load in &record.case.loads {
                super::analysis::validate_contact(
                    definition.native().map_err(super::project_geometry_error)?,
                    load,
                    cancel,
                )?;
            }
            for fastener in &record.case.fasteners {
                super::analysis::validate_fastener(
                    definition.native().map_err(super::project_geometry_error)?,
                    fastener,
                    cancel,
                )?;
            }
        }
    }
    for asset in loaded.analysis_results.values() {
        if &asset.summary.case.case.definition_id == definition.id() {
            super::analysis::validate_mesh_profile(
                definition.native().map_err(super::project_geometry_error)?,
                &asset.summary.mesh_geometry,
                cancel,
            )?;
        }
    }
    Ok(())
}
fn validate_surface_targets(
    definition: &Definition,
    overlays: &SurfaceOverlays,
) -> Result<(), ProjectError> {
    for layer in &overlays.layers {
        let target = match &layer.scope {
            SurfaceScope::Definition { definition_id } => definition_id,
            SurfaceScope::Occurrence {
                expected_definition_id,
                ..
            } => expected_definition_id,
        };
        if target == definition.id()
            && !definition
                .faces()
                .iter()
                .any(|face| face.face_id == layer.face_id)
        {
            return Err(ProjectError::new(
                ProjectErrorCode::InvalidProject,
                "stored surface target is absent from the authoritative native source face table",
            ));
        }
    }
    Ok(())
}
fn save(
    mut snapshot: SaveSnapshot,
    existing: Option<Arc<Storage>>,
    target: Option<NativePath>,
    commit: crate::session::CommitToken,
    fault: Option<crate::ProjectFault>,
    retained_source_bytes: usize,
    cancel: &AtomicBool,
) -> Result<SaveReceipt, JobError> {
    snapshot.limit_source_growth(crate::resources::source_growth(retained_source_bytes)?)?;
    let target = target.map(project_path).transpose()?;
    let receipt = storage::save(snapshot, existing, target, cancel, |stage| {
        if matches!(stage, SaveStage::BeforeCommit) && !commit.begin_commit() {
            cancel.store(true, Ordering::Release);
            return;
        }
        if fault.as_ref().is_some_and(|fault| fault.matches(stage)) {
            std::process::exit(86);
        }
    })?;
    Ok(receipt)
}
fn section(
    session: SessionId,
    artifact: ArtifactId,
    revision: SceneRevision,
    plane: PlaneMm,
    occurrences: Vec<OccurrenceRecord>,
    definitions: &BTreeMap<DefinitionId, Resident>,
    cancel: &AtomicBool,
) -> Result<EncodedSection, GeometryError> {
    // Derive publication metadata and native-query input from the same raw
    // request. Re-normalizing the first result can change its f64 coefficients
    // and break the consumer's exact normalized-request identity.
    let mut builder = SectionChunkBuilder::new(session, artifact, revision, plane)?;
    let plane = plane.normalized()?;
    let mut work = GeometryWorkBudget::query();
    let source_limit = MAX_PREPARED_SECTION_SOURCE_BYTES;
    let mut sources = BTreeMap::<DefinitionId, (usize, Option<PreparedSectionSource<'_>>)>::new();
    for occurrence in &occurrences {
        sources
            .entry(occurrence.definition_id.clone())
            .or_insert((0, None))
            .0 += 1;
    }
    let mut prepared_bytes = 0usize;
    for occurrence in occurrences {
        check_cancel(cancel)?;
        let definition = definitions.get(&occurrence.definition_id).ok_or_else(|| {
            error(
                GeometryErrorCode::StaleRevision,
                "section definition was reclaimed",
            )
        })?;
        let (remaining_uses, source) = sources
            .get_mut(&occurrence.definition_id)
            .expect("section occurrence was counted");
        let loops = if matches!(definition.definition.as_ref(), Definition::Hybrid(_)) {
            definition
                .definition
                .section_placed(occurrence.pose, plane, cancel)?
        } else {
            if source.is_none() {
                // Raw sections need native coefficient correspondence, not material
                // charts/vertex plans or their independent manufacturing admission.
                let prepared = spiling_geometry::prepare_section_source(
                    definition.definition.native()?,
                    (source_limit - prepared_bytes).min(
                        remaining_section_capacity(prepared_bytes, builder.retained_bytes())?
                            .checked_sub(MAX_PREPARED_SECTION_BYTES - source_limit)
                            .ok_or_else(|| {
                                error(
                                    GeometryErrorCode::ResourceLimit,
                                    "native section has no query headroom",
                                )
                            })?,
                    ),
                    cancel,
                    &mut work,
                )?;
                prepared_bytes = prepared_bytes
                    .checked_add(prepared.retained_bytes())
                    .filter(|bytes| *bytes <= source_limit)
                    .ok_or_else(|| {
                        error(
                            GeometryErrorCode::ResourceLimit,
                            "prepared native section sources exceed shared job capacity",
                        )
                    })?;
                *source = Some(prepared);
            }
            // Geometry owns the actual stored pose, plane pullback, metric error and
            // canonical world loops. Only immutable source preparation is reused;
            // sampled loops are consumed before another occurrence is queried.
            let native = source
                .as_ref()
                .expect("native section source was prepared")
                .section_placed_with_work(
                    occurrence.pose,
                    plane,
                    remaining_section_capacity(prepared_bytes, builder.retained_bytes())? as u32,
                    &mut work,
                    cancel,
                )?;
            native.into_loops()
        };
        let mut live_loop_bytes = loops
            .capacity()
            .saturating_mul(std::mem::size_of::<spiling_geometry::SectionLoopMm>())
            .saturating_add(
                loops
                    .iter()
                    .map(|boundary| {
                        boundary
                            .points_mm
                            .capacity()
                            .saturating_mul(std::mem::size_of::<[f64; 3]>())
                    })
                    .sum::<usize>(),
            );
        for boundary in loops {
            check_cancel(cancel)?;
            builder.set_retained_byte_limit(remaining_section_capacity(
                prepared_bytes,
                live_loop_bytes,
            )?)?;
            builder.push_loop(
                occurrence.occurrence_id,
                occurrence.definition_id.clone(),
                boundary.is_hole,
                &boundary.points_mm,
            )?;
            live_loop_bytes -= boundary
                .points_mm
                .capacity()
                .saturating_mul(std::mem::size_of::<[f64; 3]>());
        }
        *remaining_uses -= 1;
        if *remaining_uses == 0
            && let Some(source) = source.take()
        {
            prepared_bytes -= source.retained_bytes();
        }
    }
    check_cancel(cancel)?;
    builder.set_retained_byte_limit(remaining_section_capacity(prepared_bytes, 0)?)?;
    builder.finish()
}

fn remaining_section_capacity(prepared: usize, other_live: usize) -> Result<usize, GeometryError> {
    MAX_PREPARED_SECTION_BYTES
        .checked_sub(prepared)
        .and_then(|bytes| bytes.checked_sub(other_live))
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "native section coexisting owned capacity exhausted",
            )
        })
}

fn inspect_asset(
    asset: &ManufacturingAsset,
    output: VerificationOutput,
    cancel: &AtomicBool,
) -> Result<spiling_manufacturing::ManufacturingInspection, JobError> {
    let bundle = asset.decode().map_err(JobError::from)?;
    let result = spiling_manufacturing::ManufacturingResult::new(bundle, asset.program.clone())
        .map_err(JobError::from)?;
    let mut inspection =
        spiling_manufacturing::inspect_bundle(&result, &asset.record.hash, output, cancel)
            .map_err(JobError::from)?;
    let bundle = result.bundle;
    if let JobWorkflow::FixedDualMarlin { job } = &inspection.provenance.workflow {
        for face in &mut inspection.provenance.surface_color {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(ManufacturingError::new(
                    ManufacturingErrorCode::Cancelled,
                    "surface reassessment cancelled",
                )
                .into());
            }
            let construction = &job.surface_constructions[usize::from(face.construction_index)];
            if let Some(origin) = &construction.optical_origin {
                let mut exposure = [0.0; 2];
                for (i, material) in origin.materials.iter().enumerate() {
                    exposure[i] = face.exposed_height_by_tool_mm[usize::from(
                        job.tool_for_material(&material.loaded_material.id)
                            .map_err(JobError::from)?,
                    )];
                }
                let pose_matches = bundle
                    .provenance
                    .occurrences
                    .iter()
                    .find(|o| o.occurrence_id == construction.occurrence_id)
                    .is_some_and(|o| o.pose == origin.applied_pose);
                face.reference = Some(
                    spiling_appearance::optical::assess_construction(
                        origin,
                        &face.realized_patch,
                        exposure,
                        pose_matches,
                    )
                    .map_err(JobError::from)?,
                );
            }
        }
    }
    inspection.provenance.validate().map_err(JobError::from)?;
    Ok(inspection)
}

fn check_alternative_receipt_budget(encoded_bytes: usize) -> Result<(), ManufacturingError> {
    if encoded_bytes > (spiling_contracts::MAX_CONTROL_BYTES as usize).saturating_sub(4096) {
        return Err(ManufacturingError::new(
            ManufacturingErrorCode::ResourceLimit,
            "candidate receipt exceeds bounded control response; reduce captured input or report size",
        ));
    }
    Ok(())
}

fn preflight(
    request: CompileRequest,
    residents: &BTreeMap<DefinitionId, Resident>,
    cancel: &AtomicBool,
) -> Result<PreflightReport, JobError> {
    let definitions = request
        .definition_ids
        .iter()
        .map(|id| {
            residents
                .get(id)
                .map(|resident| (id.clone(), resident.definition.clone()))
                .ok_or_else(|| {
                    ManufacturingError::new(
                        ManufacturingErrorCode::StaleRevision,
                        "manufacturing definition was reclaimed",
                    )
                })
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    Ok(spiling_manufacturing::preflight(
        &spiling_manufacturing::CompilerInput {
            project_id: &request.project_id,
            revision: request.revision,
            definitions: &definitions,
            stored_definitions: &request.stored_definitions,
            occurrences: &request.occurrences,
            intent: &request.intent,
            surface_overlays: &request.surface_overlays,
            process_document: &request.process_document,
            surface_images: &request.surface_images,
        },
        cancel,
    )?)
}

fn compile(
    request: CompileRequest,
    residents: &BTreeMap<DefinitionId, Resident>,
    cancel: &AtomicBool,
) -> Result<Arc<ManufacturingAsset>, JobError> {
    let CompileRequest {
        project_id,
        revision,
        definition_ids,
        stored_definitions,
        occurrences,
        intent,
        surface_overlays,
        process_document,
        surface_images,
        byte_budget,
    } = request;
    let definitions = definition_ids
        .iter()
        .map(|id| {
            residents
                .get(id)
                .map(|resident| (id.clone(), resident.definition.clone()))
                .ok_or_else(|| {
                    ManufacturingError::new(
                        ManufacturingErrorCode::StaleRevision,
                        "manufacturing definition was reclaimed",
                    )
                })
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let spiling_manufacturing::ManufacturingResult { bundle, program } =
        spiling_manufacturing::compile(
            spiling_manufacturing::CompilerInput {
                project_id: &project_id,
                revision,
                definitions: &definitions,
                stored_definitions: &stored_definitions,
                occurrences: &occurrences,
                intent: &intent,
                surface_overlays: &surface_overlays,
                process_document: &process_document,
                surface_images: &surface_images,
            },
            cancel,
        )?;
    // Encode straight into compressed frames; no whole JSON buffer exists.
    let exceeded = || {
        ManufacturingError::new(
            ManufacturingErrorCode::ResourceLimit,
            "encoded manufacturing bundle exceeds remaining retained/staged budget",
        )
    };
    let mut writer = spiling_chunks::ChunkWriter::new(u64::from(MAX_MANUFACTURING_BUNDLE_BYTES));
    serde_json::to_writer(&mut writer, &bundle).map_err(|_| exceeded())?;
    let encoded = writer.finish().map_err(|_| exceeded())?;
    if (encoded.retained_bytes() + program.retained_bytes()) as u64 > u64::from(byte_budget) {
        return Err(exceeded().into());
    }
    if cancel.load(Ordering::Acquire) {
        return Err(ManufacturingError::new(
            ManufacturingErrorCode::Cancelled,
            "manufacturing compile cancelled",
        )
        .into());
    }
    let record = ManufacturingArtifactRecord {
        hash: SourceHash::from_digest(encoded.digest()),
        input_hash: bundle.provenance.input_hash.clone(),
        byte_count: encoded.len() as u32,
        summary: bundle.summary(),
        program: bundle.program.clone(),
    };
    drop(bundle);
    ManufacturingAsset::new(record, encoded, program)
        .map(Arc::new)
        .map_err(super::manufacturing::project_error)
        .map_err(JobError::from)
}

#[cfg(test)]
mod image_import_tests {
    use super::*;

    #[test]
    fn alternative_receipt_refuses_control_overflow_before_publication() {
        let maximum = spiling_contracts::MAX_CONTROL_BYTES as usize - 4096;
        assert!(check_alternative_receipt_budget(maximum).is_ok());
        let error = check_alternative_receipt_budget(maximum + 1).unwrap_err();
        assert_eq!(error.code, ManufacturingErrorCode::ResourceLimit);
        assert!(check_alternative_receipt_budget(usize::MAX).is_err());
    }

    #[test]
    fn captured_canonical_budget_rejects_before_retained_asset_admission() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("budget.pgm");
        let canonical = b"P5\n2 2\n255\n\x00\x40\x80\xff";
        std::fs::write(&path, canonical).unwrap();
        let native = NativePath::from_os_str(path.as_os_str()).unwrap();
        let cancel = AtomicBool::new(false);
        // The canonical copy is separate from the bounded encoded-source
        // allocation, including when those bytes happen to be identical PGM.
        let rejected = import_surface_image(native.clone(), canonical.len() - 1, &cancel);
        assert!(
            matches!(rejected, Err(JobError::Project { error }) if error.code == ProjectErrorCode::ResourceLimit)
        );
        let admitted = import_surface_image(native, canonical.len(), &cancel).unwrap();
        assert_eq!(admitted.bytes().as_slice(), canonical);
        assert!(admitted.bytes().capacity() <= canonical.len());

        let mut png_bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png_bytes, 2, 2);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&canonical[11..])
                .unwrap();
        }
        let path = directory.path().join("budget.png");
        std::fs::write(&path, png_bytes).unwrap();
        let native = NativePath::from_os_str(path.as_os_str()).unwrap();
        let rejected = import_surface_image(native.clone(), canonical.len() - 1, &cancel);
        assert!(
            matches!(rejected, Err(JobError::Project { error }) if error.code == ProjectErrorCode::ResourceLimit)
        );
        let admitted = import_surface_image(native, canonical.len(), &cancel).unwrap();
        assert_eq!(admitted.bytes().as_slice(), canonical);
        assert!(admitted.bytes().capacity() <= canonical.len());
    }
}

#[cfg(test)]
mod geometry_only_tests {
    use super::*;
    use spiling_geometry::{BoundsPolicy, GeometryWorkBudget};

    #[test]
    fn display_budget_refusal_preserves_native_queries_but_never_masks_cancellation() {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/geometry/box-mm.step");
        let cancel = AtomicBool::new(false);
        let (mut parts, mut residents) = import(
            ImportRequest {
                options: ModelImportOptions::default(),
                source: NativePath::from_os_str(path.as_os_str()).unwrap(),
                allowed_reuse: BTreeSet::new(),
                retained_source_bytes: 0,
                budget: MeshBudget {
                    vertices: 0,
                    triangles: 0,
                    bytes: 0,
                },
            },
            &BTreeMap::new(),
            &cancel,
            &mut GeometryWorkBudget::query(),
            &|_| {},
        )
        .unwrap();
        assert_eq!((parts.len(), residents.len()), (1, 1));
        let (imported, placements) = parts.pop().unwrap();
        assert_eq!(placements, vec![RigidPoseMm::IDENTITY]);
        assert!(matches!(
            imported.display,
            ImportedDisplay::Unavailable(GeometryError {
                code: GeometryErrorCode::ResourceLimit,
                ..
            })
        ));
        let resident = residents.pop().unwrap();
        assert_eq!(resident.definition.faces().len(), 6);
        let mut work = GeometryWorkBudget::new(20_000_000);
        for (axis, expected_maximum) in [20.0, 10.0, 8.0].into_iter().enumerate() {
            let mut direction = [0.0; 3];
            direction[axis] = 1.0;
            let bounds = spiling_geometry::placed_bounds(
                resident.definition.native().unwrap(),
                RigidPoseMm::IDENTITY,
                direction,
                BoundsPolicy {
                    maximum_endpoint_uncertainty_mm: 1.0e-4,
                },
                &mut work,
                &cancel,
            )
            .unwrap();
            assert!(bounds.minimum[0] <= 0.0 && bounds.minimum[1] >= 0.0);
            assert!(bounds.maximum[0] <= expected_maximum && bounds.maximum[1] >= expected_maximum);
            assert!(bounds.achieved_uncertainty_mm <= 1.0e-4);
        }
        cancel.store(true, Ordering::Release);
        let mut capacity = Capacity::new(
            &BTreeMap::new(),
            MeshBudget {
                vertices: 0,
                triangles: 0,
                bytes: 0,
            },
            0,
        );
        assert!(matches!(
            capacity.display(
                &resident.definition,
                &cancel,
                &mut GeometryWorkBudget::query(),
                &|_| {},
            ),
            Err(GeometryError {
                code: GeometryErrorCode::Cancelled,
                ..
            })
        ));
    }
}

#[cfg(test)]
mod source_admission_tests {
    use super::*;

    #[test]
    fn shared_sources_charge_once_but_recaptures_and_repeated_refusals_do_not_escape_admission() {
        let first = Arc::new(vec![1u8; 64]);
        let recapture = Arc::new(first.as_ref().clone());
        let mut capacity = Capacity::new(
            &BTreeMap::new(),
            MeshBudget {
                vertices: 0,
                triangles: 0,
                bytes: 0,
            },
            first.capacity(),
        );
        capacity.charge_source(&first).unwrap();
        capacity.charge_source(&Arc::clone(&first)).unwrap();
        for _ in 0..2 {
            assert_eq!(
                capacity.charge_source(&recapture).unwrap_err().code,
                GeometryErrorCode::ResourceLimit
            );
        }
        assert_eq!(capacity.source_bytes, 0);
    }
}
