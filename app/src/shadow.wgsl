// Shadow maps (see app/src/shadows.rs), seen from the sun (or the moon): the depth of what
// is nearest the light, written as a plain float colour (not sampled as a depth texture:
// some phone GPUs get depth comparisons wrong). The terrain's packed vertices and the
// character's triangles, nothing else.

struct ShadowView {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> s: ShadowView;

// The position part of the terrain's packed vertex (shader.wgsl, vs_main).
@vertex
fn vs_chunk(@location(0) packed: vec2<u32>, @location(1) origin: vec4<f32>) -> @builtin(position) vec4<f32> {
    let a = packed.x;
    let local = vec3<f32>(f32(a & 255u), f32((a >> 8u) & 255u), f32((a >> 16u) & 255u)) * 0.25;
    return s.view_proj * vec4<f32>(origin.xyz + local, 1.0);
}

@vertex
fn vs_char(@location(0) pos: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) uv: vec2<f32>) -> @builtin(position) vec4<f32> {
    return s.view_proj * vec4<f32>(pos, 1.0);
}

@fragment
fn fs_depth(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(p.z, 0.0, 0.0, 1.0);
}
