// Voxel shader. Vertices are the engine's 8-byte packed format (see blockforge::mesh):
//   x: pos_x(8) | pos_y(8)<<8 | pos_z(8)<<16 | face(3)<<24 | ao(2)<<27   (pos in quarter blocks)
//   y: texture(16) | sky(4)<<16 | block_light(4)<<20
// The chunk origin comes from a per-instance attribute, so one static index buffer and
// one draw call per chunk is all the CPU has to do.

struct Globals {
    view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,
    fog_color: vec4<f32>,
    fog_params: vec4<f32>,   // x = fog start, y = fog end, z = alpha, w = aspect ratio
    screen: vec4<f32>,       // x = width px, y = height px
    flags: vec4<f32>,        // x = fog enabled (1 or 0)
    palette: array<vec4<f32>, 32>,
};

@group(0) @binding(0) var<uniform> g: Globals;

struct VsIn {
    @location(0) packed: vec2<u32>,
    @location(1) origin: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) world: vec3<f32>,
};

fn face_normal(f: u32) -> vec3<f32> {
    var n = array<vec3<f32>, 6>(
        vec3<f32>(1.0, 0.0, 0.0),
        vec3<f32>(-1.0, 0.0, 0.0),
        vec3<f32>(0.0, 1.0, 0.0),
        vec3<f32>(0.0, -1.0, 0.0),
        vec3<f32>(0.0, 0.0, 1.0),
        vec3<f32>(0.0, 0.0, -1.0)
    );
    return n[min(f, 5u)];
}

@vertex
fn vs_main(v: VsIn) -> VsOut {
    let a = v.packed.x;
    let b = v.packed.y;
    let face = (a >> 24u) & 7u;
    // Merged (greedy) quads are grown by a hair so they overlap their neighbours and hide
    // pixel cracks at T-junctions. b bits 24/25 say which side of the quad this corner is on
    // along the face's two tangent axes; bit 26 marks a merged quad.
    let axis = face >> 1u;
    let su = f32((b >> 24u) & 1u) * 2.0 - 1.0;
    let sv = f32((b >> 25u) & 1u) * 2.0 - 1.0;
    let merged = f32((b >> 26u) & 1u);
    let e1 = vec3<f32>(f32(axis == 2u), f32(axis == 0u), f32(axis == 1u));
    let e2 = vec3<f32>(f32(axis == 1u), f32(axis == 2u), f32(axis == 0u));
    let grow = (e1 * su + e2 * sv) * (0.002 * merged);
    let local = vec3<f32>(f32(a & 255u), f32((a >> 8u) & 255u), f32((a >> 16u) & 255u)) * 0.25 + grow;
    let ao = (a >> 27u) & 3u;
    let tex = b & 65535u;
    let sky = f32((b >> 16u) & 15u) / 15.0;
    let blk = f32((b >> 20u) & 15u) / 15.0;

    let world = v.origin.xyz + local;
    let n = face_normal(face);
    let sun = normalize(vec3<f32>(0.4, 0.9, 0.3));
    let diffuse = max(dot(n, sun), 0.0);
    let light = 0.45 + 0.55 * diffuse;
    let ao_f = 0.55 + 0.15 * f32(ao);
    let base = g.palette[min(tex, 31u)].rgb;

    var o: VsOut;
    o.clip = g.view_proj * vec4<f32>(world, 1.0);
    o.color = base * light * ao_f * max(sky, 0.25) + base * blk;
    o.world = world;
    return o;
}

@fragment
fn fs_main(f: VsOut) -> @location(0) vec4<f32> {
    let d = distance(f.world, g.cam_pos.xyz);
    let t = clamp((d - g.fog_params.x) / (g.fog_params.y - g.fog_params.x), 0.0, 1.0) * g.flags.x;
    let col = mix(f.color, g.fog_color.rgb, t);
    return vec4<f32>(col, g.fog_params.z);
}

// ---- HUD: coloured triangles in pixel coordinates (origin top-left) ----
struct HudIn {
    @location(0) pos: vec2<f32>,
    @location(1) color: vec4<f32>,
};

struct HudOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_hud(v: HudIn) -> HudOut {
    var o: HudOut;
    let ndc = vec2<f32>(v.pos.x / g.screen.x * 2.0 - 1.0, 1.0 - v.pos.y / g.screen.y * 2.0);
    o.clip = vec4<f32>(ndc, 0.0, 1.0);
    o.color = v.color;
    return o;
}

@fragment
fn fs_hud(f: HudOut) -> @location(0) vec4<f32> {
    // HUD colours are authored as sRGB bytes; the surface is sRGB, so go to linear first.
    return vec4<f32>(pow(f.color.rgb, vec3<f32>(2.2)), f.color.a);
}
