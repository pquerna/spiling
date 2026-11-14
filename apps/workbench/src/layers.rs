// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::{manufacture::Review, replay::View};
#[cfg(test)]
use spiling_contracts::manufacturing::{
    REPLAY_LAYER_OFFSET, REPLAY_LIFECYCLE_LAYER, REPLAY_RECORD_BYTES,
};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    All,
    Through,
    Single,
    Range,
}
#[derive(Default)]
pub struct Browser {
    pub mode: Mode,
    lower_handle: bool,
    wheel: f32,
}

pub fn event_window(review: &Review, view: View) -> std::ops::Range<u32> {
    let Some(display) = &review.display else {
        return 0..0;
    };
    let Some(lower) = view.layer else {
        return 0..display.event_count;
    };
    let upper = view.layer_end.unwrap_or(lower).max(lower);
    let Some(first) = display.layer_windows.get(lower as usize) else {
        return display.event_count..display.event_count;
    };
    let last = &display.layer_windows[(upper as usize).min(display.layer_windows.len() - 1)];
    first.start..last.end.max(first.start)
}
#[cfg(test)]
// Called only with checked SPLR records. Complete jobs place unattributed lifecycle
// events at the ends; those sentinels must not break the ordered body-layer search.
fn record_window(bytes: &[u8], view: View) -> std::ops::Range<u32> {
    let records = bytes.as_chunks::<REPLAY_RECORD_BYTES>().0;
    let Some(lo) = view.layer else {
        return 0..records.len() as u32;
    };
    let layer = spiling_contracts::manufacturing::replay_record_layer;
    let first = records
        .iter()
        .position(|r| layer(r) != REPLAY_LIFECYCLE_LAYER)
        .unwrap_or(records.len());
    let last = records
        .iter()
        .rposition(|r| layer(r) != REPLAY_LIFECYCLE_LAYER)
        .map_or(first, |i| i + 1);
    let body = &records[first..last];
    let start = first + body.partition_point(|r| layer(r) < lo);
    let end = first + body.partition_point(|r| layer(r) <= view.layer_end.unwrap_or(lo).max(lo));
    start as u32..end.max(start) as u32
}
impl Browser {
    fn synchronize_selection(&mut self, view: &View) {
        self.mode = match (view.layer, view.layer_end) {
            (None, _) => Mode::All,
            (Some(_), None) => Mode::Single,
            (Some(0), Some(_)) if self.mode == Mode::Through => Mode::Through,
            (Some(_), Some(_)) => Mode::Range,
        };
    }

    fn select(&self, view: &mut View, lower: u32, upper: u32) {
        match self.mode {
            Mode::All => {
                view.layer = None;
                view.layer_end = None;
            }
            Mode::Single => {
                view.layer = Some(upper);
                view.layer_end = None;
            }
            Mode::Through => {
                view.layer = Some(0);
                view.layer_end = Some(upper);
            }
            Mode::Range => {
                view.layer = Some(lower.min(upper));
                view.layer_end = Some(upper);
            }
        }
    }
    pub fn controls(&mut self, ui: &mut egui::Ui, view: &mut View, review: &Review) {
        let count = review.bundle.plan.layers.len();
        if count == 0 {
            return;
        }
        self.synchronize_selection(view);
        let old = *view;
        let mut upper = view
            .layer_end
            .or(view.layer)
            .unwrap_or(count.saturating_sub(1) as u32);
        let mut lower = if self.mode == Mode::Range {
            view.layer.unwrap_or(0)
        } else {
            0
        };
        let previous_controls = (self.mode, lower, upper);
        ui.horizontal_wrapped(|ui| {
            ui.label("Layers");
            for (mode, label) in [
                (Mode::All, "All"),
                (Mode::Through, "Through"),
                (Mode::Single, "Single"),
                (Mode::Range, "Range"),
            ] {
                ui.selectable_value(&mut self.mode, mode, label);
            }
            if self.mode != Mode::All {
                let mut hi = upper + 1;
                if self.mode == Mode::Range {
                    let mut lo = lower + 1;
                    ui.label("From");
                    ui.add(egui::DragValue::new(&mut lo).range(1..=hi).prefix("Layer "));
                    lower = lo - 1;
                    ui.label("through");
                }
                ui.add(
                    egui::DragValue::new(&mut hi)
                        .range(lower + 1..=count as u32)
                        .prefix("Layer "),
                );
                upper = hi - 1;
            }
        });
        if previous_controls != (self.mode, lower, upper) {
            self.select(view, lower, upper);
        }
        if self.mode != Mode::All {
            ui.checkbox(&mut view.context, "Other layers")
                .on_hover_text("Dim deposited paths outside the selected layers");
        }
        if (old.layer, old.layer_end) != (view.layer, view.layer_end) {
            view.events = event_window(review, *view).end;
        }
        if let Some(index) = view.layer_end.or(view.layer).map(|layer| layer as usize) {
            let layer = &review.bundle.plan.layers[index.min(count - 1)];
            ui.weak(format!(
                "Layer {} of {} · Z {:.4}…{:.4} mm · height {:.4} mm",
                index + 1,
                count,
                layer.lower_z_mm,
                layer.z_mm,
                layer.height_mm
            ));
        }
    }
    pub fn rail(&mut self, ui: &mut egui::Ui, view: &mut View, review: &Review) {
        let count = review.bundle.plan.layers.len();
        if count == 0 {
            return;
        }
        self.synchronize_selection(view);
        let maximum = count.saturating_sub(1) as u32;
        let old = *view;
        let mut upper = view
            .layer_end
            .or(view.layer)
            .unwrap_or(maximum)
            .min(maximum);
        let mut lower = if self.mode == Mode::Range {
            view.layer.unwrap_or(0).min(upper)
        } else {
            0
        };
        let previous_controls = (self.mode, lower, upper);
        ui.vertical_centered(|ui| {
            egui::ComboBox::from_id_salt("layer-mode-rail")
                .width(65.0)
                .selected_text(match self.mode {
                    Mode::All => "All",
                    Mode::Through => "Through",
                    Mode::Single => "Single",
                    Mode::Range => "Range",
                })
                .show_ui(ui, |ui| {
                    for (mode, label) in [
                        (Mode::All, "All"),
                        (Mode::Through, "Through"),
                        (Mode::Single, "Single"),
                        (Mode::Range, "Range"),
                    ] {
                        ui.selectable_value(&mut self.mode, mode, label);
                    }
                });
            ui.small(format!("{}", upper + 1));
            let height = (ui.available_height() - 45.0).max(70.0);
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(42.0, height), egui::Sense::click_and_drag());
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::Role::Slider,
                    true,
                    "Layer range. Wheel or Up/Down steps; Shift adjusts lower handle.",
                )
            });
            let track = rect.shrink2(egui::vec2(15.0, 9.0));
            let y = |layer: u32| {
                egui::lerp(
                    track.bottom()..=track.top(),
                    (f64::from(layer) / f64::from(maximum.max(1))) as f32,
                )
            };
            if response.drag_started() || response.clicked() {
                response.request_focus();
                if let Some(pos) = response.interact_pointer_pos() {
                    self.lower_handle = self.mode == Mode::Range
                        && (pos.y - y(lower)).abs() < (pos.y - y(upper)).abs();
                }
            }
            if (response.dragged() || response.clicked())
                && let Some(pos) = response.interact_pointer_pos()
            {
                let value = ((track.bottom() - pos.y) / track.height() * maximum as f32)
                    .round()
                    .clamp(0.0, maximum as f32) as u32;
                if self.mode == Mode::All {
                    self.mode = Mode::Through;
                }
                if self.lower_handle && self.mode == Mode::Range {
                    lower = value.min(upper);
                } else {
                    upper = value.max(lower);
                }
            }
            // Response focus queries read Context; never call them under input_mut.
            let hovered = response.hovered();
            let focused = response.has_focus();
            let (step, shift) = ui.input_mut(|i| {
                let mut step = 0;
                if hovered {
                    // Consume the wheel here so the canvas/panels do not also zoom/scroll.
                    i.raw.events.retain(|event| {
                        if let egui::Event::MouseWheel { unit, delta, .. } = event {
                            self.wheel += delta.y
                                * match unit {
                                    egui::MouseWheelUnit::Point => 1.0 / 24.0,
                                    egui::MouseWheelUnit::Line => 1.0,
                                    egui::MouseWheelUnit::Page => 10.0,
                                };
                            false
                        } else {
                            true
                        }
                    });
                    step += self.wheel.trunc() as i32;
                    self.wheel -= self.wheel.trunc();
                    i.smooth_scroll_delta.y = 0.0;
                }
                if focused {
                    step += i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) as i32
                        - i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) as i32;
                    step += i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowUp) as i32
                        - i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowDown) as i32;
                }
                (step, i.modifiers.shift)
            });
            if step != 0 {
                if self.mode == Mode::All {
                    self.mode = Mode::Through;
                }
                if shift && self.mode == Mode::Range {
                    lower = (i64::from(lower) + i64::from(step)).clamp(0, i64::from(upper)) as u32;
                } else {
                    upper = (i64::from(upper) + i64::from(step))
                        .clamp(i64::from(lower), i64::from(maximum))
                        as u32;
                }
            }
            if previous_controls != (self.mode, lower, upper) {
                self.select(view, lower, upper);
            }
            let painter = ui.painter();
            painter.rect_filled(track, 3.0, ui.visuals().widgets.inactive.bg_fill);
            let lo = if self.mode == Mode::Single {
                upper
            } else {
                lower
            };
            painter.line_segment(
                [
                    egui::pos2(track.center().x, y(lo)),
                    egui::pos2(track.center().x, y(upper)),
                ],
                egui::Stroke::new(5.0, ui.visuals().selection.bg_fill),
            );
            for value in [lo, upper] {
                painter.circle_filled(
                    egui::pos2(track.center().x, y(value)),
                    6.0,
                    ui.visuals().selection.stroke.color,
                );
            }
            // Goal-adjustment markers retain physical layer ordering.
            let mut previous_row = None;
            for (index, layer) in review.bundle.plan.layers.iter().enumerate() {
                if (layer.height_mm - review.capture.report.layer_height.requested_mm).abs()
                    > review.bundle.plan.layer_quantization_mm
                {
                    let row = y(index as u32).round() as i32;
                    if previous_row == Some(row) {
                        continue;
                    }
                    previous_row = Some(row);
                    let pos = egui::pos2(track.right() + 7.0, y(index as u32));
                    painter.line_segment(
                        [pos, pos + egui::vec2(4.0, 0.0)],
                        egui::Stroke::new(1.0, ui.visuals().warn_fg_color),
                    );
                }
            }
            ui.small(format!("{}", lo + 1));
        });
        if (old.layer, old.layer_end) != (view.layer, view.layer_end) {
            view.events = event_window(review, *view).end;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn externally_projected_range_changes_single_mode_without_collapsing_selection() {
        let mut browser = Browser {
            mode: Mode::Single,
            ..Default::default()
        };
        let mut view = View {
            layer: Some(0),
            layer_end: Some(1),
            ..Default::default()
        };
        browser.synchronize_selection(&view);
        assert!(browser.mode == Mode::Range);
        assert_eq!((view.layer, view.layer_end), (Some(0), Some(1)));
        browser.select(&mut view, 0, 1);
        assert_eq!((view.layer, view.layer_end), (Some(0), Some(1)));
        view.layer_end = None;
        browser.synchronize_selection(&view);
        assert!(browser.mode == Mode::Single);
        view.layer = None;
        browser.synchronize_selection(&view);
        assert!(browser.mode == Mode::All);
    }

    #[test]
    fn layer_windows_select_layers_beyond_the_former_u16_index() {
        let mut bytes = Vec::new();
        let sentinel = REPLAY_LIFECYCLE_LAYER;
        for layer in [sentinel, 0, 65_534, 65_535, 70_000, 70_000, sentinel] {
            let mut record = [0; REPLAY_RECORD_BYTES];
            record[REPLAY_LAYER_OFFSET..REPLAY_LAYER_OFFSET + 4]
                .copy_from_slice(&layer.to_le_bytes());
            bytes.extend(record);
        }
        let view = View {
            layer: Some(70_000),
            ..Default::default()
        };
        assert_eq!(record_window(&bytes, view), 4..6);
        let through = View {
            layer: Some(65_535),
            layer_end: Some(70_000),
            ..Default::default()
        };
        assert_eq!(record_window(&bytes, through), 3..6);
    }

    #[test]
    fn layer_windows_preserve_global_ordinals_and_exclude_lifecycle_sentinels() {
        let mut bytes = Vec::new();
        let sentinel = REPLAY_LIFECYCLE_LAYER;
        for layer in [sentinel, sentinel, 0, 0, 1, 1, 3, 3, sentinel, sentinel] {
            let mut record = [0; REPLAY_RECORD_BYTES];
            record[REPLAY_LAYER_OFFSET..REPLAY_LAYER_OFFSET + 4]
                .copy_from_slice(&layer.to_le_bytes());
            bytes.extend(record);
        }
        let view = View {
            layer: Some(1),
            ..Default::default()
        };
        assert_eq!(record_window(&bytes, view), 4..6);
        assert_eq!(
            record_window(
                &bytes,
                View {
                    layer: Some(2),
                    ..view
                }
            ),
            6..6
        );
        assert_eq!(
            record_window(
                &bytes,
                View {
                    layer_end: Some(3),
                    ..view
                }
            ),
            4..8
        );
        assert_eq!(
            record_window(
                &bytes,
                View {
                    layer: Some(0),
                    layer_end: Some(1),
                    ..view
                }
            ),
            2..6
        );
        assert_eq!(record_window(&bytes, View::default()), 0..10);
        assert_eq!(record_window(&[], view), 0..0);
        let mut browser = Browser::default();
        let mut selected = View::default();
        browser.mode = Mode::Range;
        browser.select(&mut selected, 1, 3);
        assert_eq!((selected.layer, selected.layer_end), (Some(1), Some(3)));
        browser.mode = Mode::Single;
        browser.select(&mut selected, 1, 2);
        assert_eq!((selected.layer, selected.layer_end), (Some(2), None));
    }
}
