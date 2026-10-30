// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
struct Camera {view_projection:mat4x4<f32>,selected:vec4<u32>,tool_color:vec4<f32>};
@group(0) @binding(0) var<uniform> camera:Camera;
struct Output {@builtin(position) position:vec4<f32>,@location(0) color:vec3<f32>};
@vertex fn vs(@location(0) position:vec3<f32>,@location(1) color:vec3<f32>)->Output {
    var out:Output;out.position=camera.view_projection*vec4<f32>(position,1.0);out.position.z-=0.000001*out.position.w;
    out.color=select(color,camera.tool_color.xyz,camera.tool_color.w>0.0);return out;
}
@vertex fn vs_point(@location(0) position:vec3<f32>,@location(1) color:vec3<f32>,@builtin(vertex_index) vertex:u32)->Output {
    var out:Output;out.position=camera.view_projection*vec4<f32>(position,1.0);
    let corners=array<vec2<f32>,6>(vec2(-1.0,-1.0),vec2(1.0,-1.0),vec2(1.0,1.0),vec2(-1.0,-1.0),vec2(1.0,1.0),vec2(-1.0,1.0));
    out.position=vec4(out.position.xy+corners[vertex]*out.position.w*5.0/max(vec2<f32>(camera.selected.zw),vec2(1.0)),out.position.zw);
    out.color=color;return out;
}
@fragment fn fs(input:Output)->@location(0) vec4<f32>{return vec4<f32>(input.color,1.0);}
