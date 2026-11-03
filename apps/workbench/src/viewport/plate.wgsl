// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

struct Camera { view_projection:mat4x4<f32>, selected:vec4<u32>, tool_color:vec4<f32>, plane:vec4<f32>, cut:vec4<f32>, context:vec4<f32> };
@group(0) @binding(0) var<uniform> camera:Camera;
struct Output { @builtin(position) position:vec4<f32>, @location(0) color:vec3<f32> };
@vertex fn vs(@location(0) position:vec3<f32>, @location(1) color:vec3<f32>)->Output {
    var out:Output; out.position=camera.view_projection*vec4(position,1.0); out.color=color; return out;
}
// Hardware is contextual display, never source geometry or a section cap.
@fragment fn fs(input:Output)->@location(0) vec4<f32>{ return vec4(input.color,1.0); }
