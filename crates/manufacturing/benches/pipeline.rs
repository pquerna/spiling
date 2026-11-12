// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Fixed real-source workloads through production facades; no benchmark planner.
use spiling_contracts::{
    compute::{ComputeBackend, ComputeSettings},
    geometry::*,
    manufacturing::*,
    process::ProcessDocument,
    project::{ProjectId, ProjectRevision, StoredDefinition},
    surface::SurfaceOverlays,
};
use spiling_manufacturing::{
    ChunkSource, CompilerInput, ManufacturingResult, compile, inspect_bundle, verify_bundle,
};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    collections::BTreeMap,
    error::Error,
    hint::black_box,
    io::{Read, Write},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Instant,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Benchmark-only live/peak heap accounting for the `residency` phase.
/// Production binaries keep the system allocator without counters.
struct Counting;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
// SAFETY: forwards every request unchanged to `System`; counters only observe sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            grow(layout.size());
        }
        ptr
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            grow(layout.size());
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let next = unsafe { System.realloc(ptr, layout, size) };
        if !next.is_null() {
            if size >= layout.size() {
                grow(size - layout.size());
            } else {
                LIVE.fetch_sub(layout.size() - size, Ordering::Relaxed);
            }
        }
        next
    }
}
fn grow(bytes: usize) {
    let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK.fetch_max(live, Ordering::Relaxed);
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;
/// Run `f`, returning its value and the peak live heap above the entry level.
fn peak_above<T>(f: impl FnOnce() -> Result<T>) -> Result<(T, usize)> {
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let value = f()?;
    Ok((value, PEAK.load(Ordering::Relaxed).saturating_sub(base)))
}
fn live() -> usize {
    LIVE.load(Ordering::Relaxed)
}
const BORE: &[u8] = include_bytes!("../../../fixtures/geometry/through-hole.step");
const BOX: &[u8] = include_bytes!("../../../fixtures/geometry/box-mm.step");
const INTENT: &[u8] =
    include_bytes!("../../../fixtures/manufacturing/mixed-width-combined.intent.json");
const BOX_MM: &str = include_str!("../../../fixtures/geometry/box-mm.step");
const SINGLE_TOOL: &[u8] =
    include_bytes!("../../../fixtures/manufacturing/complete-sparse.intent.json");

/// The 20 x 10 x 8 mm box fixture rescaled to `size`. Every coordinate and
/// edge length in it is 0, 20, 10 or 8, so each maps to one axis.
fn scaled_box(size: [f64; 3]) -> Vec<u8> {
    let axis = |value: &str, index: usize| -> String {
        match (value.trim(), index) {
            ("20.0", 0) => size[0].to_string(),
            ("10.0", 1) => size[1].to_string(),
            ("8.0", 2) => size[2].to_string(),
            (other, _) => other.to_string(),
        }
    };
    BOX_MM
        .lines()
        .map(|line| {
            if let Some((head, rest)) = line.split_once("CARTESIAN_POINT('', (") {
                let (tuple, tail) = rest.split_once(')').unwrap();
                let values: Vec<_> = tuple
                    .split(',')
                    .enumerate()
                    .map(|(i, v)| axis(v, i))
                    .collect();
                format!("{head}CARTESIAN_POINT('', ({}){tail}", values.join(", "))
            } else if let Some((head, rest)) = line.split_once("VECTOR('', ") {
                let (direction, magnitude) = rest.split_once(", ").unwrap();
                let (value, tail) = magnitude.split_once(')').unwrap();
                let scaled = match value {
                    "20.0" => size[0],
                    "10.0" => size[1],
                    "8.0" => size[2],
                    other => other.parse().unwrap(),
                };
                format!("{head}VECTOR('', {direction}, {scaled}){tail}")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        .into_bytes()
}

struct Scene {
    project: ProjectId,
    definitions: BTreeMap<DefinitionId, Arc<spiling_geometry::source::SourceDefinition>>,
    stored: Vec<StoredDefinition>,
    occurrences: Vec<OccurrenceRecord>,
    intent: ManufacturingIntent,
    surfaces: SurfaceOverlays,
    process: ProcessDocument,
    images: BTreeMap<SourceHash, Arc<spiling_surface_fields::HeightImage>>,
}
impl Scene {
    /// One scaled box on the single-tool complete workflow.
    fn sized_box(size: [f64; 3], infill: f64, layer: f64, cancel: &AtomicBool) -> Result<Self> {
        let step = scaled_box(size);
        let definition: Arc<spiling_geometry::source::SourceDefinition> =
            Arc::new(spiling_geometry::from_step_bytes_with_cancel(&step, cancel)?.into());
        let mut intent = decode_intent(SINGLE_TOOL)?;
        intent.recipe.infill_fraction = infill;
        intent.recipe.layer_height_mm = layer;
        // Park above the scaled part, within the build envelope.
        if let JobWorkflow::CompleteMarlin { job } = &mut intent.printer.workflow {
            job.park_position_mm[2] = (size[2] + 10.0).min(intent.printer.build_envelope.max[2]);
        }
        let occurrences = vec![OccurrenceRecord {
            occurrence_id: OccurrenceId::new(1).expect("one occurrence"),
            definition_id: definition.id().clone(),
            pose: RigidPoseMm {
                translation_mm: [10.0, 10.0, 0.0],
                ..RigidPoseMm::IDENTITY
            },
        }];
        intent.validate()?;
        Ok(Self {
            project: ProjectId::parse("00000000-0000-4000-8000-000000000001")?,
            stored: vec![StoredDefinition {
                definition_id: definition.id().clone(),
                provenance: definition.provenance().clone(),
            }],
            definitions: BTreeMap::from([(definition.id().clone(), definition)]),
            occurrences,
            intent,
            surfaces: SurfaceOverlays::empty(),
            process: ProcessDocument::empty(),
            images: BTreeMap::new(),
        })
    }
    // Production CompilerInput shares native definitions on this one thread.
    fn new(count: usize, primary: bool, cancel: &AtomicBool) -> Result<Self> {
        let mut definitions = BTreeMap::new();
        let mut ids = Vec::new();
        for bytes in if count == 1 {
            vec![BORE]
        } else {
            vec![BORE, BOX]
        } {
            let definition: Arc<spiling_geometry::source::SourceDefinition> =
                Arc::new(spiling_geometry::from_step_bytes_with_cancel(bytes, cancel)?.into());
            ids.push(definition.id().clone());
            definitions.insert(definition.id().clone(), definition);
        }
        let occurrences: Vec<_> = (0..count)
            .map(|i| OccurrenceRecord {
                occurrence_id: OccurrenceId::new(i as u32 + 1).expect("bounded count"),
                definition_id: ids[usize::from(i != 0)].clone(),
                pose: RigidPoseMm {
                    translation_mm: [
                        20.0 + (i % 4) as f64 * 50.0,
                        20.0 + (i / 4) as f64 * 25.0,
                        0.0,
                    ],
                    ..RigidPoseMm::IDENTITY
                },
            })
            .collect();
        let mut intent = decode_intent(INTENT)?;
        let JobWorkflow::FixedDualMarlin { job } = &mut intent.printer.workflow else {
            unreachable!()
        };
        for tool in &mut job.tools {
            for point in &mut tool.waste_line_xy_mm {
                point[1] = 215.0;
            }
        }
        let template = job.assignments[0].clone();
        job.assignments = occurrences
            .iter()
            .enumerate()
            .map(|(i, occurrence)| {
                let [x, y, _] = occurrence.pose.translation_mm;
                let mut row = OccurrenceMaterialAssignment {
                    occurrence_id: occurrence.occurrence_id,
                    work_envelope_mm: AabbMm::new(
                        [x - 2.0, y - 2.0, 0.0],
                        [x + 22.0, y + if i == 0 { 22.0 } else { 12.0 }, 8.01],
                    )
                    .expect("fixed finite workload"),
                    ..template.clone()
                };
                if primary {
                    row.interior.as_mut().unwrap().policy_override = Some(InteriorToolOverride {
                        enabled: true,
                        value: InteriorToolPolicy::PrimaryOnly,
                    });
                }
                row
            })
            .collect();
        intent.validate()?;
        Ok(Self {
            project: ProjectId::parse("00000000-0000-4000-8000-000000000001")?,
            stored: definitions
                .values()
                .map(|d| StoredDefinition {
                    definition_id: d.id().clone(),
                    provenance: d.provenance().clone(),
                })
                .collect(),
            definitions,
            occurrences,
            intent,
            surfaces: SurfaceOverlays::empty(),
            process: ProcessDocument::empty(),
            images: BTreeMap::new(),
        })
    }
    fn input(&self) -> CompilerInput<'_> {
        CompilerInput {
            project_id: &self.project,
            revision: ProjectRevision(1),
            definitions: &self.definitions,
            stored_definitions: &self.stored,
            occurrences: &self.occurrences,
            intent: &self.intent,
            surface_overlays: &self.surfaces,
            process_document: &self.process,
            surface_images: &self.images,
        }
    }
}

/// A captured result is the bundle document at `path` plus its raw program at
/// `path.program`, written together by `--write-bundle`.
fn read_result(path: &str) -> Result<ManufacturingResult> {
    let bundle = decode_bundle(&read_bundle(path)?)?;
    let mut program = Vec::new();
    std::fs::File::open(format!("{path}.program"))?
        .take(u64::from(MAX_MANUFACTURING_PROGRAM_BYTES) + 1)
        .read_to_end(&mut program)?;
    let program = ChunkSource::from_slice(&program, u64::from(MAX_MANUFACTURING_PROGRAM_BYTES))?;
    Ok(ManufacturingResult::new(bundle, program)?)
}
/// Which frozen workload to build: the fixed mixed-tool plate, or one scaled box.
struct SceneSpec {
    parts: usize,
    primary: bool,
    size: Option<[f64; 3]>,
    infill: f64,
    layer: f64,
}
impl SceneSpec {
    fn build(&self, cancel: &AtomicBool) -> Result<Scene> {
        match self.size {
            Some(size) => Scene::sized_box(size, self.infill, self.layer, cancel),
            None => Scene::new(self.parts, self.primary, cancel),
        }
    }
    fn label(&self) -> String {
        match self.size {
            Some([x, y, z]) => format!(
                "box={x}x{y}x{z} infill={} layer={}",
                self.infill, self.layer
            ),
            None => format!("parts={}", self.parts),
        }
    }
}
fn read_bundle(path: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(u64::from(MAX_MANUFACTURING_BUNDLE_BYTES) + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_MANUFACTURING_BUNDLE_BYTES as usize {
        return Err("bundle exceeds production byte limit".into());
    }
    Ok(bytes)
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let option = |name: &str, default: &str| -> Result<String> {
        match args.iter().position(|a| a == name) {
            Some(i) => args
                .get(i + 1)
                .cloned()
                .ok_or_else(|| format!("missing {name} value").into()),
            None => Ok(default.into()),
        }
    };
    // Cargo may append --bench. All operator options are explicit pairs.
    let allowed = [
        "--parts",
        "--box",
        "--infill",
        "--layer",
        "--samples",
        "--warmup",
        "--phase",
        "--policy",
        "--bundle",
        "--write-bundle",
        "--compute-backend",
        "--bench",
    ];
    let mut i = 0;
    while i < args.len() {
        if !allowed.contains(&args[i].as_str()) {
            return Err(format!("unknown option {}", args[i]).into());
        }
        i += if args[i] == "--bench" { 1 } else { 2 };
    }
    let parts: usize = option("--parts", "1")?.parse()?;
    let size = match option("--box", "")?.as_str() {
        "" => None,
        text => {
            let values: Vec<f64> = text
                .split(',')
                .map(str::parse)
                .collect::<std::result::Result<_, _>>()?;
            let size: [f64; 3] = values
                .try_into()
                .map_err(|_| "--box takes X,Y,Z millimetres")?;
            Some(size)
        }
    };
    let infill: f64 = option("--infill", "0.25")?.parse()?;
    let layer: f64 = option("--layer", "0.2")?.parse()?;
    let samples: usize = option("--samples", "3")?.parse()?;
    let warmup: usize = option("--warmup", "0")?.parse()?;
    let phase = option("--phase", "compile")?;
    let policy = option("--policy", "automatic")?;
    if ![1, 2, 8, 32].contains(&parts) || !(1..=20).contains(&samples) || warmup > 5 {
        return Err("parts=1/2/8/32, samples=1..20, warmup=0..5".into());
    }
    if ![
        "compile",
        "encode",
        "parse",
        "decode",
        "validate",
        "verify",
        "motion",
        "route",
        "residency",
    ]
    .contains(&phase.as_str())
        || !["automatic", "primary"].contains(&policy.as_str())
    {
        return Err("unknown phase or policy".into());
    }
    let file = option("--bundle", "")?;
    let save = option("--write-bundle", "")?;
    if phase == "residency" {
        let spec = SceneSpec {
            parts,
            primary: policy == "primary",
            size,
            infill,
            layer,
        };
        return residency(&spec, &file);
    }
    if phase == "compile" && !file.is_empty() {
        return Err("compile uses frozen native sources, not --bundle".into());
    }
    let compute_backend = ComputeBackend::parse(&option("--compute-backend", "auto")?)?;
    let compute_started = Instant::now();
    let compute = spiling_compute::configure(ComputeSettings {
        default_backend: compute_backend,
        fea_backend: None,
    })?;
    let compute_setup_us = compute_started.elapsed().as_micros();
    let cancel = AtomicBool::new(false);
    let setup = Instant::now();
    let scene = if file.is_empty() {
        Some(
            SceneSpec {
                parts,
                primary: policy == "primary",
                size,
                infill,
                layer,
            }
            .build(&cancel)?,
        )
    } else {
        None
    };
    let setup_us = setup.elapsed().as_micros();
    let mut prepared = if !file.is_empty() {
        Some(read_result(&file)?)
    } else if phase != "compile" {
        Some(compile(scene.as_ref().unwrap().input(), &cancel)?)
    } else {
        None
    };
    let encoded = prepared
        .as_ref()
        .map(|r| serde_json::to_vec(&r.bundle))
        .transpose()?;
    let program = prepared.as_ref().map(|r| r.program.clone());
    let encoded_hash = encoded.as_deref().map(SourceHash::from_bytes);
    let mut expected = prepared.as_ref().map(|b| b.bundle.program.hash.clone());
    for sample in 0..warmup + samples {
        if matches!(phase.as_str(), "compile" | "parse" | "decode") {
            drop(prepared.take());
        }
        let started = Instant::now();
        let mut result = None;
        match phase.as_str() {
            "compile" => {
                result = Some(compile(scene.as_ref().unwrap().input(), &cancel)?);
            }
            "encode" => {
                black_box(serde_json::to_vec(&prepared.as_ref().unwrap().bundle)?);
            }
            "parse" => {
                result = Some(ManufacturingResult::new(
                    serde_json::from_slice::<ManufacturingBundle>(encoded.as_ref().unwrap())?,
                    program.clone().unwrap(),
                )?);
            }
            "decode" => {
                result = Some(ManufacturingResult::new(
                    decode_bundle(encoded.as_ref().unwrap())?,
                    program.clone().unwrap(),
                )?);
            }
            "validate" => {
                black_box(prepared.as_ref().unwrap()).validate()?;
            }
            "verify" => {
                black_box(verify_bundle(
                    black_box(prepared.as_ref().unwrap()),
                    &cancel,
                )?);
            }
            "route" => {
                let b = prepared.as_ref().unwrap();
                let intent = b.plan.execution_intent(&b.provenance.intent)?;
                let JobWorkflow::FixedDualMarlin { job } = &intent.printer.workflow else {
                    return Err("route benchmark needs a captured fixed-dual job".into());
                };
                let mut checksum = 0usize;
                for path in b.plan.layers.iter().flat_map(|layer| &layer.paths) {
                    checksum += job.assignment_for_path(black_box(path))?;
                }
                black_box(checksum);
            }
            "motion" => {
                let b = prepared.as_ref().unwrap();
                black_box(inspect_bundle(
                    b,
                    encoded_hash.as_ref().unwrap(),
                    VerificationOutput::ReportAndMotion,
                    &cancel,
                )?);
            }
            _ => unreachable!(),
        }
        let elapsed_us = started.elapsed().as_micros();
        if let Some(bundle) = result {
            // Untimed semantic guard also qualifies parse-only output.
            bundle.validate()?;
            prepared = Some(bundle);
        }
        let b = prepared.as_ref().unwrap();
        let program_hash = b.bundle.program.hash.clone();
        if expected.as_ref().is_some_and(|h| h != &program_hash) {
            return Err("repeated output changed; timing is not acceptance".into());
        }
        expected = Some(program_hash.clone());
        if sample >= warmup {
            println!(
                "{}",
                serde_json::json!({"benchmark":"native-manufacturing-v1", "phase":phase, "policy":if file.is_empty() { policy.as_str() } else { "captured" }, "sample":sample-warmup, "samples":samples, "warmup":warmup, "setup_us":setup_us, "compute_setup_us":compute_setup_us, "compute":compute, "elapsed_us":elapsed_us, "engine_build":spiling_contracts::ENGINE_BUILD_ID, "kernel":spiling_geometry::KERNEL_REVISION, "input_hash":b.provenance.input_hash, "program_hash":program_hash, "occurrences":b.provenance.occurrences.len(), "definitions":b.provenance.definitions.len(), "layers":b.plan.layers.len(), "deposition_segments":b.verification.deposition_segments, "geometry_work":b.plan.native_geometry.geometry_work, "selection":b.plan.interior_selection.as_ref().map(|s| s.selected), "sources":b.provenance.definitions.iter().map(|d| &d.provenance.source_hash).collect::<Vec<_>>(), "debug_assertions":cfg!(debug_assertions), "intent_source":SourceHash::from_bytes(INTENT), "available_parallelism":std::thread::available_parallelism().map(|v|v.get()).unwrap_or(1), "rayon_threads":std::env::var("RAYON_NUM_THREADS").ok(), "software_only":true})
            );
        }
    }
    if !save.is_empty() {
        let result = prepared.as_ref().unwrap();
        let bytes = serde_json::to_vec(&result.bundle)?;
        if bytes.len() > MAX_MANUFACTURING_BUNDLE_BYTES as usize {
            return Err("encoded result exceeds production byte limit".into());
        }
        let create = |path: &str| {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(Path::new(path))
        };
        create(&save)?.write_all(&bytes)?;
        result
            .program
            .write_to(&mut create(&format!("{save}.program"))?)?;
    }
    Ok(())
}

/// Peak and retained heap per resident form of one result, single-threaded.
/// Compile peak includes planner, emitter and verifier transients together;
/// retained sizes come from dropping one owned part at a time.
fn residency(spec: &SceneSpec, file: &str) -> Result<()> {
    spiling_compute::configure(ComputeSettings {
        default_backend: ComputeBackend::parse("cpu")?,
        fea_backend: None,
    })?;
    let cancel = AtomicBool::new(false);
    let (bundle, compile_peak, setup) = if file.is_empty() {
        let scene = spec.build(&cancel)?;
        let before = live();
        let (bundle, peak) = peak_above(|| Ok(compile(scene.input(), &cancel)?))?;
        (bundle, Some(peak), live() - before)
    } else {
        (read_result(file)?, None, 0)
    };
    let (encoded, encode_peak) = peak_above(|| Ok(serde_json::to_vec(&bundle.bundle)?))?;
    let encoded_hash = SourceHash::from_bytes(&encoded);
    let (decoded, decode_peak) = peak_above(|| Ok(decode_bundle(&encoded)?))?;
    let frames = ChunkSource::from_slice(&encoded, u64::MAX)?;
    let started = Instant::now();
    let (framed, frames_decode_peak) =
        peak_above(|| Ok(decode_bundle_from(frames.len(), || frames.reader())?))?;
    let frames_decode_us = started.elapsed().as_micros();
    drop(framed);
    let decoded_retained = {
        let before = live();
        drop(decoded);
        before - live()
    };
    // Motion display is separately capped; a refusal is reported, not fatal.
    let (inspection, motion_peak) = peak_above(|| {
        Ok(inspect_bundle(
            &bundle,
            &encoded_hash,
            VerificationOutput::ReportAndMotion,
            &cancel,
        ))
    })?;
    let replay_retained = inspection
        .as_ref()
        .ok()
        .and_then(|i| i.replay.as_ref())
        .map_or(0, |m| m.bytes.retained_bytes());
    let (replay_bytes, motion_refusal) = match &inspection {
        Ok(inspection) => (
            inspection.replay.as_ref().map_or(0, |m| m.bytes.len()),
            None,
        ),
        Err(error) if error.code == ManufacturingErrorCode::ResourceLimit => {
            (0, Some(error.message.clone()))
        }
        Err(error) => return Err(error.clone().into()),
    };
    drop(inspection);
    let segments = bundle.verification.deposition_segments as usize;
    let travels = bundle.verification.travel_segments as usize;
    let program_len = bundle.program.len() as usize;
    let layers = bundle.plan.layers.len();
    let (points, paths) = bundle
        .plan
        .layers
        .iter()
        .flat_map(|l| &l.paths)
        .fold((0usize, 0usize), |(p, n), path| {
            (p + path.points_mm.len(), n + 1)
        });
    let ManufacturingResult {
        bundle: ManufacturingBundle { plan, .. },
        program,
    } = bundle;
    let program_retained = {
        let before = live();
        drop(program);
        before - live()
    };
    let plan_retained = {
        let before = live();
        drop(plan);
        before - live()
    };
    let encoded_len = encoded.len();
    drop(encoded);
    let per = |bytes: usize| bytes as f64 / segments.max(1) as f64;
    println!(
        "{}",
        serde_json::json!({
            "benchmark": "native-manufacturing-residency-v1",
            "source": if file.is_empty() { spec.label() } else { file.to_owned() },
            "engine_build": spiling_contracts::ENGINE_BUILD_ID,
            "debug_assertions": cfg!(debug_assertions),
            "layers": layers, "paths": paths, "points": points,
            "deposition_segments": segments, "travel_segments": travels,
            "bytes": {
                "compile_peak": compile_peak, "compile_retained": setup,
                "plan_retained": plan_retained, "program_retained": program_retained,
                "program_len": program_len, "json_bundle": encoded_len,
                "encode_peak": encode_peak, "decode_peak": decode_peak,
                "bundle_frames": frames.retained_bytes(), "frames_decode_peak": frames_decode_peak,
                "decoded_bundle_retained": decoded_retained,
                "motion_peak": motion_peak, "replay": replay_bytes, "replay_retained": replay_retained,
            },
            "bytes_per_deposition_segment": {
                "compile_peak": compile_peak.map(per), "plan_retained": per(plan_retained),
                "program_retained": per(program_retained), "json_bundle": per(encoded_len),
                "encode_peak": per(encode_peak), "decode_peak": per(decode_peak),
                "bundle_frames": per(frames.retained_bytes()), "frames_decode_peak": per(frames_decode_peak),
                "motion_peak": per(motion_peak), "replay": replay_bytes as f64 / segments.max(1) as f64,
                "replay_retained": per(replay_retained),
            },
            "frames_decode_us": frames_decode_us,
            "motion_refusal": motion_refusal,
            "software_only": true,
        })
    );
    Ok(())
}
