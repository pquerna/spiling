// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::*;
use spiling_contracts::project::ProjectId;
use std::sync::Arc;

pub(super) struct LayoutUi {
    pub open: bool,
    pub request: LayoutRequest,
    project: Option<ProjectId>,
    focus: Option<egui::Id>,
    preview: Option<(
        SourceHash,
        std::sync::Weak<crate::scene::Scene>,
        Arc<crate::scene::Scene>,
    )>,
}
impl Default for LayoutUi {
    fn default() -> Self {
        Self {
            open: false,
            request: LayoutRequest {
                objective: LayoutObjective::Copies,
                template: None,
                quantity: 2,
                use_additional_plates: false,
                pinned: vec![],
                spacing_mm: 3.0,
                style: LayoutStyle::Balanced,
            },
            project: None,
            focus: None,
            preview: None,
        }
    }
}
impl Workbench {
    pub(super) fn open_layout(&mut self, ctx: &egui::Context) {
        self.close_analysis(ctx);
        self.close_library(ctx);
        if !self.layout.open {
            self.layout.focus = ctx.memory(|m| m.focused());
        }
        self.layout.open = true;
    }
    pub(super) fn close_layout(&mut self, ctx: &egui::Context) {
        self.layout.open = false;
        self.layout.preview = None;
        if let Some(id) = self.layout.focus.take() {
            ctx.memory_mut(|m| m.request_focus(id));
        }
    }
    fn layout_proposal(&self) -> Option<Arc<LayoutProposal>> {
        let p = self.state.layout_proposal.as_ref()?;
        let project = self.state.project.as_ref()?;
        let scene = self.state.scene.as_ref()?;
        (self.layout.open
            && p.project_id == project.project_id
            && p.project_revision == project.revision
            && p.session_id == scene.session_id
            && p.scene_revision == scene.revision
            && p.request == self.layout.request)
            .then(|| p.clone())
    }
    pub(super) fn layout_preview(&mut self) -> Option<Arc<crate::scene::Scene>> {
        let p = self.layout_proposal()?;
        let source = self.state.display.as_ref()?;
        if let Some((id, weak, preview)) = &self.layout.preview
            && id == &p.id
            && weak.upgrade().is_some_and(|s| Arc::ptr_eq(&s, source))
        {
            return Some(preview.clone());
        }
        let mut occurrences = source.occurrences.clone();
        for (index, placement) in p.placements.iter().enumerate() {
            if placement.copy {
                let mut row = source
                    .occurrences
                    .iter()
                    .find(|o| o.occurrence_id == placement.source)?
                    .clone();
                // Render-only identifiers. Preview cannot pick, inspect or commit references.
                row.occurrence_id = OccurrenceId::new(u32::MAX - index as u32).ok()?;
                row.pose = placement.pose;
                occurrences.push(row);
            } else {
                occurrences
                    .iter_mut()
                    .find(|o| o.occurrence_id == placement.source)?
                    .pose = placement.pose;
            }
        }
        let mut summary = source.summary.clone();
        let mut bounds = *p.footprints_mm.first()?;
        for b in &p.footprints_mm {
            bounds.include(b.min).ok()?;
            bounds.include(b.max).ok()?;
        }
        summary.bounds_mm = Some(bounds);
        summary.occurrence_count = occurrences.len() as u32;
        let preview = Arc::new(crate::scene::Scene {
            kind: crate::scene::SceneKind::Source,
            summary,
            definitions: source.definitions.clone(),
            occurrences,
            raw_bytes: source.raw_bytes,
            context: source.context.clone(),
            context_definitions: source.context_definitions.clone(),
            context_error: source.context_error.clone(),
        });
        self.layout.preview = Some((p.id.clone(), Arc::downgrade(source), preview.clone()));
        Some(preview)
    }
    pub(super) fn layout_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let name = self
            .state
            .project
            .as_ref()
            .and_then(|p| p.layouts.iter().find(|l| l.layout_id == p.layout_id))
            .map_or("Current plate", |l| l.name.as_str());
        let scope = if self.layout_proposal().is_some() {
            format!("{name} · proposed layout")
        } else {
            name.to_owned()
        };
        ui::pane_header(ui, "Arrange", &scope);
        if ui::close(ui, "Close") {
            self.close_layout(&ctx);
            return;
        }
        let Some(project) = self.state.project.clone() else {
            return;
        };
        if self.layout.project.as_ref() != Some(&project.project_id) {
            let open = self.layout.open;
            let focus = self.layout.focus;
            self.layout = LayoutUi {
                open,
                focus,
                project: Some(project.project_id.clone()),
                ..Default::default()
            };
        }
        let parts: Vec<_> = self
            .state
            .display
            .as_ref()
            .map(|scene| {
                scene
                    .occurrences
                    .iter()
                    .map(|o| {
                        let name = scene
                            .definition(&o.definition_id)
                            .map_or("Part", |d| d.record.provenance.source_name.as_str());
                        (
                            o.occurrence_id,
                            format!("{name} · {}", o.occurrence_id.get()),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        if self.layout.request.objective != LayoutObjective::Arrange
            && self.layout.request.template.is_none()
        {
            self.layout.request.template =
                self.occurrence.or_else(|| parts.first().map(|(id, _)| *id));
        }
        let proposal = self.layout_proposal();
        let mut apply = false;
        let mut find = false;
        if self.state.active_operation {
            ui.weak("Finding and verifying a layout…");
            self.button(ui, Action::Cancel);
        } else if let Some(p) = &proposal {
            if p.request.objective == LayoutObjective::Arrange {
                ui.label(format!("{} parts arranged", p.placed_count));
            } else {
                let quantity = 1 + p.copied_count();
                if p.request.objective == LayoutObjective::Fill {
                    ui.label(format!("{quantity} copies placed"));
                } else {
                    ui.label(format!("{} of {} requested", quantity, p.request.quantity));
                }
                ui.weak(format!(
                    "{} parts · {} plates",
                    p.total_placed(),
                    1 + p.additional_plates.len()
                ));
            }
            let filled = p.request.objective == LayoutObjective::Fill
                && p.limited_by == LayoutLimit::BuildArea;
            if filled {
                ui.label("Plate filled: no further copy fits the build area");
            } else if p.total_placed() < p.requested_count {
                ui.colored_label(
                    self.prefs.theme.tokens().warning,
                    format!(
                        "{} requested copies were not placed",
                        p.requested_count - p.total_placed()
                    ),
                );
            }
            if !filled && p.total_placed() < p.requested_count {
                ui.label(match p.limited_by {
                    LayoutLimit::BuildArea => "No further copy fits the build area",
                    LayoutLimit::ManufacturingBudget => {
                        "This machine's manufacturing working set was reached"
                    }
                    LayoutLimit::RequestedQuantity => "Quantity limit reached",
                    LayoutLimit::PlateBudget => "Project plate limit reached",
                    LayoutLimit::OccurrenceBudget => "Occurrence identity width reached",
                });
            }
            if let Some(reason) = &p.manufacturing_refusal {
                ui.label(reason);
            }
            ui.weak(format!(
                "{} layers · emitted program verified",
                p.verification.layer_height.layer_count
            ));
            apply = ui::primary(
                ui,
                "Apply layout",
                if project.read_only {
                    Some("This project is read-only")
                } else if p.placements.is_empty() && p.additional_plates.is_empty() {
                    Some("Nothing to move or copy")
                } else {
                    None
                },
            );
        } else {
            let reason = if self.state.busy {
                Some("Wait for the active action")
            } else if self.layout.request.validate().is_err() {
                Some("Choose a part, quantity and spacing from 0 to 100 mm")
            } else if self.state.manufacturing.intent.is_none() {
                Some("Apply a printer and recipe first")
            } else if self.subject_drafts() || self.project_drafts() {
                Some("Apply or discard other drafts first")
            } else {
                None
            };
            find = ui::primary(ui, "Find layout", reason);
        }
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("arrange-body")
            .show(ui, |ui| {
                ui.add_enabled_ui(!self.state.active_operation, |ui| {
                    let allow_additional_plates = !matches!(
                        self.state
                            .manufacturing
                            .intent
                            .as_ref()
                            .map(|i| &i.printer.workflow),
                        Some(spiling_contracts::manufacturing::JobWorkflow::FixedDualMarlin { .. })
                    );
                    ui::layout_fields(
                        ui,
                        &mut self.layout.request,
                        &parts,
                        allow_additional_plates,
                    )
                });
                if let Some(p) = &proposal {
                    let bed = self.state.manufacturing.intent.as_ref().map(|i| {
                        i.hardware
                            .as_ref()
                            .and_then(|h| h.selected_face().ok())
                            .map_or(i.printer.build_envelope, |f| f.usable_mm)
                    });
                    ui::additional_layout_previews(ui, p, bed);
                    egui::CollapsingHeader::new("Details").show(ui, |ui| {
                        ui.label(format!(
                            "{} placement candidates examined",
                            p.candidates_examined
                                + p.additional_plates
                                    .iter()
                                    .map(|p| p.candidates_examined)
                                    .sum::<u32>()
                        ));
                        ui.label(format!(
                            "{} manufacturing attempts",
                            p.manufacturing_attempts
                                + p.additional_plates
                                    .iter()
                                    .map(|p| p.manufacturing_attempts)
                                    .sum::<u32>()
                        ));
                        ui.label(format!(
                            "{} deposition segments",
                            p.verification.deposition_segments
                        ));
                        ui.weak("Software verification · physical printing unqualified");
                    });
                }
            });
        let Some(scene) = &self.state.scene else {
            return;
        };
        if apply && let Some(p) = proposal {
            self.queued_geometry = Some(GeometryCommand::ApplyLayout {
                session_id: scene.session_id.clone(),
                base_revision: p.scene_revision,
                project_revision: p.project_revision,
                proposal_id: p.id.clone(),
            });
        } else if find {
            self.layout.preview = None;
            self.queued_geometry = Some(GeometryCommand::PrepareLayout {
                session_id: scene.session_id.clone(),
                base_revision: scene.revision,
                project_revision: project.revision,
                request: self.layout.request.clone(),
            });
        }
    }
}
