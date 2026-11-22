// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{StageOutput, prepare};
use crate::{WorkingSet, error, planner::REALIZATION_INPUT_PRODUCER, stage_io, working_set};
use serde::{Deserialize, Serialize};
use spiling_contracts::{manufacturing::*, project::StoredDefinition};
use std::{
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

const STAGE: &str = "realization";
const FORMAT: &str = "realization-input-1";

// Borrow the compile's actual plan and intent for capture; only replay owns a
// decoded copy. The post-realization planner and independent replay stay live.
#[derive(Serialize, Deserialize)]
struct Snapshot<S, P, I, H> {
    format: S,
    producer: S,
    source: S,
    working_bytes: usize,
    plan: P,
    intent: I,
    hatch_keys: H,
}

type Owned = Snapshot<String, NormalizedPrintPlan, ManufacturingIntent, Vec<u32>>;

pub(crate) struct Capture {
    blob: PathBuf,
    start: Instant,
}

impl Capture {
    pub(crate) fn start(
        definitions: &[StoredDefinition],
        plan: &NormalizedPrintPlan,
        intent: &ManufacturingIntent,
        hatch_keys: &[u32],
    ) -> Result<Option<Self>, ManufacturingError> {
        let Some(dir) = stage_io::capture_dir(STAGE) else {
            return Ok(None);
        };
        let source = stage_io::source_key(definitions).map_err(capture_error)?;
        let value = Snapshot {
            format: FORMAT,
            producer: REALIZATION_INPUT_PRODUCER,
            source: source.as_str(),
            working_bytes: working_set::bytes(),
            plan,
            intent,
            hatch_keys,
        };
        let blob = stage_io::write_blob(&dir, REALIZATION_INPUT_PRODUCER, &source, &value)
            .map_err(capture_error)?;
        Ok(Some(Self {
            blob,
            start: Instant::now(),
        }))
    }

    pub(crate) fn finish(
        self,
        plan: &NormalizedPrintPlan,
        output: &StageOutput,
    ) -> Result<(), ManufacturingError> {
        let result = report(plan, output, self.start.elapsed()).map_err(capture_error)?;
        eprintln!(
            "{}",
            serde_json::json!({"event": "stage_capture", "blob": self.blob, "result": result})
        );
        Ok(())
    }
}

fn capture_error(err: stage_io::Failure) -> ManufacturingError {
    error(
        ManufacturingErrorCode::InvalidSpecification,
        format!("stage capture: {err}"),
    )
}

fn report(
    plan: &NormalizedPrintPlan,
    output: &StageOutput,
    elapsed: Duration,
) -> Result<serde_json::Value, stage_io::Failure> {
    let hash = stage_io::digest(&(plan, &output.estimate))?;
    Ok(serde_json::json!({
        "stage": STAGE, "output_sha256": hash, "work": output.work,
        "seconds": elapsed.as_secs_f64(),
    }))
}

pub(crate) fn replay(
    path: &Path,
    allowance: WorkingSet,
    cancel: &AtomicBool,
) -> Result<serde_json::Value, stage_io::Failure> {
    let mut input: Owned = stage_io::read_blob(path, allowance)?;
    if input.format != FORMAT || input.producer != REALIZATION_INPUT_PRODUCER {
        return Err(
            "realization input format or upstream producer differs; recapture input".into(),
        );
    }
    if input.working_bytes > allowance.bytes() || input.hatch_keys.len() != input.plan.layers.len()
    {
        return Err("realization snapshot allowance or layer table does not match".into());
    }
    input.intent.validate()?;
    let start = Instant::now();
    let output = working_set::scope(WorkingSet::new(input.working_bytes)?, || {
        prepare(&mut input.plan, &input.intent, &input.hatch_keys, cancel)
    })?;
    report(&input.plan, &output, start.elapsed())
}
