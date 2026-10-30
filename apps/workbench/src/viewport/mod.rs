// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

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
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Instance {
    rotation: [f32; 4],
    translation: [f32; 4],
}
struct Draw {
    group: wgpu::BindGroup,
    _instances: wgpu::Buffer,
    raw: Arc<wgpu::Buffer>,
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
    replay: Option<Arc<crate::manufacture::Review>>,
    replay_buffers: Vec<wgpu::Buffer>,
    point_pipeline: wgpu::RenderPipeline,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    scene: Option<Arc<Scene>>,
    draws: Vec<Draw>,
    failure: Option<String>,
    line_pipeline: wgpu::RenderPipeline,
    line_group: wgpu::BindGroup,
    lines: Option<wgpu::Buffer>,
    line_count: u32,
    section: Option<Arc<Section>>,
}

pub struct Input<'a> {
    pub scene: Option<Arc<Scene>>,
    pub section: Option<Arc<Section>>,
    pub review: Option<Arc<crate::manufacture::Review>>,
    pub view: crate::replay::View,
    pub selected: Option<&'a FaceRef>,
    pub interactive: bool,
}
pub struct Viewport {
    yaw: f64,
    pitch: f64,
    distance: f64,
    pan: DVec3,
    fit: bool,
    had_source: bool,
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
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
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
                format: egui_wgpu::depth_format_from_bits(24, 0).expect("depth format"),
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
            size: 80,
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
                visibility: wgpu::ShaderStages::VERTEX,
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
                targets: &[Some(state.target_format.into())],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: egui_wgpu::depth_format_from_bits(24, 0).expect("depth format"),
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let point_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("stationary actual-program events"),
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
                targets: &[Some(state.target_format.into())],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::PointList,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: egui_wgpu::depth_format_from_bits(24, 0).expect("depth format"),
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let line_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay camera"),
            layout: &line_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        state.renderer.write().callback_resources.insert(Resources {
            replay: None,
            replay_buffers: Vec::new(),
            point_pipeline,
            pipeline,
            layout,
            uniform,
            scene: None,
            draws: Vec::new(),
            failure: None,
            line_pipeline,
            line_group,
            lines: None,
            line_count: 0,
            section: None,
        });
        Self::default()
    }
    pub fn fit(&mut self) {
        self.fit = true;
    }
    fn matrix(&self, aspect: f64, radius: f64) -> DMat4 {
        let direction = DVec3::new(
            self.yaw.cos() * self.pitch.cos(),
            self.yaw.sin() * self.pitch.cos(),
            self.pitch.sin(),
        );
        let eye = self.pan + direction * self.distance;
        glam::dcamera::rh::proj::directx::perspective(
            45f64.to_radians(),
            aspect,
            (radius * 0.0001).max(0.0001),
            (self.distance + radius * 10.0).max(10.0),
        ) * glam::dcamera::rh::view::look_at_mat4(eye, self.pan, DVec3::Z)
    }
    /// Camera and picking use f64; GPU positions are rebased about native scene bounds.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        input: Input<'_>,
        frame: &eframe::Frame,
    ) -> Option<FaceRef> {
        let Input {
            scene,
            section,
            review,
            view,
            selected,
            interactive,
        } = input;
        let mut hit = None;
        let mut fit_source = false;
        ui.horizontal_wrapped(|ui| {
            if ui.button("Fit").clicked() {
                self.fit = true;
            }
            if ui.button("Fit source").clicked() {
                self.fit = true;
                fit_source = true;
            }
            if ui.button("Isometric").clicked() {
                self.yaw = -0.8;
                self.pitch = 0.7;
                self.fit = true;
            }
            if ui.button("Top").clicked() {
                self.pitch = 1.56;
                self.yaw = -std::f64::consts::FRAC_PI_2;
                self.fit = true;
            }
            ui.weak("Drag: orbit · right-drag: pan · wheel: zoom · click: inspect face");
        });
        let (rect, response) = ui.allocate_exact_size(
            ui.available_size().max(egui::vec2(1.0, 1.0)),
            egui::Sense::click_and_drag(),
        );
        let bounds = scene.as_ref().and_then(|s| s.summary.bounds_mm);
        let center = bounds.map_or_else(
            || {
                review
                    .as_ref()
                    .and_then(|r| r.display.as_ref())
                    .map_or(DVec3::ZERO, |r| r.origin)
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
        if bounds.is_some() && !self.had_source {
            self.fit = true;
        }
        self.had_source = bounds.is_some();
        if self.fit && bounds.is_some() {
            self.distance = radius * 3.2;
            self.pan = fit_pan;
            self.fit = false;
        }
        if response.dragged_by(egui::PointerButton::Primary) {
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
        let matrix = self.matrix(f64::from(rect.width() / rect.height()), radius);
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
            hit = pick(scene, center + near, (far - near).normalize());
        }
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
                scene,
                section,
                review,
                view,
                uniform: Uniform {
                    matrix: matrix.as_mat4().to_cols_array_2d(),
                    selected: selection,
                },
            },
        ));
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
    scene: Option<Arc<Scene>>,
    section: Option<Arc<Section>>,
    review: Option<Arc<crate::manufacture::Review>>,
    view: crate::replay::View,
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
        if changed || section_changed {
            match r.stage_section(device, self.section.as_ref()) {
                Ok((buffer, count)) => {
                    r.lines = buffer;
                    r.line_count = count;
                    r.section = self.section.clone();
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
            r.replay = self.review.clone();
        }
        queue.write_buffer(&r.uniform, 0, bytemuck::bytes_of(&self.uniform));
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
        pass.set_pipeline(&r.pipeline);
        for draw in r.draws.iter().filter(|_| self.view.source) {
            pass.set_bind_group(0, &draw.group, &[]);
            pass.draw(0..draw.indices, 0..draw.instances);
        }
        if let Some(lines) = &r.lines {
            pass.set_pipeline(&r.line_pipeline);
            pass.set_bind_group(0, &r.line_group, &[]);
            pass.set_vertex_buffer(0, lines.slice(..));
            pass.draw(0..r.line_count, 0..1);
        }
        if let Some(display) = r.replay.as_ref().and_then(|r| r.display.as_ref()) {
            for (batch, buffer) in display.batches.iter().zip(&r.replay_buffers) {
                let range = batch.range(self.view);
                if range.is_empty() {
                    continue;
                }
                pass.set_pipeline(if batch.points {
                    &r.point_pipeline
                } else {
                    &r.line_pipeline
                });
                pass.set_bind_group(0, &r.line_group, &[]);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(range, 0..1);
            }
        }
    }
}
impl Resources {
    fn stage_section(
        &self,
        device: &wgpu::Device,
        section: Option<&Arc<Section>>,
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
        let old = self
            .section
            .as_ref()
            .map_or(0, |s| s.summary.total_bytes as usize * 6);
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
                        ) + DVec3::from_array(view.plane_origin_mm)
                            - center;
                        let f = point.as_vec3();
                        if (f.as_dvec3() - point).length() > 0.001 {
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

    fn stage(&self, device: &wgpu::Device, scene: Option<Arc<Scene>>) -> Result<Vec<Draw>, String> {
        let Some(scene) = scene else {
            return Ok(vec![]);
        };
        let center = scene.summary.bounds_mm.map_or(DVec3::ZERO, |b| {
            (DVec3::from_array(b.min) + DVec3::from_array(b.max)) * 0.5
        });
        let mut previous = HashMap::new();
        if let Some(old) = &self.scene {
            for (chunk, draw) in old
                .definitions
                .iter()
                .flat_map(|d| d.chunks.iter())
                .zip(&self.draws)
            {
                previous.insert(chunk.metadata.sha256.as_str(), draw.raw.clone());
            }
        }
        let mut draws = Vec::new();
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
                    if (f.as_dvec3() - translated).length() > 0.001 {
                        return Err("Display instance projection exceeds 0.001 mm; native geometry remains available".into());
                    }
                    instances.push(Instance {
                        rotation: q.as_quat().to_array(),
                        translation: [f.x, f.y, f.z, index as f32],
                    });
                }
                let raw = previous
                    .get(chunk.metadata.sha256.as_str())
                    .cloned()
                    .unwrap_or_else(|| {
                        Arc::new(
                            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                                label: Some("checked SPLM"),
                                contents: &chunk.bytes,
                                usage: wgpu::BufferUsages::STORAGE,
                            }),
                        )
                    });
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
                    group,
                    _instances: buffer,
                    raw,
                    indices: view.indices.len() as u32,
                    instances: instances.len() as u32,
                });
            }
        }
        Ok(draws)
    }
}
pub(crate) fn pick(scene: &Scene, origin: DVec3, direction: DVec3) -> Option<FaceRef> {
    let mut nearest = f64::INFINITY;
    let mut selected = None;
    for occurrence in &scene.occurrences {
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
                if distance >= 0.0 && distance < nearest {
                    nearest = distance;
                    selected = scene.reference(
                        occurrence,
                        view.face_ordinals.get(triangle).expect("checked face") as usize,
                    );
                }
            }
        }
    }
    selected
}
