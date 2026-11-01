// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Transient catalog interaction. The engine owns compatibility and resolved values.
use super::*;
use spiling_contracts::project::ProjectInfo;

#[derive(Default)]
pub(super) struct CatalogUi {
    nonce: Option<u64>,
    show: bool,
    query: String,
    preview: Option<(String, String)>,
}

impl Workbench {
    pub(super) fn catalog_ui(
        &mut self,
        ui: &mut egui::Ui,
        project: &ProjectInfo,
        session: &SessionId,
        ready: bool,
    ) {
        let m = self.state.manufacturing.clone();
        let loaded = m
            .loaded_catalog
            .as_ref()
            .filter(|c| c.project == project.project_id && c.session == *session);
        if self.material.catalog.nonce != loaded.map(|c| c.nonce) {
            self.material.catalog = CatalogUi {
                nonce: loaded.map(|c| c.nonce),
                show: true,
                ..Default::default()
            };
        }
        if let Some(loaded) = loaded {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("Catalog: {}", loaded.label));
                if ui
                    .button(if self.material.catalog.show {
                        "Hide catalog"
                    } else {
                        "Choose another material"
                    })
                    .clicked()
                {
                    self.material.catalog.show = !self.material.catalog.show;
                }
            });
        }
        let mut request = None;
        if let Some(loaded) = loaded.filter(|_| self.material.catalog.show) {
            ui.weak(format!(
                "{} declarations · not calibrated",
                loaded.catalog.profiles.len()
            ));
            let label = ui.label("Search material catalog");
            let response = ui
                .add(
                    egui::TextEdit::singleline(&mut self.material.catalog.query)
                        .hint_text("Manufacturer, product, polymer or profile")
                        .desired_width(f32::INFINITY),
                )
                .labelled_by(label.id);
            let search = &self.material.catalog.query;
            let valid =
                search.len() <= MAX_MATERIAL_SEARCH_BYTES && !search.chars().any(char::is_control);
            let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui
                .add_enabled(ready && valid, egui::Button::new("Search catalog"))
                .clicked()
                || (ready && valid && enter)
            {
                request = Some(MaterialReviewRequest {
                    catalog: Some((*loaded.catalog).clone()),
                    query: search.clone(),
                    offset: 0,
                });
                self.material.catalog.preview = None;
            }
            if !valid {
                ui.colored_label(
                    self.prefs.theme.tokens().warning,
                    "Search accepts up to 128 UTF-8 bytes and no control characters.",
                );
            }
        }
        let review = m
            .material_review
            .as_ref()
            .filter(|r| r.current(project, session));
        let catalog_review = review.filter(|r| {
            loaded.is_some_and(|c| r.request.catalog.as_ref() == Some(c.catalog.as_ref()))
                && r.request.query == self.material.catalog.query
        });
        if let Some(review) = catalog_review.filter(|_| self.material.catalog.show) {
            let report = &review.report;
            let first = if report.candidates.is_empty() {
                0
            } else {
                report.offset + 1
            };
            ui.weak(format!(
                "Profiles {}-{} of {} matches / {} declarations · revision {}",
                first,
                report.offset + report.candidates.len() as u32,
                report.matching_profiles,
                report.catalog_profiles,
                report.input_revision.0
            ));
            if report.candidates.is_empty() {
                ui.label("No profiles match this search.");
            }
            for candidate in &report.candidates {
                let p = &candidate.selection.profile;
                let identity = (p.id.clone(), p.revision.clone());
                let selected = self.material.catalog.preview.as_ref() == Some(&identity);
                ui.push_id((&p.id, &p.revision), |ui| {
                    let name = format!("{} · {}", p.material.manufacturer, p.material.product);
                    if ui.selectable_label(selected, name).clicked() {
                        self.material.catalog.preview = Some(identity);
                    }
                    ui.weak(format!("{} / {} · {}", p.id, p.revision,
                        if candidate.issues.is_empty() { "Compatible declaration" } else { "Incompatible" }));
                    if selected {
                        if candidate.issues.is_empty() {
                            ui.label(if candidate.input_changed == Some(false) {
                                "Already captured; no input change."
                            } else { "Applying changes the manufacturing input and invalidates its review." });
                            settings(ui, &candidate.settings, false);
                            if self.material.draft.as_ref().is_some_and(|d| d.value != d.original) {
                                ui.weak("Using this profile replaces the current material draft, including overrides.");
                            }
                            if ui.add_enabled(ready, egui::Button::new("Use this profile")).clicked() {
                                self.material.ignored_nonce = m.loaded_material.as_ref().map(|l| l.nonce);
                                self.material.catalog.show = false;
                                self.material.draft = Some(MaterialDraft {
                                    revision: report.input_revision, nonce: 0,
                                    original: candidate.selection.clone(), value: candidate.selection.clone(),
                                });
                            }
                        } else {
                            for issue in &candidate.issues {
                                ui.colored_label(self.prefs.theme.tokens().warning, &issue.message);
                            }
                            ui.weak("This declaration cannot be applied unchanged.");
                        }
                    }
                });
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        ready && report.offset > 0,
                        egui::Button::new("Previous profiles"),
                    )
                    .clicked()
                {
                    request = Some(MaterialReviewRequest {
                        offset: report
                            .offset
                            .saturating_sub(MATERIAL_REVIEW_PAGE_SIZE as u32),
                        ..(*review.request).clone()
                    });
                }
                if ui
                    .add_enabled(
                        ready
                            && report.offset + (report.candidates.len() as u32)
                                < report.matching_profiles,
                        egui::Button::new("Next profiles"),
                    )
                    .clicked()
                {
                    request = Some(MaterialReviewRequest {
                        offset: report.offset + MATERIAL_REVIEW_PAGE_SIZE as u32,
                        ..(*review.request).clone()
                    });
                }
            });
        } else if loaded.is_some() && self.material.catalog.show {
            ui.weak("Search to review this catalog against the current project. Edits invalidate prior comparisons.");
        }
        if m.intent
            .as_ref()
            .is_some_and(|i| i.material_profile.is_some())
        {
            if ui
                .add_enabled(ready, egui::Button::new("Review captured settings"))
                .clicked()
            {
                request = Some(MaterialReviewRequest {
                    catalog: None,
                    query: String::new(),
                    offset: 0,
                });
            }
            if let Some(review) = review.filter(|r| r.request.catalog.is_none()) {
                ui.collapsing("Current effective settings", |ui| {
                    for candidate in &review.report.candidates {
                        settings(ui, &candidate.settings, true);
                    }
                });
            }
        }
        if let Some(request) = request {
            self.queued_manufacturing = Some(ManufacturingCommand::ReviewMaterials {
                session_id: session.clone(),
                base_revision: project.revision,
                request,
            });
        }
        ui.separator();
    }
}

fn settings(ui: &mut egui::Ui, rows: &[MaterialSettingReview], captured: bool) {
    ui.weak(if captured {
        "Captured effective values, including explicit overrides."
    } else {
        "Current input compared with profile defaults. Draft overrides below are applied separately."
    });
    egui::Grid::new("material-settings-summary")
        .num_columns(if captured { 2 } else { 3 })
        .min_col_width(0.0)
        .max_col_width(100.0)
        .spacing([6.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            ui.weak("Setting");
            if !captured {
                ui.weak("Current");
            }
            ui.weak(if captured { "Effective" } else { "Proposed" });
            ui.end_row();
            for row in rows.iter().filter(|r| {
                matches!(
                    r.field,
                    MaterialSettingField::NozzleTemperatureC
                        | MaterialSettingField::BedTemperatureC
                        | MaterialSettingField::PrintSpeedMmS
                        | MaterialSettingField::FlowMultiplier
                        | MaterialSettingField::MaxVolumetricFlowMm3S
                )
            }) {
                ui.add(egui::Label::new(setting_label(row.field)).wrap());
                if !captured {
                    ui.add(egui::Label::new(value(row.current)).extend());
                }
                ui.add(egui::Label::new(value(row.proposed)).extend());
                ui.end_row();
            }
        });
    ui.collapsing("All process settings and origins", |ui| {
        for row in rows {
            ui.label(egui::RichText::new(setting_label(row.field)).strong());
            if !captured {
                ui.label(format!(
                    "Current: {} ({})",
                    value(row.current),
                    origin(row.current_origin)
                ));
            }
            ui.label(format!(
                "{}: {} ({})",
                if captured { "Effective" } else { "Proposed" },
                value(row.proposed),
                origin(row.proposed_origin)
            ));
            ui.add_space(4.0);
        }
    });
}
fn setting_label(field: MaterialSettingField) -> &'static str {
    use MaterialSettingField::*;
    match field {
        NozzleTemperatureC => "Nozzle (°C)",
        BedTemperatureC => "Bed (°C)",
        PrintSpeedMmS => "Speed (mm/s)",
        FilamentDiameterMm => "Filament (mm)",
        FlowMultiplier => "Flow (×)",
        MaxVolumetricFlowMm3S => "Flow ceiling (mm³/s)",
        FirstLayerSpeedFactor => "First speed (×)",
        FirstLayerFlowFactor => "First flow (×)",
        FanFirstLayers => "First fan (PWM)",
        FanAfterFirstLayers => "Later fan (PWM)",
        BridgeSpeedFactor => "Bridge speed (×)",
        BridgeFlowFactor => "Bridge flow (×)",
        BridgeFanPwm => "Bridge fan (PWM)",
        MaxBridgeSpanMm => "Bridge span (mm)",
        RetractionDistanceMm => "Retract (mm)",
        RetractionSpeedMmS => "Retract (mm/s)",
        RetractionMinimumTravelMm => "Retract travel (mm)",
        RetractionZHopMm => "Z hop (mm)",
        MinBeadWidthMm => "Min width (mm)",
        MaxBeadWidthMm => "Max width (mm)",
        MinBeadHeightMm => "Min height (mm)",
        MaxBeadHeightMm => "Max height (mm)",
    }
}

fn value(v: Option<f64>) -> String {
    v.map_or_else(
        || "off".into(),
        |v| {
            if v != 0.0 && v.abs() < 0.001 {
                format!("{v:.3e}")
            } else {
                format!("{v:.3}")
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_owned()
            }
        },
    )
}
fn origin(v: MaterialValueOrigin) -> &'static str {
    match v {
        MaterialValueOrigin::CurrentIntent => "captured intent",
        MaterialValueOrigin::SelectedProfile => "selected profile",
        MaterialValueOrigin::ExplicitOverride => "explicit override",
        MaterialValueOrigin::MachineLimit => "machine limit",
        MaterialValueOrigin::MaterialLimit => "material limit",
        MaterialValueOrigin::SharedLimit => "shared machine/material limit",
    }
}
