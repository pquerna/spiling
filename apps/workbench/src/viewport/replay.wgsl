// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
struct Camera {view_projection:mat4x4<f32>,selected:vec4<u32>,tool_color:vec4<f32>,plane:vec4<f32>,cut:vec4<f32>,context:vec4<f32>};
@group(0) @binding(0) var<uniform> camera:Camera;
struct Output {@builtin(position) position:vec4<f32>,@location(0) color:vec3<f32>,@location(1) world:vec3<f32>};
@vertex fn vs(@location(0) start:vec3<f32>, @location(1) width:f32, @location(2) end:vec3<f32>, @location(3) height:f32, @location(4) kind_bits:u32, @builtin(vertex_index) vertex:u32)->Output {
 let position=select(start,end,vertex!=0u); let color=role_color(kind_bits);
    var out:Output;out.world=position;out.position=camera.view_projection*vec4<f32>(position,1.0);out.position.z-=0.000001*out.position.w;
    out.color=select(color,camera.tool_color.xyz,camera.tool_color.w>0.0);return out;
}
@vertex fn vs_point(@location(0) start:vec3<f32>, @location(1) width:f32, @location(2) end:vec3<f32>, @location(3) height:f32, @location(4) kind_bits:u32,@builtin(vertex_index) vertex:u32)->Output {
 let position=end; let color=role_color(kind_bits);
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
@vertex fn vs_seam(@location(0) start:vec3<f32>, @location(1) width:f32, @location(2) end:vec3<f32>, @location(3) height:f32, @location(4) kind_bits:u32,@builtin(vertex_index) vertex:u32)->Output {
 let position=end; let color=role_color(kind_bits);
    return seam_point(position,vec3(0.25,0.85,1.0),vertex,9.0);
}
@vertex fn vs_selected_seam(@location(0) start:vec3<f32>, @location(1) width:f32, @location(2) end:vec3<f32>, @location(3) height:f32, @location(4) kind_bits:u32,@builtin(vertex_index) vertex:u32)->Output {
 let position=end; let color=role_color(kind_bits);
    return seam_point(position,vec3(1.0,0.78,0.2),vertex,13.0);
}
struct StartOutput {
    @builtin(position) position:vec4<f32>,
    @location(0) world:vec3<f32>,
    @location(1) uv:vec2<f32>,
};
@vertex fn vs_start(@location(0) start:vec3<f32>, @location(1) width:f32, @location(2) end:vec3<f32>, @location(3) height:f32, @location(4) kind_bits:u32,@builtin(vertex_index) vertex:u32)->StartOutput {
 let position=start; let color=role_color(kind_bits);
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

fn role_color(kind_bits:u32)->vec3<f32> {
 let colors=array<vec3<f32>,16>(vec3<f32>(142.0,172.0,189.0),vec3<f32>(255.0,187.0,102.0),vec3<f32>(108.0,222.0,199.0),vec3<f32>(182.0,147.0,255.0),vec3<f32>(255.0,121.0,155.0),vec3<f32>(142.0,208.0,90.0),vec3<f32>(123.0,145.0,235.0),vec3<f32>(255.0,240.0,122.0),vec3<f32>(255.0,147.0,74.0),vec3<f32>(98.0,162.0,255.0),vec3<f32>(179.0,229.0,255.0),vec3<f32>(226.0,96.0,134.0),vec3<f32>(246.0,184.0,206.0),vec3<f32>(245.0,239.0,68.0),vec3<f32>(255.0,119.0,183.0),vec3<f32>(215.0,167.0,255.0));
 return colors[kind_bits&15u]/255.0;
}
