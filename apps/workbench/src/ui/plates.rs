// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
//! Production-shared, authority-free print-job navigation.
use spiling_contracts::project::{PrintLayoutId, PrintLayoutInfo};

pub fn plate_navigator(
    ui: &mut egui::Ui,
    plates: &[PrintLayoutInfo],
    current: &PrintLayoutId,
    enabled: bool,
) -> Option<PrintLayoutId> {
    let mut chosen = None;
    egui::ScrollArea::vertical()
        .id_salt("plate-list")
        .max_height(176.)
        .show(ui, |ui| {
            ui.add_enabled_ui(enabled, |ui| {
                for (index, plate) in plates.iter().enumerate() {
                    ui.push_id(plate.layout_id.as_str(), |ui| {
                        let response = ui.add(
                            egui::Button::selectable(
                                &plate.layout_id == current,
                                format!("{} · {}", index + 1, plate.name),
                            )
                            .truncate(),
                        );
                        if response.on_hover_text(&plate.name).clicked()
                            && &plate.layout_id != current
                        {
                            chosen = Some(plate.layout_id.clone());
                        }
                        ui.small(format!(
                            "{} {} · {}",
                            plate.occurrence_count,
                            if plate.occurrence_count == 1 {
                                "part"
                            } else {
                                "parts"
                            },
                            if plate.occurrence_count == 0 {
                                "Empty"
                            } else if plate.has_result {
                                "Prepared"
                            } else if plate.configured {
                                "Ready to prepare"
                            } else {
                                "Setup needed"
                            }
                        ));
                    });
                }
            });
        });
    chosen
}
