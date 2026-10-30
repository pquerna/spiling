// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

pub mod app;
pub mod assets;
pub mod commands;
pub mod diagnostics;
pub mod host;
pub mod launch;
pub mod manufacture;
pub mod replay;
pub mod scene;
pub mod ui;
pub mod viewport;

pub fn native_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1120.0, 760.0])
            .with_min_inner_size([640.0, 480.0]),
        renderer: eframe::Renderer::Wgpu,
        depth_buffer: 24,
        ..Default::default()
    }
}
