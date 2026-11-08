// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;
use spiling_contracts::compute::ComputeBackend;

fn label(backend: ComputeBackend) -> &'static str {
    match backend {
        ComputeBackend::Auto => "Automatic",
        ComputeBackend::Cpu => "CPU · LLVM SIMD",
        ComputeBackend::Vulkan => "Vulkan",
    }
}

impl Workbench {
    pub(super) fn compute_dialog(&mut self, ctx: &egui::Context) {
        let previous = self.prefs.compute;
        egui::Window::new("Compute settings")
            .id(egui::Id::unique("compute-settings"))
            .open(&mut self.show_compute)
            .resizable(false)
            .show(ctx, |ui| {
                ui.set_max_width(440.0);
                ui.label("Machine-local preferences · not part of the project");
                ui.separator();
                ui.label("Default CubeCL backend");
                egui::ComboBox::from_id_salt("compute-default")
                    .selected_text(label(self.prefs.compute.default_backend))
                    .show_ui(ui, |ui| {
                        for backend in ComputeBackend::ALL {
                            ui.selectable_value(
                                &mut self.prefs.compute.default_backend,
                                backend,
                                label(backend),
                            );
                        }
                    });
                ui.weak("Automatic prefers a hardware Vulkan device, otherwise CPU. It is a device policy, not a benchmark result.");
                ui.add_space(12.0);
                ui.label("FEA / Analyze backend · future feature");
                egui::ComboBox::from_id_salt("compute-fea")
                    .selected_text(self.prefs.compute.fea_backend.map_or("Inherit default", label))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.prefs.compute.fea_backend, None, "Inherit default");
                        for backend in ComputeBackend::ALL {
                            ui.selectable_value(
                                &mut self.prefs.compute.fea_backend,
                                Some(backend),
                                label(backend),
                            );
                        }
                    });
                ui.weak("Inheritance uses the same resolved default device. This preference does not enable FEA.");
                ui.separator();
                if let Some(hello) = &self.state.hello {
                    ui.label(format!("Active: {} · {}", label(hello.compute.default_device.backend), hello.compute.default_device.name));
                    if hello.compute.settings != self.prefs.compute {
                        ui.label("Saved for the next engine session. Save your project before restarting.");
                        if ui.add_enabled(!self.state.busy, egui::Button::new("Restart engine to apply")).clicked() {
                            self.queued_host = Some(Command::Restart);
                        }
                    }
                } else {
                    ui.weak("The next engine session uses these preferences.");
                }
            });
        if previous != self.prefs.compute {
            self.queued_compute = Some(self.prefs.compute);
        }
    }
}
