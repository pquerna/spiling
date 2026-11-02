// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
//! Shared, bounded native acquisition page. Reading a row never creates inventory.
use spiling_contracts::catalog::*;
use std::time::{Duration, Instant};
#[derive(Clone, Default)]
pub struct CatalogSearch {
    pub open: bool,
    pub query: String,
    kind: Option<CatalogKind>,
    changed: Option<Instant>,
    submitted: Option<CatalogQuery>,
    selected: Option<usize>,
    focus: bool,
    composing: bool,
    #[cfg(debug_assertions)]
    timing: Option<Instant>,
}
pub enum CatalogChoice {
    Review(Box<CatalogRecord>),
    Manual,
}
impl CatalogSearch {
    pub fn open(&mut self, kind: CatalogKind) {
        self.open = true;
        if self.kind != Some(kind) {
            self.query.clear();
        }
        self.kind = Some(kind);
        self.changed = Some(Instant::now());
        self.focus = true;
        self.composing = false;
        self.selected = None;
        self.submitted = None;
    }
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        page: Option<&CatalogPage>,
        error: Option<&str>,
        ready: bool,
    ) -> (Option<CatalogQuery>, Option<CatalogChoice>) {
        let kind = self.kind.unwrap_or(CatalogKind::Filament);
        let noun = if kind == CatalogKind::Filament {
            "filament"
        } else {
            "printer"
        };
        super::pane_header(
            ui,
            &format!("Add {noun}"),
            "Catalog · review a declaration before adding",
        );
        ui.horizontal(|ui| {
            if super::close(ui, "Back to library") {
                self.open = false;
            }
        });
        ui.label("Search");
        for event in ui.input(|i| i.events.clone()) {
            if let egui::Event::Ime(event) = event {
                self.composing =
                    matches!(event, egui::ImeEvent::Preedit { text, .. } if !text.is_empty());
            }
        }
        let search_id = egui::Id::unique("reference-search");
        let previous_selection = self.selected;
        if let Some(page) = page.filter(|p| {
            p.query.kind == kind
                && p.query.query == self.query
                && self.submitted.as_ref() == Some(&p.query)
        }) {
            let keyboard = ui.memory(|m| m.has_focus(search_id)) && !self.composing;
            if keyboard
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown))
                && !page.entries.is_empty()
            {
                self.selected = Some(
                    self.selected
                        .map_or(0, |n| (n + 1).min(page.entries.len() - 1)),
                );
            }
            if keyboard
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp))
            {
                self.selected = self.selected.map(|n| n.saturating_sub(1));
            }
        }
        // Single-line TextEdit consumes Enter and surrenders focus. Admit the
        // explicit selected result before the widget handles that event.
        let review_enter = ui.memory(|m| m.has_focus(search_id))
            && !self.composing
            && self.selected.is_some()
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.query)
                .id(search_id)
                .hint_text("Brand, model, material or color")
                .char_limit(256)
                .desired_width(ui.available_width().min(600.0)),
        );
        if self.focus {
            response.request_focus();
            self.focus = false;
        }
        if response.changed() {
            self.changed = Some(Instant::now());
            self.selected = None;
        }
        let matching = page.filter(|p| {
            p.query.kind == kind
                && p.query.query == self.query
                && self.submitted.as_ref() == Some(&p.query)
        });
        let mut request = None;
        if self
            .changed
            .is_some_and(|t| t.elapsed() >= Duration::from_millis(40))
            && ready
            && (self.submitted.is_none()
                || page.is_some_and(|p| Some(&p.query) == self.submitted.as_ref())
                || error.is_some())
        {
            let q = CatalogQuery {
                kind,
                query: self.query.clone(),
                offset: 0,
                snapshot: None,
            };
            #[cfg(debug_assertions)]
            {
                self.timing = self.changed;
            }
            self.submitted = Some(q.clone());
            self.changed = None;
            request = Some(q);
        }
        if self.changed.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(40));
        }
        if let Some(error) = error {
            super::problem(ui, "Search unavailable", error);
            if ui.button("Retry search").clicked() {
                self.changed = Some(Instant::now());
            }
        }
        let mut chosen = None;
        egui::Panel::bottom("catalog-actions")
            .exact_size(52.0)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let selected =
                        matching.and_then(|p| self.selected.and_then(|n| p.entries.get(n)));
                    if super::primary(
                        ui,
                        &format!("Review {noun}"),
                        if !ready {
                            Some("Wait for the current operation")
                        } else if selected.is_none() {
                            Some("Select a result")
                        } else {
                            None
                        },
                    ) || (review_enter && selected.is_some() && ready)
                    {
                        chosen = selected
                            .cloned()
                            .map(|r| CatalogChoice::Review(Box::new(r)));
                    }
                    if let Some(page) = matching
                        && let Some(offset) = page.next
                        && ui
                            .add_enabled(ready, egui::Button::new("Next results"))
                            .clicked()
                    {
                        let q = CatalogQuery {
                            kind,
                            query: self.query.clone(),
                            offset,
                            snapshot: Some(page.snapshot.clone()),
                        };
                        self.submitted = Some(q.clone());
                        self.selected = None;
                        request = Some(q);
                    }
                    if ui.button("Enter manually").clicked() {
                        chosen = Some(CatalogChoice::Manual);
                    }
                });
            });
        if let Some(page) = matching {
            ui.weak(format!("{} results", page.total));
            egui::ScrollArea::vertical()
                .id_salt("catalog-results")
                .min_scrolled_height(0.0)
                .max_height(ui.available_height())
                .show(ui, |ui| {
                    for (i, row) in page.entries.iter().enumerate() {
                        ui.push_id((&row.origin.provider, &row.origin.id), |ui| {
                            let selected = self.selected == Some(i);
                            let label = match &row.item {
                                CatalogItem::Filament {
                                    name, declaration, ..
                                } => format!(
                                    "{} · {} · {}",
                                    declaration.brand.as_deref().unwrap_or("Unknown brand"),
                                    name,
                                    declaration
                                        .material_label
                                        .as_deref()
                                        .unwrap_or("Material unknown")
                                ),
                                CatalogItem::Printer {
                                    manufacturer,
                                    model,
                                    ..
                                } => {
                                    if model.starts_with(manufacturer) {
                                        model.clone()
                                    } else {
                                        format!("{manufacturer} {model}")
                                    }
                                }
                            };
                            ui.horizontal_wrapped(|ui| {
                                if let CatalogItem::Filament { srgb, .. } = &row.item {
                                    super::ColorPresentation::known(
                                        *srgb,
                                        super::ColorMeaning::DeclaredAppearance,
                                    )
                                    .show(
                                        ui,
                                        "catalog-color",
                                        super::ColorDensity::Compact,
                                    );
                                }
                                let response = ui.selectable_label(selected, label);
                                if selected && self.selected != previous_selection {
                                    response.scroll_to_me(Some(egui::Align::Center));
                                }
                                if response.clicked() {
                                    self.selected = Some(i);
                                }
                                ui.weak(&row.origin.provider);
                            });
                        });
                    }
                    if let Some(row) = self.selected.and_then(|n| page.entries.get(n)) {
                        ui.separator();
                        match &row.item {
                            CatalogItem::Filament { declaration, .. } => {
                                ui.weak(
                                    declaration.hueforge_td_mm.map_or("TD unknown".into(), |v| {
                                        format!("HueForge TD {v} mm · declared")
                                    }),
                                );
                            }
                            CatalogItem::Printer { volume_mm, .. } => {
                                ui.weak(volume_mm.map_or("Build size unknown".into(), |v| {
                                    format!("{} × {} × {} mm · declared", v[0], v[1], v[2])
                                }));
                            }
                        }
                        ui.collapsing("Source details", |ui| {
                            ui.label(&row.origin.url);
                            ui.label(format!("{} · {}", row.origin.id, row.origin.revision));
                        });
                    }
                });
        } else {
            ui.weak("Searching…");
        }
        #[cfg(debug_assertions)]
        if matching.is_some()
            && let Some(started) = self.timing.take()
            && std::env::var_os("SPILING_CATALOG_TIMINGS").is_some()
        {
            eprintln!(
                "catalog query-to-layout: {:.3} ms",
                started.elapsed().as_secs_f64() * 1000.0
            );
        }
        if chosen.is_some() {
            self.open = false;
        }
        (request, chosen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keyboard_reviews_only_the_current_selected_query() {
        let ctx = egui::Context::default();
        let mut search = CatalogSearch::default();
        search.open(CatalogKind::Filament);
        let item = CatalogItem::Filament {
            name: "Blue".into(),
            declaration: Default::default(),
            srgb: [0, 0, 255],
        };
        let snapshot = spiling_contracts::geometry::SourceHash::from_bytes(b"snapshot");
        let query = CatalogQuery {
            kind: CatalogKind::Filament,
            query: String::new(),
            offset: 0,
            snapshot: None,
        };
        let page = CatalogPage {
            snapshot: snapshot.clone(),
            query: query.clone(),
            entries: vec![CatalogRecord {
                origin: CatalogOrigin {
                    provider: "example".into(),
                    id: "blue".into(),
                    revision: "source1".into(),
                    snapshot,
                    content: item.content_hash().unwrap(),
                    url: "https://example.org".into(),
                },
                item,
            }],
            total: 1,
            next: None,
        };
        search.submitted = Some(query);
        search.changed = None;
        fn key(key: egui::Key) -> egui::Event {
            egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }
        }
        let frame = |search: &mut CatalogSearch, events| {
            let mut choice = None;
            ctx.run_ui(
                egui::RawInput {
                    events,
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(640.0, 480.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        choice = search.show(ui, Some(&page), None, true).1;
                    });
                },
            )
            .drop_without_applying_deltas();
            choice
        };
        frame(&mut search, vec![]);
        frame(&mut search, vec![]);
        assert!(frame(&mut search, vec![key(egui::Key::Enter)]).is_none());
        let mut released = key(egui::Key::Enter);
        if let egui::Event::Key { pressed, .. } = &mut released {
            *pressed = false;
        }
        frame(&mut search, vec![released]);
        ctx.memory_mut(|m| m.request_focus(egui::Id::unique("reference-search")));
        frame(&mut search, vec![]);
        frame(&mut search, vec![key(egui::Key::ArrowDown)]);
        assert_eq!(search.selected, Some(0));
        assert!(matches!(
            frame(&mut search, vec![key(egui::Key::Enter)]),
            Some(CatalogChoice::Review(_))
        ));
        search.open = true;
        search.selected = None;
        ctx.memory_mut(|m| m.request_focus(egui::Id::unique("reference-search")));
        assert!(
            matches!(
                frame(
                    &mut search,
                    vec![key(egui::Key::ArrowDown), key(egui::Key::Enter)]
                ),
                Some(CatalogChoice::Review(_))
            ),
            "fast navigation and review in one frame must retain keyboard focus"
        );
        search.open = true;
        search.query = "new query".into();
        assert!(
            frame(&mut search, vec![key(egui::Key::Enter)]).is_none(),
            "stale rows cannot be reviewed"
        );
    }
}
