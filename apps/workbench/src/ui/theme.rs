// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use egui::{Color32, Stroke};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    System,
    Graphite,
    Paper,
}

#[derive(Clone, Copy)]
pub struct Tokens {
    pub panel: Color32,
    pub canvas: Color32,
    pub control: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub divider: Color32,
    pub boundary: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub selected: Color32,
    pub focus: Color32,
    pub focus_gap: Color32,
    pub warning: Color32,
    pub error: Color32,
    pub success: Color32,
    pub bed: Color32,
    pub source: Color32,
}

pub(crate) const fn rgb(value: u32) -> Color32 {
    Color32::from_rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

pub(super) const ACTION: [Color32; 3] = [rgb(0x124C93), rgb(0x14549E), rgb(0x0D3D78)];

impl Tokens {
    pub fn for_ui(ui: &egui::Ui) -> Self {
        Self::for_dark(ui.visuals().dark_mode)
    }

    pub fn for_dark(dark: bool) -> Self {
        if dark { Theme::Graphite } else { Theme::Paper }.tokens()
    }
}

const ACTION_FONT: &str = "Spiling primary actions";

pub(super) fn action_font(ctx: &egui::Context) -> egui::FontId {
    let installed = ctx.data(|data| {
        data.get_temp::<bool>(egui::Id::unique(ACTION_FONT))
            .unwrap_or(false)
    });
    let family = if installed {
        egui::FontFamily::Name(ACTION_FONT.into())
    } else {
        egui::FontFamily::Proportional
    };
    egui::FontId::new(14.0, family)
}

impl Theme {
    pub fn tokens(self) -> Tokens {
        match self {
            Self::System | Self::Graphite => Tokens {
                canvas: rgb(0x12161C),
                panel: rgb(0x1B212A),
                control: rgb(0x252E3A),
                hover: rgb(0x303B4B),
                pressed: rgb(0x37465A),
                divider: rgb(0x394759),
                boundary: rgb(0x839BB5),
                text: rgb(0xE8EEF6),
                muted: rgb(0xA8B7CA),
                accent: rgb(0x79B8FF),
                selected: rgb(0x263D58),
                focus: rgb(0xA8D9FF),
                focus_gap: rgb(0x12161C),
                warning: rgb(0xF4C46B),
                error: rgb(0xFFB0A8),
                success: rgb(0x86D9B2),
                bed: rgb(0x1B2634),
                source: rgb(0xA1B1C3),
            },
            Self::Paper => Tokens {
                canvas: rgb(0xE8EDF3),
                panel: rgb(0xF6F8FB),
                control: rgb(0xFFFFFF),
                hover: rgb(0xEDF2F8),
                pressed: rgb(0xE1E9F3),
                divider: rgb(0xCBD5E1),
                boundary: rgb(0x697B90),
                text: rgb(0x17212E),
                muted: rgb(0x4B5D73),
                accent: rgb(0x175FB5),
                selected: rgb(0xDFEDFC),
                focus: rgb(0x18497A),
                focus_gap: rgb(0xFFFFFF),
                warning: rgb(0x805000),
                error: rgb(0xA72B28),
                success: rgb(0x126244),
                bed: rgb(0xD9E2EC),
                source: rgb(0x67798C),
            },
        }
    }

    pub fn apply(self, ctx: &egui::Context) {
        let fonts_installed = egui::Id::unique(ACTION_FONT);
        if !ctx.data(|data| data.get_temp::<bool>(fonts_installed).unwrap_or(false)) {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(
                ACTION_FONT.to_owned(),
                egui::FontData::from_static(include_bytes!("../../fonts/IBMPlexSans-SemiBold.ttf"))
                    .into(),
            );
            let mut fallback = fonts.families[&egui::FontFamily::Proportional].clone();
            fallback.insert(0, ACTION_FONT.to_owned());
            fonts
                .families
                .insert(egui::FontFamily::Name(ACTION_FONT.into()), fallback);
            ctx.set_fonts(fonts);
            ctx.data_mut(|data| data.insert_temp(fonts_installed, true));
        }
        for (theme, native) in [
            (Self::Graphite, egui::Theme::Dark),
            (Self::Paper, egui::Theme::Light),
        ] {
            let t = theme.tokens();
            ctx.style_mut_of(native, |style| {
                let mut v = match native {
                    egui::Theme::Dark => egui::Visuals::dark(),
                    egui::Theme::Light => egui::Visuals::light(),
                };
                v.panel_fill = t.panel;
                v.window_fill = t.panel;
                v.window_stroke = Stroke::new(1.0, t.divider);
                v.extreme_bg_color = t.canvas;
                v.faint_bg_color = t.hover;
                v.override_text_color = Some(t.text);
                v.weak_text_color = Some(t.muted);
                v.selection.bg_fill = t.selected;
                v.selection.stroke = Stroke::new(1.0, t.accent);
                v.hyperlink_color = t.accent;
                v.warn_fg_color = t.warning;
                v.error_fg_color = t.error;
                v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, t.divider);
                for (widget, fill) in [
                    (&mut v.widgets.inactive, t.control),
                    (&mut v.widgets.hovered, t.hover),
                    (&mut v.widgets.active, t.pressed),
                    (&mut v.widgets.open, t.hover),
                ] {
                    widget.weak_bg_fill = fill;
                    widget.bg_fill = fill;
                    widget.bg_stroke = Stroke::new(1.0, t.boundary);
                    widget.fg_stroke.color = t.text;
                    widget.corner_radius = 4.into();
                    widget.expansion = 0.0;
                }
                style.visuals = v;
                style.spacing.item_spacing = egui::vec2(8.0, 8.0);
                style.spacing.button_padding = egui::vec2(10.0, 6.0);
                style.spacing.interact_size.y = 28.0;
                for (kind, size) in [
                    (egui::TextStyle::Body, 13.0),
                    (egui::TextStyle::Button, 13.0),
                    (egui::TextStyle::Heading, 17.0),
                    (egui::TextStyle::Small, 12.0),
                ] {
                    style
                        .text_styles
                        .insert(kind, egui::FontId::proportional(size));
                }
            });
        }
        ctx.options_mut(|options| options.fallback_theme = egui::Theme::Dark);
        ctx.set_theme(match self {
            Self::System => egui::ThemePreference::System,
            Self::Graphite => egui::ThemePreference::Dark,
            Self::Paper => egui::ThemePreference::Light,
        });
    }
}
