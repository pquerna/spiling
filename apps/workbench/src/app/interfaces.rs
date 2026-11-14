// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::inspection::FunctionalArea;
use spiling_contracts::project::{ProjectId, ProjectRevision};
use std::sync::{Arc, Weak};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interfaces_follow_the_physical_upper_layer_for_single_through_and_range() {
        let all = crate::replay::View::default();
        assert_eq!(selected_layer(all), None);
        let single = crate::replay::View {
            layer: Some(56),
            ..all
        };
        assert_eq!(selected_layer(single), Some(56));
        let through = crate::replay::View {
            layer: Some(0),
            layer_end: Some(56),
            ..all
        };
        assert_eq!(selected_layer(through), Some(56));
        let range = crate::replay::View {
            layer: Some(40),
            layer_end: Some(56),
            ..all
        };
        assert_eq!(selected_layer(range), Some(56));
        assert_ne!(
            selected_layer(range),
            selected_layer(crate::replay::View {
                layer_end: Some(57),
                ..range
            })
        );
    }

    #[test]
    fn localized_join_scope_uses_world_intervals_and_excludes_touching_layers() {
        let active = [12.0, 18.0];
        assert_eq!(
            functional_layer_scope(active, [11.8, 12.0]),
            FunctionalLayerScope::Inactive
        );
        assert_eq!(
            functional_layer_scope(active, [18.0, 18.2]),
            FunctionalLayerScope::Inactive
        );
        assert_eq!(
            functional_layer_scope(active, [12.0, 12.2]),
            FunctionalLayerScope::Active
        );
        assert_eq!(
            functional_layer_scope(active, [17.8, 18.0]),
            FunctionalLayerScope::Active
        );
        assert_eq!(
            functional_layer_scope(active, [11.9, 12.1]),
            FunctionalLayerScope::Crossing
        );
        assert_eq!(
            functional_layer_scope(active, [17.9, 18.1]),
            FunctionalLayerScope::Crossing
        );
    }

    #[test]
    fn every_functional_highlight_requires_the_current_project_capture() {
        let project = ProjectId::new();
        let other = ProjectId::new();
        let revision = ProjectRevision(7);
        for kind in [
            Kind::Functional(0),
            Kind::FunctionalCommands(FunctionalArea::Region),
            Kind::FunctionalCommands(FunctionalArea::ProtectedBase),
            Kind::FunctionalCommands(FunctionalArea::InheritedBase),
        ] {
            assert!(kind.is_functional());
        }
        assert!(current_functional_revision(
            (&project, revision),
            Some((&project, revision))
        ));
        assert!(!current_functional_revision(
            (&project, revision),
            Some((&project, revision.next().unwrap()))
        ));
        assert!(!current_functional_revision(
            (&project, revision),
            Some((&other, revision))
        ));
        assert!(!current_functional_revision((&project, revision), None));
        assert!(!Kind::Separator(0).is_functional());
        assert!(!Kind::Plate(0).is_functional());
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Separator(usize),
    Plate(usize),
    Functional(usize),
    FunctionalCommands(FunctionalArea),
}
impl Kind {
    fn is_functional(self) -> bool {
        matches!(self, Self::Functional(_) | Self::FunctionalCommands(_))
    }
}
fn current_functional_revision(
    captured: (&ProjectId, ProjectRevision),
    current: Option<(&ProjectId, ProjectRevision)>,
) -> bool {
    current == Some(captured)
}
fn functional_source_matches(
    scene: &crate::scene::Scene,
    review: &crate::manufacture::Review,
) -> bool {
    use spiling_contracts::manufacturing::JobWorkflow;
    let JobWorkflow::FixedDualMarlin { job } = &review.bundle.provenance.intent.printer.workflow
    else {
        return false;
    };
    let Some(request) = &job.functional_construction else {
        return false;
    };
    let Some(frame) = &review.bundle.plan.functional_material_frame else {
        return false;
    };
    scene.summary.session_id == review.capture.session
        && scene.summary.layout_id == review.capture.layout_id
        && frame.validate(request, job).is_ok()
        && scene.occurrences.iter().any(|o| {
            o.occurrence_id == frame.occurrence_id && o.definition_id == frame.definition_id
        })
        && scene
            .definition(&frame.definition_id)
            .is_some_and(|d| d.record.provenance.source_hash == frame.source_hash)
}
struct Selection {
    review: Weak<crate::manufacture::Review>,
    scene: Weak<crate::scene::Scene>,
    layer: usize,
    kind: Kind,
    occurrence: OccurrenceId,
    area: Arc<crate::inspection::Area>,
}
impl Selection {
    fn matches(
        &self,
        scene: &Arc<crate::scene::Scene>,
        review: &Arc<crate::manufacture::Review>,
    ) -> bool {
        self.scene.ptr_eq(&Arc::downgrade(scene)) && self.review.ptr_eq(&Arc::downgrade(review))
    }
}
#[derive(Default)]
pub(super) struct InterfacesUi {
    selected: Option<Selection>,
    problem: Option<String>,
}
fn selected_layer(view: crate::replay::View) -> Option<usize> {
    view.layer_end.or(view.layer).map(|layer| layer as usize)
}

#[derive(Debug, PartialEq, Eq)]
enum FunctionalLayerScope {
    Inactive,
    Active,
    Crossing,
}
fn functional_layer_scope(active: [f64; 2], layer: [f64; 2]) -> FunctionalLayerScope {
    if layer[1] <= active[0] || layer[0] >= active[1] {
        FunctionalLayerScope::Inactive
    } else if active[0] <= layer[0] && layer[1] <= active[1] {
        FunctionalLayerScope::Active
    } else {
        FunctionalLayerScope::Crossing
    }
}
impl Workbench {
    pub(super) fn interface_overlay(
        &mut self,
        scene: Option<&Arc<crate::scene::Scene>>,
        review: Option<&Arc<crate::manufacture::Review>>,
    ) -> Option<Arc<crate::inspection::Area>> {
        let selected = self.interfaces.selected.as_ref()?;
        if self.display_current()
            && selected_layer(self.manufacturing.replay) == Some(selected.layer)
            && scene.zip(review).is_some_and(|(scene, review)| {
                selected.matches(scene, review)
                    && review.capture.report.verified
                    && (!selected.kind.is_functional()
                        || (current_functional_revision(
                            (
                                &review.capture.provenance.project_id,
                                review.capture.observed_revision,
                            ),
                            self.state
                                .project
                                .as_ref()
                                .map(|project| (&project.project_id, project.revision)),
                        ) && functional_source_matches(scene, review)))
            })
        {
            Some(selected.area.clone())
        } else {
            self.interfaces.selected = None;
            self.viewport.cancel_area();
            None
        }
    }
    pub(super) fn construction_interfaces_ui(
        &mut self,
        ui: &mut egui::Ui,
        review: &Arc<crate::manufacture::Review>,
    ) {
        self.functional_join_review_ui(ui, review);
        ui.collapsing("Construction interfaces",|ui| {
            ui.weak("Compiled density-region construction · not mixed-material interlocks");
            if !review.capture.report.verified || !self.display_current() {
                ui.label("Current source and fresh passing output are required for interface inspection.");
                return;
            }
            let Some(layer_index)=selected_layer(self.manufacturing.replay) else {
                ui.label("Select a physical layer to inspect its separators and plates.");
                return;
            };
            let Some(layer)=review.bundle.plan.layers.get(layer_index) else { return; };
            let Some(scene)=self.state.display.clone() else { return; };
            if scene.summary.session_id!=review.capture.session || scene.summary.layout_id!=review.capture.layout_id {
                ui.label("The construction capture does not match this source session and plate.");
                return;
            }
            let coverage=&layer.region_coverage;
            if coverage.process_interfaces.is_empty() && coverage.process_diaphragms.is_empty() {
                ui.label("No compiled separators or plates on this layer. Material contacts have not been assessed by this view.");
                return;
            }
            let count=coverage.process_interfaces.len()+coverage.process_diaphragms.len();
            ui.label(format!("Layer {} · {count} compiled construction records",layer_index+1));
            let mut chosen=None;
            egui::ScrollArea::vertical().id_salt("interface-list").max_height(160.0).show_rows(ui,24.0,count,|ui,range| {
                for index in range {
                    let (kind,label)=if let Some(separator)=coverage.process_interfaces.get(index) {
                        (Kind::Separator(index),format!("Separator {} · part {} · regions {} ↔ {}",index+1,separator.occurrence_id.get(),separator.region_indices[0],separator.region_indices[1]))
                    } else {
                        let index=index-coverage.process_interfaces.len();
                        let plate=&coverage.process_diaphragms[index];
                        (Kind::Plate(index),format!("Permanent plate {} · part {}",index+1,plate.occurrence_id.get()))
                    };
                    let selected=self.interfaces.selected.as_ref().is_some_and(|selected|selected.matches(&scene,review) && selected.layer==layer_index && selected.kind==kind);
                    if ui.add_sized([ui.available_width(),24.0],egui::Button::selectable(selected,&label).truncate()).on_hover_text(&label).clicked() {chosen=Some(kind);}
                }
            });
            if let Some(kind)=chosen {
                let origin=scene.summary.bounds_mm.map(|bounds|DVec3::from_array(bounds.min)*0.5+DVec3::from_array(bounds.max)*0.5);
                let result=origin.ok_or_else(||"No current source display bounds".to_owned()).and_then(|origin| {
                    let (occurrence,definition,area)=match kind {
                        Kind::Separator(index)=>{
                            let value=&coverage.process_interfaces[index];
                            (value.occurrence_id,&value.definition_id,crate::inspection::Area::separator(value,[layer.lower_z_mm,layer.z_mm],origin))
                        }
                        Kind::Plate(index)=>{
                            let value=&coverage.process_diaphragms[index];
                            (value.occurrence_id,&value.definition_id,crate::inspection::Area::plate(value,origin))
                        }
                        Kind::Functional(_) | Kind::FunctionalCommands(_)=>return Err("Select a density construction record".into()),
                    };
                    if !scene.occurrences.iter().any(|item|item.occurrence_id==occurrence && &item.definition_id==definition) {return Err("Construction owner is absent from the current source".into());}
                    let area=Arc::new(area?);
                    Ok(Selection {review:Arc::downgrade(review),scene:Arc::downgrade(&scene),layer:layer_index,kind,occurrence,area})
                });
                match result {
                    Ok(selected)=>{
                        self.viewport.focus_area(&scene,selected.area.target,&[selected.occurrence]);
                        self.interfaces.selected=Some(selected);
                        self.interfaces.problem=None;
                        ui.ctx().request_repaint();
                    }
                    Err(error)=>{ self.viewport.cancel_area(); self.interfaces.selected=None; self.interfaces.problem=Some(error); }
                }
            }
            let mut clear=false;
            if let Some(selected)=&self.interfaces.selected
                && selected.matches(&scene,review) && selected.layer==layer_index
                && !selected.kind.is_functional()
            {
                ui.label(match selected.kind {Kind::Separator(_)=>"Highlighted: separator centerlines over their physical layer interval",Kind::Plate(_)=>"Highlighted: permanent plate extent, including hole boundaries",Kind::Functional(_) | Kind::FunctionalCommands(_)=>return});
                match selected.kind {
                    Kind::Separator(index)=>{
                        let value=&coverage.process_interfaces[index];
                        ui.label(format!("Thickness {:.3} mm · bonding overlap {:.3} mm",value.thickness_mm,value.bonding_overlap_mm));
                        ui.label(format!("Physical interval {:.6}…{:.6} mm",layer.lower_z_mm,layer.z_mm));
                    }
                    Kind::Plate(index)=>{
                        let value=&coverage.process_diaphragms[index];
                        ui.label(format!("Requested thickness {:.3} mm · physical interval {:.6}…{:.6} mm",value.requested_thickness_mm,value.physical_z_mm[0],value.physical_z_mm[1]));
                        ui.label(format!("Neighbor sparse fractions {:.1}% ↔ {:.1}%",value.neighbors[0].fraction*100.0,value.neighbors[1].fraction*100.0));
                    }
                    Kind::Functional(_) | Kind::FunctionalCommands(_)=>return,
                }
                ui.weak("Construction outlines are not deposited bead surfaces or measured bond strength. Layer/tool filters still control the actual program.");
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Focus interface").clicked() {self.viewport.focus_area(&scene,selected.area.target,&[selected.occurrence]);}
                    if ui.button("Reveal interface").clicked() {self.viewport.reveal_area(&scene,selected.area.target,&[selected.occurrence]);}
                    clear=ui.button("Clear highlight").clicked();
                });
            }
            if clear {
                self.viewport.close_reveal(ui.input(|input|input.time),ui.style().animation_time>0.0);
                self.viewport.clear_attention();
                self.interfaces.selected=None;
            }
            if let Some(problem)=&self.interfaces.problem {ui.colored_label(ui.visuals().warn_fg_color,problem);}
        });
    }
}

impl Workbench {
    fn functional_join_review_ui(
        &mut self,
        ui: &mut egui::Ui,
        review: &Arc<crate::manufacture::Review>,
    ) {
        use spiling_contracts::manufacturing::*;
        let JobWorkflow::FixedDualMarlin { job } =
            &review.bundle.provenance.intent.printer.workflow
        else {
            return;
        };
        let Some(request) = &job.functional_construction else {
            return;
        };
        ui.collapsing("Joins · functional material", |ui| {
            ui.label(format!("Captured request · {} ↔ {} · face {}", request.material_id, request.base_material_id, request.face_id.as_str()));
            if let Some(patch) = &request.face_patch {
                ui.label(format!("Requested patch · source face U {:.3}…{:.3} mm · source Z {:.3}…{:.3} mm", patch.horizontal_mm[0], patch.horizontal_mm[1], patch.source_z_mm[0], patch.source_z_mm[1]));
            } else {
                ui.label("Requested scope · full native face");
            }
            if !current_functional_revision(
                (&review.capture.provenance.project_id, review.capture.observed_revision),
                self.state.project.as_ref().map(|project| (&project.project_id, project.revision)),
            ) {
                ui.label("Earlier capture · verify this construction against the current project revision before inspecting realized joins");
                return;
            }
            let Some(frame) = &review.bundle.plan.functional_material_frame else {
                ui.label("No realized functional construction frame in this capture");
                return;
            };
            let Some(scene) = self.state.display.clone().filter(|scene| {
                self.display_current() && scene.summary.session_id == review.capture.session
                    && scene.summary.layout_id == review.capture.layout_id
                    && scene.occurrences.iter().any(|o| o.occurrence_id == frame.occurrence_id && o.definition_id == frame.definition_id)
                    && scene.definition(&frame.definition_id).is_some_and(|d| d.record.provenance.source_hash == frame.source_hash)
            }) else {
                ui.label("Current source matching this capture is required for realized join inspection");
                return;
            };
            if !review.capture.report.verified || frame.validate(request, job).is_err() {
                ui.label("Fresh passing replay and a matching functional frame are required for realized join inspection");
                return;
            }
            ui.label(format!("Selected active scope · face U {:.3}…{:.3} mm · world Z {:.3}…{:.3} mm", frame.selected_u_mm[0], frame.selected_u_mm[1], frame.region_z_mm[0], frame.region_z_mm[1]));
            ui.weak(format!("Whole source face Z {:.3}…{:.3} mm", frame.source_z_mm[0], frame.source_z_mm[1]));
            ui.label(format!("Base infill: {:.0}% · join reinforcement: solid", review.bundle.provenance.intent.recipe.infill_fraction * 100.0));
            ui.label(format!("Minimum solid base root depth: {:.3} mm beyond the join · pure region remains solid", request.minimum_solid_base_root_depth_mm));
            for zone in &frame.solid_base_zones {
                let purpose = match zone.kind {
                    FunctionalBaseSolidZoneKind::Root => "Join backbone",
                    FunctionalBaseSolidZoneKind::LowerLanding => "Lower landing",
                    FunctionalBaseSolidZoneKind::UpperCover => "Upper cover",
                };
                ui.weak(format!("{purpose} · full face width · inward depth {:.3}…{:.3} mm · world Z {:.3}…{:.3} mm", zone.inward_depth_mm[0], zone.inward_depth_mm[1], zone.physical_z_mm[0], zone.physical_z_mm[1]));
            }
            ui.label(format!("Pure-material ownership ≥ {:.3} mm · join reach {:.3} mm into region / {:.3} mm into base", request.minimum_pure_depth_mm, request.join.reach_into_region_mm, request.join.reach_into_base_mm));
            ui.label(match request.interface_policy {
                FunctionalInterfacePolicy::DeclaredSameFamily => "Policy: declared same family · not an adhesion measurement",
                FunctionalInterfacePolicy::MechanicalCapture => "Policy: nominal mechanical capture · no adhesion credit",
            });
            ui.label(format!("Fresh verified construction · {} keys · middle-head world Z {:.3}…{:.3} mm", frame.key_centers_u_mm.len(), frame.key_middle_z_mm[0], frame.key_middle_z_mm[1]));
            ui.weak("Verified nominal construction, not measured adhesion or physical strength.");
            let Some(layer_index) = selected_layer(self.manufacturing.replay) else {
                ui.label("Select a physical layer to inspect its realized region boundary and join phase");
                return;
            };
            let Some(layer) = review.bundle.plan.layers.get(layer_index) else { return; };
            ui.label(format!("Selected layer {} · world Z {:.3}…{:.3} mm", layer_index + 1, layer.lower_z_mm, layer.z_mm));
            let physical = [layer.lower_z_mm, layer.z_mm];
            let zone = crate::inspection::functional_zone_bounds(frame, physical);
            match &zone {
                Ok(Some(_)) => {
                    for zone in &frame.solid_base_zones {
                        if zone.physical_z_mm[0] <= physical[0] && physical[1] <= zone.physical_z_mm[1] {
                            let purpose = match zone.kind {
                                FunctionalBaseSolidZoneKind::Root => "Join backbone",
                                FunctionalBaseSolidZoneKind::LowerLanding => "Lower landing",
                                FunctionalBaseSolidZoneKind::UpperCover => "Upper cover",
                            };
                            ui.weak(format!("Active protected Base · {purpose} · full face U · inward depth {:.3}…{:.3} mm", zone.inward_depth_mm[0], zone.inward_depth_mm[1]));
                        }
                    }
                }
                Ok(None) => { ui.weak("No active captured solid zone · Inherited Base shows all Base command centerlines"); }
                Err(error) => { ui.colored_label(ui.visuals().warn_fg_color, error); }
            }
            ui.horizontal_wrapped(|ui| {
                for (scope, label) in [
                    (FunctionalArea::Region, "Show Region"),
                    (FunctionalArea::ProtectedBase, "Show Protected Base"),
                    (FunctionalArea::InheritedBase, "Show Inherited Base"),
                ] {
                    if ui.button(label).on_hover_text(scope.label()).clicked() {
                        let result = scene.summary.bounds_mm.ok_or_else(|| "No source display origin".to_owned()).and_then(|bounds| {
                            let origin = DVec3::from_array(bounds.min) * 0.5 + DVec3::from_array(bounds.max) * 0.5;
                            crate::inspection::Area::functional_commands(&layer.paths, physical, frame, scope, origin).map(Arc::new)
                        });
                        match result {
                            Ok(area) => {
                                self.viewport.focus_area(&scene, area.target, &[frame.occurrence_id]);
                                self.interfaces.selected = Some(Selection {
                                    review: Arc::downgrade(review), scene: Arc::downgrade(&scene), layer: layer_index,
                                    kind: Kind::FunctionalCommands(scope), occurrence: frame.occurrence_id, area,
                                });
                                self.interfaces.problem = None;
                                ui.ctx().request_repaint();
                            }
                            Err(error) => {
                                self.viewport.cancel_area();
                                self.interfaces.selected = None;
                                self.interfaces.problem = Some(error);
                            }
                        }
                    }
                }
            });
            ui.weak("Display-only actual XYZ6 command centerlines · no extrusion or material-boundary representation · no strength claim. Clipped endpoints are display cuts; inherited Base includes walls and skins.");
            ui.scope(|ui| {
                match functional_layer_scope(frame.region_z_mm, physical) {
                    FunctionalLayerScope::Inactive => {
                        ui.label("Inactive layer · outside the selected patch height; base material only, no functional join");
                        return;
                    }
                    FunctionalLayerScope::Crossing => {
                        ui.label("This layer crosses the active patch boundary; no complete functional interval is available to inspect");
                        return;
                    }
                    FunctionalLayerScope::Active => {}
                }
                let Some((index, domain)) = review.bundle.plan.material_domains.iter().enumerate().find(|(_, domain)| {
                    domain.reference.role == MaterialConstructionRole::FunctionalRegion
                        && domain.physical_z_mm[0] <= layer.lower_z_mm && layer.z_mm <= domain.physical_z_mm[1]
                }) else { ui.label("No complete functional region domain for this active physical layer"); return; };
                ui.label(format!("Realized {} phase · world Z {:.3}…{:.3} mm", if domain.physical_z_mm[0] >= frame.key_middle_z_mm[0] && domain.physical_z_mm[1] <= frame.key_middle_z_mm[1] { "middle-head" } else { "end-collar" }, domain.physical_z_mm[0], domain.physical_z_mm[1]));
                if ui.button("Show realized join").clicked() {
                    let result = scene.summary.bounds_mm.ok_or_else(|| "No source display origin".to_owned()).and_then(|bounds| {
                        let origin = DVec3::from_array(bounds.min) * 0.5 + DVec3::from_array(bounds.max) * 0.5;
                        let base = review.bundle.plan.material_domains.iter().find(|base| {
                            base.reference.role == MaterialConstructionRole::FunctionalBase
                                && base.reference.construction_index == domain.reference.construction_index
                                && base.physical_z_mm == domain.physical_z_mm
                        }).ok_or_else(|| "No matching realized base owner for this join".to_owned())?;
                        crate::inspection::Area::functional_join(domain, base, frame.stock_bounds_mm, origin).map(Arc::new)
                    });
                    match result {
                        Ok(area) => {
                            self.viewport.focus_area(&scene, area.target, &[frame.occurrence_id]);
                            self.interfaces.selected = Some(Selection {
                                review: Arc::downgrade(review), scene: Arc::downgrade(&scene), layer: layer_index,
                                kind: Kind::Functional(index), occurrence: frame.occurrence_id, area,
                            });
                            self.interfaces.problem = None;
                        }
                        Err(error) => {
                            self.viewport.cancel_area();
                            self.interfaces.selected = None;
                            self.interfaces.problem = Some(error);
                        }
                    }
                }
            });
            if let Some(selected) = self.interfaces.selected.as_ref().filter(|selected| {
                selected.matches(&scene, review) && selected.layer == layer_index && selected.kind.is_functional()
            }) {
                ui.weak(match selected.kind {
                    Kind::FunctionalCommands(scope) => scope.label(),
                    _ => "Shared interface outline; exterior edges excluded. Not deposited bead surfaces.",
                });
                let mut clear = false;
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Focus").clicked() { self.viewport.focus_area(&scene, selected.area.target, &[selected.occurrence]); }
                    if ui.button("Reveal").clicked() { self.viewport.reveal_area(&scene, selected.area.target, &[selected.occurrence]); }
                    clear = ui.button("Clear highlight").clicked();
                });
                if clear {
                    self.viewport.close_reveal(ui.input(|input| input.time), ui.style().animation_time > 0.0);
                    self.viewport.clear_attention();
                    self.interfaces.selected = None;
                }
            }
            if let Some(problem) = &self.interfaces.problem { ui.colored_label(ui.visuals().warn_fg_color, problem); }
        });
    }
}
