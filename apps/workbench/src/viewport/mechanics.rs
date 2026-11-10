// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::assets::MechanicsCapture;
use spiling_contracts::analysis::AnalysisCase;

#[derive(Clone)]
pub struct Input {
    pub editing: Option<FaceId>,
    pub contacts: Vec<Arc<spiling_contracts::analysis::load::AnalysisFacePreparation>>,
    pub capture: Option<Arc<MechanicsCapture>>,
    pub case: AnalysisCase,
    pub selected: Option<FaceId>,
    pub field: usize,
    pub stale: bool,
}

#[derive(Default)]
pub(super) struct Annotations {
    scene: std::sync::Weak<Scene>,
    occurrence: Option<OccurrenceId>,
    faces: Vec<FaceId>,
    patches: Vec<Option<(crate::scene::FocusTarget, Vec<[DVec3; 2]>)>>,
    incomplete: bool,
}

impl Annotations {
    fn prepare(&mut self, input: &Input, scene: &Arc<Scene>) {
        let mut faces: Vec<_> = input
            .case
            .loads
            .iter()
            .map(|load| load.face_id.clone())
            .chain(
                input
                    .case
                    .restraints
                    .iter()
                    .map(|restraint| restraint.face_id.clone()),
            )
            .collect();
        if let Some(face) = &input.selected
            && !faces.contains(face)
        {
            faces.push(face.clone());
        }
        let key = Arc::downgrade(scene);
        if self.scene.ptr_eq(&key)
            && self.occurrence == Some(input.case.occurrence_id)
            && self.faces == faces
        {
            return;
        }
        *self = Self {
            scene: key,
            occurrence: Some(input.case.occurrence_id),
            faces,
            ..Default::default()
        };
        let mut remaining = 4096;
        for face in &self.faces {
            let reference = FaceRef {
                session_id: scene.summary.session_id.clone(),
                scene_revision: scene.summary.revision,
                occurrence_id: input.case.occurrence_id,
                definition_id: input.case.definition_id.clone(),
                face_id: face.clone(),
            };
            match scene.focus_target(Some(&reference), Some(reference.occurrence_id)) {
                Ok(target) => {
                    let edges =
                        patch_edges(scene, &reference, &mut remaining).unwrap_or_else(|_| {
                            self.incomplete = true;
                            Vec::new()
                        });
                    self.patches.push(Some((target, edges)));
                }
                Err(_) => {
                    self.incomplete = true;
                    self.patches.push(None);
                }
            }
        }
    }
}

impl Input {
    pub fn surface(&self, scene: &Scene) -> Option<&Arc<MechanicsCapture>> {
        let capture = self.capture.as_ref()?;
        let summary = &capture.summary;
        let case = &summary.case.case;
        (case.id == self.case.id
            && case.definition_id == self.case.definition_id
            && case.occurrence_id == self.case.occurrence_id
            && case.source_hash == self.case.source_hash
            && case.layout_id == self.case.layout_id
            && case.layout_id == scene.summary.layout_id
            && scene
                .definition(&case.definition_id)
                .is_some_and(|definition| {
                    definition.record.provenance.source_hash == summary.source_hash
                })
            && scene.occurrences.iter().any(|occurrence| {
                occurrence.occurrence_id == case.occurrence_id
                    && occurrence.definition_id == case.definition_id
                    && occurrence.pose == summary.pose
            }))
        .then_some(capture)
    }

    pub(super) fn paint_ui(
        &self,
        ui: &egui::Ui,
        rect: egui::Rect,
        matrix: DMat4,
        center: DVec3,
        scene: &Arc<Scene>,
        annotations: &mut Annotations,
    ) {
        annotations.prepare(self, scene);
        let painter = ui.painter().with_clip_rect(rect);
        let project = |point: DVec3| {
            let clip = matrix * (point - center).extend(1.0);
            (clip.w > 0.0 && clip.z >= 0.0 && clip.z <= clip.w).then(|| {
                let point = clip.truncate() / clip.w;
                egui::pos2(
                    rect.left() + (point.x as f32 + 1.0) * rect.width() * 0.5,
                    rect.top() + (1.0 - point.y as f32) * rect.height() * 0.5,
                )
            })
        };
        let Some(occurrence) = scene.occurrences.iter().find(|item| {
            item.occurrence_id == self.case.occurrence_id
                && item.definition_id == self.case.definition_id
        }) else {
            return;
        };
        let rotation = DQuat::from_array(occurrence.pose.rotation_xyzw);
        let patches = self
            .case
            .loads
            .iter()
            .map(|load| (&load.face_id, Some(load)))
            .chain(
                self.case
                    .restraints
                    .iter()
                    .map(|restraint| (&restraint.face_id, None)),
            );
        for (index, (face, load)) in patches.enumerate() {
            if load.is_some() && self.editing.as_ref() == Some(face) {
                continue;
            }
            let Some((target, edges)) = &annotations.patches[index] else {
                continue;
            };
            let mut load_center = target.center();
            let disk = load.is_some_and(|load| {
                matches!(
                    load.footprint,
                    spiling_contracts::analysis::load::ForceFootprint::Disk { .. }
                )
            });
            if let Some(load) = load
                && disk
            {
                if let Some(face) = self
                    .contacts
                    .iter()
                    .find(|prepared| &prepared.reference.face_id == face)
                    && let Ok((_, center)) = face.contact(&load.footprint)
                {
                    load_center = rotation * DVec3::from_array(center)
                        + DVec3::from_array(occurrence.pose.translation_mm);
                } else if let Some(capture) = &self.capture
                    && capture.summary.case.case == self.case
                    && let Some((center, samples)) = capture.display.contacts.get(index)
                {
                    load_center =
                        rotation * *center + DVec3::from_array(occurrence.pose.translation_mm);
                    for sample in samples {
                        if let Some(position) = project(
                            rotation * *sample + DVec3::from_array(occurrence.pose.translation_mm),
                        ) {
                            painter.circle_filled(
                                position,
                                1.2,
                                ui.visuals().selection.stroke.color,
                            );
                        }
                    }
                }
            }
            let Some(point) = project(load_center) else {
                continue;
            };
            let color = match (load.is_some(), ui.visuals().dark_mode) {
                (true, true) => egui::Color32::from_rgb(255, 174, 67),
                (false, true) => egui::Color32::from_rgb(100, 225, 187),
                (true, false) => egui::Color32::from_rgb(153, 74, 0),
                (false, false) => egui::Color32::from_rgb(0, 107, 78),
            };
            for [start, end] in edges.iter().filter(|_| !disk) {
                if let (Some(start), Some(end)) = (project(*start), project(*end)) {
                    painter.line_segment([start, end], egui::Stroke::new(2.0, color));
                }
            }
            painter.circle_stroke(point, 7.0, egui::Stroke::new(2.0, color));
            let label = if let Some(load) = load {
                let force = DVec3::from_array(load.force_n);
                let size = (DVec3::from_array(target.bounds_mm.max)
                    - DVec3::from_array(target.bounds_mm.min))
                .length()
                .max(5.0);
                if let Some(direction) = force.try_normalize()
                    && let Some(tail) = project(load_center - rotation * direction * size * 0.8)
                {
                    painter.arrow(tail, point - tail, egui::Stroke::new(3.0, color));
                }
                if force.length() == 0.0 {
                    format!(
                        "L{} · M {:.3e} N·mm",
                        index + 1,
                        DVec3::from_array(load.moment_n_mm).length()
                    )
                } else if force.length() < 0.001 {
                    format!("L{} · {:.3e} N", index + 1, force.length())
                } else {
                    format!("L{} · {:.3} N", index + 1, force.length())
                }
            } else {
                let restraint = self
                    .case
                    .restraints
                    .iter()
                    .find(|restraint| &restraint.face_id == face)
                    .unwrap();
                let axes: String = restraint
                    .fixed_translation
                    .iter()
                    .zip(['X', 'Y', 'Z'])
                    .filter_map(|(fixed, axis)| fixed.then_some(axis))
                    .collect();
                format!("Fixed {axes}")
            };
            painter.text(
                point + egui::vec2(11.0, -13.0),
                egui::Align2::LEFT_BOTTOM,
                label,
                egui::FontId::proportional(12.0),
                color,
            );
        }
        if let Some(face) = &self.selected
            && let Some(index) = annotations
                .faces
                .iter()
                .position(|candidate| candidate == face)
            && let Some((_, edges)) = &annotations.patches[index]
        {
            for [start, end] in edges {
                if let (Some(start), Some(end)) = (project(*start), project(*end)) {
                    painter.line_segment(
                        [start, end],
                        egui::Stroke::new(3.0, ui.visuals().selection.stroke.color),
                    );
                }
            }
        }
        if annotations.incomplete {
            painter.text(
                rect.right_top() + egui::vec2(-12.0, 12.0),
                egui::Align2::RIGHT_TOP,
                "Some patch outlines unavailable",
                egui::FontId::proportional(12.0),
                ui.visuals().warn_fg_color,
            );
        }
        let Some(capture) = self.surface(scene) else {
            return;
        };
        let field = self.field.min(3);
        let range = capture.display.ranges[field];
        let legend = egui::Rect::from_min_size(
            rect.left_top() + egui::vec2(14.0, 14.0),
            egui::vec2(244.0, 74.0),
        );
        painter.rect_filled(legend, 6.0, ui.visuals().window_fill().gamma_multiply(0.96));
        painter.text(
            legend.left_top() + egui::vec2(10.0, 8.0),
            egui::Align2::LEFT_TOP,
            crate::mechanics::FIELD_NAMES[field],
            egui::FontId::proportional(13.0),
            ui.visuals().text_color(),
        );
        for step in 0..100 {
            let left = legend.left() + 10.0 + step as f32 * 2.24;
            let color = crate::mechanics::color(f64::from(step) / 99.0, [0.0, 1.0]);
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(left, legend.top() + 26.0),
                    egui::vec2(2.3, 7.0),
                ),
                0.0,
                egui::Color32::from_rgb(
                    (color[0] * 255.0) as u8,
                    (color[1] * 255.0) as u8,
                    (color[2] * 255.0) as u8,
                ),
            );
        }
        painter.text(
            legend.left_top() + egui::vec2(10.0, 37.0),
            egui::Align2::LEFT_TOP,
            format!("{:.4} — {:.4}", range[0], range[1]),
            egui::FontId::proportional(12.0),
            ui.visuals().text_color(),
        );
        painter.text(
            legend.left_top() + egui::vec2(10.0, 54.0),
            egui::Align2::LEFT_TOP,
            if self.stale {
                "Stale result · cell samples"
            } else {
                "Assumed materials · cell samples"
            },
            egui::FontId::proportional(12.0),
            ui.visuals().warn_fg_color,
        );
        if rect.contains(ui.input(|input| input.pointer.hover_pos().unwrap_or_default()))
            && let Some(pointer) = ui.input(|input| input.pointer.hover_pos())
        {
            let inverse = matrix.inverse();
            let horizontal = 2.0 * f64::from((pointer.x - rect.left()) / rect.width()) - 1.0;
            let vertical = 1.0 - 2.0 * f64::from((pointer.y - rect.top()) / rect.height());
            let near = center + inverse.project_point3(DVec3::new(horizontal, vertical, 0.0));
            let far = center + inverse.project_point3(DVec3::new(horizontal, vertical, 1.0));
            let rotation = DQuat::from_array(capture.summary.pose.rotation_xyzw).conjugate();
            let translation = DVec3::from_array(capture.summary.pose.translation_mm);
            if let Some(element) = capture.display.probe(
                rotation * (near - translation),
                rotation * (far - near).normalize(),
            ) {
                let value = capture.display.values[element][field];
                painter.text(
                    rect.left_bottom() + egui::vec2(14.0, -14.0),
                    egui::Align2::LEFT_BOTTOM,
                    format!(
                        "Cell {} · {:.5} · {}",
                        element + 1,
                        value,
                        crate::mechanics::FIELD_NAMES[field]
                    ),
                    egui::FontId::proportional(13.0),
                    ui.visuals().text_color(),
                );
            }
        }
    }
}

pub(super) fn patch_edges(
    scene: &Scene,
    reference: &FaceRef,
    remaining: &mut usize,
) -> Result<Vec<[DVec3; 2]>, String> {
    let definition = scene
        .definition(&reference.definition_id)
        .ok_or("Missing patch definition")?;
    let ordinal = definition
        .faces
        .iter()
        .position(|face| face.face_id == reference.face_id)
        .ok_or("Missing patch face")? as u32;
    let occurrence = scene
        .occurrences
        .iter()
        .find(|occurrence| occurrence.occurrence_id == reference.occurrence_id)
        .ok_or("Missing patch occurrence")?;
    let rotation = DQuat::from_array(occurrence.pose.rotation_xyzw);
    let translation = DVec3::from_array(occurrence.pose.translation_mm);
    let mut edges = std::collections::BTreeMap::<[[u64; 3]; 2], ([DVec3; 2], usize)>::new();
    for chunk in &definition.chunks {
        let view = validate_mesh_chunk(
            &chunk.bytes,
            &chunk.metadata,
            &scene.summary.session_id,
            chunk.metadata.artifact_id,
            &definition.record.definition_id,
            chunk.metadata.chunk_index,
        )
        .map_err(|error| error.to_string())?;
        for triangle in 0..view.face_ordinals.len() {
            if view.face_ordinals.get(triangle) != Some(ordinal) {
                continue;
            }
            if *remaining == 0 {
                return Err("Patch outline exceeds display budget".into());
            }
            *remaining -= 1;
            let points: [DVec3; 3] = std::array::from_fn(|corner| {
                let index = view.indices.get(triangle * 3 + corner).unwrap() as usize;
                DVec3::from_array(view.positions.get(index).unwrap().map(f64::from))
                    + DVec3::from_array(view.local_origin_mm)
            });
            for side in 0..3 {
                let endpoints = [points[side], points[(side + 1) % 3]];
                let mut key = endpoints.map(|point| {
                    point.to_array().map(|coordinate| {
                        if coordinate == 0.0 {
                            0.0_f64.to_bits()
                        } else {
                            coordinate.to_bits()
                        }
                    })
                });
                key.sort_unstable();
                let entry = edges
                    .entry(key)
                    .or_insert((endpoints.map(|point| rotation * point + translation), 0));
                entry.1 += 1;
            }
        }
    }
    Ok(edges
        .into_values()
        .filter_map(|(points, count)| (count == 1).then_some(points))
        .collect())
}

pub(super) struct Renderer {
    pub error: Option<String>,
    pipeline: wgpu::RenderPipeline,
    buffer: Option<wgpu::Buffer>,
    count: u32,
    key: Option<(Arc<MechanicsCapture>, usize, DVec3)>,
    pub occurrence: Option<OccurrenceId>,
}

impl Renderer {
    pub fn new(
        device: &wgpu::Device,
        layout: &wgpu::PipelineLayout,
        format: wgpu::TextureFormat,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Mechanical cell fields"),
            source: wgpu::ShaderSource::Wgsl(include_str!("lines.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Mechanical cell surfaces"),
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
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: egui_wgpu::depth_format_from_bits(24, 8).unwrap(),
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        Self {
            pipeline,
            buffer: None,
            count: 0,
            key: None,
            occurrence: None,
            error: None,
        }
    }

    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        input: Option<&Input>,
        scene: Option<&Scene>,
        center: DVec3,
    ) -> Result<(), String> {
        let eligible = input.zip(scene).and_then(|(input, scene)| {
            input
                .surface(scene)
                .map(|capture| (capture, input.field.min(3)))
        });
        let Some((capture, field)) = eligible else {
            self.buffer = None;
            self.key = None;
            self.occurrence = None;
            self.count = 0;
            return Ok(());
        };
        if self
            .key
            .as_ref()
            .is_some_and(|(previous, old_field, origin)| {
                Arc::ptr_eq(previous, capture) && *old_field == field && *origin == center
            })
        {
            return Ok(());
        }
        self.buffer = None;
        self.key = None;
        self.occurrence = None;
        self.count = 0;
        let vertices = capture
            .display
            .vertices(field, &capture.summary.pose, center)?;
        self.count = vertices.len() as u32;
        self.buffer = Some(
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Bounded mechanical exterior cells"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            }),
        );
        self.key = Some((capture.clone(), field, center));
        self.occurrence = Some(capture.summary.case.case.occurrence_id);
        Ok(())
    }

    pub fn paint(&self, pass: &mut wgpu::RenderPass<'static>, group: &wgpu::BindGroup) {
        if let Some(buffer) = &self.buffer {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, group, &[]);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..self.count, 0..1);
        }
    }
}
