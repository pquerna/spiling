// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{check_cancel, error, placement, source};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use spiling_contracts::{
    display::{EncodedSection, SectionChunkBuilder},
    geometry::*,
    manufacturing::*,
    project::{ProjectError, ProjectErrorCode, StoredDefinition},
};
use spiling_core::{
    ManufacturingAsset, Project, SaveSnapshot, SourceAsset,
    storage::{self, SaveReceipt, SaveStage, Storage},
};
use spiling_geometry::{Definition, MeshArtifact, NativeSection};
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
    pub byte_budget: u32,
}
pub enum Work {
    Import {
        source: NativePath,
        allowed_reuse: BTreeSet<DefinitionId>,
        budget: MeshBudget,
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
    Verify {
        asset: Arc<ManufacturingAsset>,
    },
    Section {
        session: SessionId,
        artifact: ArtifactId,
        revision: SceneRevision,
        plane: PlaneMm,
        occurrences: Vec<OccurrenceRecord>,
    },
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
pub struct Imported {
    pub definition_id: DefinitionId,
    pub provenance: SourceProvenance,
    pub source: Arc<SourceAsset>,
    pub bounds: AabbMm,
    pub faces: Vec<FaceInfo>,
    pub mesh: Option<MeshArtifact>,
}
pub enum Product {
    Import(Imported),
    Open {
        project: Project,
        imports: Vec<Imported>,
    },
    Saved(SaveReceipt),
    Section(EncodedSection),
    Compiled(Arc<ManufacturingAsset>),
    Verified {
        record: ManufacturingArtifactRecord,
        report: VerificationReport,
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
                        if imported.mesh.is_none() {
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
            Work::Verify { asset } => {
                verify_asset(&asset, &cancel).map(|report| Product::Verified {
                    record: asset.record.clone(),
                    report,
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
    faces: u32,
    mesh: MeshBudget,
}
impl Capacity {
    fn new(definitions: &BTreeMap<DefinitionId, Resident>, budget: MeshBudget) -> Self {
        Self {
            definitions: MAX_DEFINITIONS - definitions.len() as u32,
            source_bytes: MAX_LIVE_SOURCE_BYTES
                - definitions
                    .values()
                    .map(|r| r.definition.source_bytes())
                    .sum::<u32>(),
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
        let definition = spiling_geometry::import_step_with_face_limit(bytes, cancel, self.faces)?;
        let faces = definition.faces().len() as u32;
        if faces > self.faces {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                "live/staged native face budget exceeded",
            ));
        }
        self.definitions -= 1;
        self.source_bytes -= bytes.len() as u32;
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
            DisplayProfile::MeshMm005V1,
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
    mesh: Option<MeshArtifact>,
) -> Imported {
    Imported {
        definition_id: definition.id().clone(),
        provenance: source.record.provenance.clone(),
        source,
        bounds: definition.bounds_mm(),
        faces: definition.faces().to_vec(),
        mesh,
    }
}
fn resident(definition: Definition, source: &SourceAsset, mesh: &MeshArtifact) -> Resident {
    Resident {
        definition: Arc::new(definition),
        record: source.record.clone(),
        vertices: mesh.vertex_count,
        triangles: mesh.triangle_count,
        mesh_bytes: mesh.total_bytes,
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
    let captured = source::capture(source, MAX_SOURCE_BYTES, cancel)?;
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
        return Ok((imported(&resident.definition, source, None), None));
    }
    let mut capacity = Capacity::new(definitions, budget);
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
    let mesh = capacity.tessellate(&definition, cancel)?;
    let resident = resident(definition, &source, &mesh);
    let imported = imported(&resident.definition, source, Some(mesh));
    check_cancel(cancel)?;
    Ok((imported, Some(resident)))
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
    } = request;
    let path = project_path(path)?;
    // Old current/history allocations survive until ACK, even for identical IDs.
    // Enforce their remaining budget before the loader allocates incoming bytes.
    let source_budget = MAX_LIVE_SOURCE_BYTES
        .checked_sub(retained_source_bytes)
        .ok_or_else(|| {
            ProjectError::new(
                ProjectErrorCode::ResourceLimit,
                "retained captured source budget exceeded",
            )
        })?;
    let loaded = storage::open_reusing(
        &path,
        read_only,
        recover_previous,
        cancel,
        existing,
        source_budget as usize,
        (MAX_MANUFACTURING_RETAINED_BYTES - retained_manufacturing_bytes) as usize,
    )?;
    let mut capacity = Capacity::new(definitions, budget);
    let mut pending = BTreeMap::new();
    let mut imports = Vec::with_capacity(loaded.sources.len());
    for (id, source) in &loaded.sources {
        check_cancel(cancel)?;
        if allowed_reuse.contains(id)
            && let Some(resident) = definitions.get(id)
        {
            validate_provenance(&resident.definition, source)?;
            imports.push(imported(&resident.definition, source.clone(), None));
        } else {
            let definition = capacity.admit(source.bytes.as_slice(), cancel)?;
            validate_provenance(&definition, source)?;
            let mesh = capacity.tessellate(&definition, cancel)?;
            let resident = resident(definition, source, &mesh);
            imports.push(imported(&resident.definition, source.clone(), Some(mesh)));
            pending.insert(id.clone(), resident);
        }
    }
    check_cancel(cancel)?;
    if let Some(asset) = &loaded.manufacturing_artifact {
        verify_asset(asset, cancel)?;
    }
    let project = Project::from_loaded(loaded)?;
    check_cancel(cancel)?;
    Ok(Opened {
        project,
        imports,
        residents: pending,
    })
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
    let frame = PlaneFrameMm::from_plane(plane)?;
    let mut builder = SectionChunkBuilder::new(session, artifact, revision, plane)?;
    let mut cache = BTreeMap::<(DefinitionId, [u64; 4]), NativeSection>::new();
    let mut cache_bytes = 0usize;
    for occurrence in occurrences {
        check_cancel(cancel)?;
        let definition = definitions.get(&occurrence.definition_id).ok_or_else(|| {
            error(
                GeometryErrorCode::StaleRevision,
                "section definition was reclaimed",
            )
        })?;
        let local = placement::inverse_plane(occurrence.pose, plane)?;
        let (local, key) = placement::canonical_plane(local)?;
        let key = (occurrence.definition_id.clone(), key);
        if !cache.contains_key(&key) {
            let native = spiling_geometry::section_with_byte_limit(
                &definition.definition,
                local,
                cancel,
                MAX_SECTION_BYTES - cache_bytes as u32,
            )?;
            // Account owned loop storage plus conservative complete-loop packing overhead.
            let bytes: usize = native
                .loops
                .iter()
                .map(|l| l.points_mm.len() * 24 + 72)
                .sum::<usize>()
                + 192;
            cache_bytes = cache_bytes.checked_add(bytes).ok_or_else(|| {
                error(
                    GeometryErrorCode::ResourceLimit,
                    "native section cache overflow",
                )
            })?;
            if cache_bytes > MAX_SECTION_BYTES as usize {
                return Err(error(
                    GeometryErrorCode::ResourceLimit,
                    "native section job cache exceeds budget",
                ));
            }
            cache.insert(key.clone(), native);
        }
        let native = &cache[&key];
        let translation_relative: [f64; 3] =
            std::array::from_fn(|i| occurrence.pose.translation_mm[i] - frame.origin_mm[i]);
        let mut loops: Vec<(bool, [f64; 4], Vec<[f64; 3]>)> =
            Vec::with_capacity(native.loops.len());
        for boundary in &native.loops {
            check_cancel(cancel)?;
            let mut points: Vec<_> = boundary
                .points_mm
                .iter()
                .map(|p| {
                    let p = placement::rotate(occurrence.pose, *p);
                    std::array::from_fn(|i| p[i] + translation_relative[i])
                })
                .collect();
            let projected = |p: [f64; 3]| [dot(p, frame.x_axis), dot(p, frame.y_axis)];
            if points.len() < 4 {
                return Err(error(
                    GeometryErrorCode::KernelFailure,
                    "native section returned an invalid loop",
                ));
            }
            points.pop();
            let first = (0..points.len())
                .min_by(|a, b| compare2(projected(points[*a]), projected(points[*b])))
                .unwrap();
            points.rotate_left(first);
            points.push(points[0]);
            let mut bounds = [
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            ];
            for point in &points {
                let [x, y] = projected(*point);
                bounds[0] = bounds[0].min(x);
                bounds[1] = bounds[1].min(y);
                bounds[2] = bounds[2].max(x);
                bounds[3] = bounds[3].max(y);
            }
            loops.push((boundary.is_hole, bounds, points));
        }
        loops.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| {
                    a.1.iter()
                        .zip(b.1)
                        .map(|(a, b)| a.total_cmp(&b))
                        .find(|c| !c.is_eq())
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| {
                    a.2.iter()
                        .zip(&b.2)
                        .map(|(a, b)| {
                            compare2(
                                [dot(*a, frame.x_axis), dot(*a, frame.y_axis)],
                                [dot(*b, frame.x_axis), dot(*b, frame.y_axis)],
                            )
                        })
                        .find(|c| !c.is_eq())
                        .unwrap_or_else(|| a.2.len().cmp(&b.2.len()))
                })
        });
        for (hole, _, points) in loops {
            builder.push_loop_relative(
                occurrence.occurrence_id,
                occurrence.definition_id.clone(),
                hole,
                &points,
            )?;
        }
    }
    check_cancel(cancel)?;
    builder.finish()
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}
fn compare2(a: [f64; 2], b: [f64; 2]) -> std::cmp::Ordering {
    a[0].total_cmp(&b[0]).then_with(|| a[1].total_cmp(&b[1]))
}

fn verify_asset(
    asset: &ManufacturingAsset,
    cancel: &AtomicBool,
) -> Result<VerificationReport, JobError> {
    let bundle = decode_bundle(asset.bytes.as_slice()).map_err(JobError::from)?;
    spiling_manufacturing::verify_bundle(&bundle, cancel).map_err(JobError::from)
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
