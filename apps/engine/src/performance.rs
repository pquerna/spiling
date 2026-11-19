// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Opt-in opening measurements. No paths, source bytes or persistent project state.
use std::{sync::OnceLock, time::Instant};

static CLOCK: OnceLock<Option<Instant>> = OnceLock::new();
fn clock() -> Option<Instant> {
    *CLOCK.get_or_init(|| {
        (std::env::var_os("SPILING_OPEN_TRACE").as_deref() == Some(std::ffi::OsStr::new("1")))
            .then(Instant::now)
    })
}
pub(crate) fn mark(stage: &'static str) {
    if let Some(origin) = clock() {
        emit(stage, origin.elapsed().as_micros(), 0);
    }
}
fn emit(stage: &str, at_us: u128, elapsed_us: u128) {
    let record = serde_json::json!({
        "schema": 1, "process": "engine", "pid": std::process::id(),
        "stage": stage, "at_us": at_us, "elapsed_us": elapsed_us,
        "kernel": spiling_geometry::KERNEL_REVISION
    });
    write_record(&record);
}

pub(crate) fn step_import(
    metrics: spiling_geometry::StepImportMeasurements,
    bytes: usize,
    solids: usize,
) {
    let Some(origin) = clock() else {
        return;
    };
    let record = serde_json::json!({
        "schema": 1, "process": "engine", "pid": std::process::id(),
        "stage": "import_step_source_work", "at_us": origin.elapsed().as_micros(),
        "elapsed_us": 0, "kernel": spiling_geometry::KERNEL_REVISION,
        "source_reads": 1, "source_bytes": bytes, "solids": solids,
        "structure_parses": 1, "structure_parse_us": metrics.structure_parse_time.as_micros(),
        "entity_indexes": metrics.entity_indexes, "entity_index_us": metrics.entity_index_time.as_micros(),
        "native_parses": metrics.native_parses, "native_parse_us": metrics.native_parse_time.as_micros(),
        "native_parse_reuses": metrics.native_parse_reuses,
        "exchange_admission_us": metrics.exchange_admission_time.as_micros(),
        "peak_exchange_retained_bytes": metrics.peak_exchange_retained_bytes,
        "import_source_hashes": 1, "import_source_hash_us": metrics.source_hash_time.as_micros(),
        "source_asset_validations": solids
    });
    write_record(&record);
}

fn write_record(record: &serde_json::Value) {
    // These processes share a pipe. Serialize before writing, so a bounded record
    // is one write below PIPE_BUF rather than many interleaved Display fragments.
    let line = format!("SPILING_OPEN_TRACE {record}\n");
    let _ = std::io::Write::write_all(&mut std::io::stderr().lock(), line.as_bytes());
}
pub(crate) struct Span {
    stage: &'static str,
    started: Option<Instant>,
}
pub(crate) fn span(stage: &'static str) -> Span {
    Span {
        stage,
        started: clock().map(|_| Instant::now()),
    }
}
impl Drop for Span {
    fn drop(&mut self) {
        if let (Some(started), Some(origin)) = (self.started, clock()) {
            emit(
                self.stage,
                origin.elapsed().as_micros(),
                started.elapsed().as_micros(),
            );
        }
    }
}
