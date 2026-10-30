struct Camera {view_projection:mat4x4<f32>,selected:vec4<u32>};
@group(0) @binding(0) var<uniform> camera:Camera;
struct Output {@builtin(position) position:vec4<f32>,@location(0) color:vec3<f32>};
@vertex fn vs(@location(0) position:vec3<f32>,@location(1) color:vec3<f32>)->Output {
    var out:Output;out.position=camera.view_projection*vec4<f32>(position,1.0);out.position.z-=0.000001*out.position.w;out.color=color;return out;
}
@fragment fn fs(input:Output)->@location(0) vec4<f32>{return vec4<f32>(input.color,1.0);}
