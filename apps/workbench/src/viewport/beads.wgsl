// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
struct Camera { matrix:mat4x4<f32>, selected:vec4<u32>, tool_color:vec4<f32> };
@group(0) @binding(0) var<uniform> camera:Camera;
struct Output {
    @builtin(position) position:vec4<f32>,
    @location(0) normal:vec3<f32>,
    @location(1) color:vec3<f32>,
};
// Two semicircles and explicit flat top/bottom edges. Elliptical bridge
// envelopes collapse the flats; they do not assert substrate flattening or sag.
fn section(index:u32, facets:u32, width:f32, height:f32, elliptical:u32)->vec4<f32> {
    let half=facets/2u;
    let left=index>half;
    let i=select(index,index-half-1u,left);
    let angle=select(-1.570796326795,1.570796326795,left)+3.14159265359*f32(i)/f32(half);
    let c=cos(angle); let s=sin(angle);
    let rx=select(height*0.5,width*0.5,elliptical!=0u);
    let middle=select(max(width-height,0.0)*0.5,0.0,elliptical!=0u);
    let radii=vec2(height*0.5,rx)/max(rx,height*0.5);
    let gradient=vec2(c,s)*radii;
    let n=gradient/max(length(gradient),1e-20);
    return vec4(select(middle,-middle,left)+rx*c,height*0.5*s,n);
}
@vertex fn vs(
    @location(0) start:vec3<f32>, @location(1) width:f32,
    @location(2) end:vec3<f32>, @location(3) height:f32,
    @location(4) sideways:vec2<f32>, @location(5) color:vec3<f32>,
    @location(6) elliptical:u32, @location(7) facets:u32,
    @builtin(vertex_index) vertex:u32
)->Output {
    let edge=vertex/12u; let slot=vertex%12u;
    let next=(edge+1u)%(facets+2u);
    // Two side triangles followed by one flat start and one flat end cap.
    let corners=array<u32,12>(edge,next,edge, next,next,edge, edge,next,edge, edge,edge,next);
    let ends=array<u32,12>(0u,0u,1u, 0u,1u,1u, 0u,0u,0u, 1u,1u,1u);
    let center=slot==8u || slot==9u;
    let cross_section=section(corners[slot],facets,width,height,elliptical);
    let endpoint=select(start,end,ends[slot]!=0u);
    let offset=select(cross_section.xy,vec2(0.0),center);
    let position=endpoint+vec3(sideways*offset.x,offset.y-height*0.5);
    let direction=vec3(sideways.y,-sideways.x,0.0);
    var out:Output;
    out.position=camera.matrix*vec4(position,1.0);
    out.normal=select(vec3(sideways*cross_section.z,cross_section.w),select(-direction,direction,ends[slot]!=0u),slot>=6u);
    out.color=select(color,camera.tool_color.xyz,camera.tool_color.w>0.0);
    return out;
}
@fragment fn fs(input:Output)->@location(0) vec4<f32> {
    let light=0.4+0.6*max(dot(input.normal/max(length(input.normal),1e-20),normalize(vec3(0.4,-0.5,0.8))),0.0);
    return vec4(input.color*light,1.0);
}
