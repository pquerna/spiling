// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;

/// Production controls with local illustrative values; no host or engine access.
pub fn gallery(ui: &mut Ui, theme: &mut Theme) {
    theme_choice(ui, theme);
    pane_header(
        ui,
        "Component gallery",
        "Illustrative values · no project changes",
    );
    let id = ui.make_persistent_id("gallery-category");
    let mut category = ui.data_mut(|d| d.get_temp::<u8>(id).unwrap_or(0));
    ui.horizontal_wrapped(|ui| {
        for (index, title) in ["Panels", "Fields", "Color", "Acquisition"]
            .iter()
            .enumerate()
        {
            ui.selectable_value(&mut category, index as u8, *title);
        }
    });
    ui.data_mut(|d| d.insert_temp(id, category));
    ui.separator();
    if category == 3 {
        acquisition(ui);
        return;
    }
    egui::ScrollArea::vertical()
        .id_salt(("gallery-content", category))
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width().min(650.0));
            match category {
                0 => panel(ui),
                1 => fields(ui, theme.tokens()),
                _ => colors(ui),
            }
        });
}
fn panel(ui: &mut Ui) {
    let id = ui.make_persistent_id("gallery-panel-state");
    let mut state = ui.data_mut(|d| d.get_temp::<u8>(id).unwrap_or(0));
    let previous_state = state;
    let value_id = ui.make_persistent_id("panel-height");
    let mut value = ui.data_mut(|d| d.get_temp::<f64>(value_id).unwrap_or(0.2));
    ui.horizontal_wrapped(|ui| {
        for (index, title) in [
            "Ready",
            "Changed",
            "Blocked",
            "Pending",
            "Uncertain",
            "Invalid",
            "Read-only",
            "Stale",
        ]
        .iter()
        .enumerate()
        {
            ui.selectable_value(&mut state, index as u8, *title);
        }
    });
    if state != previous_state {
        value = if state == 5 {
            -0.2
        } else if state == 1 {
            0.25
        } else {
            0.2
        };
    }
    let open_id = ui.make_persistent_id("gallery-panel-open");
    let mut open = ui.data_mut(|d| d.get_temp::<bool>(open_id).unwrap_or(true));
    if !open {
        if ui.button("Reopen specimen").clicked() {
            open = true;
        }
    } else {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            pane_header(ui, "Construction", "Housing · Project settings");
            if close(ui, "Close specimen") { open = false; }
            match state {
                1 => { ui.label("Unapplied changes"); }
                2 => { ui.label("Choose a material before compiling"); }
                3 => { ui.spinner(); ui.label("Applying settings · waiting for receipt"); }
                4 => { ui.label("Save outcome uncertain. Inspect the saved project."); }
                5 => { problem(ui, "Enter a valid height", "The illustrative field requires a finite positive height."); }
                6 => { ui.label("Read-only project"); }
                7 => { ui.label("Program is out of date. Verify current output."); }
                _ => { ui.label("Current settings"); }
            }
            group(ui, "Geometry");
            ui.add_enabled_ui(!matches!(state, 3 | 6), |ui| {
                if number(ui, "Layer height (mm)", &mut value, 0.01).changed() { state = if value.is_finite() && value > 0.0 { 1 } else { 5 }; }
            });
            help(ui, "specimen-help", "Construction settings", "Apply commits a complete settings draft in production. This specimen only demonstrates presentation and never dispatches engine work.");
            ui.separator();
            let reason = match state { 2 => Some("Choose a material first"), 3 => Some("Waiting for the operation receipt"), 4 => Some("Resolve the save outcome first"), 5 => Some("Correct the height before applying"), 6 => Some("Open a writable project to edit settings"), _ => None };
            if primary(ui, if state == 1 { "Apply settings" } else if state == 7 { "Verify program" } else { "Compile program" }, reason) { ui.data_mut(|d| d.insert_temp(ui_id("gallery-last-action"), true)); }
            if ui.data(|d| d.get_temp::<bool>(ui_id("gallery-last-action")).unwrap_or(false)) { ui.label("Example action selected · no engine request"); }
        });
    }
    ui.data_mut(|d| {
        d.insert_temp(id, state);
        d.insert_temp(value_id, value);
        d.insert_temp(open_id, open);
    });
}
fn ui_id(label: &'static str) -> egui::Id {
    egui::Id::unique(label)
}
fn fields(ui: &mut Ui, tokens: Tokens) {
    property(
        ui,
        "Long source label",
        "Housing — 接口 — finite source-bound manufacturing requirements",
        tokens,
    );
    let id = ui.make_persistent_id("gallery-dimension");
    let mut value = ui.data_mut(|d| d.get_temp::<f64>(id).unwrap_or(0.25));
    number(ui, "Layer height (mm)", &mut value, 0.01);
    ui.data_mut(|d| d.insert_temp(id, value));
    ui.add_enabled_ui(false, |ui| {
        number(ui, "Inherited (mm)", &mut 0.4, 0.01);
    });
    group(ui, "Layer height goal");
    let goal_id = ui.make_persistent_id("gallery-layer-goal");
    let mut intent = ui
        .data_mut(|d| d.get_temp::<ManufacturingIntent>(goal_id))
        .unwrap_or_else(|| {
            serde_json::from_str(include_str!(
                "../../../../fixtures/manufacturing/preferred-layers.intent.json"
            ))
            .expect("checked illustrative intent")
        });
    layer_height_goal(ui, &mut intent, tokens);
    ui.data_mut(|d| d.insert_temp(goal_id, intent));
}
fn colors(ui: &mut Ui) {
    let color_id = ui.make_persistent_id("gallery-color-reference");
    let mut inspector = ui.data_mut(|d| d.get_temp::<ColorInspector>(color_id).unwrap_or_default());
    inspector.show(ui);
    ui.data_mut(|d| d.insert_temp(color_id, inspector));
    ui.heading("Color presentation");
    ui.label("The same sample in compact, standard and expanded views");
    let coral = ColorPresentation::known([207, 82, 78], ColorMeaning::DeclaredAppearance)
        .declared_name("Supplier coral");
    for (id, density) in [
        ("compact", ColorDensity::Compact),
        ("standard", ColorDensity::Standard),
        ("expanded", ColorDensity::Expanded),
    ] {
        ui.label(id);
        coral.show(ui, id, density);
    }
    ui.separator();
    for (id, rgb, meaning) in [
        ("white", [255; 3], ColorMeaning::Target),
        ("black", [0; 3], ColorMeaning::CatalogEstimate),
        ("estimate", [189, 120, 101], ColorMeaning::ReferenceEstimate),
    ] {
        ColorPresentation::known(rgb, meaning).show(ui, id, ColorDensity::Standard);
    }
    ColorPresentation::unspecified(ColorMeaning::DeclaredAppearance).show(
        ui,
        "unknown-color",
        ColorDensity::Standard,
    );
    ColorPresentation::unspecified(ColorMeaning::DeclaredAppearance)
        .declared_name("Supplier natural")
        .show(ui, "named-without-sample", ColorDensity::Standard);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn panel_reflows_within_its_available_width_in_both_themes() {
        for mut theme in [Theme::Graphite, Theme::Paper] {
            for width in [240.0, 320.0, 400.0, 600.0] {
                let ctx = egui::Context::default();
                theme.apply(&ctx);
                for _ in 0..2 {
                    let mut right = 0.0;
                    let output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 800.0),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            gallery(ui, &mut theme);
                            right = ui.min_rect().right();
                        },
                    );
                    output.drop_without_applying_deltas();
                    assert!(
                        right <= width + 1.0,
                        "{theme:?}: content used {right} points in {width}"
                    );
                }
            }
        }
    }
}

#[derive(Clone, Default)]
struct AcquisitionSpecimen {
    search: super::CatalogSearch,
    page: Option<spiling_contracts::catalog::CatalogPage>,
    reviewed: Option<spiling_contracts::catalog::CatalogRecord>,
}
fn acquisition(ui: &mut Ui) {
    use spiling_contracts::{catalog::*, geometry::SourceHash};
    let id = ui.make_persistent_id("acquisition-specimen");
    let mut state = ui.data_mut(|d| {
        d.get_temp::<AcquisitionSpecimen>(id).unwrap_or_else(|| {
            let mut state = AcquisitionSpecimen::default();
            state.search.open(CatalogKind::Filament);
            state
        })
    });
    if let Some(row) = state.reviewed.clone() {
        pane_header(
            ui,
            "Review filament",
            "Illustrative declaration · no inventory writes",
        );
        if close(ui, "Back to search") {
            state.reviewed = None;
            state.search.open(CatalogKind::Filament);
        }
        if let CatalogItem::Filament {
            name,
            declaration,
            srgb,
        } = &row.item
        {
            ui.label(name);
            ColorPresentation::known(*srgb, ColorMeaning::DeclaredAppearance).show(
                ui,
                "specimen-color",
                ColorDensity::Standard,
            );
            ui.label(format!(
                "{} · diameter unknown",
                declaration
                    .material_label
                    .as_deref()
                    .unwrap_or("Material unknown")
            ));
        }
        let mut next = row.clone();
        if let CatalogItem::Filament { declaration, .. } = &mut next.item {
            declaration.hueforge_td_mm = Some(3.2);
        }
        next.origin.content = next.item.content_hash().unwrap();
        next.origin.revision = "illustrative-two".into();
        let _ = super::catalog_review(ui, &row, Some(&Ok(next)), true);
        ui.weak("Production saves use the owning library editor and engine receipt.");
    } else {
        let (query, choice) = state.search.show(ui, state.page.as_ref(), None, true);
        if let Some(query) = query {
            let snapshot = SourceHash::from_bytes(b"gallery-only");
            let item = CatalogItem::Filament {
                name: "Ocean blue".into(),
                declaration: spiling_contracts::library::FilamentDeclaration {
                    brand: Some("Illustrative brand".into()),
                    material_label: Some("PLA".into()),
                    diameter_mm: None,
                    hueforge_td_mm: Some(2.1),
                },
                srgb: [0, 128, 180],
            };
            let row = CatalogRecord {
                origin: CatalogOrigin {
                    provider: "gallery-example".into(),
                    id: "ocean-blue".into(),
                    revision: "illustrative-one".into(),
                    snapshot: snapshot.clone(),
                    content: item.content_hash().unwrap(),
                    url: "https://example.org/illustrative-catalog".into(),
                },
                item,
            };
            let matches = "illustrative brand ocean blue pla".contains(&query.query.to_lowercase());
            state.page = Some(CatalogPage {
                snapshot,
                query,
                entries: if matches { vec![row] } else { vec![] },
                total: u32::from(matches),
                next: None,
            });
            ui.ctx().request_repaint();
        }
        if let Some(super::CatalogChoice::Review(row)) = choice {
            state.reviewed = Some(*row);
        }
        if !state.search.open && state.reviewed.is_none() {
            state.search.open(CatalogKind::Filament);
        }
    }
    ui.data_mut(|d| d.insert_temp(id, state));
}
