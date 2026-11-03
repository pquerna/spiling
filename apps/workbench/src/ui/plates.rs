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
) -> Option<PlateEvent> {
    let mut chosen = None;
    let focus_key = ui.make_persistent_id("plate-navigator-focus");
    let focus = (
        current.clone(),
        plates.iter().position(|p| &p.layout_id == current),
    );
    let reveal = ui.data_mut(|d| {
        let previous = d.get_temp::<(PrintLayoutId, Option<usize>)>(focus_key);
        let reveal = previous.as_ref() != Some(&focus);
        d.insert_temp(focus_key, focus);
        reveal
    });
    egui::ScrollArea::vertical()
        .id_salt("plate-list")
        .max_height(176.)
        .show(ui, |ui| {
            ui.add_enabled_ui(enabled, |ui| {
                for (index, plate) in plates.iter().enumerate() {
                    let row = ui.push_id(plate.layout_id.as_str(), |ui| {
                        let response = ui.add(
                            egui::Button::selectable(
                                &plate.layout_id == current,
                                format!("{} · {}", index + 1, plate.name),
                            )
                            .truncate(),
                        );
                        let response = response.on_hover_text(&plate.name);
                        if enabled
                            && &plate.layout_id != current
                            && let Some(part) = response.dnd_release_payload::<PlatePartDrag>()
                        {
                            chosen = Some(PlateEvent::Drop {
                                target: plate.layout_id.clone(),
                                part,
                            });
                        }
                        if response.clicked() && &plate.layout_id != current {
                            chosen = Some(PlateEvent::Focus(plate.layout_id.clone()));
                        }
                        ui.small(format!(
                            "{} {} · {}",
                            plate.occurrence_count,
                            if plate.occurrence_count == 1 {
                                "part"
                            } else {
                                "parts"
                            },
                            plate_status(plate)
                        ));
                    });
                    if reveal && &plate.layout_id == current {
                        row.response.scroll_to_me(Some(egui::Align::Center));
                    }
                }
            });
        });
    chosen
}

#[derive(Clone)]
pub struct PlatePartDrag {
    pub project: spiling_contracts::project::ProjectId,
    pub revision: spiling_contracts::project::ProjectRevision,
    pub session: spiling_contracts::geometry::SessionId,
    pub scene: spiling_contracts::geometry::SceneRevision,
    pub source: PrintLayoutId,
    pub part: spiling_contracts::geometry::OccurrenceId,
}
pub enum PlateEvent {
    Focus(PrintLayoutId),
    Drop {
        target: PrintLayoutId,
        part: std::sync::Arc<PlatePartDrag>,
    },
}

/// Scaled source boxes are navigation context, deliberately separate from native picking.
pub fn plate_overview(
    ui: &mut egui::Ui,
    plates: &[PrintLayoutInfo],
    overview: &spiling_contracts::geometry::PrintLayoutOverview,
    current: &PrintLayoutId,
    enabled: bool,
    tokens: super::Tokens,
) -> Option<PlateEvent> {
    use spiling_contracts::geometry::AabbMm;
    let domain = |layout: &spiling_contracts::geometry::LayoutOverview| {
        let mut bounds = layout.build_area_mm;
        for part in &layout.parts {
            if let Some(b) = &mut bounds {
                for a in 0..2 {
                    b.min[a] = b.min[a].min(part.source_bounds_mm.min[a]);
                    b.max[a] = b.max[a].max(part.source_bounds_mm.max[a]);
                }
            } else {
                bounds = Some(part.source_bounds_mm);
            }
        }
        bounds
    };
    // Every card uses the same physical scale, including different configured machines.
    let span = overview
        .layouts
        .iter()
        .filter_map(domain)
        .map(|b| (b.max[0] - b.min[0]).max(b.max[1] - b.min[1]))
        .fold(1.0_f64, f64::max);
    if !span.is_finite() {
        ui.label("Overview extent is too large");
        return None;
    }
    let columns = if ui.available_width() >= 540.0 { 2 } else { 1 };
    let margin = egui::Frame::group(ui.style())
        .stroke(egui::Stroke::new(2.0, tokens.accent))
        .total_margin()
        .sum()
        .x;
    let width = (ui.available_width() - margin * columns as f32 - 12.0 * (columns - 1) as f32)
        / columns as f32;
    let mut event = None;
    egui::Grid::new("plate-overview-grid")
        .num_columns(columns)
        .spacing(egui::vec2(12.0, 12.0))
        .show(ui, |ui| {
            for (index, (plate, layout)) in plates.iter().zip(&overview.layouts).enumerate() {
                let active = &plate.layout_id == current;
                let frame =
                    egui::Frame::group(ui.style())
                        .fill(tokens.panel)
                        .stroke(egui::Stroke::new(
                            if active { 2.0 } else { 1.0 },
                            if active {
                                tokens.accent
                            } else {
                                ui.visuals().widgets.noninteractive.bg_stroke.color
                            },
                        ));
                let card = ui
                    .push_id(plate.layout_id.as_str(), |ui| {
                        frame.show(ui, |ui| {
                            ui.set_width(width.max(160.0));
                            ui.vertical(|ui| {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(format!(
                                            "{} · {}",
                                            index + 1,
                                            plate.name
                                        ))
                                        .strong(),
                                    )
                                    .truncate(),
                                )
                                .on_hover_text(&plate.name);
                                ui.weak(format!(
                                    "{} {} · {}",
                                    plate.occurrence_count,
                                    if plate.occurrence_count == 1 {
                                        "part"
                                    } else {
                                        "parts"
                                    },
                                    plate_status(plate)
                                ));
                                let (rect, response) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), 160.0),
                                    egui::Sense::click(),
                                );
                                let center = domain(layout).unwrap_or(AabbMm {
                                    min: [0.0; 3],
                                    max: [1.0; 3],
                                });
                                let scale =
                                    f64::from(rect.width().min(rect.height()) - 16.0) / span;
                                let point = |p: [f64; 3]| {
                                    egui::pos2(
                                        rect.center().x
                                            + ((p[0]
                                                - center.min[0]
                                                - (center.max[0] - center.min[0]) * 0.5)
                                                * scale)
                                                as f32,
                                        rect.center().y
                                            - ((p[1]
                                                - center.min[1]
                                                - (center.max[1] - center.min[1]) * 0.5)
                                                * scale)
                                                as f32,
                                    )
                                };
                                let project = |b: AabbMm| {
                                    egui::Rect::from_two_pos(point(b.min), point(b.max))
                                };
                                let painter = ui.painter_at(rect);
                                if let Some(bed) = layout.build_area_mm {
                                    let r = project(bed);
                                    painter.rect_filled(
                                        r,
                                        2.0,
                                        tokens.accent.gamma_multiply(if active {
                                            0.12
                                        } else {
                                            0.06
                                        }),
                                    );
                                    painter.rect_stroke(
                                        r,
                                        2.0,
                                        egui::Stroke::new(1.0, tokens.muted),
                                        egui::StrokeKind::Inside,
                                    );
                                }
                                for part in &layout.parts {
                                    let r = project(part.source_bounds_mm).expand(0.75);
                                    painter.rect_filled(
                                        r,
                                        1.0,
                                        tokens.accent.gamma_multiply(if active {
                                            0.8
                                        } else {
                                            0.4
                                        }),
                                    );
                                    painter.rect_stroke(
                                        r,
                                        1.0,
                                        egui::Stroke::new(1.0, tokens.accent),
                                        egui::StrokeKind::Inside,
                                    );
                                }
                                if layout.build_area_mm.is_none() {
                                    ui.weak("Build area not configured");
                                }
                                if enabled && response.clicked() {
                                    event = Some(PlateEvent::Focus(plate.layout_id.clone()));
                                }
                                if ui
                                    .add_enabled(
                                        enabled,
                                        egui::Button::new(if active {
                                            "Return to plate"
                                        } else {
                                            "Focus plate"
                                        }),
                                    )
                                    .clicked()
                                {
                                    event = Some(PlateEvent::Focus(plate.layout_id.clone()));
                                }
                            });
                        })
                    })
                    .inner;
                if enabled
                    && !active
                    && let Some(part) = card.response.dnd_release_payload::<PlatePartDrag>()
                {
                    event = Some(PlateEvent::Drop {
                        target: plate.layout_id.clone(),
                        part,
                    });
                }
                if (index + 1) % columns == 0 {
                    ui.end_row();
                }
            }
        });
    event
}
fn plate_status(plate: &PrintLayoutInfo) -> &'static str {
    if plate.occurrence_count == 0 {
        "Empty"
    } else if plate.has_result {
        "Prepared"
    } else if plate.configured {
        "Ready to prepare"
    } else {
        "Setup needed"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn focusing_a_later_plate_reveals_its_complete_navigation_label() {
        for theme in [crate::ui::Theme::Graphite, crate::ui::Theme::Paper] {
            let ctx = egui::Context::default();
            theme.apply(&ctx);
            let plates = (1..=12)
                .map(|n| PrintLayoutInfo {
                    layout_id: PrintLayoutId::new(),
                    name: format!("Plate {n}"),
                    occurrence_count: 1,
                    configured: true,
                    has_result: false,
                })
                .collect::<Vec<_>>();
            let mut frame = 0;
            for index in [0, 11, 2] {
                let mut visible = false;
                for _ in 0..24 {
                    frame += 1;
                    let output = ctx.run_ui(
                        egui::RawInput {
                            time: Some(f64::from(frame) / 60.0),
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(240.0, 300.0),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            plate_navigator(ui, &plates, &plates[index].layout_id, true);
                        },
                    );
                    let label = format!("{} · Plate {}", index + 1, index + 1);
                    visible = output.shapes.iter().any(|s| {
                        matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == label)
                            && s.clip_rect.contains_rect(s.shape.visual_bounding_rect())
                    });
                    output.drop_without_applying_deltas();
                }
                assert!(
                    visible,
                    "{theme:?}: focused plate {} remains outside the navigator",
                    index + 1
                );
            }
        }
    }
}
