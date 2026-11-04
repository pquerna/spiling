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
impl SurfaceUi {
    pub(super) fn has_changes(&self) -> bool {
        self.dirty
    }
    pub(super) fn discard(&mut self) {
        *self = Self::default();
    }
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
    pub(super) fn finish_pane(&mut self, ui: &mut egui::Ui) {
        ui::pane_header(
            ui,
            "Surface finish",
            "Selected source face · additive construction",
        );
        if ui::close(ui, "Back to preparation") {
            self.show_panel(Panel::Parts);
        }
        self.finish_actions(ui);
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("finish-body")
            .show(ui, |ui| self.surface_ui(ui));
    }
    fn finish_actions(&mut self, ui: &mut egui::Ui) {
        let (Some(project), Some(hello)) = (self.state.project.clone(), self.state.hello.clone())
        else {
            return;
        };
        let Some(inspection) = self
            .state
            .inspection
            .as_ref()
            .filter(|i| Some(&i.reference) == self.selected.as_ref())
        else {
            ui.label("Select a source face to add a finish");
            return;
        };
        let reference = inspection.reference.clone();
        let plane = matches!(
            inspection.face.geometry.analytic,
            Some(AnalyticSurfaceDescription::Plane { .. })
        );
        let cylinder = matches!(
            inspection.face.geometry.analytic,
            Some(AnalyticSurfaceDescription::Cylinder { .. })
        );
        let ready = self.state.phase == Phase::Running
            && !self.state.busy
            && !self.state.picker_open
            && self.awaiting_serial.is_none()
            && self.state.confirmation.is_none()
            && self.display_current();
        let s = &mut self.surface;
        if s.dirty {
            ui.label("Unapplied finish draft");
        }
        let reason = if !ready {
            Some("Wait for the current native operation")
        } else if project.read_only {
            Some("Project is read-only")
        } else if s.revision != Some(project.revision) {
            Some("Reload this draft against the current project revision")
        } else if !match s.kind {
            0 => true,
            1 => plane,
            2 => cylinder,
            _ => plane || cylinder,
        } {
            Some("This finish requires a native plane or cylinder")
        } else {
            None
        };
        if s.kind == 3 && s.stored.is_none() {
            let validation = s.image.validate().err().map(|e| e.to_string());
            if ui::primary(
                ui,
                "Choose and apply image…",
                reason.or(validation.as_deref()),
            ) {
                s.submitted = Some(self.state.serial);
                self.queued_image = Some(crate::assets::ImageRequest {
                    session: hello.session_id.clone(),
                    revision: s.revision.expect("captured finish revision"),
                    reference,
                    scope: s.scope,
                    parameters: s.image.clone(),
                });
            }
        } else {
            let finish = match s.kind {
                0 => SurfaceFinish::Smooth,
                1 => SurfaceFinish::VerticalRibs {
                    depth_mm: s.ribs[0],
                    pitch_mm: s.ribs[1],
                    edge_margin_mm: s.ribs[2],
                    phase_mm: s.ribs[3],
                },
                2 => SurfaceFinish::CylindricalRibs {
                    depth_mm: s.ribs[0],
                    rib_count: s.cylinder_count,
                    phase_fraction: s.ribs[3],
                },
                _ => SurfaceFinish::ImageRelief {
                    image_hash: s.stored.clone().expect("selected canonical image"),
                    parameters: s.image.clone(),
                },
            };
            let validation = finish.validate().err().map(|e| e.to_string());
            if ui::primary(ui, "Add finish", reason.or(validation.as_deref())) {
                s.submitted = Some(self.state.serial);
                self.queued_project = Some(ProjectCommand::AddSurfaceFinish {
                    session_id: hello.session_id.clone(),
                    base_revision: s.revision.expect("captured finish revision"),
                    face_ref: reference,
                    scope: s.scope,
                    finish,
                });
            }
        }
        if s.dirty && ui.button("Discard finish draft").clicked() {
            s.discard();
        }
    }
    pub(super) fn surface_ui(&mut self, ui: &mut egui::Ui) {
        ui::help(
            ui,
            "finish-display",
            "Finish construction",
            "Source display remains the original BREP. Compile and freshly verify the program to inspect additive construction.",
        );
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
                    ui::help(ui,"image-frame","Image coordinates","Pixel nodes use bilinear mapping with periodic U and source Z. Image top points toward +source Z. Cylinder U begins at source +X; planes use the canonical native tangent.");
                    let before=s.stored.clone();egui::ComboBox::from_id_salt("canonical image").selected_text(s.stored.as_ref().and_then(|hash|project.surface_overlays.images.iter().find(|i|&i.image_hash==hash)).map_or("Import a new PNG / PGM",|i|i.source_name.as_str())).show_ui(ui,|ui|{ui.selectable_value(&mut s.stored,None,"Import a new PNG / PGM");for image in &project.surface_overlays.images{ui.selectable_value(&mut s.stored,Some(image.image_hash.clone()),&image.source_name);}});s.dirty|=before!=s.stored;
                    for(label,value)in[("Maximum additive depth (mm)",&mut s.image.depth_mm),("U phase (cycle fraction)",&mut s.image.u_phase_fraction),("Source-Z period (mm)",&mut s.image.z_period_mm),("Source-Z phase (mm)",&mut s.image.z_phase_mm),("Protected edge margin (mm; cylinder 0)",&mut s.image.edge_margin_mm),("Vertical displacement error (mm)",&mut s.image.vertical_error_mm)]{changed|=numeric(ui,label,value,0.01);}
                    changed|=ui.add(egui::DragValue::new(&mut s.image.u_repeats).range(1..=256).prefix("U repeats ")).changed();
                    if let Err(error)=s.image.validate(){ui.colored_label(self.prefs.theme.tokens().warning,error.to_string());}
                }
                s.dirty|=changed;
            });
        } else {
            ui.weak("Select a source face by click or the keyboard-accessible face list.");
        }
        ui.separator();
        ui::group(ui, "Finish layers");
        ui::help(
            ui,
            "finish-layers",
            "Layer precedence",
            "Enabled definition layers precede occurrence layers. The last matching layer wins; Smooth can override lower layers.",
        );
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
        ui::group(ui, "Stored images");
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
    ui::number(ui, label, value, speed).changed()
}

fn finish_label(finish: &SurfaceFinish) -> &'static str {
    match finish {
        SurfaceFinish::Smooth => "Smooth override",
        SurfaceFinish::VerticalRibs { .. } => "Planar ribs",
        SurfaceFinish::CylindricalRibs { .. } => "Cylindrical ribs",
        SurfaceFinish::ImageRelief { .. } => "Canonical image relief",
    }
}
