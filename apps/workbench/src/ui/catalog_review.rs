// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use spiling_contracts::catalog::{CatalogItem, CatalogRecord};
pub enum CatalogReviewAction {
    Check,
    Review(Box<CatalogRecord>),
}
/// Reference maintenance never writes; the owning editor retains CAS admission.
pub fn catalog_review(
    ui: &mut egui::Ui,
    captured: &CatalogRecord,
    latest: Option<&Result<CatalogRecord, String>>,
    ready: bool,
) -> Option<CatalogReviewAction> {
    let mut action = None;
    ui.collapsing("Catalog updates", |ui| {
        if ui
            .add_enabled(ready, egui::Button::new("Check bundled catalog"))
            .clicked()
        {
            action = Some(CatalogReviewAction::Check);
        }
        let Some(latest) = latest else {
            return;
        };
        match latest {
            Err(error) => {
                ui.label(error);
            }
            Ok(current) if current.key() != captured.key() => {
                ui.label("Select this source again to check it.");
            }
            Ok(current) if current == captured => {
                ui.label("Captured declaration matches the bundled catalog.");
            }
            Ok(current) => {
                ui.label("Source update available");
                let changes = catalog_changes(&captured.item, &current.item);
                if changes.is_empty() {
                    ui.weak("Source identity changed; declared values are unchanged.");
                }
                egui::Grid::new("catalog-update-values")
                    .striped(true)
                    .show(ui, |ui| {
                        for (field, before, after) in changes {
                            ui.label(field);
                            ui.label(before);
                            ui.label("→");
                            ui.label(after);
                            ui.end_row();
                        }
                    });
                ui.weak(
                    "Review a draft before saving. Local edits and process settings are retained.",
                );
                if ui
                    .add_enabled(ready, egui::Button::new("Review update"))
                    .clicked()
                {
                    action = Some(CatalogReviewAction::Review(Box::new(current.clone())));
                }
                ui.collapsing("Source identities", |ui| {
                    ui.label(format!("Captured: {}", captured.origin.revision));
                    ui.label(format!("Bundled: {}", current.origin.revision));
                    ui.label(&current.origin.url);
                });
            }
        }
    });
    action
}
fn catalog_changes(
    before: &CatalogItem,
    after: &CatalogItem,
) -> Vec<(&'static str, String, String)> {
    fn value<T: ToString>(v: &Option<T>) -> String {
        v.as_ref().map_or("Unknown".into(), ToString::to_string)
    }
    let mut rows = Vec::new();
    let mut field = |label, a: String, b: String| {
        if a != b {
            rows.push((label, a, b));
        }
    };
    match (before, after) {
        (
            CatalogItem::Filament {
                name: a,
                declaration: ad,
                srgb: ac,
            },
            CatalogItem::Filament {
                name: b,
                declaration: bd,
                srgb: bc,
            },
        ) => {
            field("Name", a.clone(), b.clone());
            field("Brand", value(&ad.brand), value(&bd.brand));
            field(
                "Material",
                value(&ad.material_label),
                value(&bd.material_label),
            );
            field(
                "Diameter (mm)",
                value(&ad.diameter_mm),
                value(&bd.diameter_mm),
            );
            field(
                "HueForge TD (mm)",
                value(&ad.hueforge_td_mm),
                value(&bd.hueforge_td_mm),
            );
            field(
                "Declared RGB",
                spiling_contracts::color_naming::srgb8_hex(*ac),
                spiling_contracts::color_naming::srgb8_hex(*bc),
            );
        }
        (
            CatalogItem::Printer {
                manufacturer: a,
                model: am,
                volume_mm: av,
                ..
            },
            CatalogItem::Printer {
                manufacturer: b,
                model: bm,
                volume_mm: bv,
                ..
            },
        ) => {
            field("Manufacturer", a.clone(), b.clone());
            field("Model", am.clone(), bm.clone());
            let size = |v: &Option<[f64; 3]>| {
                v.map_or("Unknown".into(), |v| {
                    format!("{} × {} × {} mm", v[0], v[1], v[2])
                })
            };
            field("Build size", size(av), size(bv));
        }
        _ => {}
    }
    rows
}
