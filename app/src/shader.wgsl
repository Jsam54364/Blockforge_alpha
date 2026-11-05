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
    let local = vec3<f32>(f32(a & 255u), f32((a >> 8u) & 255u), f32((a >> 16u) & 255u)) * 0.25;
    let face = (a >> 24u) & 7u;
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
    let t = clamp((d - g.fog_params.x) / (g.fog_params.y - g.fog_params.x), 0.0, 1.0);
    let col = mix(f.color, g.fog_color.rgb, t);
    return vec4<f32>(col, g.fog_params.z);
}

// ---- crosshair: two thin rectangles generated from the vertex index, no buffers ----
@vertex
fn vs_ui(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, 1.0)
    );
    let c = corners[i % 6u];
    let aspect = g.fog_params.w;
    let len = 0.025;
    let thick = 0.003;
    var hs = vec2<f32>(len / aspect, thick);
    if (i >= 6u) {
        hs = vec2<f32>(thick / aspect, len);
    }
    return vec4<f32>(c * hs, 0.0, 1.0);
}

@fragment
fn fs_ui() -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 1.0, 1.0, 0.85);
}
