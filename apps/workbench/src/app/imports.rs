// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::{inspection::Area, scene::Scene};
use std::sync::{Arc, Weak};

pub(super) struct ImportSelection {
    scene: Weak<Scene>,
    occurrence: OccurrenceId,
    finding: usize,
    area: Option<Arc<Area>>,
}
impl Workbench {
    pub(super) fn import_overlay(&mut self) -> Option<Arc<Area>> {
        let selection = self.import_selection.as_ref()?;
        if self.display_current()
            && !self.state.busy
            && self
                .state
                .display
                .as_ref()
                .is_some_and(|s| Weak::ptr_eq(&selection.scene, &Arc::downgrade(s)))
        {
            selection.area.clone()
        } else {
            self.import_selection = None;
            None
        }
    }
    pub(super) fn import_details_ui(
        &mut self,
        ui: &mut egui::Ui,
        scene: &Arc<Scene>,
        occurrence: &OccurrenceRecord,
        source: &NormalizedSource,
    ) {
        let selected = self
            .import_selection
            .as_ref()
            .filter(|s| {
                s.occurrence == occurrence.occurrence_id
                    && Weak::ptr_eq(&s.scene, &Arc::downgrade(scene))
            })
            .map(|s| s.finding);
        if let Some(definition) = scene.definition(&occurrence.definition_id) {
            let bounds = definition.record.bounds_mm;
            let format = match source.format {
                MeshSourceFormat::Stl => "STL",
                MeshSourceFormat::ThreeMf => "3MF",
            };
            ui.weak(format!(
                "{format} · {:.2} × {:.2} × {:.2} mm",
                bounds.max[0] - bounds.min[0],
                bounds.max[1] - bounds.min[1],
                bounds.max[2] - bounds.min[2]
            ));
        }
        let action = ui
            .push_id(("source_report", occurrence.occurrence_id), |ui| {
                report(ui, source, selected, self.prefs.theme.tokens().warning)
            })
            .inner;
        match action {
            Some(ReportAction::Select(index)) => {
                self.viewport.cancel_area();
                let finding = &source.findings[index];
                let area = finding
                    .bounds_mm
                    .map(|bounds| {
                        Area::import_location(bounds, occurrence.pose, scene.summary.bounds_mm)
                            .map(Arc::new)
                    })
                    .transpose();
                match area {
                    Ok(area) => {
                        self.import_selection = Some(ImportSelection {
                            scene: Arc::downgrade(scene),
                            occurrence: occurrence.occurrence_id,
                            finding: index,
                            area,
                        })
                    }
                    Err(e) => {
                        self.import_selection = None;
                        self.local_problem = Some(e);
                    }
                }
            }
            Some(ReportAction::Locate) => {
                if let Some(selected) = &self.import_selection {
                    let target = selected.area.as_ref().map(|a| a.target).or_else(|| {
                        scene
                            .focus_target(None, Some(occurrence.occurrence_id))
                            .ok()
                    });
                    if let Some(target) = target {
                        self.viewport
                            .focus_area(scene, target, &[occurrence.occurrence_id]);
                    }
                }
            }
            Some(ReportAction::Clear) => {
                self.import_selection = None;
                self.viewport.cancel_area();
            }
            None => {}
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReportAction {
    Select(usize),
    Locate,
    Clear,
}
/// UI emits explicit intents. Selecting a row never moves the camera or writes
/// process settings; Locate is separate from selection.
fn report(
    ui: &mut egui::Ui,
    source: &NormalizedSource,
    selected: Option<usize>,
    warning: egui::Color32,
) -> Option<ReportAction> {
    let changes = source
        .findings
        .iter()
        .filter(|f| f.kind == ImportFindingKind::Changed)
        .count();
    let warnings = source
        .findings
        .iter()
        .filter(|f| f.kind == ImportFindingKind::NotApplied)
        .count();
    if source.findings.is_empty() {
        return None;
    }
    let mut action = None;
    let title = match (changes, warnings) {
        (0, 0) => "Import details".into(),
        (0, w) => format!("Import · {w} not applied"),
        (c, 0) => format!("Import · {c} changes"),
        (c, w) => format!("Import · {c} changes · {w} not applied"),
    };
    egui::CollapsingHeader::new(title)
        .default_open(warnings > 0 || selected.is_some())
        .show(ui, |ui| {
            for (kind, title) in [
                (ImportFindingKind::Changed, "Changed"),
                (ImportFindingKind::NotApplied, "Not applied"),
                (ImportFindingKind::Information, "Source"),
            ] {
                if !source.findings.iter().any(|f| f.kind == kind) {
                    continue;
                }
                ui.strong(title);
                for (index, finding) in source
                    .findings
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| f.kind == kind)
                {
                    ui.push_id(index, |ui| {
                        let active = selected == Some(index);
                        let text =
                            if finding.count > 1 || finding.kind == ImportFindingKind::Changed {
                                format!("{} · {}", finding.label, finding.count)
                            } else {
                                finding.label.clone()
                            };
                        if ui
                            .selectable_label(
                                active,
                                egui::RichText::new(text).color(
                                    if kind == ImportFindingKind::NotApplied {
                                        warning
                                    } else {
                                        ui.visuals().text_color()
                                    },
                                ),
                            )
                            .on_hover_text(&finding.detail)
                            .clicked()
                        {
                            action = Some(ReportAction::Select(index));
                        }
                        if active {
                            ui.weak(&finding.detail);
                            if let Some(d) = finding.displacement_mm {
                                ui.label(format!("Max displacement · {d:.6} mm"));
                            }
                            ui.horizontal(|ui| {
                                if ui
                                    .button(if finding.bounds_mm.is_some() {
                                        "Locate"
                                    } else {
                                        "Locate part"
                                    })
                                    .clicked()
                                {
                                    action = Some(ReportAction::Locate);
                                }
                                if ui.button("Clear").clicked() {
                                    action = Some(ReportAction::Clear);
                                }
                            });
                            ui.weak(if finding.bounds_mm.is_some() {
                                "Affected bounds · display only"
                            } else {
                                "Source object · no geometric location"
                            });
                        }
                    });
                }
            }
        });
    action
}

#[cfg(test)]
mod import_tests {
    use super::*;
    fn text(shape: &egui::Shape, out: &mut String) {
        match shape {
            egui::Shape::Text(t) => {
                out.push_str(t.galley.text());
                out.push('\n');
            }
            egui::Shape::Vec(v) => {
                for s in v {
                    text(s, out)
                }
            }
            _ => {}
        }
    }
    #[test]
    fn findings_disclose_details_only_on_selection_and_distinguish_unlocated_controls() {
        let source = NormalizedSource {
            hash: SourceHash::from_bytes(b"source"),
            format: MeshSourceFormat::ThreeMf,
            selector: "object:1".into(),
            object_name: "part".into(),
            findings: vec![
                ImportFinding {
                    kind: ImportFindingKind::NotApplied,
                    code: ImportFindingCode::ForeignSettings,
                    count: 1,
                    label: "Process settings".into(),
                    detail: "Foreign profile retained but not applied".into(),
                    bounds_mm: None,
                    displacement_mm: None,
                },
                ImportFinding {
                    kind: ImportFindingKind::Changed,
                    code: ImportFindingCode::BoundaryWeld,
                    count: 1,
                    label: "Boundary joins".into(),
                    detail: "Paired source-boundary vertices".into(),
                    bounds_mm: Some(AabbMm::new([0.; 3], [1.; 3]).unwrap()),
                    displacement_mm: Some(0.00001),
                },
            ],
        };
        for selected in [None, Some(0), Some(1)] {
            let ctx = egui::Context::default();
            let mut content = String::new();
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600., 600.),
                    )),
                    ..Default::default()
                },
                |ui| {
                    assert_eq!(report(ui, &source, selected, egui::Color32::YELLOW), None);
                },
            );
            for s in &out.shapes {
                text(&s.shape, &mut content);
            }
            out.drop_without_applying_deltas();
            assert!(content.contains("Not applied"));
            assert_eq!(
                content.contains("Foreign profile retained"),
                selected == Some(0)
            );
            assert_eq!(content.contains("Affected bounds"), selected == Some(1));
            assert_eq!(content.contains("Locate part"), selected == Some(0));
        }
    }
}
