// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

pub mod analysis;
pub mod app;
pub mod assets;
pub mod commands;
pub mod diagnostics;
pub mod host;
pub mod inspection;
pub mod launch;
pub mod layers;
pub mod manufacture;
mod performance;
pub mod replay;
pub mod scene;
pub mod ui;
pub mod viewport;

/// Initialize the opt-in process clock before native graphics/window creation.
pub fn start_open_trace() {
    performance::mark("launch");
}

pub fn native_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("org.spiling.Workbench")
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../icons/icon.png"))
                    .expect("checked application icon"),
            )
            .with_inner_size([1120.0, 760.0])
            .with_min_inner_size([640.0, 480.0]),
        renderer: eframe::Renderer::Wgpu,
        depth_buffer: 24,
        stencil_buffer: 8,
        ..Default::default()
    }
}
