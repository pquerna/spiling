// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
struct Camera {view_projection:mat4x4<f32>,selected:vec4<u32>,tool_color:vec4<f32>,plane:vec4<f32>,cut:vec4<f32>,context:vec4<f32>};
@group(0) @binding(0) var<uniform> camera:Camera;
struct Output {@builtin(position) position:vec4<f32>,@location(0) color:vec3<f32>,@location(1) world:vec3<f32>};
@vertex fn vs(@location(0) position:vec3<f32>,@location(1) color:vec3<f32>)->Output {
    var out:Output;out.world=position;out.position=camera.view_projection*vec4<f32>(position,1.0);out.position.z-=0.000001*out.position.w;
    out.color=select(color,camera.tool_color.xyz,camera.tool_color.w>0.0);return out;
}
@vertex fn vs_point(@location(0) position:vec3<f32>,@location(1) color:vec3<f32>,@builtin(vertex_index) vertex:u32)->Output {
    var out:Output;out.world=position;out.position=camera.view_projection*vec4<f32>(position,1.0);
    let corners=array<vec2<f32>,6>(vec2(-1.0,-1.0),vec2(1.0,-1.0),vec2(1.0,1.0),vec2(-1.0,-1.0),vec2(1.0,1.0),vec2(-1.0,1.0));
    out.position=vec4(out.position.xy+corners[vertex]*out.position.w*5.0/max(vec2<f32>(camera.selected.zw),vec2(1.0)),out.position.zw);
    out.color=color;return out;
}
fn seam_point(position:vec3<f32>,color:vec3<f32>,vertex:u32,size:f32)->Output {
    var out:Output;out.world=position;out.position=camera.view_projection*vec4<f32>(position,1.0);
    let corners=array<vec2<f32>,6>(vec2(0.0,-1.0),vec2(1.0,0.0),vec2(0.0,1.0),vec2(0.0,-1.0),vec2(0.0,1.0),vec2(-1.0,0.0));
    out.position=vec4(out.position.xy+corners[vertex]*out.position.w*size/max(vec2<f32>(camera.selected.zw),vec2(1.0)),out.position.zw);
    out.color=color;return out;
}
@vertex fn vs_seam(@location(0) position:vec3<f32>,@location(1) color:vec3<f32>,@builtin(vertex_index) vertex:u32)->Output {
    return seam_point(position,color,vertex,9.0);
}
@vertex fn vs_selected_seam(@location(0) position:vec3<f32>,@location(1) color:vec3<f32>,@builtin(vertex_index) vertex:u32)->Output {
    return seam_point(position,vec3(1.0,0.78,0.2),vertex,13.0);
}
struct StartOutput {
    @builtin(position) position:vec4<f32>,
    @location(0) world:vec3<f32>,
    @location(1) uv:vec2<f32>,
};
@vertex fn vs_start(@location(0) position:vec3<f32>,@location(1) color:vec3<f32>,@builtin(vertex_index) vertex:u32)->StartOutput {
    let corners=array<vec2<f32>,6>(vec2(-1.0,-1.0),vec2(1.0,-1.0),vec2(1.0,1.0),vec2(-1.0,-1.0),vec2(1.0,1.0),vec2(-1.0,1.0));
    var out:StartOutput;
    out.world=position;
    out.uv=corners[vertex];
    out.position=camera.view_projection*vec4(position,1.0);
    out.position=vec4(out.position.xy+out.uv*out.position.w*9.0/max(vec2<f32>(camera.selected.zw),vec2(1.0)),out.position.zw);
    return out;
}
@fragment fn fs_start(input:StartOutput)->@location(0) vec4<f32> {
    let radius=length(input.uv);
    let feather=max(fwidth(radius),0.001);
    if camera.cut.x>0.0 && dot(camera.plane,vec4(input.world,1.0))>0.000001 {discard;}
    let alpha=1.0-smoothstep(1.0-feather,1.0,radius);
    if alpha<=0.0 {discard;}
    // A dark rim keeps the white center legible on both workbench themes.
    let white=1.0-smoothstep(0.65-feather,0.65+feather,radius);
    return vec4(mix(vec3(0.12),vec3(1.0),white),alpha);
}
@fragment fn fs(input:Output)->@location(0) vec4<f32>{if camera.cut.x>0.0 && dot(camera.plane,vec4(input.world,1.0))>0.000001 {discard;} return select(vec4<f32>(input.color,1.0),camera.context,camera.context.w>0.0);}
