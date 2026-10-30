// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use spiling_contracts::{
    project::{ProjectCommand, ProjectRevision},
    surface::*,
};

pub(super) struct SurfaceUi {
    reference: Option<FaceRef>,
    revision: Option<ProjectRevision>,
    scope: SurfaceScopeKind,
    kind: u8,
    ribs: [f64; 4],
    cylinder_count: u32,
    image: ImageReliefParameters,
    stored: Option<SourceHash>,
    dirty: bool,
    submitted: Option<u64>,
    texture: Option<(SourceHash, egui::TextureHandle)>,
}
impl Default for SurfaceUi {
    fn default() -> Self {
        Self {
            reference: None,
            revision: None,
            scope: SurfaceScopeKind::Occurrence,
            kind: 0,
            ribs: [0.1, 2.0, 1.0, 0.0],
            cylinder_count: 8,
            image: ImageReliefParameters {
                depth_mm: 0.5,
                u_repeats: 1,
                u_phase_fraction: 0.0,
                z_period_mm: 10.0,
                z_phase_mm: 0.0,
                edge_margin_mm: 1.0,
                vertical_error_mm: 0.05,
            },
            stored: None,
            dirty: false,
            submitted: None,
            texture: None,
        }
    }
}
impl Workbench {
    pub(super) fn image_texture(&mut self, ctx: &egui::Context) {
        let image = self.state.image_preview.as_ref().filter(|image| {
            self.state.project.as_ref().is_some_and(|p| {
                p.project_id == image.project && p.surface_overlays.images.contains(&image.record)
            })
        });
        match image {
            Some(image)
                if self
                    .surface
                    .texture
                    .as_ref()
                    .is_none_or(|(hash, _)| *hash != image.record.image_hash) =>
            {
                self.surface.texture = Some((
                    image.record.image_hash.clone(),
                    ctx.load_texture(
                        "Canonical height nodes",
                        image.pixels.clone(),
                        egui::TextureOptions::NEAREST,
                    ),
                ));
            }
            None => self.surface.texture = None,
            _ => {}
        }
    }
    pub(super) fn surface_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Source-face finish");
        ui.label("Additive source construction. Display mesh stays the original BREP; freshly compiled actual motion shows the result.");
        let (Some(project), Some(hello)) = (self.state.project.clone(), self.state.hello.clone())
        else {
            return;
        };
        let ready = self.state.phase == Phase::Running
            && !self.state.busy
            && !self.state.picker_open
            && self.awaiting_serial.is_none()
            && self.state.confirmation.is_none();
        let writable = ready && !project.read_only && self.display_current();
        let s = &mut self.surface;
        if let Some(inspection) = self
            .state
            .inspection
            .clone()
            .filter(|i| Some(&i.reference) == self.selected.as_ref())
        {
            let reference = inspection.reference.clone();
            if s.submitted.is_some_and(|serial| {
                self.state.serial > serial && !self.state.busy && !self.state.picker_open
            }) {
                if s.revision != Some(project.revision) {
                    s.dirty = false;
                }
                s.submitted = None;
            }
            if s.reference.as_ref() != Some(&reference)
                || (!s.dirty && s.revision != Some(project.revision))
            {
                s.reference = Some(reference.clone());
                s.revision = Some(project.revision);
                s.dirty = false;
            }
            let conflict = s.revision != Some(project.revision);
            if conflict {
                ui.colored_label(
                    self.prefs.theme.tokens().warning,
                    "Project changed while editing. Reload this finish draft before applying.",
                );
            }
            if ui.button("Reload finish revision").clicked() {
                s.revision = Some(project.revision);
                s.dirty = false;
            }
            ui.label(format!(
                "Occurrence {} · face {}",
                reference.occurrence_id.get(),
                reference.face_id.as_str()
            ));
            let plane = matches!(
                inspection.face.geometry.analytic,
                Some(AnalyticSurfaceDescription::Plane { .. })
            );
            let cylinder = matches!(
                inspection.face.geometry.analytic,
                Some(AnalyticSurfaceDescription::Cylinder { .. })
            );
            ui.add_enabled_ui(writable && !conflict,|ui|{
                let old_scope=s.scope;egui::ComboBox::from_id_salt("finish scope").selected_text(format!("Scope: {:?}",s.scope)).show_ui(ui,|ui|{ui.selectable_value(&mut s.scope,SurfaceScopeKind::Occurrence,"Occurrence override");ui.selectable_value(&mut s.scope,SurfaceScopeKind::Definition,"Shared source definition");});s.dirty|=old_scope!=s.scope;
                let old=s.kind;egui::ComboBox::from_id_salt("finish kind").selected_text(["Smooth override","Planar vertical ribs","Cylindrical ribs","Image relief"][s.kind as usize]).show_ui(ui,|ui|{for (index,label) in ["Smooth override","Planar vertical ribs","Cylindrical ribs","Image relief"].iter().enumerate(){ui.selectable_value(&mut s.kind,index as u8,*label);}});s.dirty|=old!=s.kind;
                let supported=match s.kind{0=>true,1=>plane,2=>cylinder,_=>plane||cylinder};
                if !supported{ui.colored_label(self.prefs.theme.tokens().warning,"This finish needs an exact native plane/cylinder. Other carriers are not supported.");}
                let mut changed=false;
                if matches!(s.kind,1|2){
                    changed|=numeric(ui,"Depth (mm)",&mut s.ribs[0],0.01);
                    if s.kind==1{for(i,label)in[(1,"Pitch (mm)"),(2,"Protected edge margin (mm)"),(3,"Source-local phase (mm)")]{changed|=numeric(ui,label,&mut s.ribs[i],0.01);}}
                    else{changed|=ui.add(egui::DragValue::new(&mut s.cylinder_count).prefix("Rib count ").range(2..=256)).changed();changed|=numeric(ui,"Phase (cycle fraction)",&mut s.ribs[3],0.01);}
                }
                if s.kind==3{
                    ui.label("Pixel-node bilinear mapping, periodic U / source Z; image top points toward +source Z. Cylinder U starts at source +X with source +Z axis; planar U uses the canonical native tangent.");
                    let before=s.stored.clone();egui::ComboBox::from_id_salt("canonical image").selected_text(s.stored.as_ref().and_then(|hash|project.surface_overlays.images.iter().find(|i|&i.image_hash==hash)).map_or("Import a new PNG / PGM",|i|i.source_name.as_str())).show_ui(ui,|ui|{ui.selectable_value(&mut s.stored,None,"Import a new PNG / PGM");for image in &project.surface_overlays.images{ui.selectable_value(&mut s.stored,Some(image.image_hash.clone()),&image.source_name);}});s.dirty|=before!=s.stored;
                    for(label,value)in[("Maximum additive depth (mm)",&mut s.image.depth_mm),("U phase (cycle fraction)",&mut s.image.u_phase_fraction),("Source-Z period (mm)",&mut s.image.z_period_mm),("Source-Z phase (mm)",&mut s.image.z_phase_mm),("Protected edge margin (mm; cylinder 0)",&mut s.image.edge_margin_mm),("Vertical displacement error (mm)",&mut s.image.vertical_error_mm)]{changed|=numeric(ui,label,value,0.01);}
                    changed|=ui.add(egui::DragValue::new(&mut s.image.u_repeats).range(1..=256).prefix("U repeats ")).changed();
                    if let Err(error)=s.image.validate(){ui.colored_label(self.prefs.theme.tokens().warning,error.to_string());}
                    if s.stored.is_none() && ui.add_enabled(supported && s.image.validate().is_ok(),egui::Button::new("Choose image and apply relief…")).clicked(){s.submitted=Some(self.state.serial);self.queued_image=Some(crate::assets::ImageRequest{session:hello.session_id.clone(),revision:s.revision.expect("captured finish revision"),reference:reference.clone(),scope:s.scope,parameters:s.image.clone()});}
                }
                s.dirty|=changed;
                let finish=match s.kind{0=>Some(SurfaceFinish::Smooth),1=>Some(SurfaceFinish::VerticalRibs{depth_mm:s.ribs[0],pitch_mm:s.ribs[1],edge_margin_mm:s.ribs[2],phase_mm:s.ribs[3]}),2=>Some(SurfaceFinish::CylindricalRibs{depth_mm:s.ribs[0],rib_count:s.cylinder_count,phase_fraction:s.ribs[3]}),_=>s.stored.as_ref().map(|hash|SurfaceFinish::ImageRelief{image_hash:hash.clone(),parameters:s.image.clone()})};
                if let Some(finish)=finish{
                    let valid=finish.validate();if let Err(error)=&valid{ui.colored_label(self.prefs.theme.tokens().warning,error.to_string());}
                    if ui.add_enabled(supported && valid.is_ok(),egui::Button::new("Append finish layer")).clicked(){s.submitted=Some(self.state.serial);self.queued_project=Some(ProjectCommand::AddSurfaceFinish{session_id:hello.session_id.clone(),base_revision:s.revision.expect("captured finish revision"),face_ref:reference.clone(),scope:s.scope,finish});}
                }
            });
        } else {
            ui.weak("Select a source face by click or the keyboard-accessible face list.");
        }
        ui.separator();
        ui.label("Enabled definition layers precede occurrence layers. The last matching layer wins; Smooth can override lower layers.");
        for layer in &project.surface_overlays.layers {
            ui.push_id(layer.layer_id, |ui| {
                let scope = match &layer.scope {
                    SurfaceScope::Definition { .. } => "Shared definition".to_owned(),
                    SurfaceScope::Occurrence { occurrence_id, .. } => {
                        format!("Occurrence {}", occurrence_id.get())
                    }
                };
                ui.label(format!(
                    "Layer {} · {} · {}",
                    layer.layer_id.get(),
                    scope,
                    finish_label(&layer.finish)
                ));
                ui.weak(format!("Native face {}", layer.face_id.as_str()));
                ui.collapsing("Layer parameters and identity", |ui| {
                    ui.label(format!("{:#?}", layer));
                });
                ui.horizontal(|ui| {
                    let mut enabled = layer.enabled;
                    if ui
                        .add_enabled(writable, egui::Checkbox::new(&mut enabled, "Enabled"))
                        .changed()
                    {
                        self.queued_project = Some(ProjectCommand::SetSurfaceFinishEnabled {
                            session_id: hello.session_id.clone(),
                            base_revision: project.revision,
                            layer_id: layer.layer_id,
                            enabled,
                        });
                    }
                    if ui
                        .add_enabled(writable, egui::Button::new("Remove"))
                        .clicked()
                    {
                        self.queued_project = Some(ProjectCommand::RemoveSurfaceFinish {
                            session_id: hello.session_id.clone(),
                            base_revision: project.revision,
                            layer_id: layer.layer_id,
                        });
                    }
                });
            });
        }
        ui.separator();
        ui.heading("Captured canonical images");
        for image in &project.surface_overlays.images {
            ui.label(format!(
                "{} · {} × {} nodes",
                image.source_name, image.width, image.height
            ));
            if ui
                .add_enabled(
                    ready && !self.state.image_loading,
                    egui::Button::new(format!("Preview {}", image.source_name)),
                )
                .clicked()
            {
                self.queued_project = Some(ProjectCommand::GetSurfaceImage {
                    session_id: hello.session_id.clone(),
                    base_revision: project.revision,
                    image_hash: image.image_hash.clone(),
                });
            }
        }
        if self.state.image_loading {
            ui.spinner();
        }
        if let Some((_, texture)) = &s.texture {
            ui.label("Canonical height nodes · display only; no physical fidelity claim");
            let size = texture.size_vec2();
            ui.image((texture.id(), size * (220.0 / size.max_elem())));
        }
    }
}
fn numeric(ui: &mut egui::Ui, label: &str, value: &mut f64, speed: f64) -> bool {
    ui.horizontal_wrapped(|ui| {
        ui.label(label);
        ui.add(egui::DragValue::new(value).speed(speed)).changed()
    })
    .inner
}

fn finish_label(finish: &SurfaceFinish) -> &'static str {
    match finish {
        SurfaceFinish::Smooth => "Smooth override",
        SurfaceFinish::VerticalRibs { .. } => "Planar ribs",
        SurfaceFinish::CylindricalRibs { .. } => "Cylindrical ribs",
        SurfaceFinish::ImageRelief { .. } => "Canonical image relief",
    }
}
