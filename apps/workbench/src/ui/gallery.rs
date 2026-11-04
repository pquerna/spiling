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
        for (index, title) in [
            "Panels",
            "Fields",
            "Color",
            "Acquisition",
            "Matching",
            "Wall starts",
            "Try / Compare",
            "Build area",
            "Arrange",
            "Plates",
            "Navigation",
        ]
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
    if category == 6 {
        crate::app::alternative_gallery(ui, theme.tokens());
        return;
    }
    egui::ScrollArea::vertical()
        .id_salt(("gallery-content", category))
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width().min(650.0));
            match category {
                0 => panel(ui),
                1 => fields(ui, theme.tokens()),
                4 => matching(ui),
                5 => wall_starts(ui),
                7 => build_area(ui),
                8 => arrange(ui),
                9 => plates(ui),
                10 => navigation(ui),
                _ => colors(ui),
            }
        });
}
fn wall_starts(ui: &mut Ui) {
    use crate::replay::{
        View,
        seams::{SeamStart, SectionClip},
    };
    use spiling_contracts::manufacturing::SeamPolicy;
    pane_header(
        ui,
        "Wall starts",
        "Illustrative settings and read-only review · no engine request",
    );
    let id = ui.make_persistent_id("gallery-seam-policy");
    let mut policy = ui
        .data_mut(|data| data.get_temp::<SeamPolicy>(id))
        .unwrap_or(SeamPolicy::Distributed);
    seam_policy_fields(ui, &mut policy);
    ui.weak(format!("Draft · {}", seam_summary(&policy)));
    primary(
        ui,
        "Apply settings",
        Some("Illustrative draft · no project is open"),
    );
    ui.separator();
    let starts = [0, 1].map(|tool| SeamStart {
        position: [10.0 + f64::from(tool) * 5.0, 20.0, 0.4],
        physical_z: [0.2, 0.4],
        layer: 1,
        tool,
        role: 1,
        entry_event: tool * 6,
        first_event: tool * 6 + 1,
        closing_event: tool * 6 + 4,
        command_line: tool * 8 + 20,
        closing_line: tool * 8 + 23,
    });
    let view_id = ui.make_persistent_id("gallery-seam-view");
    let selected_id = ui.make_persistent_id("gallery-seam-selection");
    let mut view = ui
        .data_mut(|data| data.get_temp::<View>(view_id))
        .unwrap_or(View {
            seams: true,
            layer: Some(1),
            ..Default::default()
        });
    let mut selected = ui
        .data_mut(|data| data.get_temp::<Option<u32>>(selected_id))
        .flatten();
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(&mut view.tool, None, "All tools");
        ui.selectable_value(&mut view.tool, Some(0), "Tool 0");
        ui.selectable_value(&mut view.tool, Some(2), "Empty selection");
    });
    seam_review(
        ui,
        &starts,
        &mut selected,
        &mut view,
        SectionClip::default(),
        glam::DVec3::ZERO,
        &SeamPolicy::Distributed,
    );
    ui.data_mut(|data| {
        data.insert_temp(id, policy);
        data.insert_temp(view_id, view);
        data.insert_temp(selected_id, selected);
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

    #[test]
    fn plate_cards_keep_both_columns_inside_the_surface() {
        for theme in [Theme::Graphite, Theme::Paper] {
            for width in [240.0, 640.0, 900.0] {
                let ctx = egui::Context::default();
                theme.apply(&ctx);
                for _ in 0..3 {
                    let mut measured = (0.0, 0.0);
                    ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 1600.0),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            let right = ui.max_rect().right();
                            plates(ui);
                            measured = (ui.min_rect().right(), right);
                        },
                    )
                    .drop_without_applying_deltas();
                    assert!(
                        measured.0 <= measured.1 + 1.0,
                        "{theme:?}/{width}: {measured:?}"
                    );
                }
            }
        }
    }
    #[test]
    fn review_and_build_area_controls_reflow_in_both_themes_and_scaled_panes() {
        for theme in [Theme::Graphite, Theme::Paper] {
            for width in [240.0, 320.0, 400.0] {
                for scale in [1.0, 1.5, 2.0] {
                    let ctx = egui::Context::default();
                    theme.apply(&ctx);
                    ctx.set_zoom_factor(scale);
                    for (name, specimen) in [
                        ("wall starts", wall_starts as fn(&mut Ui)),
                        ("build area", build_area),
                        ("arrange", arrange),
                    ] {
                        for _ in 0..2 {
                            let mut measured = (0.0, 0.0);
                            ctx.run_ui(
                                egui::RawInput {
                                    screen_rect: Some(egui::Rect::from_min_size(
                                        egui::Pos2::ZERO,
                                        egui::vec2(width * scale, 1600.0 * scale),
                                    )),
                                    ..Default::default()
                                },
                                |ui| {
                                    let right = ui.max_rect().right();
                                    specimen(ui);
                                    measured = (ui.min_rect().right(), right);
                                },
                            )
                            .drop_without_applying_deltas();
                            assert!(
                                measured.0 <= measured.1 + 1.0,
                                "{name}: {theme:?} / {width} points / {scale} scale: {measured:?}"
                            );
                        }
                    }
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

/// Frozen illustrative values exercise shared presentation, never the evaluator.
fn matching(ui: &mut Ui) {
    use spiling_contracts::{
        appearance::*, geometry::SourceHash, library::*, optical::*, stock::*,
    };
    let target_id = ui.make_persistent_id("matching-target");
    let mut hex = ui.data_mut(|d| d.get_temp::<String>(target_id).unwrap_or("#667FCC".into()));
    stock_target(ui, &mut hex);
    ui.data_mut(|d| d.insert_temp(target_id, hex));
    let query = StockQuery {
        scope: StockScope::Project,
        on_hand_only: false,
        target: OpticalTarget {
            color: AppearanceColor::Srgb {
                channels: [0.4, 0.5, 0.8],
            },
            delta_e00_tolerance: 2.0,
            lightness_tolerance: None,
        },
        patch: OpticalPatch {
            origin_mm: [0.0; 3],
            normal_xy: [0.0, -1.0],
            width_mm: 10.0,
            height_mm: 10.0,
        },
        slot_height_mm: 0.2,
        max_evaluations: 65536,
    };
    let candidates = [
        ("Ocean blue", Some([0.1, 0.3, 0.9])),
        ("White", Some([1.0; 3])),
        ("Unmeasured PLA", None),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (name, rgb))| StockCandidate {
        filament: FilamentRef {
            id: FilamentId(format!("00000000-0000-4000-a000-{i:012}")),
            revision: SourceHash::from_bytes(name.as_bytes()),
        },
        name: name.into(),
        appearance: rgb.map(|channels| DeclaredAppearance {
            color: AppearanceColor::Srgb { channels },
            source: DefinitionSource::Operator,
        }),
        hueforge_td_mm: None,
        in_project: true,
        in_library: false,
        on_hand_spools: 0,
        profile_count: 0,
    })
    .collect::<Vec<_>>();
    let cycle = LayerCycle {
        slots: vec![
            OpticalSlot {
                material: 0,
                height_mm: 0.2,
            },
            OpticalSlot {
                material: 1,
                height_mm: 0.2,
            },
        ],
        phase_slot: 0,
        origin_z_mm: 0.0,
    };
    let report = StockMatchReport {
        producer: STOCK_PRODUCER.into(),
        input_hash: StockMatchReport::input_hash(&query, &None, &candidates).unwrap(),
        query,
        library: None,
        candidates,
        singles: vec![
            StockSingle {
                candidate: 0,
                delta_e00: 12.4,
                within_tolerance: false,
            },
            StockSingle {
                candidate: 1,
                delta_e00: 35.8,
                within_tolerance: false,
            },
        ],
        pairs: vec![StockPair {
            candidates: [0, 1],
            cycle,
            assessment: OpticalAssessment::Estimate {
                exposure: CycleExposure {
                    period_mm: 0.4,
                    phase_at_patch_mm: 0.0,
                    complete_periods: 25,
                    exposed_height_mm: [5.0, 5.0, 0.0],
                    fractions: [0.5, 0.5, 0.0],
                },
                predicted_lab: [55.0, 10.0, -30.0],
                display_srgb8: [109, 132, 202],
                delta_e00: 3.2,
                delta_lightness: 1.0,
                within_tolerance: false,
            },
        }],
        unassessed_singles: 1,
        unassessed_pairs: 2,
        total_pairs: 3,
        visited_pairs: 3,
        evaluations: 96,
        status: StockSearchStatus::Complete,
    };
    let selection_id = ui.make_persistent_id("matching-selection");
    let mut selected = ui.data_mut(|d| d.get_temp::<Option<StockChoice>>(selection_id).flatten());
    stock_results(ui, &report, &mut selected);
    ui.data_mut(|d| d.insert_temp(selection_id, selected));
    ui.separator();
    primary(
        ui,
        "Keep recipe",
        Some("Illustrative reference estimates · no project changes"),
    );
}

fn build_area(ui: &mut Ui) {
    use crate::viewport::plate::Plate;
    use spiling_contracts::hardware::{ProjectHardwareCapture, builtin_catalog};
    use std::sync::Arc;
    pane_header(
        ui,
        "Build area",
        "Illustrative captured hardware · no project changes",
    );
    let catalog = builtin_catalog().expect("validated built-in hardware");
    let plate = Plate::Hardware(Arc::new(ProjectHardwareCapture {
        setup: catalog.setups[0].clone(),
        plate: catalog.plates[0].clone(),
        face: "textured".into(),
        accessories: vec![],
    }));
    let id = ui.make_persistent_id("gallery-height-limits");
    let mut height = ui.data_mut(|data| data.get_temp::<bool>(id).unwrap_or(false));
    let _ = plate.controls(ui, &mut height);
    ui.data_mut(|data| data.insert_temp(id, height));
    ui.weak("Fit plate frames the canvas in the workbench. This specimen has no 3D scene.");
}

fn arrange(ui: &mut Ui) {
    use spiling_contracts::geometry::*;
    pane_header(
        ui,
        "Arrange",
        "Illustrative current plate · no project changes",
    );
    let id = ui.make_persistent_id("gallery-layout-request");
    let mut request = ui
        .data_mut(|d| d.get_temp::<LayoutRequest>(id))
        .unwrap_or(LayoutRequest {
            objective: LayoutObjective::Copies,
            template: Some(OccurrenceId::new(1).unwrap()),
            quantity: 2,
            use_additional_plates: false,
            pinned: vec![],
            spacing_mm: 3.0,
            style: LayoutStyle::Balanced,
        });
    primary(
        ui,
        "Find layout",
        Some("Illustrative controls · no engine request"),
    );
    super::layout_fields(
        ui,
        &mut request,
        &[
            (OccurrenceId::new(1).unwrap(), "Housing · 1".into()),
            (
                OccurrenceId::new(2).unwrap(),
                "Long named bracket — 固定部品 · 2".into(),
            ),
        ],
    );
    ui.data_mut(|d| d.insert_temp(id, request));
}

fn plates(ui: &mut Ui) {
    use spiling_contracts::project::{PrintLayoutId, PrintLayoutInfo};
    pane_header(ui, "Plates", "Independent print jobs · illustrative values");
    let ids = [
        "11111111-1111-4111-8111-111111111111",
        "22222222-2222-4222-8222-222222222222",
        "33333333-3333-4333-8333-333333333333",
        "44444444-4444-4444-8444-444444444444",
    ]
    .map(|id| PrintLayoutId::parse(id).unwrap());
    let rows = [
        ("Housing", 2, true, true),
        ("Spare parts — 接口", 12, true, false),
        ("New plate", 0, false, false),
        (
            "Long production plate name with several parts",
            3,
            true,
            true,
        ),
    ]
    .into_iter()
    .zip(&ids)
    .map(
        |((name, count, configured, has_result), id)| PrintLayoutInfo {
            layout_id: id.clone(),
            name: name.into(),
            occurrence_count: count,
            configured,
            has_result,
        },
    )
    .collect::<Vec<_>>();
    let key = ui.make_persistent_id("gallery-plate");
    let mut selected = ui.data_mut(|d| {
        d.get_temp::<PrintLayoutId>(key)
            .unwrap_or_else(|| ids[0].clone())
    });
    if let Some(PlateEvent::Focus(id)) = plate_navigator(ui, &rows, &selected, true) {
        selected = id;
    }
    let mut next_part = 1;
    let overview = spiling_contracts::geometry::PrintLayoutOverview {
        session_id: spiling_contracts::geometry::SessionId::parse(
            "550e8400-e29b-41d4-a716-446655440000",
        )
        .unwrap(),
        scene_revision: spiling_contracts::geometry::SceneRevision(1),
        project_id: spiling_contracts::project::ProjectId::parse(
            "550e8400-e29b-41d4-a716-446655440001",
        )
        .unwrap(),
        project_revision: spiling_contracts::project::ProjectRevision(1),
        definitions: vec![spiling_contracts::geometry::DefinitionRecord {
            definition_id: spiling_contracts::geometry::DefinitionId::from_source_sha256(&[1; 32]),
            provenance: spiling_contracts::geometry::SourceProvenance {
                source_hash: spiling_contracts::geometry::SourceHash::parse("01".repeat(32))
                    .unwrap(),
                source_name: "Gallery specimen".into(),
                source_unit: spiling_contracts::geometry::SourceUnit::Millimetre,
                uncertainty_mm: None,
            },
            face_count: 6,
            bounds_mm: spiling_contracts::geometry::AabbMm {
                min: [0.0; 3],
                max: [30.0, 30.0, 20.0],
            },
            display: spiling_contracts::geometry::DefinitionDisplay::Mesh {
                artifact_id: spiling_contracts::geometry::ArtifactId::new(1).unwrap(),
            },
        }],
        layouts: rows
            .iter()
            .map(|row| spiling_contracts::geometry::LayoutOverview {
                layout_id: row.layout_id.clone(),
                captured_plate: false,
                build_area_mm: row
                    .configured
                    .then_some(spiling_contracts::geometry::AabbMm {
                        min: [0.0; 3],
                        max: [220.0, 220.0, 0.0],
                    }),
                parts: (0..row.occurrence_count)
                    .map(|i| {
                        let translation_mm = [
                            30.0 + f64::from(i % 4) * 40.0,
                            30.0 + f64::from(i / 4) * 40.0,
                            0.0,
                        ];
                        let part = spiling_contracts::geometry::LayoutPartBounds {
                            definition_id:
                                spiling_contracts::geometry::DefinitionId::from_source_sha256(
                                    &[1; 32],
                                ),
                            pose: spiling_contracts::geometry::RigidPoseMm {
                                translation_mm,
                                ..spiling_contracts::geometry::RigidPoseMm::IDENTITY
                            },
                            occurrence_id: spiling_contracts::geometry::OccurrenceId::new(
                                next_part,
                            )
                            .unwrap(),
                            source_bounds_mm: spiling_contracts::geometry::AabbMm {
                                min: translation_mm,
                                max: [translation_mm[0] + 30.0, translation_mm[1] + 30.0, 20.0],
                            },
                        };
                        next_part += 1;
                        part
                    })
                    .collect(),
            })
            .collect(),
    };
    let tokens = if ui.visuals().dark_mode {
        Theme::Graphite
    } else {
        Theme::Paper
    }
    .tokens();
    if let Some(PlateEvent::Focus(id)) = plate_overview(
        ui,
        &rows,
        &overview,
        &selected,
        true,
        tokens,
        |ui, rect, _, _| {
            ui.painter_at(rect).text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Source preview in project",
                egui::FontId::proportional(12.0),
                tokens.muted,
            );
        },
    ) {
        selected = id;
    }
    ui.data_mut(|d| d.insert_temp(key, selected.clone()));
    ui.separator();
    ui.label(format!(
        "Current: {}",
        rows.iter().find(|p| p.layout_id == selected).unwrap().name
    ));
    help(
        ui,
        "plate-gallery-scope",
        "Print jobs",
        "Each plate owns its setup, parts and prepared output. Physical sheets remain My Hardware inventory. Switching never moves parts or applies a draft.",
    );
}

fn navigation(ui: &mut Ui) {
    use crate::commands::{Action, Workspace};
    let id = ui.make_persistent_id("gallery-workspace");
    let mut workspace = ui.data_mut(|d| d.get_temp::<Workspace>(id).unwrap_or_default());
    let task_id = id.with("task");
    let mut task = ui.data_mut(|d| d.get_temp::<Action>(task_id));
    ui.horizontal(|ui| {
        if let Some(action) = workspace_tabs(ui, workspace) {
            workspace = if action == Action::PrepareWorkspace {
                Workspace::Prepare
            } else {
                Workspace::Review
            };
            task = None;
        }
    });
    if let Some(action) = workspace_commands(ui, workspace, task, |_| None) {
        task = Some(action);
    }
    ui.separator();
    if let Some(task) = task {
        ui.label(task.label());
    }
    ui.weak("Illustrative navigation · no project changes");
    ui.data_mut(|d| {
        d.insert_temp(id, workspace);
        if let Some(task) = task {
            d.insert_temp(task_id, task);
        } else {
            d.remove::<Action>(task_id);
        }
    });
}
