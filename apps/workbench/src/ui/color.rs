// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Shared color presentation, independent of material identity and process settings.
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

/// Meaning is explicit at the call site; a generated name cannot establish it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMeaning {
    DeclaredAppearance,
    Target,
    CatalogEstimate,
    ReferenceEstimate,
    Sample,
}
impl ColorMeaning {
    pub fn label(self) -> &'static str {
        match self {
            Self::DeclaredAppearance => "Declared appearance",
            Self::Target => "Target",
            Self::CatalogEstimate => "Catalog estimate",
            Self::ReferenceEstimate => "Reference estimate",
            Self::Sample => "sRGB sample",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorDensity {
    /// Lists and legends; the owning selection/details surface supplies inspection.
    Compact,
    /// Color, hex and a keyboard-accessible disclosure.
    Standard,
    /// The same information with details already visible.
    Expanded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColorSample {
    Srgb8([u8; 3]),
    Unspecified,
}

/// Transient borrowed presentation. No stock color is inferred from material names.
#[derive(Debug, Clone, Copy)]
pub struct ColorPresentation<'a> {
    sample: ColorSample,
    declared_name: Option<&'a str>,
    meaning: ColorMeaning,
}
impl<'a> ColorPresentation<'a> {
    pub fn known(srgb8: [u8; 3], meaning: ColorMeaning) -> Self {
        Self {
            sample: ColorSample::Srgb8(srgb8),
            declared_name: None,
            meaning,
        }
    }
    pub fn unspecified(meaning: ColorMeaning) -> Self {
        Self {
            sample: ColorSample::Unspecified,
            declared_name: None,
            meaning,
        }
    }
    pub fn declared_name(mut self, name: &'a str) -> Self {
        self.declared_name = Some(name).filter(|name| !name.trim().is_empty());
        self
    }
    fn label<'b>(&'b self, named: Option<&'b NamedColor>) -> &'b str {
        self.declared_name
            .or_else(|| named.and_then(|n| n.name.as_ref().map(|n| n.label.as_str())))
            .unwrap_or(match self.sample {
                ColorSample::Srgb8(_) => "No exact name",
                ColorSample::Unspecified => "Color not specified",
            })
    }
    /// Text-only consumers share the component's naming precedence and source label.
    pub fn summary(self) -> String {
        let named = match self.sample {
            ColorSample::Srgb8(rgb) => Some(spiling_appearance::naming::describe(
                rgb,
                ColorNamingMode::Descriptive,
            )),
            ColorSample::Unspecified => None,
        };
        self.summary_with(named.as_ref())
    }
    fn summary_with(self, named: Option<&NamedColor>) -> String {
        let missing_sample =
            if self.sample == ColorSample::Unspecified && self.declared_name.is_some() {
                " · Sample not specified"
            } else {
                ""
            };
        format!(
            "{} · {}{missing_sample}",
            self.meaning.label(),
            self.label(named)
        )
    }
    /// `id_salt` identifies the owning field/row, independent of its changing value.
    /// Returns the disclosure response in Standard density, otherwise the color row.
    pub fn show(
        self,
        ui: &mut Ui,
        id_salt: impl egui::AsIdSalt,
        density: ColorDensity,
    ) -> egui::Response {
        let named = match self.sample {
            ColorSample::Srgb8(rgb) => Some(cached(ui, rgb)),
            ColorSample::Unspecified => None,
        };
        self.show_named(ui, id_salt, density, named.as_deref())
    }
    fn show_named(
        self,
        ui: &mut Ui,
        id_salt: impl egui::AsIdSalt,
        density: ColorDensity,
        named: Option<&NamedColor>,
    ) -> egui::Response {
        ui.push_id(id_salt, |ui| {
            let row = ui
                .horizontal_wrapped(|ui| {
                    swatch(ui, self.sample, &self.summary_label(named));
                    ui.label(self.label(named));
                    if density != ColorDensity::Compact
                        && let Some(named) = named
                    {
                        ui.label(RichText::new(&named.hex).monospace().small());
                    }
                    if self.sample == ColorSample::Unspecified && self.declared_name.is_some() {
                        ui.weak("Sample not specified");
                    }
                    ui.weak(self.meaning.label());
                })
                .response;
            match density {
                ColorDensity::Compact => row,
                ColorDensity::Standard => {
                    ui.collapsing("Color details", |ui| self.details(ui, named))
                        .header_response
                }
                ColorDensity::Expanded => {
                    self.details(ui, named);
                    row
                }
            }
        })
        .inner
    }
    fn summary_label(self, named: Option<&NamedColor>) -> String {
        let sample = named.map_or_else(String::new, |n| format!(" · {}", n.hex));
        format!("{}{sample}", self.summary_with(named))
    }
    fn details(self, ui: &mut Ui, named: Option<&NamedColor>) {
        if let Some(declared) = self.declared_name {
            ui.label(format!("Declared name: {declared}"));
        }
        if let Some(named) = named {
            naming_details(ui, named);
        } else {
            ui.label("No sRGB sample was declared.");
        }
    }
}

fn swatch(ui: &mut Ui, sample: ColorSample, accessible_label: &str) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
    match sample {
        ColorSample::Srgb8([r, g, b]) => {
            ui.painter()
                .rect_filled(rect, 3, Color32::from_rgb(r, g, b));
        }
        ColorSample::Unspecified => {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "?",
                egui::FontId::proportional(13.0),
                ui.visuals().text_color(),
            );
        }
    }
    ui.painter().rect_stroke(
        rect,
        3,
        egui::Stroke::new(1.0, ui.visuals().text_color()),
        egui::StrokeKind::Inside,
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, accessible_label));
}

fn naming_details(ui: &mut Ui, named: &NamedColor) {
    let Some(name) = &named.name else {
        ui.label("No exact match in the pinned xkcd survey.");
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
                    // Reference-mode selection is confined to this diagnostic inspector.
                    ColorPresentation::known(rgb, ColorMeaning::Sample).show_named(
                        ui,
                        "sample",
                        ColorDensity::Standard,
                        Some(&named),
                    );
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
    fn rendered(theme: super::super::Theme, width: f32, draw: impl FnOnce(&mut Ui)) -> String {
        let ctx = egui::Context::default();
        theme.apply(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 2000.0),
            )),
            ..Default::default()
        };
        let mut draw = Some(draw);
        let output = ctx.run_ui(input, |ui| draw.take().unwrap()(ui));
        let mut text = String::new();
        for shape in &output.shapes {
            shape_text(&shape.shape, &mut text);
        }
        output.drop_without_applying_deltas();
        text
    }

    #[test]
    fn summaries_preserve_declared_names_meaning_and_missing_samples() {
        for meaning in [
            ColorMeaning::DeclaredAppearance,
            ColorMeaning::Target,
            ColorMeaning::CatalogEstimate,
            ColorMeaning::ReferenceEstimate,
            ColorMeaning::Sample,
        ] {
            let color = ColorPresentation::known([207, 82, 78], meaning);
            assert_eq!(color.summary(), format!("{} · Dark coral", meaning.label()));
            let declared = color.declared_name("Supplier sunset").summary();
            assert_eq!(declared, format!("{} · Supplier sunset", meaning.label()));
            assert!(!declared.contains("Dark coral"));
            assert_eq!(color.declared_name("  ").summary(), color.summary());
        }
        let unknown = ColorPresentation::unspecified(ColorMeaning::DeclaredAppearance);
        assert_eq!(
            unknown.summary(),
            "Declared appearance · Color not specified"
        );
        assert_eq!(
            unknown.declared_name("Supplier natural").summary(),
            "Declared appearance · Supplier natural · Sample not specified"
        );
    }

    #[test]
    fn density_changes_disclosure_without_changing_identity_in_both_themes() {
        for theme in [super::super::Theme::Graphite, super::super::Theme::Paper] {
            for width in [320.0, 640.0] {
                for density in [
                    ColorDensity::Compact,
                    ColorDensity::Standard,
                    ColorDensity::Expanded,
                ] {
                    let text = rendered(theme, width, |ui| {
                        ColorPresentation::known([207, 82, 78], ColorMeaning::CatalogEstimate)
                            .declared_name("Supplier sunset")
                            .show(ui, "appearance", density);
                    });
                    assert!(text.contains("Supplier sunset"));
                    assert!(text.contains("Catalog estimate"));
                    assert_eq!(text.contains("#CF524E"), density != ColorDensity::Compact);
                    assert_eq!(
                        text.contains("Color details"),
                        density == ColorDensity::Standard
                    );
                    assert_eq!(
                        text.contains("Exact survey sample: Dark coral"),
                        density == ColorDensity::Expanded
                    );
                    let missing = rendered(theme, width, |ui| {
                        ColorPresentation::unspecified(ColorMeaning::DeclaredAppearance)
                            .show(ui, "unknown", density);
                    });
                    assert!(missing.contains("Color not specified"));
                    assert!(!missing.contains("#"));
                    assert!(!missing.contains("White"));
                    assert_eq!(
                        missing.contains("No sRGB sample was declared."),
                        density == ColorDensity::Expanded
                    );
                }
            }
        }
    }

    #[test]
    fn reference_modes_and_invalid_editing_do_not_change_normal_presentation() {
        let mut inspector = ColorInspector {
            hex: "#123456".into(),
            mode: ColorNamingMode::Exact,
        };
        let exact = rendered(super::super::Theme::Paper, 640.0, |ui| {
            inspector.show(ui);
            ColorPresentation::known([18, 52, 86], ColorMeaning::Target).show(
                ui,
                "target",
                ColorDensity::Compact,
            );
        });
        assert!(exact.contains("No exact name"));
        let expected =
            spiling_appearance::naming::describe([18, 52, 86], ColorNamingMode::Descriptive);
        assert!(exact.contains(&expected.name.unwrap().label));
        assert!(exact.contains("Target"));
        assert_eq!(inspector.hex, "#123456");
        inspector.hex = "#12345678".into();
        let invalid = rendered(super::super::Theme::Graphite, 320.0, |ui| {
            inspector.show(ui)
        });
        assert!(invalid.contains("alpha is not supported"));
        assert_eq!(inspector.hex, "#12345678");
    }
    #[test]
    fn keyboard_disclosure_keeps_field_identity_and_neighbor_details_separate() {
        let ctx = egui::Context::default();
        super::super::Theme::Graphite.apply(&ctx);
        ctx.global_style_mut(|style| style.animation_time = 0.0);
        let frame = |events, rgb| {
            let mut fields = None;
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(640.0, 1000.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let first = ColorPresentation::known(rgb, ColorMeaning::DeclaredAppearance)
                        .declared_name("First supplier name")
                        .show(ui, "first", ColorDensity::Standard);
                    let second =
                        ColorPresentation::known([255; 3], ColorMeaning::DeclaredAppearance)
                            .declared_name("Second supplier name")
                            .show(ui, "second", ColorDensity::Standard);
                    fields = Some((first.id, second.id));
                },
            );
            let mut text = String::new();
            for shape in &output.shapes {
                shape_text(&shape.shape, &mut text);
            }
            output.drop_without_applying_deltas();
            (fields.unwrap(), text)
        };
        let (fields, text) = frame(Vec::new(), [0; 3]);
        assert_ne!(fields.0, fields.1);
        assert!(!text.contains("Declared name:"));
        ctx.memory_mut(|memory| memory.request_focus(fields.0));
        frame(Vec::new(), [0; 3]);
        let (_, text) = frame(
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            }],
            [0; 3],
        );
        assert!(text.contains("Declared name: First supplier name"));
        assert!(!text.contains("Declared name: Second supplier name"));
        let (updated, text) = frame(Vec::new(), [127; 3]);
        assert_eq!(
            updated, fields,
            "sample edits must preserve field/disclosure identity"
        );
        assert!(text.contains("Declared name: First supplier name"));
        assert!(text.contains("#7F7F7F"));
        assert!(!text.contains("Declared name: Second supplier name"));
    }
}
