// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
struct Gallery {
    theme: spiling_workbench::ui::Theme,
}
impl eframe::App for Gallery {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let before = self.theme;
        egui::Frame::central_panel(ui.style())
            .fill(self.theme.tokens().canvas)
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                spiling_workbench::ui::gallery(ui, &mut self.theme);
            });
        if before != self.theme {
            self.theme.apply(ui.ctx());
        }
    }
}
fn main() -> eframe::Result {
    let mut options = spiling_workbench::native_options();
    options.viewport = options.viewport.with_app_id("org.spiling.Gallery");
    eframe::run_native(
        "Spiling component gallery",
        options,
        Box::new(|cc| {
            let theme = spiling_workbench::ui::Theme::Graphite;
            theme.apply(&cc.egui_ctx);
            Ok(Box::new(Gallery { theme }))
        }),
    )
}
