// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

mod beads;
pub mod overview;
pub mod plate;
mod section;
use crate::scene::{Scene, Section};
use egui_wgpu::wgpu::{self, util::DeviceExt};
use glam::{DMat4, DQuat, DVec3};
use spiling_contracts::{display::validate_mesh_chunk, geometry::*};
use std::{collections::HashMap, sync::Arc};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    matrix: [[f32; 4]; 4],
    selected: [u32; 4],
    tool_color: [f32; 4],
    plane: [f32; 4],
    cut: [f32; 4],
    /// Neutral RGB/alpha for contextual lines; zero alpha keeps normal colors.
    context: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Instance {
    rotation: [f32; 4],
    translation: [f32; 4],
}
struct Draw {
    definition: DefinitionId,
    group: wgpu::BindGroup,
    _instances: wgpu::Buffer,
    indices: u32,
    instances: u32,
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct LineVertex {
    position: [f32; 3],
    color: [f32; 3],
}
struct Resources {
    meshes: HashMap<SourceHash, Arc<wgpu::Buffer>>,
    overview: overview::Renderer,
    plate: plate::Renderer,
    replay: Option<Arc<crate::manufacture::Review>>,
    replay_buffers: Vec<wgpu::Buffer>,
    bead_buffers: Vec<wgpu::Buffer>,
    bead_pipeline: wgpu::RenderPipeline,
    source_stencil: wgpu::RenderPipeline,
    bead_stencil: wgpu::RenderPipeline,
    source_cap: wgpu::RenderPipeline,
    actual_cap: wgpu::RenderPipeline,
    clear_stencil: wgpu::RenderPipeline,
    point_pipeline: wgpu::RenderPipeline,
    seam_pipeline: wgpu::RenderPipeline,
    selected_seam_pipeline: wgpu::RenderPipeline,
    tool_uniforms: [wgpu::Buffer; 2],
    tool_groups: [wgpu::BindGroup; 2],
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    scene: Option<Arc<Scene>>,
    draws: Vec<Draw>,
    failure: Option<String>,
    line_pipeline: wgpu::RenderPipeline,
    line_group: wgpu::BindGroup,
    context_uniform: wgpu::Buffer,
    context_group: wgpu::BindGroup,
    lines: Option<wgpu::Buffer>,
    line_count: u32,
    section: Option<Arc<Section>>,
    section_scope: Option<OccurrenceId>,
}

pub struct Input<'a> {
    pub project_id: Option<spiling_contracts::project::ProjectId>,
    pub plate: Option<plate::Plate>,
    pub layout_preview: bool,
    pub scene: Option<Arc<Scene>>,
    pub section: Option<Arc<Section>>,
    pub review: Option<Arc<crate::manufacture::Review>>,
    pub view: crate::replay::View,
    pub selected: Option<&'a FaceRef>,
    pub interactive: bool,
    pub analysis: &'a mut crate::analysis::SectionView,
    pub plane: PlaneMm,
    pub scope: Option<OccurrenceId>,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct CameraState {
    yaw: f64,
    pitch: f64,
    distance: f64,
    pan: [f64; 3],
    orthographic: bool,
}
impl CameraState {
    pub fn valid(&self) -> bool {
        self.yaw.is_finite()
            && self.pitch.is_finite()
            && self.distance.is_finite()
            && self.distance > 0.0
            && self.pan.iter().all(|v| v.is_finite())
    }
}
pub struct Viewport {
    yaw: f64,
    pitch: f64,
    distance: f64,
    pan: DVec3,
    fit: bool,
    had_source: bool,
    had_plate: bool,
    camera_restored: bool,
    project_id: Option<spiling_contracts::project::ProjectId>,
    show_height: bool,
    orthographic: bool,
}
impl Default for Viewport {
    fn default() -> Self {
        Self {
            yaw: -0.8,
            pitch: 0.7,
            distance: 100.0,
            pan: DVec3::ZERO,
            fit: true,
            had_source: false,
            had_plate: false,
            camera_restored: false,
            project_id: None,
            show_height: false,
            orthographic: false,
        }
    }
}
impl Viewport {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let state = cc.wgpu_render_state.as_ref().expect("native wgpu renderer");
        let device = &state.device;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Spiling source layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Spiling native source"),
            source: wgpu::ShaderSource::Wgsl(include_str!("source.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Spiling source pipeline"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Spiling source"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(state.target_format.into())],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: egui_wgpu::depth_format_from_bits(24, 8).expect("depth format"),
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Spiling camera"),
            size: std::mem::size_of::<Uniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let line_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native overlay"),
            source: wgpu::ShaderSource::Wgsl(include_str!("lines.wgsl").into()),
        });
        let line_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay camera"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let line_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay pipeline"),
            bind_group_layouts: &[Some(&line_layout)],
            immediate_size: 0,
        });
        let line_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("native section lines"),
            layout: Some(&line_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &line_shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 24,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x3],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &line_shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: state.target_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: egui_wgpu::depth_format_from_bits(24, 8).expect("depth format"),
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let bead_pipeline =
            beads::pipeline(device, &line_pipeline_layout, state.target_format, false);
        let bead_stencil =
            beads::pipeline(device, &line_pipeline_layout, state.target_format, true);
        let source_stencil =
            section::source_stencil(device, &pipeline_layout, &shader, state.target_format);
        let source_cap = section::cap(device, &line_pipeline_layout, state.target_format, true);
        let actual_cap = section::cap(device, &line_pipeline_layout, state.target_format, false);
        let clear_stencil = section::clear(device, &line_pipeline_layout, state.target_format);
        let points = |entry, label| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&line_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &line_shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: 24,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x3],
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &line_shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(state.target_format.into())],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: egui_wgpu::depth_format_from_bits(24, 8).expect("depth format"),
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::Always),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let point_pipeline = points("vs_point", "stationary actual-program events");
        let seam_pipeline = points("vs_seam", "closed actual wall starts");
        let selected_seam_pipeline = points("vs_selected_seam", "selected actual wall start");
        let line_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay camera"),
            layout: &line_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let context_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Other-layer context camera"),
            size: std::mem::size_of::<Uniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let context_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Other-layer context"),
            layout: &line_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: context_uniform.as_entire_binding(),
            }],
        });
        let tool_uniforms = std::array::from_fn(|_| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Declared tool tint camera"),
                size: std::mem::size_of::<Uniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        let tool_groups = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Declared tool tint"),
                layout: &line_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: tool_uniforms[i].as_entire_binding(),
                }],
            })
        });
        let plate = plate::Renderer::new(device, state.target_format, &line_pipeline_layout);
        let overview = overview::Renderer::new(
            device,
            state.target_format,
            &line_pipeline_layout,
            line_layout.clone(),
            line_pipeline.clone(),
        );
        state.renderer.write().callback_resources.insert(Resources {
            meshes: HashMap::new(),
            overview,
            plate,
            replay: None,
            replay_buffers: Vec::new(),
            bead_buffers: Vec::new(),
            bead_pipeline,
            source_stencil,
            bead_stencil,
            source_cap,
            actual_cap,
            clear_stencil,
            point_pipeline,
            seam_pipeline,
            selected_seam_pipeline,
            tool_uniforms,
            tool_groups,
            pipeline,
            layout,
            uniform,
            scene: None,
            draws: Vec::new(),
            failure: None,
            line_pipeline,
            line_group,
            context_uniform,
            context_group,
            lines: None,
            line_count: 0,
            section: None,
            section_scope: None,
        });
        Self::default()
    }
    pub fn camera(&self) -> CameraState {
        CameraState {
            yaw: self.yaw,
            pitch: self.pitch,
            distance: self.distance,
            pan: self.pan.to_array(),
            orthographic: self.orthographic,
        }
    }
    pub fn restore_camera(&mut self, c: &CameraState) {
        if c.valid() {
            self.yaw = c.yaw;
            self.pitch = c.pitch;
            self.distance = c.distance;
            self.pan = DVec3::from_array(c.pan);
            self.orthographic = c.orthographic;
            self.fit = false;
            self.camera_restored = true;
        }
    }
    pub fn fit(&mut self) {
        self.camera_restored = false;
        self.fit = true;
    }
    pub fn normal_to(&mut self, normal: DVec3) {
        let n = normal.normalize();
        self.yaw = n.y.atan2(n.x);
        self.pitch = n.z.asin();
        self.orthographic = true;
    }
    fn matrix(&self, aspect: f64, radius: f64) -> DMat4 {
        let direction = DVec3::new(
            self.yaw.cos() * self.pitch.cos(),
            self.yaw.sin() * self.pitch.cos(),
            self.pitch.sin(),
        );
        let eye = self.pan + direction * self.distance;
        // Keep depth precision tied to the view, including when distant lifecycle
        // moves enlarge replay bounds. A near plane almost at zero makes fine
        // deposited layers fight for the same depth values with stencil formats.
        let extent = radius + self.pan.length();
        let near = (self.distance - extent * 1.05)
            .max(self.distance * 0.005)
            .max(0.00001);
        let far = (self.distance + extent * 2.0).max(near * 2.0);
        let projection = if self.orthographic {
            let half = self.distance * (22.5f64.to_radians()).tan();
            glam::dcamera::rh::proj::directx::orthographic(
                -half * aspect,
                half * aspect,
                -half,
                half,
                near,
                far,
            )
        } else {
            glam::dcamera::rh::proj::directx::perspective(45f64.to_radians(), aspect, near, far)
        };
        let up = if direction.z.abs() > 0.999 {
            DVec3::Y
        } else {
            DVec3::Z
        };
        projection * glam::dcamera::rh::view::look_at_mat4(eye, self.pan, up)
    }
    /// Camera and picking use f64; GPU positions are rebased about native scene bounds.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        input: Input<'_>,
        frame: &eframe::Frame,
    ) -> Option<FaceRef> {
        let Input {
            project_id,
            plate,
            layout_preview,
            scene,
            section,
            review,
            view,
            selected,
            interactive,
            analysis,
            mut plane,
            scope,
        } = input;
        if self.project_id != project_id {
            self.project_id = project_id;
            self.fit = true;
            self.pan = DVec3::ZERO;
            self.had_source = false;
            self.had_plate = false;
        }
        let mut hit = None;
        if layout_preview {
            ui.label("Layout preview · not applied");
        }
        let mut fit_source = false;
        let mut fit_plate = false;
        ui.horizontal_wrapped(|ui| {
            if ui.button("Fit").clicked() {
                self.fit = true;
            }
            if ui
                .add_enabled(
                    scene
                        .as_ref()
                        .is_some_and(|s| s.summary.bounds_mm.is_some()),
                    egui::Button::new("Fit parts"),
                )
                .clicked()
            {
                self.fit = true;
                fit_source = true;
            }
            if ui.button("Isometric").clicked() {
                self.orthographic = false;
                self.yaw = -0.8;
                self.pitch = 0.7;
            }
            if ui.button("Top").clicked() {
                self.pitch = 1.56;
                self.yaw = -std::f64::consts::FRAC_PI_2;
            }
            ui.checkbox(&mut self.orthographic, "Orthographic");
            if analysis.enabled && ui.button("Normal to section").clicked() {
                self.normal_to(
                    DVec3::from_array(plane.normal) * if analysis.flipped { -1.0 } else { 1.0 },
                );
            }
            ui.weak("Drag: orbit · right-drag: pan · wheel: zoom");
        });
        if let Some(plate) = &plate
            && plate.controls(ui, &mut self.show_height)
        {
            self.fit = true;
            fit_plate = true;
        }
        let (rect, response) = ui.allocate_exact_size(
            ui.available_size().max(egui::vec2(1.0, 1.0)),
            egui::Sense::click_and_drag(),
        );
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Canvas, true, "3D viewport. Arrows orbit; Shift arrows pan; plus/minus zoom; F fits; 1 top; 0 isometric. Native faces are also available in the model tree."));
        if response.clicked() {
            response.request_focus();
        }
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect.shrink(1.0),
                0.0,
                ui.visuals().selection.stroke,
                egui::StrokeKind::Inside,
            );
            let (delta, shift, zoom, fit, top, iso) = ui.input_mut(|i| {
                let delta = egui::vec2(
                    f32::from(i.key_pressed(egui::Key::ArrowRight) as u8)
                        - f32::from(i.key_pressed(egui::Key::ArrowLeft) as u8),
                    f32::from(i.key_pressed(egui::Key::ArrowUp) as u8)
                        - f32::from(i.key_pressed(egui::Key::ArrowDown) as u8),
                );
                (
                    delta,
                    i.modifiers.shift,
                    f64::from(i.key_pressed(egui::Key::Minus) as u8)
                        - f64::from(i.key_pressed(egui::Key::Plus) as u8),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::F),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Num1),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Num0),
                )
            });
            if shift {
                let right = DVec3::new(-self.yaw.sin(), self.yaw.cos(), 0.0);
                let up = right.cross(DVec3::new(
                    self.yaw.cos() * self.pitch.cos(),
                    self.yaw.sin() * self.pitch.cos(),
                    self.pitch.sin(),
                ));
                self.pan +=
                    (right * f64::from(delta.x) + up * f64::from(delta.y)) * self.distance * 0.03;
            } else {
                self.yaw += f64::from(delta.x) * 0.1;
                self.pitch = (self.pitch + f64::from(delta.y) * 0.1).clamp(-1.55, 1.55);
            }
            self.distance *= (zoom * 0.1).exp();
            if fit {
                self.fit = true;
            }
            if top {
                self.pitch = 1.56;
                self.yaw = -std::f64::consts::FRAC_PI_2;
            }
            if iso {
                self.pitch = 0.7;
                self.yaw = -0.8;
            }
        }
        let bounds = scene.as_ref().and_then(|s| s.summary.bounds_mm);
        let plate_bounds = plate
            .as_ref()
            .filter(|p| p.usable())
            .map(plate::Plate::bounds);
        let center = bounds.map_or_else(
            || {
                review
                    .as_ref()
                    .and_then(|r| r.display.as_ref())
                    .map_or_else(
                        || {
                            plate_bounds.map_or(DVec3::ZERO, |b| {
                                (DVec3::from_array(b.min) + DVec3::from_array(b.max)) * 0.5
                            })
                        },
                        |r| r.origin,
                    )
            },
            |b| (DVec3::from_array(b.min) + DVec3::from_array(b.max)) * 0.5,
        );
        let mut radius = bounds
            .map_or(1.0, |b| {
                (DVec3::from_array(b.max) - DVec3::from_array(b.min)).length() * 0.5
            })
            .max(0.01);
        let replay_bounds = review
            .as_ref()
            .filter(|_| view.visible)
            .and_then(|r| r.capture.replay.as_ref())
            .map(|r| r.info.bounds_mm);
        let mut display_radius = replay_bounds.map_or(radius, |b| {
            let replay_center = (DVec3::from_array(b.min) + DVec3::from_array(b.max)) * 0.5;
            let replay_radius =
                (DVec3::from_array(b.max) - DVec3::from_array(b.min)).length() * 0.5;
            radius.max((replay_center - center).length() + replay_radius)
        });
        let mut fit_pan = DVec3::ZERO;
        if let Some(b) = replay_bounds
            && !fit_source
        {
            let replay_center = (DVec3::from_array(b.min) + DVec3::from_array(b.max)) * 0.5;
            let replay_radius =
                (DVec3::from_array(b.max) - DVec3::from_array(b.min)).length() * 0.5;
            if view.source {
                radius = radius.max((replay_center - center).length() + replay_radius);
            } else {
                radius = replay_radius.max(0.01);
                fit_pan = replay_center - center;
            }
        }
        if let Some(b) = plate_bounds {
            let c = (DVec3::from_array(b.min) + DVec3::from_array(b.max)) * 0.5;
            let r = (DVec3::from_array(b.max) - DVec3::from_array(b.min)).length() * 0.5;
            display_radius = display_radius.max((c - center).length() + r);
            if fit_plate || (bounds.is_none() && replay_bounds.is_none()) {
                radius = r;
                fit_pan = c - center;
            } else if !fit_source {
                radius = display_radius;
            }
            if self.show_height {
                let envelope = plate.as_ref().expect("plate bounds").envelope();
                display_radius = display_radius
                    .max((DVec3::from_array(envelope.max) - center).length())
                    .max((DVec3::from_array(envelope.min) - center).length());
                if !fit_plate && !fit_source {
                    radius = display_radius;
                }
            }
        }
        if !self.camera_restored
            && ((bounds.is_some() && !self.had_source) || (plate.is_some() && !self.had_plate))
        {
            self.fit = true;
        }
        self.had_source = bounds.is_some();
        self.had_plate = plate.is_some();
        if bounds.is_some() || plate.is_some() {
            self.camera_restored = false;
        }
        if self.fit && (bounds.is_some() || plate.is_some() || replay_bounds.is_some()) {
            let aspect = f64::from(rect.width() / rect.height());
            self.distance = radius * 3.2 / aspect.min(1.0);
            self.pan = fit_pan;
            self.fit = false;
        }
        // A separate handle moves the inspection plane; dragging the surrounding canvas orbits.
        let mut handle = None;
        if analysis.enabled {
            let matrix = self.matrix(f64::from(rect.width() / rect.height()), display_radius);
            let n = DVec3::from_array(plane.normal);
            let point = n * n.dot(DVec3::from_array(plane.origin_mm) - center);
            let screen = |p: DVec3| {
                let p = matrix.project_point3(p);
                rect.center()
                    + egui::vec2(
                        p.x as f32 * rect.width() * 0.5,
                        -p.y as f32 * rect.height() * 0.5,
                    )
            };
            let position = screen(point);
            if position.is_finite() && rect.shrink(14.0).contains(position) {
                let r = ui
                    .interact(
                        egui::Rect::from_center_size(position, egui::vec2(26.0, 26.0)),
                        response.id.with("section-plane"),
                        egui::Sense::drag(),
                    )
                    .on_hover_text("Drag to move section plane in millimetres");
                if r.dragged() {
                    let axis = screen(point + n * radius * 0.25) - position;
                    let mm_per_pixel = if axis.length_sq() > 4.0 {
                        axis * (radius as f32 * 0.25 / axis.length_sq())
                    } else {
                        egui::vec2(0.0, -radius as f32 / rect.height())
                    };
                    let delta = f64::from(r.drag_motion().dot(mm_per_pixel));
                    analysis.offset_mm += delta;
                    plane.origin_mm = (DVec3::from_array(plane.origin_mm) + n * delta).to_array();
                }
                handle = Some((position, r.dragged()));
            }
        }
        if response.dragged_by(egui::PointerButton::Primary)
            && !handle.is_some_and(|(_, dragging)| dragging)
        {
            let d = response.drag_motion();
            self.yaw -= f64::from(d.x) * 0.007;
            self.pitch = (self.pitch + f64::from(d.y) * 0.007).clamp(-1.55, 1.55);
        }
        if response.dragged_by(egui::PointerButton::Secondary) {
            let d = response.drag_motion();
            let right = DVec3::new(-self.yaw.sin(), self.yaw.cos(), 0.0);
            let up = right.cross(DVec3::new(
                self.yaw.cos() * self.pitch.cos(),
                self.yaw.sin() * self.pitch.cos(),
                self.pitch.sin(),
            ));
            self.pan += (-right * f64::from(d.x) + up * f64::from(d.y)) * self.distance
                / f64::from(rect.height());
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            self.distance = (self.distance * (-f64::from(scroll) * 0.002).exp())
                .clamp(radius * 0.02, radius * 1000.0);
        }
        self.distance = self.distance.clamp(radius * 0.02, radius * 1000.0);
        let matrix = self.matrix(f64::from(rect.width() / rect.height()), display_radius);
        if interactive
            && view.source
            && response.clicked()
            && let (Some(scene), Some(pointer)) = (&scene, response.interact_pointer_pos())
        {
            let x = 2.0 * f64::from((pointer.x - rect.left()) / rect.width()) - 1.0;
            let y = 1.0 - 2.0 * f64::from((pointer.y - rect.top()) / rect.height());
            let inverse = matrix.inverse();
            let near = inverse.project_point3(DVec3::new(x, y, 0.0));
            let far = inverse.project_point3(DVec3::new(x, y, 1.0));
            hit = pick_clipped(
                scene,
                center + near,
                (far - near).normalize(),
                analysis,
                plane,
                scope,
            );
        }
        let clip =
            crate::replay::seams::SectionClip::project(analysis, plane, center, display_radius);
        let projected = clip.plane;
        let cut_enabled = clip.enabled;
        if analysis.enabled && !cut_enabled {
            ui.painter().text(
                rect.left_top() + egui::vec2(12.0, 12.0),
                egui::Align2::LEFT_TOP,
                "Section exceeds 0.001 mm display precision; cut disabled.",
                egui::FontId::proportional(13.0),
                ui.visuals().warn_fg_color,
            );
            hit = None;
        }
        let selected_scope = if analysis.enabled && analysis.selected_only {
            scope
                .and_then(|id| {
                    scene
                        .as_ref()?
                        .occurrences
                        .iter()
                        .position(|o| o.occurrence_id == id)
                })
                .map_or(0.0, |i| i as f32 + 1.0)
        } else {
            0.0
        };
        let mut selection = [u32::MAX, u32::MAX, 0, 0];
        if let (Some(scene), Some(selected)) = (&scene, selected)
            && selected.scene_revision == scene.summary.revision
            && selected.session_id == scene.summary.session_id
        {
            if let Some(i) = scene
                .occurrences
                .iter()
                .position(|o| o.occurrence_id == selected.occurrence_id)
            {
                selection[1] = i as u32;
            }
            if let Some(d) = scene.definition(&selected.definition_id)
                && let Some(i) = d.faces.iter().position(|f| f.face_id == selected.face_id)
            {
                selection[0] = i as u32;
            }
        }
        ui.painter().add(egui_wgpu::Callback::new_paint_callback(
            rect,
            Callback {
                plate: plate.clone(),
                plate_origin: center,
                dark: ui.visuals().dark_mode,
                show_height: self.show_height,
                scene,
                section,
                review,
                view,
                context_color: if ui.visuals().dark_mode {
                    [0.55, 0.62, 0.71, 0.16]
                } else {
                    [0.21, 0.29, 0.39, 0.14]
                },
                uniform: Uniform {
                    matrix: matrix.as_mat4().to_cols_array_2d(),
                    selected: [
                        selection[0],
                        selection[1],
                        (rect.width() * ui.ctx().pixels_per_point()) as u32,
                        (rect.height() * ui.ctx().pixels_per_point()) as u32,
                    ],
                    tool_color: [0.0; 4],
                    context: if layout_preview {
                        [0.36, 0.72, 0.77, 1.0]
                    } else {
                        [0.0; 4]
                    },
                    plane: if cut_enabled {
                        projected
                    } else {
                        [0.0, 0.0, 1.0, 0.0]
                    },
                    cut: [
                        cut_enabled as u8 as f32,
                        (cut_enabled && analysis.section_only) as u8 as f32,
                        selected_scope,
                        (display_radius * 4.0) as f32,
                    ],
                },
            },
        ));
        if let Some((position, dragging)) = handle {
            let painter = ui.painter();
            painter.circle_filled(position, 10.0, ui.visuals().extreme_bg_color);
            painter.circle_stroke(
                position,
                10.0,
                egui::Stroke::new(2.0, ui.visuals().selection.stroke.color),
            );
            painter.line_segment(
                [
                    position - egui::vec2(0.0, 5.0),
                    position + egui::vec2(0.0, 5.0),
                ],
                egui::Stroke::new(1.5, ui.visuals().selection.stroke.color),
            );
            if dragging {
                painter.text(
                    position + egui::vec2(16.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    format!("{:.3} mm", analysis.offset_mm),
                    egui::FontId::proportional(12.0),
                    ui.visuals().text_color(),
                );
            }
        }
        if let Some(state) = frame.wgpu_render_state()
            && let Some(resources) = state.renderer.read().callback_resources.get::<Resources>()
            && let Some(error) = resources.plate.error
        {
            ui.painter().text(
                rect.left_top() + egui::vec2(12.0, 36.0),
                egui::Align2::LEFT_TOP,
                error,
                egui::FontId::proportional(13.0),
                ui.visuals().warn_fg_color,
            );
        }
        if let Some(state) = frame.wgpu_render_state()
            && let Some(resources) = state.renderer.read().callback_resources.get::<Resources>()
            && let Some(error) = &resources.failure
        {
            hit = None;
            ui.painter().text(
                rect.left_top() + egui::vec2(12.0, 12.0),
                egui::Align2::LEFT_TOP,
                error,
                egui::FontId::proportional(14.0),
                egui::Color32::LIGHT_RED,
            );
        }
        hit
    }
}
struct Callback {
    plate: Option<plate::Plate>,
    plate_origin: DVec3,
    dark: bool,
    show_height: bool,
    scene: Option<Arc<Scene>>,
    section: Option<Arc<Section>>,
    review: Option<Arc<crate::manufacture::Review>>,
    view: crate::replay::View,
    context_color: [f32; 4],
    uniform: Uniform,
}
impl egui_wgpu::CallbackTrait for Callback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(r) = resources.get_mut::<Resources>() else {
            return vec![];
        };
        r.overview.clear();
        if self.scene.is_none() {
            r.meshes.clear();
        }
        r.plate
            .prepare(device, self.plate.as_ref(), self.plate_origin, self.dark);
        let changed = match (&r.scene, &self.scene) {
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            (None, None) => false,
            _ => true,
        };
        if changed {
            match r.stage(device, self.scene.clone()) {
                Ok(draws) => {
                    r.draws = draws;
                    r.scene = self.scene.clone();
                    r.failure = None;
                }
                Err(error) => {
                    r.draws.clear();
                    r.scene = None;
                    r.failure = Some(error);
                }
            }
        }
        let section_changed = match (&r.section, &self.section) {
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            (None, None) => false,
            _ => true,
        };
        let section_scope = if self.uniform.cut[2] > 0.0 {
            self.scene
                .as_ref()
                .and_then(|s| s.occurrences.get((self.uniform.cut[2] - 1.0) as usize))
                .map(|o| o.occurrence_id)
        } else {
            None
        };
        if changed || section_changed || section_scope != r.section_scope {
            match r.stage_section(device, self.section.as_ref(), section_scope) {
                Ok((buffer, count)) => {
                    r.lines = buffer;
                    r.line_count = count;
                    r.section = self.section.clone();
                    r.section_scope = section_scope;
                }
                Err(error) => {
                    r.lines = None;
                    r.line_count = 0;
                    r.failure = Some(error);
                }
            }
        }
        let replay_changed = match (&r.replay, &self.review) {
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            (None, None) => false,
            _ => true,
        };
        if replay_changed {
            r.replay_buffers.clear();
            r.bead_buffers.clear();
            if let Some(display) = self.review.as_ref().and_then(|r| r.display.as_ref()) {
                for batch in &display.batches {
                    r.replay_buffers.push(device.create_buffer_init(
                        &wgpu::util::BufferInitDescriptor {
                            label: Some("checked SPLR6 actual events"),
                            contents: bytemuck::cast_slice(&batch.vertices),
                            usage: wgpu::BufferUsages::VERTEX,
                        },
                    ));
                }
            }
            if let Some(beads) = self
                .review
                .as_ref()
                .and_then(|review| review.display.as_ref())
                .and_then(|display| display.beads.as_ref().ok())
            {
                for batch in &beads.batches {
                    r.bead_buffers.push(device.create_buffer_init(
                        &wgpu::util::BufferInitDescriptor {
                            label: Some("Checked nominal bead instances"),
                            contents: bytemuck::cast_slice(&batch.instances),
                            usage: wgpu::BufferUsages::VERTEX,
                        },
                    ));
                }
            }
            r.replay = self.review.clone();
        }
        queue.write_buffer(&r.uniform, 0, bytemuck::bytes_of(&self.uniform));
        queue.write_buffer(
            &r.context_uniform,
            0,
            bytemuck::bytes_of(&Uniform {
                context: self.context_color,
                ..self.uniform
            }),
        );
        let colors = self
            .review
            .as_ref()
            .filter(|_| self.view.material)
            .and_then(|review| {
                crate::replay::declared_tool_colors(&review.capture.provenance.workflow)
            });
        for (i, buffer) in r.tool_uniforms.iter().enumerate() {
            let uniform = Uniform {
                tool_color: colors.map_or([0.0; 4], |c| c[i]),
                ..self.uniform
            };
            queue.write_buffer(buffer, 0, bytemuck::bytes_of(&uniform));
        }
        vec![]
    }
    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(r) = resources.get::<Resources>() else {
            return;
        };
        r.plate.paint(pass, &r.line_group, self.show_height);
        pass.set_pipeline(&r.pipeline);
        for draw in r
            .draws
            .iter()
            .filter(|_| self.view.source && self.uniform.cut[1] == 0.0)
        {
            pass.set_bind_group(0, &draw.group, &[]);
            pass.draw(0..draw.indices, 0..draw.instances);
        }
        if self.uniform.cut[0] > 0.0 && self.view.source {
            pass.set_stencil_reference(0);
            // All mesh chunks of one occurrence contribute to the same parity.
            // Resolve coincident occurrences separately so their holes cannot cancel.
            for chunks in r.draws.chunk_by(|a, b| a.definition == b.definition) {
                for instance in 0..chunks[0].instances {
                    pass.set_pipeline(&r.clear_stencil);
                    pass.set_bind_group(0, &r.line_group, &[]);
                    pass.draw(0..3, 0..1);
                    pass.set_pipeline(&r.source_stencil);
                    for draw in chunks {
                        pass.set_bind_group(0, &draw.group, &[]);
                        pass.draw(0..draw.indices, instance..instance + 1);
                    }
                    pass.set_pipeline(&r.source_cap);
                    pass.set_bind_group(0, &r.line_group, &[]);
                    pass.draw(0..6, 0..1);
                }
            }
        }
        if self.view.source
            && let Some(lines) = &r.lines
        {
            pass.set_pipeline(&r.line_pipeline);
            pass.set_bind_group(0, &r.line_group, &[]);
            pass.set_vertex_buffer(0, lines.slice(..));
            pass.draw(0..r.line_count, 0..1);
        }
        if let Some(display) = r.replay.as_ref().and_then(|r| r.display.as_ref()) {
            // Draw neutral context first without depth writes. Selected beads/paths
            // remain foreground; no statistics or caps include unselected paths.
            // Section-only hides them too, and normal cuts still clip the lines.
            if self.uniform.cut[1] == 0.0 {
                pass.set_pipeline(&r.line_pipeline);
                pass.set_bind_group(0, &r.context_group, &[]);
                for (batch, buffer) in display.batches.iter().zip(&r.replay_buffers) {
                    for range in batch.context_ranges(self.view) {
                        if !range.is_empty() {
                            pass.set_vertex_buffer(0, buffer.slice(..));
                            pass.draw(range, 0..1);
                        }
                    }
                }
            }
            let beads = display.beads.as_ref().ok().filter(|_| self.view.beads);
            if let Some(beads) = beads {
                pass.set_pipeline(&r.bead_pipeline);
                for (batch, buffer) in beads
                    .batches
                    .iter()
                    .zip(&r.bead_buffers)
                    .filter(|_| self.uniform.cut[1] == 0.0)
                {
                    let range = batch.range(self.view);
                    if range.is_empty() {
                        continue;
                    }
                    pass.set_bind_group(0, &r.tool_groups[batch.tool as usize], &[]);
                    pass.set_vertex_buffer(0, buffer.slice(..));
                    pass.draw(0..beads.vertices_per_instance(), range);
                }
            }
            if self.uniform.cut[0] > 0.0
                && let Some(beads) = beads
            {
                for tool in 0..2 {
                    pass.set_pipeline(&r.clear_stencil);
                    pass.set_bind_group(0, &r.line_group, &[]);
                    pass.draw(0..3, 0..1);
                    pass.set_stencil_reference(0);
                    pass.set_pipeline(&r.bead_stencil);
                    pass.set_bind_group(0, &r.tool_groups[tool], &[]);
                    for (batch, buffer) in beads
                        .batches
                        .iter()
                        .zip(&r.bead_buffers)
                        .filter(|(b, _)| b.tool as usize == tool)
                    {
                        let range = batch.range(self.view);
                        if range.is_empty() {
                            continue;
                        }
                        pass.set_vertex_buffer(0, buffer.slice(..));
                        pass.draw(0..beads.vertices_per_instance(), range);
                    }
                    pass.set_pipeline(&r.actual_cap);
                    pass.draw(0..6, 0..1);
                }
            }
            for (batch, buffer) in display
                .batches
                .iter()
                .zip(&r.replay_buffers)
                .filter(|_| self.uniform.cut[1] == 0.0)
            {
                if !batch.seam
                    && beads.is_some_and(|beads| {
                        beads
                            .batches
                            .iter()
                            .any(|b| b.kind == batch.kind && b.tool == batch.tool)
                    })
                {
                    continue;
                }
                let range = batch.range(self.view);
                if range.is_empty() {
                    continue;
                }
                pass.set_pipeline(if batch.seam {
                    &r.seam_pipeline
                } else if batch.points {
                    &r.point_pipeline
                } else {
                    &r.line_pipeline
                });
                let deposition = matches!(batch.kind,1..=5|8..=10|13|15);
                pass.set_bind_group(
                    0,
                    if deposition && !batch.seam {
                        &r.tool_groups[batch.tool as usize]
                    } else {
                        &r.line_group
                    },
                    &[],
                );
                pass.set_vertex_buffer(0, buffer.slice(..));
                if batch.points {
                    pass.draw(0..6, range.clone());
                    if batch.seam
                        && let Some(event) = self.view.selected_seam
                        && let Ok(index) = batch
                            .events
                            .binary_search_by_key(&event, |(ordinal, _)| *ordinal)
                        && range.contains(&(index as u32))
                    {
                        pass.set_pipeline(&r.selected_seam_pipeline);
                        pass.draw(0..6, index as u32..index as u32 + 1);
                    }
                } else {
                    pass.draw(range, 0..1);
                }
            }
        }
        if self.uniform.cut[0] > 0.0 {
            pass.set_pipeline(&r.clear_stencil);
            pass.set_bind_group(0, &r.line_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}
impl Resources {
    fn retain_meshes(&mut self, scene: &Scene) {
        let hashes: std::collections::HashSet<_> = scene
            .context_definitions
            .iter()
            .flat_map(|d| d.chunks.iter().map(|c| &c.metadata.sha256))
            .collect();
        self.meshes.retain(|hash, _| hashes.contains(hash));
    }
    fn mesh_buffer(
        &mut self,
        device: &wgpu::Device,
        chunk: &crate::scene::Chunk,
    ) -> Arc<wgpu::Buffer> {
        self.meshes
            .entry(chunk.metadata.sha256.clone())
            .or_insert_with(|| {
                Arc::new(
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("Shared checked SPLM"),
                        contents: &chunk.bytes,
                        usage: wgpu::BufferUsages::STORAGE,
                    }),
                )
            })
            .clone()
    }

    fn stage_section(
        &self,
        device: &wgpu::Device,
        section: Option<&Arc<Section>>,
        scope: Option<OccurrenceId>,
    ) -> Result<(Option<wgpu::Buffer>, u32), String> {
        let (Some(section), Some(scene)) = (section, &self.scene) else {
            return Ok((None, 0));
        };
        if section.summary.session_id != scene.summary.session_id
            || section.summary.revision != scene.summary.revision
        {
            return Err("Section is stale".into());
        }
        let points = section
            .loops
            .iter()
            .map(|l| l.point_count as usize)
            .sum::<usize>();
        let old = self.section.as_ref().map_or(0, |s| {
            s.summary.total_bytes as usize * crate::scene::SECTION_DISPLAY_MULTIPLIER
        });
        if scene.raw_bytes * 2 + old + section.summary.total_bytes as usize + points * 2 * 24 * 2
            > MAX_DISPLAY_BUFFER_BYTES as usize
        {
            return Err("Active/staged section display exceeds CPU/GPU capacity".into());
        }
        let center = scene.summary.bounds_mm.map_or(DVec3::ZERO, |b| {
            (DVec3::from_array(b.min) + DVec3::from_array(b.max)) * 0.5
        });
        let mut vertices = Vec::with_capacity(points * 2);
        for chunk in &section.chunks {
            let view = spiling_contracts::display::validate_section_chunk(
                &chunk.bytes,
                &chunk.metadata,
                &section.summary,
                &scene.summary.session_id,
                section.summary.artifact_id,
                chunk.metadata.chunk_index,
            )
            .map_err(|e| e.to_string())?;
            for loop_index in 0..chunk.metadata.loop_count as usize {
                let metadata =
                    &section.loops[chunk.metadata.first_loop_ordinal as usize + loop_index];
                if scope.is_some_and(|id| id != metadata.occurrence_id) {
                    continue;
                }
                let start = view
                    .loop_offsets
                    .get(loop_index)
                    .ok_or("Section loop start missing")? as usize;
                let end = view
                    .loop_offsets
                    .get(loop_index + 1)
                    .ok_or("Section loop end missing")? as usize;
                for i in start..end {
                    for p in [i, if i + 1 == end { start } else { i + 1 }] {
                        let point = DVec3::from_array(
                            view.points_relative_mm
                                .get(p)
                                .ok_or("Section point missing")?,
                        ) + (DVec3::from_array(view.plane_origin_mm) - center);
                        let f = point.as_vec3();
                        if !point.is_finite()
                            || !f.is_finite()
                            || (f.as_dvec3() - point).length() > 0.001
                        {
                            return Err("Section projection exceeds 0.001 mm".into());
                        }
                        vertices.push(LineVertex {
                            position: f.to_array(),
                            color: [1.0, 0.72, 0.22],
                        });
                    }
                }
            }
        }
        if vertices.is_empty() {
            return Ok((None, 0));
        }
        let count = vertices.len() as u32;
        Ok((
            Some(
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("checked native section"),
                    contents: bytemuck::cast_slice(&vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                }),
            ),
            count,
        ))
    }

    fn stage(
        &mut self,
        device: &wgpu::Device,
        scene: Option<Arc<Scene>>,
    ) -> Result<Vec<Draw>, String> {
        let Some(scene) = scene else {
            return Ok(vec![]);
        };
        let center = scene.summary.bounds_mm.map_or(DVec3::ZERO, |b| {
            (DVec3::from_array(b.min) + DVec3::from_array(b.max)) * 0.5
        });
        self.retain_meshes(&scene);
        let mut draws = Vec::new();
        let mut instance_bytes = 0usize;
        for definition in &scene.definitions {
            for chunk in &definition.chunks {
                let view = validate_mesh_chunk(
                    &chunk.bytes,
                    &chunk.metadata,
                    &scene.summary.session_id,
                    chunk.metadata.artifact_id,
                    &definition.record.definition_id,
                    chunk.metadata.chunk_index,
                )
                .map_err(|e| e.to_string())?;
                let radius = view
                    .positions
                    .iter()
                    .map(|p| DVec3::from_array(p.map(f64::from)).length())
                    .fold(0.0, f64::max);
                let mut instances = Vec::new();
                for (index, occurrence) in scene
                    .occurrences
                    .iter()
                    .enumerate()
                    .filter(|(_, o)| o.definition_id == definition.record.definition_id)
                {
                    let q = DQuat::from_array(occurrence.pose.rotation_xyzw);
                    let translated = q * DVec3::from_array(view.local_origin_mm)
                        + DVec3::from_array(occurrence.pose.translation_mm)
                        - center;
                    let f = translated.as_vec3();
                    let rounded = q.as_quat().as_dquat();
                    let rotation_error = [DVec3::X, DVec3::Y, DVec3::Z]
                        .iter()
                        .map(|v| ((rounded * *v) - (q * *v)).length_squared())
                        .sum::<f64>()
                        .sqrt()
                        * radius;
                    if rotation_error + (f.as_dvec3() - translated).length() > 0.001 {
                        return Err("Display instance projection exceeds 0.001 mm; native geometry remains available".into());
                    }
                    instances.push(Instance {
                        rotation: q.as_quat().to_array(),
                        translation: [f.x, f.y, f.z, index as f32],
                    });
                }
                instance_bytes = instance_bytes
                    .checked_add(instances.len() * std::mem::size_of::<Instance>())
                    .filter(|n| *n <= crate::scene::MAX_INSTANCE_BUFFER_BYTES)
                    .ok_or("Source instance buffers exceed the reserved display pool")?;
                let raw = self.mesh_buffer(device, chunk);
                let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("native shared occurrences"),
                    contents: bytemuck::cast_slice(&instances),
                    usage: wgpu::BufferUsages::STORAGE,
                });
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("source draw"),
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: self.uniform.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: raw.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: buffer.as_entire_binding(),
                        },
                    ],
                });
                draws.push(Draw {
                    definition: definition.record.definition_id.clone(),
                    group,
                    _instances: buffer,
                    indices: view.indices.len() as u32,
                    instances: instances.len() as u32,
                });
            }
        }
        Ok(draws)
    }
}
#[cfg(test)]
pub(crate) fn pick(scene: &Scene, origin: DVec3, direction: DVec3) -> Option<FaceRef> {
    pick_clipped(
        scene,
        origin,
        direction,
        &crate::analysis::SectionView::default(),
        PlaneMm {
            origin_mm: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
        },
        None,
    )
}
pub(crate) fn pick_clipped(
    scene: &Scene,
    origin: DVec3,
    direction: DVec3,
    analysis: &crate::analysis::SectionView,
    plane: PlaneMm,
    scope: Option<OccurrenceId>,
) -> Option<FaceRef> {
    let mut nearest = f64::INFINITY;
    let mut selected = None;
    if analysis.enabled && analysis.section_only {
        return None;
    }
    let mut cap_blocks = false;
    let denominator = DVec3::from_array(plane.normal).dot(direction);
    let plane_distance = if denominator.abs() > 1e-12 {
        DVec3::from_array(plane.normal).dot(DVec3::from_array(plane.origin_mm) - origin)
            / denominator
    } else {
        f64::INFINITY
    };
    for occurrence in &scene.occurrences {
        let mut cap_crossings = Vec::new();
        if analysis.enabled && analysis.selected_only && scope != Some(occurrence.occurrence_id) {
            continue;
        }
        let Some(definition) = scene.definition(&occurrence.definition_id) else {
            continue;
        };
        let inverse = DQuat::from_array(occurrence.pose.rotation_xyzw).conjugate();
        let source_origin = inverse * (origin - DVec3::from_array(occurrence.pose.translation_mm));
        let source_direction = inverse * direction;
        for chunk in &definition.chunks {
            let Ok(view) = validate_mesh_chunk(
                &chunk.bytes,
                &chunk.metadata,
                &scene.summary.session_id,
                chunk.metadata.artifact_id,
                &definition.record.definition_id,
                chunk.metadata.chunk_index,
            ) else {
                continue;
            };
            let ray = source_origin - DVec3::from_array(view.local_origin_mm);
            for triangle in 0..view.face_ordinals.len() {
                let vertex = |i| {
                    DVec3::from_array(view.positions.get(view.indices.get(triangle*3+i).expect("checked index") as usize).expect("checked vertex").map(f64::from))
                };
                let a = vertex(0);
                let e1 = vertex(1) - a;
                let e2 = vertex(2) - a;
                let p = source_direction.cross(e2);
                let determinant = e1.dot(p);
                if determinant.abs() < e1.length() * e2.length() * 1e-12 {
                    continue;
                }
                let inverse_det = 1.0 / determinant;
                let delta = ray - a;
                let u = delta.dot(p) * inverse_det;
                if !(0.0..=1.0).contains(&u) {
                    continue;
                }
                let q = delta.cross(e1);
                let v = source_direction.dot(q) * inverse_det;
                if v < 0.0 || u + v > 1.0 {
                    continue;
                }
                let distance = e2.dot(q) * inverse_det;
                if analysis.enabled && !cap_blocks && distance >= 0.0 && distance < plane_distance {
                    // Bound supplementary cap occlusion work. Ambiguous caps never mint a FaceRef.
                    if cap_crossings.len() == 4096 {
                        return None;
                    }
                    cap_crossings.push(distance);
                }
                if !analysis.keep(plane, origin + direction * distance) {
                    continue;
                }
                if distance >= 0.0 && distance < nearest {
                    nearest = distance;
                    selected = scene.reference(
                        occurrence,
                        view.face_ordinals.get(triangle).expect("checked face") as usize,
                    );
                }
            }
        }
        cap_crossings.sort_unstable_by(f64::total_cmp);
        cap_crossings.dedup_by(|a, b| (*a - *b).abs() < 1e-8);
        cap_blocks |= cap_crossings.len() % 2 == 1;
    }
    if analysis.enabled
        && plane_distance > 0.0
        && plane_distance < nearest
        && !analysis.keep(plane, origin)
        && cap_blocks
    {
        return None;
    }
    selected
}

#[cfg(test)]
mod depth_tests {
    use super::*;
    #[test]
    fn distant_lifecycle_bounds_do_not_collapse_fine_deposition_depths() {
        let viewport = Viewport {
            distance: 70.0,
            yaw: 0.0,
            pitch: 0.0,
            ..Default::default()
        };
        let matrix = viewport.matrix(1.5, 200.0);
        let near = matrix.project_point3(DVec3::X * 0.005).z as f32;
        let far = matrix.project_point3(DVec3::ZERO).z as f32;
        assert!(
            far - near > 4.0 / 16_777_216.0,
            "fine layer depth needs several 24-bit steps"
        );
    }
}

#[cfg(test)]
mod shader_tests {
    use super::*;
    #[test]
    fn native_shaders_parse_and_validate_with_the_shared_camera_layout() {
        for (name, source) in [
            ("source", include_str!("source.wgsl")),
            ("lines", include_str!("lines.wgsl")),
            ("plate", include_str!("plate.wgsl")),
            ("beads", include_str!("beads.wgsl")),
            ("section", include_str!("section.wgsl")),
        ] {
            let module = wgpu::naga::front::wgsl::parse_str(source)
                .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
            wgpu::naga::valid::Validator::new(
                wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::empty(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
            for (_, global) in module.global_variables.iter() {
                if global.space == wgpu::naga::AddressSpace::Uniform {
                    let wgpu::naga::TypeInner::Struct { span, .. } = module.types[global.ty].inner
                    else {
                        panic!("{name}: camera is not a struct");
                    };
                    assert!(
                        span as usize <= size_of::<Uniform>(),
                        "{name}: camera exceeds CPU allocation"
                    );
                }
            }
        }
    }
}
