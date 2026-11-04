// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! GPU winding caps over checked display geometry; never native metrology.
use egui_wgpu::wgpu;
pub(super) fn depth(stencil: bool) -> wgpu::DepthStencilState {
    let face = |op| wgpu::StencilFaceState {
        compare: wgpu::CompareFunction::Always,
        fail_op: wgpu::StencilOperation::Keep,
        depth_fail_op: wgpu::StencilOperation::Keep,
        pass_op: op,
    };
    wgpu::DepthStencilState {
        format: egui_wgpu::depth_format_from_bits(24, 8).expect("depth/stencil format"),
        depth_write_enabled: Some(!stencil),
        depth_compare: Some(if stencil {
            wgpu::CompareFunction::Always
        } else {
            wgpu::CompareFunction::LessEqual
        }),
        stencil: if stencil {
            wgpu::StencilState {
                front: face(wgpu::StencilOperation::IncrementWrap),
                back: face(wgpu::StencilOperation::DecrementWrap),
                read_mask: 255,
                write_mask: 255,
            }
        } else {
            Default::default()
        },
        bias: Default::default(),
    }
}
pub(super) fn source_stencil(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    target: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    // Display chunks need not share an outward triangle winding. Per-occurrence
    // even/odd coverage preserves cavities without inferring native orientation.
    let mut state = depth(true);
    for face in [&mut state.stencil.front, &mut state.stencil.back] {
        face.pass_op = wgpu::StencilOperation::Invert;
    }
    state.stencil.read_mask = 1;
    state.stencil.write_mask = 1;
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Source section parity"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_stencil"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: target,
                blend: None,
                write_mask: wgpu::ColorWrites::empty(),
            })],
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(state),
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}
pub(super) fn cap(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    target: wgpu::TextureFormat,
    source: bool,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Section plane fill"),
        source: wgpu::ShaderSource::Wgsl(include_str!("section.wgsl").into()),
    });
    // Clear winding on both depth outcomes and on zero coverage. Every cap leaves
    // the shared stencil clean for the next source/tool and egui callback.
    let face = wgpu::StencilFaceState {
        compare: wgpu::CompareFunction::NotEqual,
        fail_op: wgpu::StencilOperation::Zero,
        depth_fail_op: wgpu::StencilOperation::Zero,
        pass_op: wgpu::StencilOperation::Zero,
    };
    let mut state = depth(false);
    state.stencil = wgpu::StencilState {
        front: face,
        back: face,
        read_mask: 255,
        write_mask: 255,
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Section filled cut faces"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some(if source { "source_fs" } else { "actual_fs" }),
            compilation_options: Default::default(),
            targets: &[Some(target.into())],
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(state),
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}

/// Clear the entire callback viewport, including rays whose plane is behind the
/// camera or outside the cap quad. No stencil can leak into another pass/frame.
pub(super) fn clear(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    target: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Section stencil reset"),
        source: wgpu::ShaderSource::Wgsl(include_str!("section.wgsl").into()),
    });
    let face = wgpu::StencilFaceState {
        compare: wgpu::CompareFunction::Always,
        fail_op: wgpu::StencilOperation::Zero,
        depth_fail_op: wgpu::StencilOperation::Zero,
        pass_op: wgpu::StencilOperation::Zero,
    };
    let mut state = depth(true);
    state.stencil = wgpu::StencilState {
        front: face,
        back: face,
        read_mask: 255,
        write_mask: 255,
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Clear section stencil"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("clear_vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("clear_fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: target,
                blend: None,
                write_mask: wgpu::ColorWrites::empty(),
            })],
        }),
        primitive: Default::default(),
        depth_stencil: Some(state),
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}
