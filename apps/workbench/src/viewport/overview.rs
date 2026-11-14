// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
//! Fixed three-quarter source previews. Shared SPLM buffers, separate per-card cameras;
//! no picking, pose editing, manufacturing authority or redraw-driven native work.
use super::*;

pub(super) struct Renderer {
    scene: Option<Arc<Scene>>,
    dark: Option<bool>,
    cards: Vec<Option<Result<Card, String>>>,
    fill: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    line_layout: wgpu::BindGroupLayout,
}
struct Card {
    uniform: wgpu::Buffer,
    line_uniform: wgpu::Buffer,
    line_group: wgpu::BindGroup,
    bed: wgpu::Buffer,
    bed_count: u32,
    lines: wgpu::Buffer,
    line_count: u32,
    draws: Vec<Draw>,
}
impl Renderer {
    pub(super) fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        layout: &wgpu::PipelineLayout,
        line_layout: wgpu::BindGroupLayout,
        lines: wgpu::RenderPipeline,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Plate preview bed"),
            source: wgpu::ShaderSource::Wgsl(include_str!("lines.wgsl").into()),
        });
        let fill = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Plate preview fill"),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 24,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x3],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: egui_wgpu::depth_format_from_bits(24, 8).unwrap(),
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        Self {
            scene: None,
            dark: None,
            cards: vec![],
            fill,
            lines,
            line_layout,
        }
    }
    pub(super) fn clear(&mut self) {
        self.scene = None;
        self.dark = None;
        self.cards.clear();
    }
}
fn domain(layout: &LayoutOverview) -> AabbMm {
    let mut bounds = layout.build_area_mm;
    for part in &layout.parts {
        if let Some(b) = &mut bounds {
            for a in 0..3 {
                b.min[a] = b.min[a].min(part.source_bounds_mm.min[a]);
                b.max[a] = b.max[a].max(part.source_bounds_mm.max[a]);
            }
        } else {
            bounds = Some(part.source_bounds_mm);
        }
    }
    bounds.unwrap_or(AabbMm {
        min: [-0.5; 3],
        max: [0.5; 3],
    })
}
fn origin(layout: &LayoutOverview) -> DVec3 {
    let b = domain(layout);
    DVec3::from_array(b.min) + (DVec3::from_array(b.max) - DVec3::from_array(b.min)) * 0.5
}
fn camera(overview: &PrintLayoutOverview, aspect: f64) -> Result<[[f32; 4]; 4], String> {
    let direction = DVec3::new(1.0, -1.0, 0.85).normalize();
    let rotation = glam::dcamera::rh::view::look_at_mat4(direction, DVec3::ZERO, DVec3::Z);
    let mut half = 0.5_f64;
    let mut radius = 0.5_f64;
    for layout in &overview.layouts {
        let b = domain(layout);
        let center = origin(layout);
        for i in 0..8 {
            let p = DVec3::from_array(std::array::from_fn(|a| {
                if i & (1 << a) == 0 {
                    b.min[a]
                } else {
                    b.max[a]
                }
            })) - center;
            let v = rotation.transform_vector3(p);
            half = half.max(v.y.abs()).max(v.x.abs() / aspect.max(0.01));
            radius = radius.max(p.length());
        }
    }
    let half = half * 1.12;
    let view = glam::dcamera::rh::view::look_at_mat4(
        direction * (radius * 3.0 + 1.0),
        DVec3::ZERO,
        DVec3::Z,
    );
    let m = (glam::dcamera::rh::proj::directx::orthographic(
        -half * aspect,
        half * aspect,
        -half,
        half,
        0.01,
        radius * 8.0 + 2.0,
    ) * view)
        .as_mat4();
    if !m.is_finite() {
        return Err("Preview extent exceeds display precision".into());
    }
    Ok(m.to_cols_array_2d())
}
fn message(ui: &mut egui::Ui, rect: egui::Rect, text: &str, color: egui::Color32) {
    ui.put(
        rect.shrink(8.0),
        egui::Label::new(egui::RichText::new(text).color(color)).wrap(),
    );
}
pub fn paint(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    scene: Option<&Arc<Scene>>,
    index: usize,
    active: bool,
    tokens: crate::ui::Tokens,
) {
    let Some(scene) = scene else {
        message(ui, rect, "Loading source preview…", tokens.muted);
        return;
    };
    let Some(layout) = scene.context.layouts.get(index) else {
        return;
    };
    if layout.parts.iter().any(|p| {
        scene
            .context_definitions
            .iter()
            .find(|d| d.record.definition_id == p.definition_id)
            .is_none_or(|d| matches!(d.record.display, DefinitionDisplay::Unavailable { .. }))
    }) {
        message(
            ui,
            rect,
            scene
                .context_error
                .as_deref()
                .unwrap_or("Geometry preview unavailable · focus plate to inspect"),
            tokens.muted,
        );
        return;
    }
    let key = ui.make_persistent_id(("plate-preview-error", index));
    if let Some((old, error)) = ui.data(|d| d.get_temp::<(std::sync::Weak<Scene>, String)>(key))
        && old.upgrade().is_some_and(|s| Arc::ptr_eq(&s, scene))
    {
        message(ui, rect, &error, tokens.muted);
        return;
    }
    let matrix = match camera(&scene.context, f64::from(rect.width() / rect.height())) {
        Ok(m) => m,
        Err(e) => {
            message(ui, rect, &e, tokens.muted);
            return;
        }
    };
    let tint = if active {
        [0.52, 0.68, 0.83, 1.0]
    } else if ui.visuals().dark_mode {
        [0.34, 0.41, 0.49, 1.0]
    } else {
        [0.61, 0.67, 0.73, 1.0]
    };
    ui.painter().add(egui_wgpu::Callback::new_paint_callback(
        rect,
        Frame {
            scene: scene.clone(),
            index,
            ctx: ui.ctx().clone(),
            error_key: key,
            uniform: Uniform {
                matrix,
                selected: [u32::MAX; 4],
                tool_color: [0.0; 4],
                plane: [0.0; 4],
                cut: [0.0; 4],
                context: tint,
                emphasis: [0.0; 4],
            },
            dark: ui.visuals().dark_mode,
        },
    ));
}
struct Frame {
    scene: Arc<Scene>,
    index: usize,
    ctx: egui::Context,
    error_key: egui::Id,
    uniform: Uniform,
    dark: bool,
}
impl egui_wgpu::CallbackTrait for Frame {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _: &egui_wgpu::ScreenDescriptor,
        _: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(r) = resources.get_mut::<Resources>() else {
            return vec![];
        };
        if r.overview
            .scene
            .as_ref()
            .is_none_or(|s| !Arc::ptr_eq(s, &self.scene))
            || r.overview.dark != Some(self.dark)
        {
            r.overview.clear();
            r.scene = None;
            r.draws.clear();
            r.section = None;
            r.lines = None;
            r.inspection = None;
            r.inspection_buffer = None;
            r.replay = None;
            r.replay_buffers.clear();
            r.replay_upload = None;
            r.replay_pending.clear();
            r.replay_failure = None;
            r.retain_meshes(&self.scene);
            r.overview.scene = Some(self.scene.clone());
            r.overview.dark = Some(self.dark);
            r.overview.cards = (0..self.scene.context.layouts.len())
                .map(|_| None)
                .collect();
        }
        if r.overview.cards[self.index].is_none() {
            let card = stage(r, device, &self.scene, self.index, self.dark);
            if let Err(error) = &card {
                self.ctx.data_mut(|d| {
                    d.insert_temp(self.error_key, (Arc::downgrade(&self.scene), error.clone()))
                });
                self.ctx.request_repaint();
            }
            r.overview.cards[self.index] = Some(card);
        }
        if let Some(Ok(card)) = &r.overview.cards[self.index] {
            queue.write_buffer(&card.uniform, 0, bytemuck::bytes_of(&self.uniform));
            queue.write_buffer(
                &card.line_uniform,
                0,
                bytemuck::bytes_of(&Uniform {
                    context: [0.0; 4],
                    ..self.uniform
                }),
            );
        }
        vec![]
    }
    fn paint(
        &self,
        _: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(r) = resources.get::<Resources>() else {
            return;
        };
        let Some(Some(Ok(card))) = r.overview.cards.get(self.index) else {
            return;
        };
        pass.set_bind_group(0, &card.line_group, &[]);
        if card.bed_count > 0 {
            pass.set_pipeline(&r.overview.fill);
            pass.set_vertex_buffer(0, card.bed.slice(..));
            pass.draw(0..card.bed_count, 0..1);
        }
        if card.line_count > 0 {
            pass.set_pipeline(&r.overview.lines);
            pass.set_vertex_buffer(0, card.lines.slice(..));
            pass.draw(0..card.line_count, 0..1);
        }
        pass.set_pipeline(&r.pipeline);
        pass.set_bind_group(1, &r.participants_group, &[]);
        for draw in &card.draws {
            pass.set_bind_group(0, &draw.group, &[]);
            pass.draw(0..draw.indices, 0..draw.instances);
        }
    }
}
fn stage(
    r: &mut Resources,
    device: &wgpu::Device,
    scene: &Scene,
    index: usize,
    dark: bool,
) -> Result<Card, String> {
    let plate = &scene.context.layouts[index];
    let center = origin(plate);
    let uniform = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Plate preview camera"),
        size: std::mem::size_of::<Uniform>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let line_uniform = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Plate preview bed camera"),
        size: std::mem::size_of::<Uniform>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let line_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Plate preview lines"),
        layout: &r.overview.line_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: line_uniform.as_entire_binding(),
        }],
    });
    let mut draws = vec![];
    for definition in &scene.context_definitions {
        let parts: Vec<_> = plate
            .parts
            .iter()
            .filter(|p| p.definition_id == definition.record.definition_id)
            .collect();
        if parts.is_empty() {
            continue;
        }
        for chunk in &definition.chunks {
            let mesh = chunk
                .view(&scene.summary.session_id, &definition.record.definition_id)
                .map_err(|e| e.to_string())?;
            let radius = chunk.radius_mm();
            let mut instances = vec![];
            for part in &parts {
                let q = DQuat::from_array(part.pose.rotation_xyzw);
                let t = q * DVec3::from_array(mesh.local_origin_mm)
                    + DVec3::from_array(part.pose.translation_mm)
                    - center;
                let rounded = q.as_quat().as_dquat();
                let rotation_error = [DVec3::X, DVec3::Y, DVec3::Z]
                    .iter()
                    .map(|v| ((rounded * *v) - (q * *v)).length_squared())
                    .sum::<f64>()
                    .sqrt()
                    * radius;
                if !t.as_vec3().is_finite()
                    || rotation_error + (t.as_vec3().as_dvec3() - t).length() > 0.001
                {
                    return Err("Preview exceeds the 0.001 mm display allowance".into());
                }
                instances.push(Instance {
                    rotation: q.as_quat().to_array(),
                    translation: [t.x as f32, t.y as f32, t.z as f32],
                    occurrence: 0,
                });
            }
            // Every plate's parts are charged in the shared display pool at
            // scene admission; one buffer must still fit the device binding.
            let buffer_bytes = instances.len() * std::mem::size_of::<Instance>();
            if buffer_bytes as u64 > device.limits().max_storage_buffer_binding_size {
                return Err(
                    "Plate preview instances exceed this GPU's storage binding size".into(),
                );
            }
            let raw = r.mesh_buffer(device, chunk);
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Plate preview poses"),
                contents: bytemuck::cast_slice(&instances),
                usage: wgpu::BufferUsages::STORAGE,
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Plate preview source"),
                layout: &r.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
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
                indices: mesh.indices.len() as u32,
                instances: instances.len() as u32,
            });
        }
    }
    let mut bed = vec![];
    let mut lines = vec![];
    if let Some(b) = plate.build_area_mm {
        let position = |x: f64, y: f64| -> Result<[f32; 3], String> {
            let p = DVec3::new(x, y, b.min[2]) - center;
            let f = p.as_vec3();
            if !f.is_finite() || (f.as_dvec3() - p).length() > 0.001 {
                return Err("Plate bounds exceed display precision".into());
            }
            Ok(f.to_array())
        };
        let color = if dark {
            [0.10, 0.14, 0.19]
        } else {
            [0.80, 0.84, 0.89]
        };
        for [x, y] in [
            [b.min[0], b.min[1]],
            [b.max[0], b.min[1]],
            [b.max[0], b.max[1]],
            [b.min[0], b.min[1]],
            [b.max[0], b.max[1]],
            [b.min[0], b.max[1]],
        ] {
            bed.push(LineVertex {
                position: position(x, y)?,
                color,
            });
        }
        for i in 0..=10 {
            let f = f64::from(i) / 10.0;
            let x = b.min[0] + (b.max[0] - b.min[0]) * f;
            let y = b.min[1] + (b.max[1] - b.min[1]) * f;
            for [x, y] in [[x, b.min[1]], [x, b.max[1]], [b.min[0], y], [b.max[0], y]] {
                lines.push(LineVertex {
                    position: position(x, y)?,
                    color: if dark {
                        [0.23, 0.29, 0.35]
                    } else {
                        [0.57, 0.64, 0.71]
                    },
                });
            }
        }
    }
    let buffer = |vertices: &[LineVertex]| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Plate preview bed geometry"),
            contents: bytemuck::cast_slice(vertices),
            usage: wgpu::BufferUsages::VERTEX,
        })
    };
    Ok(Card {
        uniform,
        line_uniform,
        line_group,
        bed: buffer(&bed),
        bed_count: bed.len() as u32,
        lines: buffer(&lines),
        line_count: lines.len() as u32,
        draws,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plates_beyond_the_former_instance_pool_are_charged_in_the_shared_display_budget() {
        // 40,000 parts: past the ~32k instances the former 1 MiB pool held.
        let part = |index: u32| LayoutPartBounds {
            occurrence_id: OccurrenceId::new(index + 1).unwrap(),
            definition_id: DefinitionId::from_source_sha256(&[1; 32]),
            pose: RigidPoseMm::IDENTITY,
            source_bounds_mm: AabbMm {
                min: [0.0, 0.0, 0.0],
                max: [1.0, 1.0, 1.0],
            },
        };
        let plate = |parts: std::ops::Range<u32>| LayoutOverview {
            layout_id: spiling_contracts::project::PrintLayoutId::new(),
            captured_plate: false,
            build_area_mm: None,
            parts: parts.map(part).collect(),
        };
        let overview = PrintLayoutOverview {
            session_id: SessionId::new(),
            scene_revision: SceneRevision(1),
            project_id: spiling_contracts::project::ProjectId::new(),
            project_revision: spiling_contracts::project::ProjectRevision(1),
            definitions: vec![],
            layouts: vec![plate(0..30_000), plate(30_000..40_000)],
        };
        let charged = crate::scene::instance_display_bytes(&overview).unwrap();
        assert_eq!(charged, 40_000 * crate::scene::INSTANCE_DISPLAY_BYTES);
        assert!(charged > 1024 * 1024);
        assert!(charged < MAX_DISPLAY_BUFFER_BYTES as usize);
        assert!(40_000 * std::mem::size_of::<Instance>() > 1024 * 1024);
    }

    #[test]
    fn common_three_quarter_camera_contains_tall_and_off_origin_geometry() {
        let mut overview = PrintLayoutOverview {
            session_id: SessionId::new(),
            scene_revision: SceneRevision(1),
            project_id: spiling_contracts::project::ProjectId::new(),
            project_revision: spiling_contracts::project::ProjectRevision(1),
            definitions: vec![],
            layouts: vec![LayoutOverview {
                layout_id: spiling_contracts::project::PrintLayoutId::new(),
                captured_plate: false,
                build_area_mm: Some(AabbMm {
                    min: [1000.0, -500.0, 10.0],
                    max: [1200.0, -400.0, 10.0],
                }),
                parts: vec![LayoutPartBounds {
                    occurrence_id: OccurrenceId::new(1).unwrap(),
                    definition_id: DefinitionId::from_source_sha256(&[1; 32]),
                    pose: RigidPoseMm::IDENTITY,
                    source_bounds_mm: AabbMm {
                        min: [990.0, -480.0, 10.0],
                        max: [1010.0, -460.0, 310.0],
                    },
                }],
            }],
        };
        let mut second = overview.layouts[0].clone();
        second.layout_id = spiling_contracts::project::PrintLayoutId::new();
        second.parts[0].source_bounds_mm = AabbMm {
            min: [1000.0, -500.0, -50.0],
            max: [1300.0, -390.0, 20.0],
        };
        overview.layouts.push(second);
        for aspect in [0.5, 1.0, 2.4, 4.0] {
            let matrix =
                glam::Mat4::from_cols_array_2d(&camera(&overview, aspect).unwrap()).as_dmat4();
            for plate in &overview.layouts {
                let b = domain(plate);
                let o = origin(plate);
                for i in 0..8 {
                    let p = DVec3::from_array(std::array::from_fn(|a| {
                        if i & (1 << a) == 0 {
                            b.min[a]
                        } else {
                            b.max[a]
                        }
                    })) - o;
                    let v = matrix.project_point3(p);
                    assert!(
                        v.x.abs() < 1.0 && v.y.abs() < 1.0 && (0.0..=1.0).contains(&v.z),
                        "{aspect}: {v:?}"
                    );
                }
            }
            let xy = matrix.transform_vector3(DVec3::X);
            let z = matrix.transform_vector3(DVec3::Z);
            assert!(
                xy.y.abs() > 1e-6 && z.y.abs() > 1e-6,
                "preview must show depth and height"
            );
        }
    }
}
