// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Shared presentation components; never mutate numeric color or material identity.
use egui::{Color32, RichText, Ui};
use spiling_contracts::color_naming::*;
use std::{collections::BTreeMap, sync::Arc};
#[derive(Clone, Default)]
struct Names(BTreeMap<[u8; 3], Arc<NamedColor>>);
fn cached(ui: &Ui, rgb: [u8; 3]) -> Arc<NamedColor> {
    ui.data_mut(|data| {
        let cache = data.get_temp_mut_or_default::<Names>(egui::Id::unique("color-names-v1"));
        if let Some(value) = cache.0.get(&rgb) {
            return value.clone();
        }
        if cache.0.len() == 256 {
            cache.0.pop_first();
        }
        let value = Arc::new(spiling_appearance::naming::describe(
            rgb,
            ColorNamingMode::Descriptive,
        ));
        cache.0.insert(rgb, value.clone());
        value
    })
}
/// Shared text for candidate lists and compact legends. The hex sample remains explicit.
pub fn color_summary(rgb: [u8; 3]) -> String {
    let named = spiling_appearance::naming::describe(rgb, ColorNamingMode::Descriptive);
    format!(
        "{} · {}",
        named.name.as_ref().expect("descriptive name").label,
        named.hex
    )
}
pub fn color_swatch(ui: &mut Ui, rgb: [u8; 3]) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, 3, Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
    ui.painter().rect_stroke(
        rect,
        3,
        egui::Stroke::new(1.0, ui.visuals().text_color()),
        egui::StrokeKind::Inside,
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::Label,
            true,
            format!("sRGB color sample {}", srgb8_hex(rgb)),
        )
    });
    response
}
/// Declared names win; the generated name remains inspectable in details.
pub fn color_chip(ui: &mut Ui, rgb: [u8; 3], declared_name: Option<&str>) {
    let named = cached(ui, rgb);
    color_chip_named(ui, &named, declared_name);
}
fn color_chip_named(ui: &mut Ui, named: &NamedColor, declared_name: Option<&str>) {
    ui.horizontal_wrapped(|ui| {
        color_swatch(ui, named.srgb8);
        ui.label(
            declared_name
                .or_else(|| named.name.as_ref().map(|n| n.label.as_str()))
                .unwrap_or("No exact name"),
        );
        ui.label(RichText::new(&named.hex).monospace().small());
    });
}
pub fn color_details(ui: &mut Ui, named: &NamedColor) {
    let Some(name) = &named.name else {
        ui.weak("No exact match in the pinned xkcd survey.");
        return;
    };
    let kind = if named.srgb8 == name.reference_srgb8 {
        "Exact survey sample"
    } else if name.generated {
        "Generated descriptor"
    } else {
        "Approximate reference name"
    };
    ui.label(format!("{kind}: {}", name.label));
    ui.label(format!(
        "Reference: {} · {}",
        name.reference_name,
        srgb8_hex(name.reference_srgb8)
    ));
    ui.label(format!(
        "OKLab distance {:.5} · not a confidence probability",
        name.distance_oklab
    ));
    ui.weak(format!(
        "xkcd CC0 survey · naming v{COLOR_NAMING_VERSION} · {}",
        &COLOR_NAMING_DATASET_SHA256[..12]
    ));
}
#[derive(Clone)]
pub struct ColorInspector {
    pub hex: String,
    pub mode: ColorNamingMode,
}
impl Default for ColorInspector {
    fn default() -> Self {
        Self {
            hex: "#C75C3C".into(),
            mode: ColorNamingMode::Descriptive,
        }
    }
}
impl ColorInspector {
    pub fn show(&mut self, ui: &mut Ui) {
        ui.push_id("color-reference", |ui| {
            ui.heading("Color reference");
            ui.label("Name an sRGB sample");
            ui.horizontal_wrapped(|ui| {
                let label = ui.label("Hex");
                ui.add(
                    egui::TextEdit::singleline(&mut self.hex)
                        .id(ui.make_persistent_id("hex"))
                        .char_limit(16)
                        .desired_width(110.0),
                )
                .labelled_by(label.id);
            });
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut self.mode, ColorNamingMode::Exact, "Exact");
                ui.selectable_value(&mut self.mode, ColorNamingMode::Nearest, "Nearest");
                ui.selectable_value(&mut self.mode, ColorNamingMode::Descriptive, "Descriptive");
            });
            match parse_srgb8_hex(&self.hex) {
                Ok(rgb) => {
                    let named = if self.mode == ColorNamingMode::Descriptive {
                        cached(ui, rgb)
                    } else {
                        Arc::new(spiling_appearance::naming::describe(rgb, self.mode))
                    };
                    color_chip_named(ui, &named, None);
                    ui.collapsing("Name details", |ui| color_details(ui, &named));
                }
                Err(error) => {
                    ui.label(error);
                }
            }
            ui.weak("Derived display label. It does not change a material name or predict printed appearance.");
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn shape_text(shape: &egui::Shape, text: &mut String) {
        match shape {
            egui::Shape::Text(t) => {
                text.push_str(&t.galley.job.text);
                text.push('\n');
            }
            egui::Shape::Vec(shapes) => {
                for s in shapes {
                    shape_text(s, text);
                }
            }
            _ => {}
        }
    }
    #[test]
    fn production_components_retain_declared_names_hex_and_valid_invalid_states_in_both_themes() {
        for theme in [super::super::Theme::Graphite, super::super::Theme::Paper] {
            let ctx = egui::Context::default();
            theme.apply(&ctx);
            let mut inspector = ColorInspector::default();
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                color_chip(ui, [255; 3], Some("Supplier ivory"));
                color_chip(ui, [0; 3], None);
                inspector.show(ui);
            });
            let mut text = String::new();
            for shape in &output.shapes {
                shape_text(&shape.shape, &mut text);
            }
            assert!(text.contains("Supplier ivory"));
            assert!(text.contains("#FFFFFF"));
            assert!(text.contains("Black"));
            assert!(text.contains("#000000"));
            output.drop_without_applying_deltas();
            inspector.hex = "#12345678".into();
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                inspector.show(ui);
            });
            let mut text = String::new();
            for shape in &output.shapes {
                shape_text(&shape.shape, &mut text);
            }
            assert!(text.contains("alpha is not supported"));
            assert_eq!(inspector.hex, "#12345678");
            output.drop_without_applying_deltas();
        }
    }
}
