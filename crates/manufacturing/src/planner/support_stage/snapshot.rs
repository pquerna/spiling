// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Developer cache, separate from persisted manufacturing artifacts. The
//! compiler remains the only producer of admitted stage inputs.
use super::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spiling_contracts::geometry::{OccurrenceId, SourceHash};
use spiling_geometry::{GeometryWorkBudget, tolerant::TolerantModel};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const FORMAT: &str = "support-input-1";
const CAPTURE_DIR: &str = "SPILING_STAGE_CAPTURE_DIR";
type Models = BTreeMap<OccurrenceId, Arc<TolerantModel>>;
type Classification = (usize, classification::ClassifiedRegions, usize, usize);
type Failure = Box<dyn std::error::Error>;

#[derive(Serialize, Deserialize)]
struct Header {
    format: String,
    planner: String,
    source: String,
    build_bounds: [[f64; 2]; 2],
    nominal_height_mm: f64,
    source_z_allowance_mm: f64,
    native_points: usize,
    native_owned_bytes: usize,
    support_classified_bytes: usize,
    retained_points: usize,
    neighborhood_points: usize,
    neighborhood_bytes: usize,
    segments: usize,
    planar: usize,
    support_owned: usize,
    anchors: usize,
    native_work: u64,
    native_remaining: Option<u64>,
    working_bytes: usize,
}

#[derive(Serialize, Deserialize)]
struct Section<P> {
    start: u32,
    end: u32,
    material: usize,
    process_domains: P,
    occupied: Option<usize>,
    source_area_mm2: f64,
}

#[derive(Serialize, Deserialize)]
struct Occupied<P> {
    pieces: P,
    tolerant: Vec<(OccurrenceId, [f64; 2])>,
    point_count: usize,
    payload_bytes: usize,
}

// The borrowed serial form writes owners once without cloning their payloads.
// The owned form uses the identical field order and schema.
#[derive(Serialize, Deserialize)]
struct Snapshot<R, L, Z, M, P, O, T, C, H, F, D, V> {
    header: Header,
    recipe: R,
    layers: L,
    schedule: Z,
    materials: M,
    sections: P,
    occupied: O,
    models: T,
    classifications: C,
    phases: H,
    roof: F,
    cathedral_roads: D,
    void_memo: V,
}

type Owned = Snapshot<
    PlanarPrintRecipe,
    Vec<PrintLayer>,
    Vec<LayerElevation>,
    Vec<Arc<MultiPolygon<f64>>>,
    Vec<Section<Vec<ProcessDomain>>>,
    Vec<Occupied<Vec<OccupiedPiece>>>,
    Models,
    Vec<Classification>,
    Vec<region::OrdinaryHatchPhase>,
    Option<cathedral::Field>,
    Vec<Vec<DepositionPath>>,
    tolerant::VoidMemo,
>;

struct HashWriter<W> {
    writer: W,
    hash: Sha256,
}

impl<W: Write> Write for HashWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let len = self.writer.write(bytes)?;
        self.hash.update(&bytes[..len]);
        Ok(len)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

pub(in crate::planner) struct Capture {
    blob: PathBuf,
}

impl Capture {
    #[expect(
        clippy::too_many_arguments,
        reason = "Capture observes the same borrowed stage owners and entry counters"
    )]
    pub(in crate::planner) fn start(
        definitions: &[spiling_contracts::project::StoredDefinition],
        layers: &[PrintLayer],
        input: &StageInput<'_>,
        segments: usize,
        neighborhoods: &mut SkinNeighborhoods<'_>,
        void_memo: &tolerant::VoidMemo,
        geometry: &GeometryWorkBudget,
        work: &mut support::SupportWork,
    ) -> Result<Option<Self>, ManufacturingError> {
        let Some(dir) = std::env::var_os(CAPTURE_DIR).filter(|_| !input.tentative) else {
            return Ok(None);
        };
        capture(
            Path::new(&dir),
            definitions,
            layers,
            input,
            segments,
            neighborhoods,
            void_memo,
            geometry,
            work,
        )
        .map(Some)
        .map_err(|err| {
            error(
                ManufacturingErrorCode::InvalidSpecification,
                format!("stage capture: {err}"),
            )
        })
    }

    pub(in crate::planner) fn finish(
        self,
        layers: &[PrintLayer],
        outcome: &Result<support::SupportAdmission, ManufacturingError>,
        work: &mut support::SupportWork,
        elapsed: Duration,
    ) -> Result<(), ManufacturingError> {
        let phases = work.phases.take();
        if let Ok(support::SupportAdmission::Complete(withdrawal)) = outcome {
            let report = report(
                layers,
                withdrawal.as_ref(),
                phases.as_deref().unwrap_or_default(),
                elapsed,
            )
            .map_err(|err| {
                error(
                    ManufacturingErrorCode::InvalidSpecification,
                    format!("stage output: {err}"),
                )
            })?;
            eprintln!(
                "{}",
                serde_json::json!({"event": "stage_capture", "blob": self.blob, "result": report})
            );
        }
        Ok(())
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Capture observes the same borrowed stage owners and entry counters"
)]
fn capture(
    dir: &Path,
    definitions: &[spiling_contracts::project::StoredDefinition],
    layers: &[PrintLayer],
    input: &StageInput<'_>,
    segments: usize,
    neighborhoods: &mut SkinNeighborhoods<'_>,
    void_memo: &tolerant::VoidMemo,
    geometry: &GeometryWorkBudget,
    work: &mut support::SupportWork,
) -> Result<Capture, Failure> {
    let mut hashes: Vec<&SourceHash> = definitions
        .iter()
        .map(|d| &d.provenance.source_hash)
        .collect();
    hashes.sort_unstable();
    hashes.dedup();
    let source = match hashes.as_slice() {
        [hash] => hash.as_str().to_owned(),
        [] => return Err("no source hashes for the support input".into()),
        _ => digest(&hashes)?,
    };
    let mut materials = Vec::new();
    let mut material_ids = HashMap::new();
    let mut occupied = Vec::new();
    let mut occupied_ids = HashMap::new();
    let model_ids: HashMap<_, _> = input
        .models
        .iter()
        .map(|(id, model)| (Arc::as_ptr(model), id))
        .collect();
    let mut sections = Vec::new();
    for span in input.support.sections {
        let material = *material_ids
            .entry(Arc::as_ptr(&span.material))
            .or_insert_with(|| {
                materials.push(span.material.as_ref());
                materials.len() - 1
            });
        let occupied_id = if let Some(value) = &span.occupied {
            let next = occupied.len();
            let id = *occupied_ids.entry(Arc::as_ptr(value)).or_insert(next);
            if id == next {
                let tolerant = value
                    .tolerant
                    .iter()
                    .map(|owner| {
                        let id = model_ids
                            .get(&Arc::as_ptr(&owner.model))
                            .ok_or("occupied model lost its stage owner")?;
                        Ok((**id, owner.interval))
                    })
                    .collect::<Result<_, Failure>>()?;
                occupied.push(Occupied {
                    pieces: &value.pieces,
                    tolerant,
                    point_count: value.point_count,
                    payload_bytes: value.payload_bytes,
                });
            }
            Some(id)
        } else {
            None
        };
        sections.push(Section {
            start: span.start,
            end: span.end,
            material,
            process_domains: &span.process_domains,
            occupied: occupied_id,
            source_area_mm2: span.source_area_mm2,
        });
    }
    let snapshot = Snapshot {
        header: Header {
            format: FORMAT.into(),
            planner: crate::PLANNER_VERSION.into(),
            source: source.clone(),
            build_bounds: input.support.build_bounds,
            nominal_height_mm: input.support.nominal_height_mm,
            source_z_allowance_mm: input.support.source_z_allowance_mm,
            native_points: input.support.native_points,
            native_owned_bytes: input.support.native_owned_bytes,
            support_classified_bytes: input.support_classified_bytes,
            retained_points: input.retained_points,
            neighborhood_points: neighborhoods.retained_points(),
            neighborhood_bytes: neighborhoods.retained_bytes(),
            segments,
            planar: *neighborhoods.work_mut(),
            support_owned: work.support_owned,
            anchors: work.anchors,
            native_work: geometry.consumed(),
            native_remaining: (!geometry.is_monitored()).then(|| geometry.remaining()),
            working_bytes: crate::working_set::bytes(),
        },
        recipe: input.support.recipe,
        layers,
        schedule: input.support.schedule,
        materials,
        sections,
        occupied,
        models: input.models,
        classifications: input.classifications,
        phases: input.phase_by_layer,
        roof: input.web_roof,
        cathedral_roads: input.cathedral_roads,
        void_memo,
    };
    let path = dir.join(crate::PLANNER_VERSION).join(source);
    fs::create_dir_all(&path)?;
    // A capture cannot publish a partial blob. Concurrent writers of the same
    // exact content converge on the same final name.
    let temp = path.join(format!(".{}.pending", std::process::id()));
    let writer = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let blob = (|| -> Result<PathBuf, Failure> {
        let mut writer = HashWriter {
            writer: BufWriter::new(writer),
            hash: Sha256::new(),
        };
        serde_json::to_writer(&mut writer, &snapshot)?;
        writer.flush()?;
        let blob = path.join(format!("{:x}.json", writer.hash.finalize()));
        fs::rename(&temp, &blob)?;
        Ok(blob)
    })();
    if blob.is_err() {
        let _ = fs::remove_file(&temp);
    }
    work.phases = Some(Vec::new());
    Ok(Capture { blob: blob? })
}

fn digest(value: &impl Serialize) -> Result<String, Failure> {
    let mut w = HashWriter {
        writer: io::sink(),
        hash: Sha256::new(),
    };
    serde_json::to_writer(&mut w, value)?;
    Ok(format!("{:x}", w.hash.finalize()))
}

fn report(
    layers: &[PrintLayer],
    withdrawal: Option<&native::SupportWithdrawal>,
    phases: &[support::PhaseWork],
    elapsed: Duration,
) -> Result<serde_json::Value, Failure> {
    let mut output = HashWriter {
        writer: io::sink(),
        hash: Sha256::new(),
    };
    serde_json::to_writer(&mut output, &layers)?;
    output.write_all(&[u8::from(withdrawal.is_some())])?;
    if let Some(withdrawal) = withdrawal {
        withdrawal.hash_stage_output(&mut output)?;
    }
    Ok(
        serde_json::json!({"stage": "support", "output_sha256": format!("{:x}", output.hash.finalize()), "work": phases, "seconds": elapsed.as_secs_f64()}),
    )
}

pub(crate) fn replay(
    path: &Path,
    working_set: crate::WorkingSet,
    cancel: &AtomicBool,
) -> Result<serde_json::Value, Failure> {
    use io::{Read, Seek};
    let mut file = File::open(path)?;
    // Local dev artifacts are input, not an import or manufacturing authority.
    // JSON decoding needs extra capacity for nested structs and grown vectors.
    let limit = u64::try_from(working_set.bytes() / 16)?;
    let size = file.metadata()?.len();
    if size > limit {
        return Err("stage blob exceeds replay decode admission".into());
    }
    // Check the content address in a streaming pass, then decode the same file.
    file.rewind()?;
    let mut writer = HashWriter {
        writer: io::sink(),
        hash: Sha256::new(),
    };
    io::copy(&mut (&mut file).take(size), &mut writer)?;
    let name = format!("{:x}.json", writer.hash.finalize());
    if path.file_name().and_then(|name| name.to_str()) != Some(name.as_str()) {
        return Err("stage blob content address mismatch".into());
    }
    file.rewind()?;
    let value: Owned = serde_json::from_reader(BufReader::new(file.take(size)))?;
    if value.header.format != FORMAT || value.header.planner != crate::PLANNER_VERSION {
        return Err("support snapshot format or planner identity differs; recapture input".into());
    }
    if value.header.working_bytes > working_set.bytes()
        || value.header.native_owned_bytes > value.header.working_bytes
    {
        return Err("working set cannot reproduce the capture's admission".into());
    }
    crate::working_set::scope(crate::WorkingSet::new(value.header.working_bytes)?, || {
        run_snapshot(value, cancel)
    })
}

fn run_snapshot(value: Owned, cancel: &AtomicBool) -> Result<serde_json::Value, Failure> {
    let Snapshot {
        header,
        recipe,
        mut layers,
        schedule,
        materials,
        sections,
        occupied,
        models,
        classifications,
        phases,
        roof,
        cathedral_roads,
        mut void_memo,
    } = value;
    if schedule.is_empty()
        || schedule.len() != layers.len()
        || phases.len() != layers.len()
        || schedule
            .iter()
            .any(|z| !z.z.is_finite() || !z.lower_z.is_finite() || z.z <= z.lower_z)
        || layers
            .iter()
            .zip(&schedule)
            .enumerate()
            .any(|(i, (l, z))| l.index as usize != i || l.z_mm != z.z || l.lower_z_mm != z.lower_z)
        || classifications.first().is_none_or(|c| c.0 != 0)
        || classifications.windows(2).any(|w| w[0].0 >= w[1].0)
        || classifications.last().is_some_and(|c| c.0 >= layers.len())
        || models
            .values()
            .any(|model| !model.validate_stage_snapshot())
    {
        return Err(
            "support snapshot does not bind a valid complete schedule and model inventory".into(),
        );
    }
    let occupied: Vec<_> = occupied
        .into_iter()
        .map(|o| -> Result<_, Failure> {
            let tolerant = o
                .tolerant
                .into_iter()
                .map(|(id, interval)| {
                    let model = models.get(&id).ok_or("unknown occupied model")?;
                    if !interval.iter().all(|v| v.is_finite()) || interval[0] > interval[1] {
                        return Err("invalid occupied interval");
                    }
                    Ok(TolerantOccupied {
                        model: Arc::clone(model),
                        interval,
                    })
                })
                .collect::<Result<_, _>>()?;
            Ok(Arc::new(ContinuousOccupied {
                pieces: o.pieces,
                tolerant,
                point_count: o.point_count,
                payload_bytes: o.payload_bytes,
            }))
        })
        .collect::<Result<_, _>>()?;
    let mut end = 0;
    let sections: Vec<_> = sections
        .into_iter()
        .map(|s| -> Result<_, Failure> {
            if s.start != end || s.end <= s.start || s.end as usize > schedule.len() {
                return Err("invalid support section schedule span".into());
            }
            end = s.end;
            Ok(SectionSpan {
                start: s.start,
                end: s.end,
                material: Arc::clone(
                    materials
                        .get(s.material)
                        .ok_or("unknown section material")?,
                ),
                process_domains: s.process_domains,
                occupied: s
                    .occupied
                    .map(|id| occupied.get(id).cloned().ok_or("unknown section occupancy"))
                    .transpose()?,
                source_area_mm2: s.source_area_mm2,
            })
        })
        .collect::<Result<_, _>>()?;
    if end as usize != schedule.len() {
        return Err("support section inventory is incomplete".into());
    }
    drop((materials, occupied));
    let mut segments = header.segments;
    let mut geometry = if let Some(remaining) = header.native_remaining {
        GeometryWorkBudget::new(
            header
                .native_work
                .checked_add(remaining)
                .ok_or("native work overflow")?,
        )
    } else {
        GeometryWorkBudget::monitored()
    };
    geometry.charge(header.native_work, cancel)?;
    let mut work = support::SupportWork {
        planar: header.planar,
        support_owned: header.support_owned,
        anchors: header.anchors,
        phases: Some(Vec::new()),
    };
    let start = Instant::now();
    let outcome = run(
        &mut layers,
        &StageInput {
            support: support::SupportInput {
                build_bounds: header.build_bounds,
                sections: &sections,
                schedule: &schedule,
                native_points: header.native_points,
                native_owned_bytes: header.native_owned_bytes,
                recipe: &recipe,
                nominal_height_mm: header.nominal_height_mm,
                source_z_allowance_mm: header.source_z_allowance_mm,
            },
            classifications: &classifications,
            phase_by_layer: &phases,
            models: &models,
            retained_points: header.retained_points,
            support_classified_bytes: header.support_classified_bytes,
            web_roof: roof.as_ref(),
            cathedral_roads: &cathedral_roads,
            tentative: false,
        },
        StageWork {
            segments: &mut segments,
            geometry: &mut geometry,
            support: &mut work,
            neighborhoods: RasterWork::Replay {
                work: header.planar,
                points: header.neighborhood_points,
                bytes: header.neighborhood_bytes,
            },
            void_memo: &mut void_memo,
        },
        cancel,
    )?;
    let elapsed = start.elapsed();
    match outcome {
        support::SupportAdmission::Complete(withdrawal) => report(
            &layers,
            withdrawal.as_ref(),
            work.phases.as_deref().unwrap_or_default(),
            elapsed,
        ),
        support::SupportAdmission::RefineUnreachableLayer { failure, .. } => Err(failure.into()),
    }
}
