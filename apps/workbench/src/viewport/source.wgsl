// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
struct Camera { view_projection: mat4x4<f32>, selected: vec4<u32> };
struct Instance { rotation: vec4<f32>, translation: vec4<f32> };
@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<storage,read> mesh: array<u32>;
@group(0) @binding(2) var<storage,read> instances: array<Instance>;
fn rotate(q:vec4<f32>,v:vec3<f32>)->vec3<f32> {return v+2.0*cross(q.xyz,cross(q.xyz,v)+q.w*v);}
struct Output { @builtin(position) position:vec4<f32>, @location(0) normal:vec3<f32>, @location(1) @interpolate(flat) selected:u32 };
@vertex fn vs(@builtin(vertex_index) vertex:u32,@builtin(instance_index) instance:u32)->Output {
    let vertices=mesh[2]; let indices=mesh[3];
    let index=mesh[16u+vertices*6u+vertex];
    let position=vec3<f32>(bitcast<f32>(mesh[16u+index*3u]),bitcast<f32>(mesh[17u+index*3u]),bitcast<f32>(mesh[18u+index*3u]));
    let offset=16u+vertices*3u+index*3u;
    let normal=vec3<f32>(bitcast<f32>(mesh[offset]),bitcast<f32>(mesh[offset+1u]),bitcast<f32>(mesh[offset+2u]));
    let model=instances[instance];
    var out:Output;
    out.position=camera.view_projection*vec4<f32>(rotate(model.rotation,position)+model.translation.xyz,1.0);
    out.normal=rotate(model.rotation,normal);
    let face=mesh[16u+vertices*6u+indices+vertex/3u];
    out.selected=select(0u,1u,face==camera.selected.x && u32(model.translation.w)==camera.selected.y);
    return out;
}
@fragment fn fs(input:Output,@builtin(front_facing) front:bool)->@location(0) vec4<f32> {
    let normal=normalize(input.normal)*select(-1.0,1.0,front);
    let light=0.28+0.72*max(dot(normal,normalize(vec3<f32>(0.4,-0.5,0.8))),0.0);
    let base=select(vec3<f32>(0.52,0.64,0.77),vec3<f32>(1.0,0.67,0.22),input.selected!=0u);
    return vec4<f32>(base*light,1.0);
}
