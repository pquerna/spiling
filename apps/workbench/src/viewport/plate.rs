// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Disposable hardware display, independent of source picking and section clipping.
use super::LineVertex;
use egui_wgpu::wgpu::{self, util::DeviceExt};
use glam::DVec3;
use spiling_contracts::{
    geometry::AabbMm,
    hardware::{AccessoryEffect, ProjectHardwareCapture},
    manufacturing::ManufacturingIntent,
};
use std::sync::Arc;

/// Only committed, checked captures reach this view. A process envelope is not a sheet.
#[derive(Clone)]
pub enum Plate {
    Hardware(Arc<ProjectHardwareCapture>),
    Envelope(Arc<ManufacturingIntent>),
}
impl Plate {
    pub fn current(
        hardware: Option<&Arc<ProjectHardwareCapture>>,
        intent: Option<&Arc<ManufacturingIntent>>,
    ) -> Option<Self> {
        hardware
            .map(|h| Self::Hardware(h.clone()))
            .or_else(|| intent.map(|i| Self::Envelope(i.clone())))
    }
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Hardware(a), Self::Hardware(b)) => Arc::ptr_eq(a, b),
            (Self::Envelope(a), Self::Envelope(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
    pub fn bounds(&self) -> AabbMm {
        match self {
            Self::Hardware(h) => {
                let mut b = h.selected_face().expect("checked captured face").usable_mm;
                let machine = h.setup.printer.build_envelope;
                for axis in 0..2 {
                    b.min[axis] = b.min[axis].max(machine.min[axis]);
                    b.max[axis] = b.max[axis].min(machine.max[axis]);
                }
                b.min[2] = h.plate.surface_z_mm;
                b.max[2] = h.plate.surface_z_mm;
                b
            }
            Self::Envelope(i) => {
                let mut b = i.printer.build_envelope;
                b.max[2] = b.min[2];
                b
            }
        }
    }
    pub fn envelope(&self) -> AabbMm {
        match self {
            Self::Hardware(h) => h.setup.printer.build_envelope,
            Self::Envelope(i) => i.printer.build_envelope,
        }
    }
    pub fn controls(&self, ui: &mut egui::Ui, show_height: &mut bool) -> bool {
        let mut fit = false;
        ui.add(egui::Label::new(egui::RichText::new(self.caption()).weak()).wrap());
        ui.horizontal_wrapped(|ui| {
            fit = ui.add_enabled(self.usable(), egui::Button::new("Fit plate")).clicked();
            ui.checkbox(show_height, "Height limits");
            crate::ui::help(ui, "build-area-help", "Build area",
                "The rectangle shows the committed usable build area, not a measured outer sheet outline. The grid is in millimetres. Marked boxes are declared keepouts or fixture bounds. Height limits show the declared machine envelope. This display does not move parts or establish physical collision clearance.");
        });
        fit
    }
    pub fn usable(&self) -> bool {
        let b = self.bounds();
        (0..2).all(|a| b.min[a] < b.max[a])
    }
    pub fn caption(&self) -> String {
        let b = self.bounds();
        if !self.usable() {
            return "No usable intersection with the machine envelope".into();
        }
        let size = format!(
            "{} × {} mm",
            dimension(b.max[0] - b.min[0]),
            dimension(b.max[1] - b.min[1])
        );
        match self {
            Self::Hardware(h) => format!("Build area · {size} · {}", h.plate.label),
            Self::Envelope(_) => format!("Declared print envelope · {size}"),
        }
    }
}
fn dimension(v: f64) -> String {
    if v.fract().abs() < 1e-6 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}

struct Geometry {
    surface: Vec<LineVertex>,
    lines: Vec<LineVertex>,
    height: Vec<LineVertex>,
}
impl Geometry {
    fn new(plate: &Plate, origin: DVec3, dark: bool) -> Result<Self, &'static str> {
        let b = plate.bounds();
        if !(0..2).all(|a| b.min[a] < b.max[a]) {
            return Err("Build area has no usable intersection with the machine envelope");
        }
        let surface = if dark {
            [0.15, 0.19, 0.24]
        } else {
            [0.76, 0.80, 0.85]
        };
        let grid = if dark {
            [0.23, 0.29, 0.35]
        } else {
            [0.59, 0.65, 0.72]
        };
        let edge = if dark {
            [0.47, 0.57, 0.68]
        } else {
            [0.30, 0.40, 0.51]
        };
        let warning = if dark {
            [0.65, 0.46, 0.24]
        } else {
            [0.58, 0.39, 0.16]
        };
        let fixture = if dark {
            [0.27, 0.25, 0.21]
        } else {
            [0.68, 0.62, 0.51]
        };
        let mut g = Self {
            surface: vec![],
            lines: vec![],
            height: vec![],
        };
        let mut sheet = b;
        if let Plate::Hardware(h) = plate {
            sheet.min[2] -= h.plate.thickness_mm;
        }
        g.box_surface(sheet, surface, origin)?;
        Self::outline(&mut g.lines, b, edge, origin)?;
        let extent = (b.max[0] - b.min[0]).max(b.max[1] - b.min[1]);
        // At most 64 lines per axis at every zoom/bed size; no redraw-dependent geometry.
        let step = 10.0_f64.powf((extent / 32.0).log10().ceil());
        for axis in 0..2 {
            let first = (b.min[axis] / step).ceil() * step;
            for i in 0..64 {
                let v = first + f64::from(i) * step;
                if v >= b.max[axis] {
                    break;
                }
                if v <= b.min[axis] {
                    continue;
                }
                let mut a = b.min;
                let mut z = b.max;
                a[axis] = v;
                z[axis] = v;
                Self::line(&mut g.lines, a, z, grid, origin)?;
            }
        }
        if let Plate::Hardware(h) = plate {
            for keepout in &h
                .selected_face()
                .expect("checked captured face")
                .keepouts_mm
            {
                Self::outline(&mut g.lines, *keepout, warning, origin)?;
                // Bounded diagonal marks distinguish a reservation from the regular grid.
                let mut a = keepout.min;
                let mut z = keepout.max;
                a[2] = b.min[2];
                z[2] = b.min[2];
                Self::line(&mut g.lines, a, z, warning, origin)?;
                std::mem::swap(&mut a[0], &mut z[0]);
                Self::line(&mut g.lines, a, z, warning, origin)?;
            }
            for accessory in &h.accessories {
                if let AccessoryEffect::Fixture { bounds_mm } = accessory.effect {
                    g.box_surface(bounds_mm, fixture, origin)?;
                    Self::outline(&mut g.lines, bounds_mm, warning, origin)?;
                }
            }
        }
        let machine = plate.envelope();
        Self::outline(&mut g.height, machine, edge, origin)?;
        let zero = [0.0, 0.0, b.min[2]];
        if (0..2).all(|a| b.min[a] <= 0.0 && b.max[a] >= 0.0) {
            let length = (extent * 0.08).min(20.0);
            Self::line(
                &mut g.lines,
                zero,
                [length.min(b.max[0]), 0.0, b.min[2]],
                [0.85, 0.37, 0.31],
                origin,
            )?;
            Self::line(
                &mut g.lines,
                zero,
                [0.0, length.min(b.max[1]), b.min[2]],
                [0.28, 0.65, 0.47],
                origin,
            )?;
        }
        if (g.surface.len() + g.lines.len() + g.height.len()) * std::mem::size_of::<LineVertex>()
            > 64 * 1024
        {
            return Err("Build area exceeds its bounded display allocation");
        }
        Ok(g)
    }
    fn vertex(p: [f64; 3], color: [f32; 3], origin: DVec3) -> Result<LineVertex, &'static str> {
        let local = DVec3::from_array(p) - origin;
        let projected = local.as_vec3();
        if !local.is_finite()
            || !projected.is_finite()
            || (projected.as_dvec3() - local).abs().max_element() > 0.001
        {
            return Err("Build area exceeds 0.001 mm display precision; use a nearer source frame");
        }
        Ok(LineVertex {
            position: projected.to_array(),
            color,
        })
    }
    fn line(
        out: &mut Vec<LineVertex>,
        a: [f64; 3],
        b: [f64; 3],
        color: [f32; 3],
        origin: DVec3,
    ) -> Result<(), &'static str> {
        out.extend([
            Self::vertex(a, color, origin)?,
            Self::vertex(b, color, origin)?,
        ]);
        Ok(())
    }
    fn corners(b: AabbMm) -> [[f64; 3]; 8] {
        std::array::from_fn(|i| {
            std::array::from_fn(|a| {
                if i & (1 << a) == 0 {
                    b.min[a]
                } else {
                    b.max[a]
                }
            })
        })
    }
    fn outline(
        out: &mut Vec<LineVertex>,
        b: AabbMm,
        color: [f32; 3],
        origin: DVec3,
    ) -> Result<(), &'static str> {
        let c = Self::corners(b);
        for i in 0..8 {
            for a in 0..3 {
                if i & (1 << a) == 0 {
                    Self::line(out, c[i], c[i | (1 << a)], color, origin)?;
                }
            }
        }
        Ok(())
    }
    fn box_surface(
        &mut self,
        b: AabbMm,
        color: [f32; 3],
        origin: DVec3,
    ) -> Result<(), &'static str> {
        let c = Self::corners(b);
        for face in [
            [0, 1, 3, 2],
            [4, 5, 7, 6],
            [0, 1, 5, 4],
            [2, 3, 7, 6],
            [0, 2, 6, 4],
            [1, 3, 7, 5],
        ] {
            for i in [0, 1, 2, 0, 2, 3] {
                self.surface.push(Self::vertex(c[face[i]], color, origin)?);
            }
        }
        Ok(())
    }
}

pub(super) struct Renderer {
    solid: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    source: Option<Plate>,
    origin: DVec3,
    dark: bool,
    buffers: Vec<(wgpu::Buffer, u32)>,
    pub error: Option<&'static str>,
}
impl Renderer {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        layout: &wgpu::PipelineLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Build area"),
            source: wgpu::ShaderSource::Wgsl(include_str!("plate.wgsl").into()),
        });
        let pipeline = |topology, write| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Build area"),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<LineVertex>() as u64,
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
                primitive: wgpu::PrimitiveState {
                    topology,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: egui_wgpu::depth_format_from_bits(24, 8).expect("depth format"),
                    depth_write_enabled: Some(write),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: wgpu::DepthBiasState {
                        constant: if write { 16 } else { 0 },
                        slope_scale: if write { 1.0 } else { 0.0 },
                        ..Default::default()
                    },
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        Self {
            solid: pipeline(wgpu::PrimitiveTopology::TriangleList, true),
            lines: pipeline(wgpu::PrimitiveTopology::LineList, false),
            source: None,
            origin: DVec3::ZERO,
            dark: false,
            buffers: vec![],
            error: None,
        }
    }
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        plate: Option<&Plate>,
        origin: DVec3,
        dark: bool,
    ) {
        if self.origin == origin
            && self.dark == dark
            && match (&self.source, plate) {
                (Some(a), Some(b)) => a.same(b),
                (None, None) => true,
                _ => false,
            }
        {
            return;
        }
        self.source = plate.cloned();
        self.origin = origin;
        self.dark = dark;
        self.buffers.clear();
        self.error = None;
        let Some(plate) = plate else {
            return;
        };
        match Geometry::new(plate, origin, dark) {
            Ok(g) => {
                for vertices in [g.surface, g.lines, g.height] {
                    self.buffers.push((
                        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("Bounded build area display"),
                            contents: bytemuck::cast_slice(&vertices),
                            usage: wgpu::BufferUsages::VERTEX,
                        }),
                        vertices.len() as u32,
                    ));
                }
            }
            Err(e) => self.error = Some(e),
        }
    }
    pub fn paint(
        &self,
        pass: &mut wgpu::RenderPass<'static>,
        group: &wgpu::BindGroup,
        height: bool,
    ) {
        pass.set_bind_group(0, group, &[]);
        for (i, (buffer, count)) in self.buffers.iter().enumerate() {
            if i == 2 && !height {
                continue;
            }
            pass.set_pipeline(if i == 0 { &self.solid } else { &self.lines });
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..*count, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn capture() -> ProjectHardwareCapture {
        let catalog = spiling_contracts::hardware::builtin_catalog().unwrap();
        ProjectHardwareCapture {
            setup: catalog.setups[0].clone(),
            plate: catalog.plates[0].clone(),
            face: "textured".into(),
            accessories: vec![],
        }
    }
    #[test]
    fn bed_uses_selected_face_and_single_installed_datum_without_mutating_capture() {
        let mut h = capture();
        h.plate.surface_z_mm = 7.0;
        h.setup.printer.build_envelope.min[2] = 7.0;
        h.plate.thickness_mm = 2.0;
        h.plate.faces[0].usable_mm.min[0] = 13.0;
        h.validate().unwrap();
        let before = h.clone();
        let plate = Plate::Hardware(Arc::new(h));
        assert_eq!(
            plate.bounds().min,
            [13.0, before.plate.faces[0].usable_mm.min[1], 7.0]
        );
        let g = Geometry::new(&plate, DVec3::new(100.0, 100.0, 0.0), true).unwrap();
        let z = g.surface.iter().map(|v| v.position[2]).collect::<Vec<_>>();
        assert!(z.contains(&5.0) && z.contains(&7.0));
        assert!(!z.contains(&3.0));
        if let Plate::Hardware(h) = plate {
            assert_eq!(*h, before);
        }
    }
    #[test]
    fn maximum_fixture_and_keepout_display_fits_the_reserved_pool() {
        let mut h = capture();
        let b = AabbMm {
            min: [1.0, 2.0, 0.0],
            max: [2.0, 3.0, 4.0],
        };
        h.plate.faces[0].keepouts_mm = vec![b; 16];
        h.accessories = (0..16)
            .map(|i| spiling_contracts::hardware::AccessoryDefinition {
                id: format!("fixture-{i}"),
                label: "Fixture".into(),
                mounting_interface: h.plate.mounting_interface.clone(),
                effect: AccessoryEffect::Fixture { bounds_mm: b },
            })
            .collect();
        h.validate().unwrap();
        let g = Geometry::new(&Plate::Hardware(Arc::new(h)), DVec3::ZERO, true).unwrap();
        let bytes = (g.surface.capacity() + g.lines.capacity() + g.height.capacity())
            * std::mem::size_of::<LineVertex>();
        // CPU staging plus two GPU generations fit the host's permanent reservation.
        assert!(bytes * 3 < crate::scene::CONTEXT_DISPLAY_RESERVE_BYTES);
        assert!(Plate::current(None, None).is_none());
    }
    #[test]
    fn plate_display_refuses_lost_precision_and_bounds_work_for_large_beds() {
        let mut h = capture();
        h.plate.faces[0].usable_mm.max[0] = 100000.0;
        h.setup.printer.build_envelope.max[0] = 100000.0;
        let plate = Plate::Hardware(Arc::new(h));
        let g = Geometry::new(&plate, DVec3::ZERO, false).unwrap();
        assert!(g.lines.len() < 1024);
        assert!(Geometry::new(&plate, DVec3::splat(1.0e14), false).is_err());
    }
}
