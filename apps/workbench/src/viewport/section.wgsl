// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
struct Camera { matrix:mat4x4<f32>, selected:vec4<u32>, tool_color:vec4<f32>, plane:vec4<f32>, cut:vec4<f32>,context:vec4<f32>,emphasis:vec4<f32>,participants:array<vec4<u32>,2> };
@group(0) @binding(0) var<uniform> camera:Camera;
struct Output { @builtin(position) position:vec4<f32>, @location(0) uv:vec2<f32>,@location(1) @interpolate(flat) occurrence:u32 };
@vertex fn vs(@builtin(vertex_index) index:u32,@builtin(instance_index) instance:u32)->Output {
    let n=camera.plane.xyz;
    let u=normalize(cross(n,select(vec3(0.0,0.0,1.0),vec3(1.0,0.0,0.0),abs(n.z)>0.9)));
    let v=cross(n,u);
    let corners=array<vec2<f32>,6>(vec2(-1.0,-1.0),vec2(1.0,-1.0),vec2(1.0,1.0),vec2(-1.0,-1.0),vec2(1.0,1.0),vec2(-1.0,1.0));
    let uv=corners[index]*camera.cut.w;
    let point=-camera.plane.w*n+uv.x*u+uv.y*v;
    var out:Output; out.position=camera.matrix*vec4(point,1.0); out.uv=uv; out.occurrence=instance; return out;
}
@fragment fn source_fs(input:Output)->@location(0) vec4<f32> {
    if camera.emphasis.w>0.0 {
        let word=camera.participants[input.occurrence/128u][(input.occurrence/32u)%4u];
        let participant=(word & (1u << (input.occurrence%32u)))!=0u;
        let pattern=array<u32,16>(0u,8u,2u,10u,12u,4u,14u,6u,3u,11u,1u,9u,15u,7u,13u,5u);
        let pixel=(u32(input.position.x)%4u)+(u32(input.position.y)%4u)*4u;
        if pattern[pixel]>=select(3u,12u,participant) { discard; }
        if !participant { return vec4(camera.emphasis.rgb,1.0); }
    }
    let hatch=select(0.0,0.13,fract((input.uv.x+input.uv.y)*0.7)<0.14);
    return vec4(vec3(0.28,0.47,0.64)+hatch,1.0);
}
@fragment fn actual_fs(input:Output)->@location(0) vec4<f32> {
    return vec4(select(vec3(1.0,0.64,0.27),camera.tool_color.xyz,camera.tool_color.w>0.0),1.0);
}

@vertex fn clear_vs(@builtin(vertex_index) index:u32)->@builtin(position) vec4<f32> {
    let p=array<vec2<f32>,3>(vec2(-1.0,-1.0),vec2(3.0,-1.0),vec2(-1.0,3.0));
    return vec4(p[index],0.0,1.0);
}
@fragment fn clear_fs()->@location(0) vec4<f32> { return vec4(0.0); }
