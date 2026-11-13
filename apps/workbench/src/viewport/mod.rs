// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

mod beads;
mod cube;
pub mod fastener;
mod focus;
mod navigation;
pub use navigation::{InputDevice, Settings as NavigationSettings, StandardView, ViewAction};
pub mod force;
pub mod mechanics;
pub mod overview;
pub mod plate;
mod reveal;
mod section;
use crate::scene::{Scene, Section};
use egui_wgpu::wgpu::{self, util::DeviceExt};
use glam::{DMat4, DQuat, DVec3};
use spiling_contracts::geometry::*;
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
    emphasis: [f32; 4],
    participants: [[u32; 4]; 2],
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
type UploadResult =
    std::pin::Pin<Box<dyn std::future::Future<Output = Option<wgpu::Error>> + Send>>;
struct PendingUpload {
    index: usize,
    buffer: wgpu::Buffer,
    instances: Arc<Vec<crate::replay::beads::Instance>>,
    result: std::sync::Mutex<UploadResult>,
}
struct Resources {
    mechanics: mechanics::Renderer,
    meshes: HashMap<SourceHash, Arc<wgpu::Buffer>>,
    overview: overview::Renderer,
    plate: plate::Renderer,
    replay: Option<Arc<crate::manufacture::Review>>,
    replay_buffers: Vec<Option<wgpu::Buffer>>,
    replay_pending: Vec<PendingUpload>,
    replay_failure: Option<String>,
    replay_upload:
        Option<std::sync::Mutex<std::sync::mpsc::Receiver<crate::replay::upload::Prepared>>>,
    replay_line_pipeline: wgpu::RenderPipeline,
    bead_pipeline: wgpu::RenderPipeline,
    source_stencil: wgpu::RenderPipeline,
    bead_stencil: wgpu::RenderPipeline,
    source_cap: wgpu::RenderPipeline,
    actual_cap: wgpu::RenderPipeline,
    clear_stencil: wgpu::RenderPipeline,
    point_pipeline: wgpu::RenderPipeline,
    start_pipeline: wgpu::RenderPipeline,
    seam_pipeline: wgpu::RenderPipeline,
    selected_seam_pipeline: wgpu::RenderPipeline,
    tool_uniforms: [wgpu::Buffer; 2],
    tool_groups: [wgpu::BindGroup; 2],
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    scene: Option<Arc<Scene>>,
    draws: Vec<Draw>,
    source_traced: std::sync::atomic::AtomicBool,
    failure: Option<String>,
    line_pipeline: wgpu::RenderPipeline,
    line_group: wgpu::BindGroup,
    context_uniform: wgpu::Buffer,
    context_group: wgpu::BindGroup,
    lines: Option<wgpu::Buffer>,
    line_count: u32,
    section: Option<Arc<Section>>,
    section_scope: Option<OccurrenceId>,
    inspection: Option<Arc<crate::inspection::Area>>,
    inspection_buffer: Option<wgpu::Buffer>,
}

pub struct Input<'a> {
    pub force: Option<&'a mut force::Canvas>,
    pub fastener: Option<&'a mut fastener::Canvas>,
    pub mechanics: Option<mechanics::Input>,
    pub inspection: Option<Arc<crate::inspection::Area>>,
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
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CameraState {
    #[serde(default)]
    rotation: [f64; 4],
    distance: f64,
    pan: [f64; 3],
    orthographic: bool,
}
impl CameraState {
    pub fn valid(&self) -> bool {
        self.rotation.iter().all(|value| value.is_finite())
            && (DQuat::from_array(self.rotation).length_squared() - 1.0).abs() < 1e-6
            && self.distance.is_finite()
            && self.distance > 0.0
            && self.pan.iter().all(|v| v.is_finite())
    }
}
pub struct Viewport {
    force_interaction: force::Interaction,
    fastener_ghosts: fastener::Ghosts,
    mechanics_annotations: mechanics::Annotations,
    rotation: DQuat,
    navigation: navigation::Input,
    pub navigation_settings: NavigationSettings,
    pending_view: Option<ViewAction>,
    fit_scope: Option<ViewAction>,
    normal: Option<DVec3>,
    normal_cache: Option<navigation::NormalTarget>,
    distance: f64,
    pan: DVec3,
    fit: bool,
    had_source: bool,
    had_plate: bool,
    camera_restored: bool,
    project_id: Option<spiling_contracts::project::ProjectId>,
    show_height: bool,
    orthographic: bool,
    motion: Option<focus::Motion>,
    pending_focus: Option<focus::Request>,
    previous_camera: Option<CameraState>,
    focus_scene: Option<SceneSummary>,
    focus_problem: Option<&'static str>,
    dim_context: bool,
    reveal: Option<reveal::Reveal>,
    last_selection: Option<(Option<FaceRef>, Option<OccurrenceId>)>,
    attention: Option<[[u32; 4]; 2]>,
}
impl Default for Viewport {
    fn default() -> Self {
        Self {
            force_interaction: Default::default(),
            fastener_ghosts: Default::default(),
            mechanics_annotations: Default::default(),
            rotation: navigation::orientation(StandardView::Isometric.direction()),
            navigation: Default::default(),
            navigation_settings: Default::default(),
            pending_view: None,
            fit_scope: None,
            normal: None,
            normal_cache: None,
            distance: 100.0,
            pan: DVec3::ZERO,
            fit: true,
            had_source: false,
            had_plate: false,
            camera_restored: false,
            project_id: None,
            show_height: false,
            orthographic: false,
            motion: None,
            pending_focus: None,
            previous_camera: None,
            focus_scene: None,
            focus_problem: None,
            dim_context: false,
            reveal: None,
            last_selection: None,
            attention: None,
        }
    }
}
impl Viewport {
    pub fn cancel_force(&mut self) -> Option<force::Event> {
        self.force_interaction.cancel()
    }
}
impl Viewport {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let _opening = crate::performance::span("viewport_initialize");
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
        let replay_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Shared compact actual motion"),
            source: wgpu::ShaderSource::Wgsl(include_str!("replay.wgsl").into()),
        });
        let replay_line_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("native section lines"),
            layout: Some(&line_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &replay_shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<crate::replay::beads::Instance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0=>Float32x3,1=>Float32,2=>Float32x3,3=>Float32,4=>Uint32],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &replay_shader,
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
        let points = |entry, fragment, label| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&line_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &replay_shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<crate::replay::beads::Instance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![0=>Float32x3,1=>Float32,2=>Float32x3,3=>Float32,4=>Uint32],
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &replay_shader,
                    entry_point: Some(fragment),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: state.target_format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
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
        let point_pipeline = points("vs_point", "fs", "stationary actual-program events");
        let start_pipeline = points("vs_start", "fs_start", "actual toolpath start");
        let seam_pipeline = points("vs_seam", "fs", "closed actual wall starts");
        let selected_seam_pipeline = points("vs_selected_seam", "fs", "selected actual wall start");
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
        let mechanics =
            mechanics::Renderer::new(device, &line_pipeline_layout, state.target_format);
        state.renderer.write().callback_resources.insert(Resources {
            mechanics,
            meshes: HashMap::new(),
            overview,
            plate,
            replay: None,
            replay_buffers: Vec::new(),
            replay_pending: Vec::new(),
            replay_failure: None,
            replay_upload: None,
            replay_line_pipeline,
            bead_pipeline,
            source_stencil,
            bead_stencil,
            source_cap,
            actual_cap,
            clear_stencil,
            point_pipeline,
            start_pipeline,
            seam_pipeline,
            selected_seam_pipeline,
            tool_uniforms,
            tool_groups,
            pipeline,
            layout,
            uniform,
            scene: None,
            draws: Vec::new(),
            source_traced: std::sync::atomic::AtomicBool::new(false),
            failure: None,
            line_pipeline,
            line_group,
            context_uniform,
            context_group,
            lines: None,
            line_count: 0,
            section: None,
            section_scope: None,
            inspection: None,
            inspection_buffer: None,
        });
        Self::default()
    }
    pub fn camera(&self) -> CameraState {
        CameraState {
            rotation: self.rotation.to_array(),
            distance: self.distance,
            pan: self.pan.to_array(),
            orthographic: self.orthographic,
        }
    }
    pub fn restore_camera(&mut self, c: &CameraState) {
        self.interrupt_focus();
        if let Some(reveal) = self.reveal.take() {
            self.dim_context = reveal.dim_context;
        }
        self.attention = None;
        self.previous_camera = None;
        self.apply_camera(c);
    }
    fn apply_camera(&mut self, c: &CameraState) {
        if c.valid() {
            self.rotation = DQuat::from_array(c.rotation).normalize();
            self.distance = c.distance;
            self.pan = DVec3::from_array(c.pan);
            self.orthographic = c.orthographic;
            self.fit = false;
            self.camera_restored = true;
        }
    }
    pub fn fit(&mut self) {
        self.interrupt_focus();
        self.camera_restored = false;
        self.fit = true;
    }
    pub fn normal_to(&mut self, normal: DVec3, now: f64, animations_enabled: bool) {
        let Some(n) = normal.try_normalize() else {
            return;
        };
        self.normal = Some(n);
        self.navigate(ViewAction::Normal, now, animations_enabled);
    }
    pub fn focus_on(&mut self, scene: &Scene, target: crate::scene::FocusTarget) -> bool {
        if !target.valid() {
            return false;
        }
        self.pending_focus = Some(focus::Request {
            summary: scene.summary.clone(),
            target,
            participants: None,
            reveal: false,
            started_reveal: false,
            area_applied: false,
        });
        self.focus_problem = None;
        true
    }
    fn interrupt_focus(&mut self) {
        self.motion = None;
        self.pending_focus = None;
    }
    pub fn focus_area(
        &mut self,
        scene: &Scene,
        target: crate::scene::FocusTarget,
        participants: &[OccurrenceId],
    ) -> bool {
        let Some(mask) = reveal::participant_mask(scene, participants) else {
            return false;
        };
        if !self.focus_on(scene, target) {
            return false;
        }
        self.pending_focus.as_mut().unwrap().participants = Some(mask);
        self.pending_focus.as_mut().unwrap().reveal = self.reveal.is_some();
        true
    }
    pub fn clear_attention(&mut self) {
        self.attention = None;
    }
    pub fn cancel_area(&mut self) {
        self.interrupt_focus();
        if let Some(reveal) = self.reveal.take() {
            self.dim_context = reveal.dim_context;
        }
        self.attention = None;
    }
    pub fn reveal_area(
        &mut self,
        scene: &Scene,
        target: crate::scene::FocusTarget,
        participants: &[OccurrenceId],
    ) -> bool {
        if !self.focus_area(scene, target, participants) {
            return false;
        }
        self.pending_focus.as_mut().unwrap().reveal = true;
        true
    }
    pub fn close_reveal(&mut self, now: f64, animations_enabled: bool) -> bool {
        let Some(reveal) = self.reveal.take() else {
            return false;
        };
        self.interrupt_focus();
        self.previous_camera = reveal.previous_camera;
        self.dim_context = reveal.dim_context;
        self.attention = reveal.attention;
        self.animate_to(reveal.camera, now, animations_enabled);
        true
    }
    fn apply_focus_area(&mut self) {
        let Some(mut request) = self.pending_focus.take() else {
            return;
        };
        if !request.area_applied {
            if let Some(participants) = request.participants {
                if request.reveal {
                    let mut reveal =
                        reveal::Reveal::new(request.target, self.camera(), self.dim_context);
                    reveal.attention = self.attention;
                    reveal.previous_camera = self.previous_camera.clone();
                    if let Some(active) = &mut self.reveal {
                        active.target = request.target;
                        active.normal = reveal.normal;
                        active.offset_mm = reveal.offset_mm;
                    } else {
                        self.reveal = Some(reveal);
                        request.started_reveal = true;
                    }
                }
                self.attention = Some(participants);
                self.dim_context = true;
            }
            request.area_applied = true;
        }
        self.pending_focus = Some(request);
    }
    pub fn stop_focus(&mut self) -> bool {
        let active = self.motion.is_some() || self.pending_focus.is_some();
        self.interrupt_focus();
        active
    }
    fn animate_to(&mut self, camera: CameraState, now: f64, animations_enabled: bool) {
        if self.navigation_settings.animate && animations_enabled {
            self.motion = Some(focus::Motion::new(self.camera(), camera, now));
        } else {
            self.motion = None;
            self.apply_camera(&camera);
        }
    }
    fn advance_focus(&mut self, now: f64) -> bool {
        let Some(motion) = &self.motion else {
            return false;
        };
        let (camera, done) = motion.sample(now);
        self.apply_camera(&camera);
        if done {
            self.motion = None;
        }
        !done
    }
    fn matrix(&self, aspect: f64, radius: f64) -> DMat4 {
        let direction = self.rotation * DVec3::Z;
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
        let up = self.rotation * DVec3::Y;
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
            mut force,
            mut fastener,
            mechanics,
            inspection,
            project_id,
            plate,
            layout_preview,
            scene,
            mut section,
            review,
            view,
            selected,
            interactive,
            analysis,
            mut plane,
            scope,
        } = input;
        self.reconcile_inspection(
            scene.as_ref().map(|scene| &scene.summary),
            interactive && !layout_preview,
            (selected.cloned(), scope),
            ui.input(|input| input.time),
            ui.style().animation_time > 0.0,
        );
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
        self.normal = if let Some(reveal) = &self.reveal {
            Some(reveal.normal)
        } else if analysis.enabled {
            DVec3::from_array(plane.normal)
                .try_normalize()
                .map(|normal| normal * if analysis.flipped { -1.0 } else { 1.0 })
        } else {
            self.selection_normal(scene.as_ref(), selected, scope)
        };
        ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            if ui.button("Fit").clicked() {
                self.queue_view(ViewAction::Fit);
            }
            ui.menu_button("View…", |ui| {
            self.navigation_menu(ui);
            ui.separator();
            let focus_available = interactive && !layout_preview && view.source
                && scene.as_ref().is_some_and(|scene| scope.is_some_and(|id| scene.occurrences.iter().any(|item| item.occurrence_id == id)));
            if ui.add_enabled(focus_available, egui::Button::new("Focus on selection")).clicked()
                && let Some(scene) = &scene
            {
                match scene.focus_target(selected, scope) {
                    Ok(target) => { self.focus_on(scene, target); }
                    Err(error) => { self.focus_problem = Some(error); }
                }
                ui.close();
            }
            if self.reveal.is_some() {
                if ui.button("Exit reveal").clicked() { self.close_reveal(ui.input(|input| input.time),ui.style().animation_time>0.0); ui.close(); }
            } else if ui.add_enabled(focus_available,egui::Button::new("Reveal selection")).clicked()
                && let Some(scene) = &scene
            {
                match scene.focus_target(selected,scope) {
                    Ok(target) => { self.reveal_area(scene,target,&[scope.unwrap()]); }
                    Err(error) => { self.focus_problem=Some(error); }
                }
                ui.close();
            }
            ui.checkbox(&mut self.dim_context,"Dim surroundings");
            crate::ui::help(ui, "viewport-navigation", "Viewport navigation", "Drag to orbit; middle-drag orbits even with a tool armed. Right-drag or Shift+middle pans. Wheel zooms. Trackpad mode: two-finger pan, pinch zoom, Option/Alt+drag orbit. The cube snaps to sides, edges and corners. Canvas keys: 0 Isometric; 1 Top; 2 Bottom; 3 Front; 4 Back; 5 Left; 6 Right; F fits; G focuses; P projection; N normal; Shift+N opposite. Hold arrows to orbit, Shift+arrows to pan, plus/minus to zoom. Release stops; Escape cancels the current gesture. View changes never edit the part or run analysis.");
            });
        });
        self.apply_focus_area();
        if let Some(reveal) = &mut self.reveal {
            let radius = reveal.radius();
            ui.horizontal_wrapped(|ui| {
                ui.label("Display cutaway · no material edit");
                ui.add(
                    egui::Slider::new(&mut reveal.offset_mm, -radius..=radius)
                        .text("Cut offset (mm)"),
                );
            });
        }
        let mut reveal_analysis = crate::analysis::SectionView {
            enabled: true,
            offset_mm: self.reveal.as_ref().map_or(0.0, |reveal| reveal.offset_mm),
            ..Default::default()
        };
        let analysis = if let Some(reveal) = &self.reveal {
            plane = reveal.plane();
            section = None;
            &mut reveal_analysis
        } else {
            analysis
        };
        if let Some(problem) = self.focus_problem {
            ui.colored_label(ui.visuals().warn_fg_color, problem);
        }
        if let Some(plate) = &plate
            && plate.controls(ui, &mut self.show_height)
        {
            self.queue_view(ViewAction::FitPlate);
        }
        let (rect, response) = ui.allocate_exact_size(
            ui.available_size().max(egui::vec2(1.0, 1.0)),
            egui::Sense::click_and_drag(),
        );
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Canvas, true, "3D viewport. Arrows orbit; Shift arrows pan; plus/minus zoom; F fits; G focuses; 0 to 6 snap views; P projection; N normal. View menu exposes every view."));
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
            let (action, focus) = navigation::keys(ui);
            if let Some(action) = action {
                self.queue_view(action);
            }
            if focus
                && interactive
                && view.source
                && let Some(scene) = &scene
                && let Ok(target) = scene.focus_target(selected, scope)
            {
                self.focus_on(scene, target);
            }
        }
        if let Some(action) = self.pending_view.take() {
            self.navigate(
                action,
                ui.input(|input| input.time),
                ui.style().animation_time > 0.0,
            );
            response.request_focus();
        }
        let fit_source = self.fit_scope == Some(ViewAction::FitParts);
        let fit_plate = self.fit_scope == Some(ViewAction::FitPlate);
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
            .filter(|_| view.visible && !fit_source)
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
            self.fit_scope = None;
        }
        let now = ui.input(|input| input.time);
        if let Some(request) = self.pending_focus.take()
            && scene
                .as_ref()
                .is_some_and(|scene| scene.same_revision(&request.summary))
            && let Some(destination) = focus::plan(
                &self.camera(),
                request.target,
                center,
                f64::from(rect.width() / rect.height()),
            )
        {
            if request.started_reveal {
                let camera = self.camera();
                if let Some(reveal) = &mut self.reveal {
                    reveal.camera = camera;
                }
            }
            if self.previous_camera.is_none() {
                self.previous_camera = Some(self.camera());
            }
            self.animate_to(destination, now, ui.style().animation_time > 0.0);
        }
        if self.advance_focus(now) {
            ui.ctx().request_repaint();
        }
        self.navigation.begin(ui, rect, cube::bounds(rect));
        if ui.input(|input| {
            input.pointer.any_pressed()
                && input
                    .pointer
                    .press_origin()
                    .is_some_and(|point| rect.contains(point))
        }) {
            self.interrupt_focus();
        }
        let navigation_claimed = self.navigation.reserves_pointer();
        // A separate handle moves the inspection plane; dragging the surrounding canvas orbits.
        let mut handle = None;
        if analysis.enabled {
            let matrix = self.matrix(f64::from(rect.width() / rect.height()), display_radius);
            let n = DVec3::from_array(plane.normal);
            let point = self.reveal.as_ref().map_or_else(
                || n * n.dot(DVec3::from_array(plane.origin_mm) - center),
                |reveal| reveal.target.center() + n * reveal.offset_mm - center,
            );
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
                        if navigation_claimed {
                            egui::Sense::hover()
                        } else {
                            egui::Sense::drag()
                        },
                    )
                    .on_hover_text("Drag to move section plane in millimetres");
                if r.dragged() {
                    self.interrupt_focus();
                    let axis = screen(point + n * radius * 0.25) - position;
                    let mm_per_pixel = if axis.length_sq() > 4.0 {
                        axis * (radius as f32 * 0.25 / axis.length_sq())
                    } else {
                        egui::vec2(0.0, -radius as f32 / rect.height())
                    };
                    let delta = f64::from(r.drag_motion().dot(mm_per_pixel));
                    analysis.offset_mm += delta;
                    if let Some(reveal) = &mut self.reveal {
                        reveal.offset_mm += delta;
                    }
                    plane.origin_mm = (DVec3::from_array(plane.origin_mm) + n * delta).to_array();
                }
                handle = Some((position, r.dragged()));
            }
        }
        let force_owns_pointer = if let Some(canvas) =
            force.as_mut().filter(|_| interactive && view.source)
        {
            self.force_interaction.show(
                ui,
                &force::Projection {
                    matrix: self.matrix(f64::from(rect.width() / rect.height()), display_radius),
                    center,
                    rect,
                },
                canvas,
                !navigation_claimed,
            )
        } else {
            false
        };
        let plane_owns_pointer = handle.is_some_and(|(point, dragging)| {
            dragging
                || ui.input(|input| {
                    input
                        .pointer
                        .press_origin()
                        .is_some_and(|origin| origin.distance(point) < 14.0)
                })
        });
        let navigation_cancelled = self.navigation.suppressed();
        let pointer_motion = self
            .navigation
            .pointer(ui, force_owns_pointer || plane_owns_pointer);
        let key_motion = self.navigation.keyboard(ui, response.has_focus());
        if !navigation_cancelled
            && !self.navigation.owns_tool()
            && (pointer_motion.orbit != egui::Vec2::ZERO || pointer_motion.pan != egui::Vec2::ZERO)
        {
            response.request_focus();
        }
        self.orbit(pointer_motion.orbit + key_motion.orbit);
        self.pan_pixels(
            pointer_motion.pan + key_motion.pan * rect.height(),
            rect.height(),
        );
        let mut zoom = key_motion.zoom;
        let scroll_allowed = (response.hovered() || force_owns_pointer)
            && !self.navigation.owns_tool()
            && !cube::bounds(rect).is_some_and(|cube| {
                ui.input(|input| {
                    input
                        .pointer
                        .hover_pos()
                        .is_some_and(|point| cube.contains(point))
                })
            });
        let scroll = navigation::scroll(ui, self.navigation_settings, scroll_allowed);
        self.pan_pixels(scroll.pan, rect.height());
        zoom += scroll.zoom;
        if zoom != 0.0 {
            self.interrupt_focus();
            let old_matrix = self.matrix(f64::from(rect.width() / rect.height()), display_radius);
            let anchor = ui
                .input(|input| input.pointer.hover_pos())
                .filter(|pointer| rect.contains(*pointer));
            let zoom_normal = self.rotation * DVec3::Z;
            let zoom_target = self.pan;
            let plane_hit = |matrix: DMat4, pointer: egui::Pos2| {
                let point = DVec3::new(
                    2.0 * f64::from((pointer.x - rect.left()) / rect.width()) - 1.0,
                    1.0 - 2.0 * f64::from((pointer.y - rect.top()) / rect.height()),
                    0.0,
                );
                let inverse = matrix.inverse();
                let near = inverse.project_point3(point);
                let ray = inverse.project_point3(point + DVec3::Z) - near;
                near + ray * ((zoom_target - near).dot(zoom_normal) / ray.dot(zoom_normal))
            };
            let before = anchor.map(|pointer| plane_hit(old_matrix, pointer));
            self.distance =
                (self.distance * zoom.clamp(-4.0, 4.0).exp()).clamp(0.001, radius * 1000.0);
            if let (Some(pointer), Some(before)) = (anchor, before) {
                let after = plane_hit(
                    self.matrix(f64::from(rect.width() / rect.height()), display_radius),
                    pointer,
                );
                let delta = before - after;
                if delta.is_finite() {
                    self.pan += delta;
                }
            }
        }
        self.distance = self.distance.clamp(0.001, radius * 1000.0);
        let matrix = self.matrix(f64::from(rect.width() / rect.height()), display_radius);
        if interactive
            && view.source
            && !force_owns_pointer
            && !pointer_motion.used
            && !ui.ctx().text_edit_focused()
            && (force.as_ref().is_some_and(|canvas| canvas.armed)
                || fastener.as_ref().is_some_and(|canvas| canvas.armed))
            && response.hovered()
            && let (Some(scene), Some(pointer)) =
                (&scene, ui.input(|input| input.pointer.hover_pos()))
        {
            let horizontal = 2.0 * f64::from((pointer.x - rect.left()) / rect.width()) - 1.0;
            let vertical = 1.0 - 2.0 * f64::from((pointer.y - rect.top()) / rect.height());
            let inverse = matrix.inverse();
            let near = inverse.project_point3(DVec3::new(horizontal, vertical, 0.0));
            let far = inverse.project_point3(DVec3::new(horizontal, vertical, 1.0));
            if let Some((reference, seed_mm)) = pick_clipped_hit(
                scene,
                center + near,
                (far - near).normalize(),
                analysis,
                plane,
                scope,
            ) {
                if let Some(canvas) = fastener.as_mut().filter(|canvas| canvas.armed) {
                    canvas.event = Some(fastener::Event::Point {
                        reference,
                        place: response.clicked(),
                    });
                } else if let Some(canvas) = force.as_mut() {
                    canvas.event = Some(force::Event::Point {
                        reference,
                        seed_mm,
                        place: response.clicked(),
                    });
                }
            } else {
                if let Some(canvas) = fastener.as_mut().filter(|canvas| canvas.armed) {
                    canvas.event = Some(fastener::Event::Leave);
                } else if let Some(canvas) = force.as_mut() {
                    canvas.event = Some(force::Event::Leave);
                }
            }
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        if interactive
            && view.source
            && !force_owns_pointer
            && !pointer_motion.used
            && !force.as_ref().is_some_and(|canvas| canvas.armed)
            && !fastener.as_ref().is_some_and(|canvas| canvas.armed)
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
        let participants = self
            .attention
            .or_else(|| {
                scene
                    .as_ref()
                    .and_then(|scene| reveal::participant_mask(scene, &[scope?]))
            })
            .unwrap_or([[0; 4]; 2]);
        ui.painter().add(egui_wgpu::Callback::new_paint_callback(
            rect,
            Callback {
                context: ui.ctx().clone(),
                mechanics: mechanics.clone(),
                inspection,
                plate: plate.clone(),
                plate_origin: center,
                dark: ui.visuals().dark_mode,
                show_height: self.show_height,
                scene: scene.clone(),
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
                    emphasis: if self.dim_context
                        && view.source
                        && (scope.is_some() || self.attention.is_some())
                    {
                        if ui.visuals().dark_mode {
                            [0.45, 0.50, 0.58, 1.0]
                        } else {
                            [0.40, 0.44, 0.50, 1.0]
                        }
                    } else {
                        [0.0; 4]
                    },
                    participants,
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
        if view.source
            && let (Some(canvas), Some(scene)) = (&fastener, &scene)
        {
            self.fastener_ghosts
                .paint(ui, canvas, scene, matrix, center, rect);
        } else {
            self.fastener_ghosts = Default::default();
        }
        if let (Some(mechanics), Some(scene)) = (&mechanics, &scene) {
            mechanics.paint_ui(
                ui,
                rect,
                matrix,
                center,
                scene,
                &mut self.mechanics_annotations,
            );
        } else {
            self.mechanics_annotations = Default::default();
        }
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
        if let Some(direction) =
            cube::show(ui, rect, self.rotation, plate.is_some()).filter(|_| !navigation_cancelled)
        {
            let axes = direction.to_array().map(|value| {
                if value.abs() < 1e-9 {
                    0
                } else if value > 0.0 {
                    1
                } else {
                    -1
                }
            });
            self.queue_view(ViewAction::Direction(axes));
            response.request_focus();
            ui.ctx().request_repaint();
        }
        if let Some(state) = frame.wgpu_render_state()
            && let Some(resources) = state.renderer.read().callback_resources.get::<Resources>()
            && !resources.replay_buffers.is_empty()
            && resources.replay_buffers.iter().any(Option::is_none)
            && resources.replay_failure.is_none()
        {
            let loaded = resources
                .replay_buffers
                .iter()
                .filter(|b| b.is_some())
                .count();
            ui.painter().text(
                rect.left_top() + egui::vec2(12.0, 12.0),
                egui::Align2::LEFT_TOP,
                format!(
                    "Loading toolpaths · {}%",
                    loaded * 100 / resources.replay_buffers.len()
                ),
                egui::FontId::proportional(13.0),
                ui.visuals().weak_text_color(),
            );
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
            && let Some(error) = resources
                .failure
                .as_ref()
                .or(resources.replay_failure.as_ref())
                .or(resources.mechanics.error.as_ref())
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
    context: egui::Context,
    mechanics: Option<mechanics::Input>,
    inspection: Option<Arc<crate::inspection::Area>>,
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
        let inspection_changed = match (&r.inspection, &self.inspection) {
            (Some(previous), Some(current)) => !Arc::ptr_eq(previous, current),
            (None, None) => false,
            _ => true,
        };
        if inspection_changed {
            r.inspection_buffer = None;
            r.inspection = self.inspection.clone();
            if let Some(area) = &self.inspection {
                r.inspection_buffer = Some(device.create_buffer_init(
                    &wgpu::util::BufferInitDescriptor {
                        label: Some("Bounded compiled construction outline"),
                        contents: bytemuck::cast_slice(&area.vertices),
                        usage: wgpu::BufferUsages::VERTEX,
                    },
                ));
            }
        }
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
            r.source_traced
                .store(false, std::sync::atomic::Ordering::Relaxed);
            let _opening = crate::performance::span("source_gpu_stage");
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
        r.mechanics.error = r
            .mechanics
            .prepare(
                device,
                self.mechanics.as_ref(),
                r.scene.as_deref(),
                self.plate_origin,
            )
            .err();
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
            r.replay_pending.clear();
            r.replay_upload = None;
            r.replay_failure = None;
            if let Some(display) = self.review.as_ref().and_then(|r| r.display.as_ref()) {
                r.replay_buffers.resize_with(display.batches.len(), || None);
                match crate::replay::upload::start(&display.batches) {
                    Ok(receiver) => r.replay_upload = Some(std::sync::Mutex::new(receiver)),
                    Err(error) => r.replay_failure = Some(error),
                }
            }
            r.replay = self.review.clone();
        }
        // Publish buffers only after the device has accepted their allocations.
        // Poll without waiting; neither file reads nor GPU waits belong on paint.
        let mut pending = Vec::new();
        for mut upload in r.replay_pending.drain(..) {
            let status = upload
                .result
                .get_mut()
                .expect("exclusive pending upload")
                .as_mut()
                .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()));
            match status {
                std::task::Poll::Ready(None) => {
                    match upload.buffer.get_mapped_range_mut(..) {
                        Ok(mut mapped) => mapped
                            .copy_from_slice(bytemuck::cast_slice(upload.instances.as_slice())),
                        Err(error) => {
                            r.replay_failure = Some(format!(
                                "Toolpath GPU upload failed: {error}. Verification and export remain available."
                            ));
                            continue;
                        }
                    }
                    upload.buffer.unmap();
                    r.replay_buffers[upload.index] = Some(upload.buffer);
                }
                std::task::Poll::Ready(Some(error)) => {
                    upload.buffer.destroy();
                    r.replay_failure = Some(format!(
                        "Toolpath GPU allocation failed: {error}. Verification and export remain available."
                    ));
                }
                std::task::Poll::Pending => pending.push(upload),
            }
        }
        r.replay_pending = pending;
        if r.replay_failure.is_some() {
            r.replay_upload = None;
            r.replay_pending.clear();
            r.replay_buffers
                .iter_mut()
                .for_each(|buffer| *buffer = None);
        } else if r.replay_pending.is_empty()
            && let Some(receiver) = &mut r.replay_upload
        {
            let receiver = receiver
                .get_mut()
                .expect("upload receiver is exclusively owned");
            let mut staged = 0usize;
            while staged < 16 * 1024 * 1024 {
                match receiver.try_recv() {
                    Ok(Ok((index, instances))) => {
                        let bytes: &[u8] = bytemuck::cast_slice(instances.as_slice());
                        if bytes.len() as u64 > device.limits().max_buffer_size {
                            r.replay_failure = Some(
                                "A replay page exceeds the device buffer representation".into(),
                            );
                            break;
                        }
                        let scope = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
                        let size = bytes.len();
                        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some("Shared compact actual-motion page"),
                            size: size as u64,
                            mapped_at_creation: true,
                            usage: wgpu::BufferUsages::VERTEX,
                        });
                        r.replay_pending.push(PendingUpload {
                            index,
                            buffer,
                            instances,
                            result: std::sync::Mutex::new(Box::pin(scope.pop())),
                        });
                        staged += size;
                    }
                    Ok(Err(error)) => {
                        r.replay_failure = Some(error);
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        if r.replay_buffers.iter().filter(|b| b.is_none()).count()
                            != r.replay_pending.len()
                        {
                            r.replay_failure = Some(
                                "Toolpath page preparation ended before completing the display"
                                    .into(),
                            );
                        }
                        break;
                    }
                }
            }
        }
        if r.replay_buffers.iter().any(Option::is_none) && r.replay_failure.is_none() {
            self.context
                .request_repaint_after(std::time::Duration::from_millis(8));
        }
        if r.replay_upload.is_some() && r.replay_buffers.iter().all(Option::is_some) {
            r.replay_upload = None;
            crate::performance::mark("replay_gpu_ready");
            self.context.request_repaint();
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
        if self.view.source
            && self.uniform.cut[1] == 0.0
            && r.draws.iter().any(|d| d.indices > 0 && d.instances > 0)
            && !r
                .source_traced
                .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            crate::performance::mark("source_drawn");
        }
        r.plate.paint(pass, &r.line_group, self.show_height);
        pass.set_pipeline(&r.pipeline);
        for draw in r
            .draws
            .iter()
            .filter(|_| self.view.source && self.uniform.cut[1] == 0.0)
        {
            pass.set_bind_group(0, &draw.group, &[]);
            if let (Some(target), Some(scene)) = (r.mechanics.occurrence, &r.scene) {
                for (instance, occurrence) in scene
                    .occurrences
                    .iter()
                    .filter(|occurrence| occurrence.definition_id == draw.definition)
                    .enumerate()
                {
                    if occurrence.occurrence_id != target {
                        pass.draw(0..draw.indices, instance as u32..instance as u32 + 1);
                    }
                }
            } else {
                pass.draw(0..draw.indices, 0..draw.instances);
            }
        }
        if self.view.source && self.uniform.cut[1] == 0.0 {
            r.mechanics.paint(pass, &r.line_group);
        }
        if self.uniform.cut[0] > 0.0 && self.view.source {
            pass.set_stencil_reference(0);
            // All mesh chunks of one occurrence contribute to the same parity.
            // Resolve coincident occurrences separately so their holes cannot cancel.
            for chunks in r.draws.chunk_by(|a, b| a.definition == b.definition) {
                let Some(scene) = &r.scene else {
                    continue;
                };
                for (instance, (occurrence, _)) in scene
                    .occurrences
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| item.definition_id == chunks[0].definition)
                    .enumerate()
                {
                    let instance = instance as u32;
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
                    pass.draw(0..6, occurrence as u32..occurrence as u32 + 1);
                }
            }
            pass.set_pipeline(&r.clear_stencil);
            pass.set_bind_group(0, &r.line_group, &[]);
            pass.draw(0..3, 0..1);
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
                pass.set_pipeline(&r.replay_line_pipeline);
                pass.set_bind_group(0, &r.context_group, &[]);
                for (batch, buffer) in display
                    .batches
                    .iter()
                    .zip(&r.replay_buffers)
                    .filter_map(|(batch, buffer)| buffer.as_ref().map(|buffer| (batch, buffer)))
                {
                    for range in batch.context_ranges(self.view) {
                        if !range.is_empty() {
                            pass.set_vertex_buffer(0, buffer.slice(..));
                            pass.draw(0..2, range);
                        }
                    }
                }
            }
            let beads = display
                .beads
                .as_ref()
                .ok()
                .filter(|_| self.view.beads && r.replay_upload.is_none());
            if let Some(beads) = beads {
                pass.set_pipeline(&r.bead_pipeline);
                for (batch, buffer) in display
                    .batches
                    .iter()
                    .zip(&r.replay_buffers)
                    .filter_map(|(batch, buffer)| buffer.as_ref().map(|buffer| (batch, buffer)))
                    .filter(|(b, _)| {
                        !b.points
                            && !b.seam
                            && matches!(b.kind, 1..=5 | 8..=10 | 13 | 15)
                            && self.uniform.cut[1] == 0.0
                    })
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
                    for (batch, buffer) in display
                        .batches
                        .iter()
                        .zip(&r.replay_buffers)
                        .filter_map(|(batch, buffer)| buffer.as_ref().map(|buffer| (batch, buffer)))
                        .filter(|(b, _)| {
                            !b.points
                                && !b.seam
                                && matches!(b.kind, 1..=5 | 8..=10 | 13 | 15)
                                && b.tool as usize == tool
                        })
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
                .filter_map(|(batch, buffer)| buffer.as_ref().map(|buffer| (batch, buffer)))
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
                    &r.replay_line_pipeline
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
                    pass.draw(0..2, range);
                }
            }
            if self.uniform.cut[1] == 0.0
                && let Some((batch, vertex)) = display.toolpath_start(self.view)
            {
                pass.set_pipeline(&r.start_pipeline);
                pass.set_bind_group(0, &r.line_group, &[]);
                if let Some(buffer) = &r.replay_buffers[batch] {
                    pass.set_vertex_buffer(0, buffer.slice(..));
                    pass.draw(0..6, vertex..vertex + 1);
                }
            }
        }
        if let (Some(area), Some(buffer)) = (&r.inspection, &r.inspection_buffer) {
            pass.set_pipeline(&r.line_pipeline);
            pass.set_bind_group(0, &r.line_group, &[]);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..area.vertices.len() as u32, 0..1);
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
                let view = chunk
                    .view(&scene.summary.session_id, &definition.record.definition_id)
                    .map_err(|e| e.to_string())?;
                let radius = chunk.radius_mm();
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
    pick_clipped_hit(scene, origin, direction, analysis, plane, scope).map(|hit| hit.0)
}
fn pick_clipped_hit(
    scene: &Scene,
    origin: DVec3,
    direction: DVec3,
    analysis: &crate::analysis::SectionView,
    plane: PlaneMm,
    scope: Option<OccurrenceId>,
) -> Option<(FaceRef, [f64; 3])> {
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
            let Ok(view) = chunk.view(&scene.summary.session_id, &definition.record.definition_id)
            else {
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
                    selected = scene
                        .reference(
                            occurrence,
                            view.face_ordinals.get(triangle).expect("checked face") as usize,
                        )
                        .map(|reference| {
                            (
                                reference,
                                (source_origin + source_direction * distance).to_array(),
                            )
                        });
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
            rotation: navigation::orientation(DVec3::X),
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
            ("replay", include_str!("replay.wgsl")),
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
