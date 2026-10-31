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
    project::{ProjectError, ProjectErrorCode, StoredDefinition},
    surface::{SurfaceOverlays, SurfaceScope},
};
use spiling_core::{
    ManufacturingAsset, Project, SaveSnapshot, SourceAsset, SurfaceImageAsset,
    storage::{self, SaveReceipt, SaveStage, Storage},
};
use spiling_geometry::{
    Definition, GeometryWorkBudget, MAX_PREPARED_SECTION_BYTES, MAX_PREPARED_SECTION_SOURCE_BYTES,
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
    // handles on this same worker; Definition's native caches are not Sync.
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

pub struct OpenRequest {
    pub path: NativePath,
    pub read_only: bool,
    pub recover_previous: bool,
    pub existing: Option<Arc<Storage>>,
    pub allowed_reuse: BTreeSet<DefinitionId>,
    pub retained_source_bytes: u32,
    pub retained_manufacturing_bytes: u64,
    pub budget: MeshBudget,
    pub retained_surface_image_bytes: usize,
    pub retained_appearance_bytes: usize,
}
struct Opened {
    project: Project,
    imports: Vec<Imported>,
    residents: BTreeMap<DefinitionId, Resident>,
}
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
    ApplyOpticalConstruction {
        snapshot: Arc<spiling_core::Snapshot>,
        base: spiling_contracts::project::ProjectRevision,
        application: spiling_contracts::manufacturing::ApplyOpticalConstruction,
    },
    Import {
        source: NativePath,
        allowed_reuse: BTreeSet<DefinitionId>,
        budget: MeshBudget,
    },
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
        existing: Option<Arc<Storage>>,
        target: Option<NativePath>,
        commit: crate::session::CommitToken,
        fault: Option<crate::ProjectFault>,
    },
    Compile(CompileRequest),
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
    OpticalConstruction(spiling_core::OpticalConstructionCandidate),
    Import(Imported),
    Open {
        project: Project,
        imports: Vec<Imported>,
    },
    Saved(SaveReceipt),
    SurfaceImage(Arc<SurfaceImageAsset>),
    AppearancePalette(Arc<spiling_core::PaletteAsset>),
    AppearanceReport(Arc<super::appearance::ReportPublication>),
    OpticalReport(Arc<super::optical::OpticalReportPublication>),
    Section(EncodedSection),
    Compiled(Arc<ManufacturingAsset>),
    Preflight(PreflightReport),
    BedPosePrepared(BedPosePreparation),
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
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Worker {
    pub fn new() -> Self {
        let (command, commands) = mpsc::sync_channel(1);
        let (results, result) = mpsc::sync_channel(1);
        let live = Arc::new(Mutex::new(LiveSnapshot::default()));
        let snapshot = live.clone();
        let thread = std::thread::spawn(move || {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(commands, results, snapshot)
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
fn run(commands: Receiver<Command>, results: SyncSender<Event>, live: Live) {
    let mut definitions = BTreeMap::<DefinitionId, Resident>::new();
    let mut epoch = 0;
    loop {
        reclaim(&mut definitions, &live, &mut epoch);
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
        let result = match work {
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
            Work::Import {
                source,
                allowed_reuse,
                budget,
            } => import(source, &definitions, &allowed_reuse, budget, &cancel).map(
                |(product, resident)| {
                    if let Some(resident) = resident {
                        pending.insert(resident.definition.id().clone(), resident);
                    }
                    Product::Import(product)
                },
            ),
            Work::Open(request) => open(request, &definitions, &cancel).map(
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
                existing,
                target,
                commit,
                fault,
            } => save(snapshot, existing, target, commit, fault, &cancel).map(Product::Saved),
            Work::Compile(request) => {
                compile(request, &definitions, &cancel).map(Product::Compiled)
            }
            Work::Preflight(request) => {
                preflight(request, &definitions, &cancel).map(Product::Preflight)
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
    source_bytes: u32,
    query_proof_bytes: usize,
    faces: u32,
    mesh: MeshBudget,
}
impl Capacity {
    fn new(definitions: &BTreeMap<DefinitionId, Resident>, budget: MeshBudget) -> Self {
        Self {
            definitions: MAX_DEFINITIONS - definitions.len() as u32,
            source_bytes: (MAX_LIVE_SOURCE_BYTES as usize).saturating_sub(
                definitions.values().fold(0usize, |bytes, resident| {
                    bytes
                        .saturating_add(resident.definition.source_bytes() as usize)
                        .saturating_add(resident.definition.retained_query_proof_bytes())
                }),
            ) as u32,
            query_proof_bytes: MAX_LIVE_SOURCE_BYTES as usize,
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
    fn admit(&mut self, bytes: &[u8], cancel: &AtomicBool) -> Result<Definition, GeometryError> {
        check_cancel(cancel)?;
        if self.definitions == 0 {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                "resident/staged native definition cap exceeded",
            ));
        }
        if bytes.len() > self.source_bytes as usize {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                "live/staged native source budget exceeded",
            ));
        }
        let definition = spiling_geometry::from_step_bytes_with_limits(
            bytes,
            cancel,
            self.faces,
            self.query_proof_bytes
                .min(self.source_bytes as usize - bytes.len()),
        )?;
        let query_proof_bytes = definition.retained_query_proof_bytes();
        let accounted_bytes = bytes
            .len()
            .checked_add(query_proof_bytes)
            .filter(|bytes| *bytes <= self.source_bytes as usize)
            .ok_or_else(|| {
                error(
                    GeometryErrorCode::ResourceLimit,
                    "live/staged native source and query-proof budget exceeded",
                )
            })?;
        let remaining_proof_bytes = self
            .query_proof_bytes
            .checked_sub(query_proof_bytes)
            .ok_or_else(|| {
                error(
                    GeometryErrorCode::ResourceLimit,
                    "captured source allocations leave no native query-proof capacity",
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
        self.source_bytes -= accounted_bytes as u32;
        self.query_proof_bytes = remaining_proof_bytes;
        self.faces -= faces;
        check_cancel(cancel)?;
        Ok(definition)
    }
    fn tessellate(
        &mut self,
        definition: &Definition,
        cancel: &AtomicBool,
    ) -> Result<MeshArtifact, GeometryError> {
        let mesh = spiling_geometry::tessellate_with_budget(
            definition,
            DisplayProfile::MeshTrimmedMm005V2,
            cancel,
            self.mesh,
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
    ) -> Result<ImportedDisplay, GeometryError> {
        match self.tessellate(definition, cancel) {
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
fn import(
    source: NativePath,
    definitions: &BTreeMap<DefinitionId, Resident>,
    reuse: &BTreeSet<DefinitionId>,
    budget: MeshBudget,
    cancel: &AtomicBool,
) -> Result<(Imported, Option<Resident>), JobError> {
    check_cancel(cancel)?;
    let mut capacity = Capacity::new(definitions, budget);
    let captured = source::capture(source, capacity.source_bytes.min(MAX_SOURCE_BYTES), cancel)?;
    let digest: [u8; 32] = Sha256::digest(&captured.bytes).into();
    let id = DefinitionId::from_source_sha256(&digest);
    if reuse.contains(&id)
        && let Some(resident) = definitions.get(&id)
    {
        // Transfer the exact captured allocation, but preserve the first admitted
        // source's label and immutable record on an exact-byte reimport.
        let source = Arc::new(SourceAsset::new(resident.record.clone(), captured.bytes)?);
        validate_provenance(&resident.definition, &source)?;
        check_cancel(cancel)?;
        return Ok((
            imported(&resident.definition, source, ImportedDisplay::Reused),
            None,
        ));
    }
    let definition = capacity.admit(&captured.bytes, cancel)?;
    let mut provenance = definition.provenance().clone();
    provenance.source_name = captured.label;
    let source = Arc::new(SourceAsset::new(
        StoredDefinition {
            definition_id: definition.id().clone(),
            provenance,
        },
        captured.bytes,
    )?);
    let display = capacity.display(&definition, cancel)?;
    let resident = resident(definition, &source, &display);
    let imported = imported(&resident.definition, source, display);
    check_cancel(cancel)?;
    Ok((imported, Some(resident)))
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
fn project_path(path: NativePath) -> Result<PathBuf, ProjectError> {
    path.to_os_string()
        .map(PathBuf::from)
        .map_err(|e| ProjectError::new(ProjectErrorCode::Io, e.message))
}
fn open(
    request: OpenRequest,
    definitions: &BTreeMap<DefinitionId, Resident>,
    cancel: &AtomicBool,
) -> Result<Opened, JobError> {
    let OpenRequest {
        path,
        read_only,
        recover_previous,
        existing,
        allowed_reuse,
        retained_source_bytes,
        retained_manufacturing_bytes,
        budget,
        retained_surface_image_bytes,
        retained_appearance_bytes,
    } = request;
    let path = project_path(path)?;
    // Old current/history allocations survive until ACK, even for identical IDs.
    // Enforce their remaining budget before the loader allocates incoming bytes.
    let retained_query_proof_bytes = definitions.values().fold(0usize, |bytes, resident| {
        bytes.saturating_add(resident.definition.retained_query_proof_bytes())
    });
    let source_budget = (MAX_LIVE_SOURCE_BYTES as usize)
        .checked_sub(retained_source_bytes as usize)
        .and_then(|bytes| bytes.checked_sub(retained_query_proof_bytes))
        .ok_or_else(|| {
            ProjectError::new(
                ProjectErrorCode::ResourceLimit,
                "retained captured source and native query-proof budget exceeded",
            )
        })?;
    let loaded = storage::open_reusing(
        &path,
        read_only,
        recover_previous,
        cancel,
        existing,
        source_budget,
        (MAX_MANUFACTURING_RETAINED_BYTES - retained_manufacturing_bytes) as usize,
        spiling_contracts::surface::MAX_SURFACE_IMAGE_RETAINED_BYTES
            .checked_sub(retained_surface_image_bytes)
            .ok_or_else(|| {
                ProjectError::new(
                    ProjectErrorCode::ResourceLimit,
                    "retained surface image budget exceeded",
                )
            })?,
        spiling_core::MAX_APPEARANCE_RETAINED_BYTES
            .checked_sub(retained_appearance_bytes)
            .ok_or_else(|| {
                ProjectError::new(
                    ProjectErrorCode::ResourceLimit,
                    "retained appearance palette budget exceeded",
                )
            })?,
    )?;
    let mut capacity = Capacity::new(definitions, budget);
    let incoming_source_bytes = loaded.sources.values().fold(0usize, |bytes, source| {
        bytes.saturating_add(source.bytes.len())
    });
    capacity.query_proof_bytes = source_budget
        .checked_sub(incoming_source_bytes)
        .ok_or_else(|| {
            ProjectError::new(
                ProjectErrorCode::ResourceLimit,
                "captured incoming sources leave no native query-proof capacity",
            )
        })?;
    let mut pending = BTreeMap::new();
    let mut imports = Vec::with_capacity(loaded.sources.len());
    for (id, source) in &loaded.sources {
        check_cancel(cancel)?;
        if allowed_reuse.contains(id)
            && let Some(resident) = definitions.get(id)
        {
            validate_provenance(&resident.definition, source)?;
            validate_surface_targets(&resident.definition, &loaded.manifest.surface_overlays)?;
            imports.push(imported(
                &resident.definition,
                source.clone(),
                ImportedDisplay::Reused,
            ));
        } else {
            let definition = capacity.admit(source.bytes.as_slice(), cancel)?;
            validate_provenance(&definition, source)?;
            validate_surface_targets(&definition, &loaded.manifest.surface_overlays)?;
            let display = capacity.display(&definition, cancel)?;
            let resident = resident(definition, source, &display);
            imports.push(imported(&resident.definition, source.clone(), display));
            pending.insert(id.clone(), resident);
        }
    }
    check_cancel(cancel)?;
    if let Some(asset) = &loaded.manufacturing_artifact {
        inspect_asset(asset, VerificationOutput::ReportOnly, cancel)?;
    }
    let project = Project::from_loaded(loaded)?;
    check_cancel(cancel)?;
    Ok(Opened {
        project,
        imports,
        residents: pending,
    })
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
    snapshot: SaveSnapshot,
    existing: Option<Arc<Storage>>,
    target: Option<NativePath>,
    commit: crate::session::CommitToken,
    fault: Option<crate::ProjectFault>,
    cancel: &AtomicBool,
) -> Result<SaveReceipt, JobError> {
    let target = target.map(project_path).transpose()?;
    let receipt = storage::save(snapshot, existing, target, cancel, |stage| {
        if matches!(stage, SaveStage::BeforeManifestReplace) && !commit.begin_commit() {
            cancel.store(true, Ordering::Release);
            return;
        }
        if fault.as_ref().is_some_and(|fault| fault.matches(stage)) {
            std::process::exit(86);
        }
    })?;
    // No cancellation check here: once replacement starts, the core completes
    // durability and returns the receipt even if the cooperative flag races.
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
    let plane = plane.normalized()?;
    let mut builder = SectionChunkBuilder::new(session, artifact, revision, plane)?;
    let mut work = GeometryWorkBudget::new(50_000_000);
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
        if source.is_none() {
            // Raw sections need native coefficient correspondence, not material
            // charts/vertex plans or their independent manufacturing admission.
            let prepared = spiling_geometry::prepare_section_source(
                &definition.definition,
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
        let loops = native.into_loops();
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
        if *remaining_uses == 0 {
            prepared_bytes -= source
                .take()
                .expect("last section use retains its source")
                .retained_bytes();
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
    let bundle = decode_bundle(asset.bytes.as_slice()).map_err(JobError::from)?;
    let mut inspection =
        spiling_manufacturing::inspect_bundle(&bundle, &asset.record.hash, output, cancel)
            .map_err(JobError::from)?;
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

struct BundleWriter {
    bytes: Vec<u8>,
    limit: usize,
}
impl std::io::Write for BundleWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let length = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .filter(|length| *length <= self.limit)
            .ok_or_else(|| {
                std::io::Error::other("manufacturing bundle exceeds remaining budget")
            })?;
        if length > self.bytes.capacity() {
            let capacity = length
                .max(self.bytes.capacity().saturating_mul(2))
                .min(self.limit);
            self.bytes
                .try_reserve_exact(capacity - self.bytes.len())
                .map_err(std::io::Error::other)?;
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
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
    let bundle = spiling_manufacturing::compile(
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
    let mut writer = BundleWriter {
        bytes: Vec::new(),
        limit: byte_budget.min(MAX_MANUFACTURING_BUNDLE_BYTES) as usize,
    };
    serde_json::to_writer(&mut writer, &bundle).map_err(|_| {
        ManufacturingError::new(
            ManufacturingErrorCode::ResourceLimit,
            "encoded manufacturing bundle exceeds remaining retained/staged budget",
        )
    })?;
    if cancel.load(Ordering::Acquire) {
        return Err(ManufacturingError::new(
            ManufacturingErrorCode::Cancelled,
            "manufacturing compile cancelled",
        )
        .into());
    }
    let hash = SourceHash::from_bytes(&writer.bytes);
    let record = ManufacturingArtifactRecord {
        hash,
        input_hash: bundle.provenance.input_hash.clone(),
        byte_count: writer.bytes.len() as u32,
        summary: bundle.summary(),
    };
    drop(bundle);
    ManufacturingAsset::new(record, writer.bytes)
        .map(Arc::new)
        .map_err(super::manufacturing::project_error)
        .map_err(JobError::from)
}

#[cfg(test)]
mod image_import_tests {
    use super::*;

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
        let (imported, resident) = import(
            NativePath::from_os_str(path.as_os_str()).unwrap(),
            &BTreeMap::new(),
            &BTreeSet::new(),
            MeshBudget {
                vertices: 0,
                triangles: 0,
                bytes: 0,
            },
            &cancel,
        )
        .unwrap();
        assert!(matches!(
            imported.display,
            ImportedDisplay::Unavailable(GeometryError {
                code: GeometryErrorCode::ResourceLimit,
                ..
            })
        ));
        let resident = resident.unwrap();
        assert_eq!(resident.definition.faces().len(), 6);
        let mut work = GeometryWorkBudget::new(20_000_000);
        for (axis, expected_maximum) in [20.0, 10.0, 8.0].into_iter().enumerate() {
            let mut direction = [0.0; 3];
            direction[axis] = 1.0;
            let bounds = spiling_geometry::placed_bounds(
                &resident.definition,
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
        );
        assert!(matches!(
            capacity.display(&resident.definition, &cancel),
            Err(GeometryError {
                code: GeometryErrorCode::Cancelled,
                ..
            })
        ));
    }
}
