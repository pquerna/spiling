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
