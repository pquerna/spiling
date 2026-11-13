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
        "schema": 1, "process": "workbench", "pid": std::process::id(),
        "stage": stage, "at_us": at_us, "elapsed_us": elapsed_us
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

/// Readback completion bounds time to rendered pixels; it is not compositor presentation.
/// Only the opt-in trace requests the two extra GPU readbacks.
#[derive(Default)]
pub(crate) struct Frames {
    first: bool,
    confirmation: Option<u64>,
    scene: Option<std::sync::Weak<crate::scene::Scene>>,
}
impl Frames {
    pub(crate) fn confirmation(&mut self, ctx: &egui::Context, id: u64) {
        if clock().is_some() && self.confirmation != Some(id) {
            self.confirmation = Some(id);
            ctx.request_screenshot(|_| mark("discard_confirmation_frame"));
        }
    }
    pub(crate) fn observe(
        &mut self,
        ctx: &egui::Context,
        scene: Option<&std::sync::Arc<crate::scene::Scene>>,
    ) {
        if clock().is_none() {
            return;
        }
        if !self.first {
            self.first = true;
            mark("first_ui");
            ctx.request_screenshot(|_| mark("first_frame_readback"));
        }
        if let Some(scene) = scene.filter(|s| s.summary.definition_count > 0)
            && self
                .scene
                .as_ref()
                .and_then(std::sync::Weak::upgrade)
                .as_ref()
                .is_none_or(|old| !std::sync::Arc::ptr_eq(old, scene))
        {
            self.scene = Some(std::sync::Arc::downgrade(scene));
            mark("source_ui");
            ctx.request_screenshot(|_| mark("source_frame_readback"));
        }
    }
}

/// Opt-in native acceptance reports the visible control's location after pixels
/// are rendered. Automation uses normal pointer input; this never dispatches an action.
pub(crate) fn control(ctx: &egui::Context, stage: &'static str, response: &egui::Response) {
    if clock().is_none() || std::env::var_os("SPILING_IMPORT_INTERACTION_TRACE").is_none() {
        return;
    }
    let id = egui::Id::unique(stage);
    let (ready, settling) = ctx.data_mut(|data| {
        let (previous, count, sent) = data
            .get_temp::<(egui::Rect, usize, Option<egui::Rect>)>(id)
            .unwrap_or((response.rect, 0, None));
        let count = if previous == response.rect {
            count.saturating_add(1)
        } else {
            1
        };
        let ready = count >= 2 && sent != Some(response.rect);
        data.insert_temp(
            id,
            (
                response.rect,
                count,
                if ready { Some(response.rect) } else { sent },
            ),
        );
        (ready, count < 2)
    });
    if !ready {
        if settling {
            ctx.request_repaint();
        }
        return;
    }
    let point = response.rect.center() * ctx.pixels_per_point();
    ctx.request_screenshot(move |_| {
        if let Some(origin)=clock() {
            let record=serde_json::json!({"schema":1,"process":"workbench","pid":std::process::id(),"stage":stage,"at_us":origin.elapsed().as_micros(),"elapsed_us":0,"x":point.x,"y":point.y});
            let line=format!("SPILING_OPEN_TRACE {record}\n");
            let _=std::io::Write::write_all(&mut std::io::stderr().lock(),line.as_bytes());
        }
    });
}
pub(crate) fn after_frame(ctx: &egui::Context, stage: &'static str) {
    if clock().is_some() {
        ctx.request_screenshot(move |_| mark(stage));
    }
}
