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

// See-through foliage (leaves, grass): only its leafy texels cast, so light comes through the
// gaps in a canopy and grass casts blades, not squares. The textures are the main shader's
// (group 1); a coarser mip level is plenty for a shadow.
@group(1) @binding(0) var blocks: texture_2d_array<f32>;
@group(1) @binding(1) var blocks_sampler: sampler;

struct CutOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) layer: u32,
};

// shader.wgsl's face_uv.
fn face_uv(face: u32, p: vec3<f32>) -> vec2<f32> {
    switch face {
        case 0u: { return vec2<f32>(-p.z, -p.y); }
        case 1u: { return vec2<f32>(p.z, -p.y); }
        case 2u: { return vec2<f32>(p.x, p.z); }
        case 3u: { return vec2<f32>(p.x, -p.z); }
        case 4u: { return vec2<f32>(p.x, -p.y); }
        case 6u: { return vec2<f32>(p.z, -p.y); }
        case 7u: { return vec2<f32>(p.x, -p.y); }
        default: { return vec2<f32>(-p.x, -p.y); }
    }
}

@vertex
fn vs_cutout(@location(0) packed: vec2<u32>, @location(1) origin: vec4<f32>) -> CutOut {
    let a = packed.x;
    let b = packed.y;
    let face = (a >> 24u) & 7u;
    var local = vec3<f32>(f32(a & 255u), f32((a >> 8u) & 255u), f32((a >> 16u) & 255u)) * 0.25;
    // A plant's offset in its cell, as shader.wgsl's vs_main moves it.
    if (face >= 6u) {
        let top = f32((b >> 27u) & 1u);
        let far = f32((b >> 28u) & 1u);
        let along = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(1.0, 0.0, 0.0), face == 7u);
        let cell = floor(origin.xyz + local - along * far - vec3<f32>(0.0, top, 0.0) + vec3<f32>(0.01, 0.01, 0.01));
        let q = vec3<i32>(floor(cell + vec3<f32>(0.5)));
        var h = (u32(q.x) * 0x8da6b343u) ^ (u32(q.y) * 0xd8163841u) ^ (u32(q.z) * 0xcb1ab31fu);
        h = (h ^ (h >> 15u)) * 0x2c1b3c6du;
        h = h ^ (h >> 12u);
        let off = vec3<f32>(f32(h & 15u) - 7.5, 0.0, f32((h >> 4u) & 15u) - 7.5) * (0.75 / 32.0);
        local += round(off * 32.0) / 32.0;
    }
    var o: CutOut;
    o.pos = s.view_proj * vec4<f32>(origin.xyz + local, 1.0);
    o.uv = face_uv(face, local);
    o.layer = packed.y & 65535u;
    return o;
}

@fragment
fn fs_cutout(o: CutOut) -> @location(0) vec4<f32> {
    let a = textureSampleLevel(blocks, blocks_sampler, o.uv, o.layer, 1.0).a;
    // (app/src/textures.rs: tinted foliage keeps alpha in 0..0.5, untinted in 0.5..1.)
    let cover = select((a - 0.502) / 0.498, a / 0.498, a < 0.5);
    if (cover < 0.5) {
        discard;
    }
    return vec4<f32>(o.pos.z, 0.0, 0.0, 1.0);
}
