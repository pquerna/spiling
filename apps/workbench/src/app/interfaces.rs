// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
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
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Separator(usize),
    Plate(usize),
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
    view.layer_end.or(view.layer).map(usize::from)
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
                selected.matches(scene, review) && review.capture.report.verified
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
            {
                ui.label(match selected.kind {Kind::Separator(_)=>"Highlighted: separator centerlines over their physical layer interval",Kind::Plate(_)=>"Highlighted: permanent plate extent, including hole boundaries"});
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
