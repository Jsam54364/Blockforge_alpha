// Voxel shader. Vertices are the engine's 8-byte packed format (see blockforge::mesh):
//   x: pos_x(8) | pos_y(8)<<8 | pos_z(8)<<16 | face(3)<<24 | ao(2)<<27   (pos in quarter blocks)
//   y: texture(16) | sky(4)<<16 | block_light(4)<<20 | grow(3)<<24 | tex_top(2)<<27
// The chunk origin comes from a per-instance attribute, so one static index buffer and
// one draw call per chunk is all the CPU has to do.

struct Globals {
    view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,
    fog_color: vec4<f32>,
    fog_params: vec4<f32>,   // x = fog start, y = fog end, z = alpha, w = aspect ratio
    screen: vec4<f32>,       // x = width px, y = height px
    flags: vec4<f32>,        // x = fog enabled, y = seconds, z = water texture layer, w = tint levels ready (bits)
    palette: array<vec4<f32>, 32>,
    lod_window: vec4<f32>,   // xy = chunk column at bit 0 of lod_mask
    lod_mask: array<vec4<u32>, 32>, // 64 x 64 bits: chunk columns drawn by real terrain
    tint_window: vec4<f32>,  // tint map level centres in blocks: x0, z0, x1, z1
};

@group(0) @binding(0) var<uniform> g: Globals;
// Block textures: one layer per texture id, mipmapped, repeating (see gfx::block_textures).
@group(0) @binding(1) var blocks: texture_2d_array<f32>;
@group(0) @binding(2) var blocks_sampler: sampler;
// Biome colours (rgb) and brightness variation (a) around the camera; see gfx/tint.rs.
@group(0) @binding(3) var tint_map: texture_2d_array<f32>;
@group(0) @binding(4) var tint_sampler: sampler;

struct VsIn {
    @location(0) packed: vec2<u32>,
    @location(1) origin: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    // Light reaching the surface (multiplies the texture).
    @location(0) color: vec3<f32>,
    @location(1) world: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) @interpolate(flat) layer: u32,
    @location(4) @interpolate(flat) face: u32,
};

// Texture coordinates for a face, from a position in blocks: one texture per block, upright
// on the sides (v grows downwards from the top of each block). Positions are relative to a
// whole-block origin (the chunk or LOD section), so they stay exact far from the world origin.
fn face_uv(face: u32, p: vec3<f32>) -> vec2<f32> {
    switch face {
        case 0u: { return vec2<f32>(-p.z, -p.y); }
        case 1u: { return vec2<f32>(p.z, -p.y); }
        case 2u: { return vec2<f32>(p.x, p.z); }
        case 3u: { return vec2<f32>(p.x, -p.z); }
        case 4u: { return vec2<f32>(p.x, -p.y); }
        default: { return vec2<f32>(-p.x, -p.y); }
    }
}

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

    var o: VsOut;
    o.clip = g.view_proj * vec4<f32>(world, 1.0);
    o.color = vec3<f32>(light * ao_f * max(sky, 0.25) + blk);
    o.world = world;
    // Side faces: lift the texture so its top row starts at the quad's top (half-slabs).
    let lift = f32((b >> 27u) & 3u) * 0.25;
    o.uv = face_uv(face, local + vec3<f32>(0.0, lift, 0.0));
    o.layer = tex;
    o.face = face;
    return o;
}

// ---- Level-of-detail terrain (see blockforge::lod) ----
// Vertex: a = x(6) | z(6)<<6 | face(3)<<12 | in_x(2)<<15 | in_z(2)<<17 | top(1)<<19 | tex(8)<<20
//         b = y in quarter blocks (i32)
// Instance: x = section origin x, y = origin z, z = cell size in blocks, w = 1 if masked.
@vertex
fn vs_lod(v: VsIn) -> VsOut {
    let a = v.packed.x;
    let cell = v.origin.z;
    let face = (a >> 12u) & 7u;
    let inx = f32((a >> 15u) & 3u) - 1.0;
    let inz = f32((a >> 17u) & 3u) - 1.0;
    let top = f32((a >> 19u) & 1u);
    let tex = (a >> 20u) & 255u;
    let base = vec3<f32>(
        v.origin.x + f32(a & 63u) * cell,
        f32(bitcast<i32>(v.packed.y)) * 0.25,
        v.origin.y + f32((a >> 6u) & 63u) * cell
    );

    var o: VsOut;
    // Where the real terrain is drawn, drop the quad: every corner looks half a cell into
    // the quad, lands in the same chunk column, and is sent outside the clip volume.
    if (v.origin.w > 0.5) {
        let p = vec2<f32>(base.x + inx * 0.5 * cell, base.z + inz * 0.5 * cell);
        let col = vec2<i32>(floor(p / 32.0)) - vec2<i32>(g.lod_window.xy);
        if (col.x >= 0 && col.y >= 0 && col.x < 64 && col.y < 64) {
            let bit = u32(col.y * 64 + col.x);
            let word = g.lod_mask[bit / 128u][(bit / 32u) % 4u];
            if (((word >> (bit % 32u)) & 1u) == 1u) {
                o.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
                o.color = vec3<f32>(0.0);
                o.world = base;
                return o;
            }
        }
    }

    // Grow each quad by a few hundredths of a cell over its neighbours, hiding the pixel
    // cracks merged quads leave at T-junctions (they show against the sky far away). Side
    // faces only grow downwards: grown upwards their top edge pokes through flat ground and
    // water next to them as a dark line (the top face's own growth covers that edge).
    let eps = 0.03 * cell;
    let up = (top - 1.0) * eps;
    var grow = vec3<f32>(-inx * eps, 0.0, -inz * eps);
    if (face == 0u || face == 1u) {
        grow = vec3<f32>(0.0, up, -inz * eps);
    } else if (face == 4u || face == 5u) {
        grow = vec3<f32>(-inx * eps, up, 0.0);
    }
    let world = base + grow;

    let n = face_normal(face);
    let sun = normalize(vec3<f32>(0.4, 0.9, 0.3));
    let light = 0.45 + 0.55 * max(dot(n, sun), 0.0);
    o.clip = g.view_proj * vec4<f32>(world, 1.0);
    o.color = vec3<f32>(light);
    o.world = world;
    o.uv = face_uv(face, world - vec3<f32>(v.origin.x, 0.0, v.origin.y));
    o.layer = tex;
    o.face = face;
    return o;
}

// Grass colour of the plains (#91BD59, linear), until the tint map has arrived.
const DEFAULT_TINT = vec3<f32>(0.283, 0.509, 0.100);

// Biome colour (rgb) and brightness variation (a, 0.5 = none) at a world position: the fine
// level near the camera, fading into the coarse one before the fine one ends.
fn tint_at(p: vec2<f32>) -> vec4<f32> {
    let ready = u32(g.flags.w + 0.5);
    var t = vec4<f32>(DEFAULT_TINT, 0.5);
    if ((ready & 2u) != 0u) {
        t = textureSampleLevel(tint_map, tint_sampler, p / (128.0 * 256.0), 1, 0.0);
    }
    if ((ready & 1u) != 0u) {
        let d = max(abs(p.x - g.tint_window.x), abs(p.y - g.tint_window.y));
        let w = 1.0 - smoothstep(1700.0, 1950.0, d);
        if (w > 0.0) {
            t = mix(t, textureSampleLevel(tint_map, tint_sampler, p / (16.0 * 256.0), 0, 0.0), w);
        }
    }
    return t;
}

fn hash_block(p: vec3<f32>) -> u32 {
    let q = vec3<i32>(floor(p));
    var h = (u32(q.x) * 0x8da6b343u) ^ (u32(q.y) * 0xd8163841u) ^ (u32(q.z) * 0xcb1ab31fu);
    h = (h ^ (h >> 15u)) * 0x2c1b3c6du;
    return h ^ (h >> 12u);
}

// Every block turns and mirrors its texture its own way (tops: 8 ways; sides: mirrored or
// not, so grass fringes stay on top), which makes the repeats much harder to see.
fn block_uv(face: u32, uv: vec2<f32>, world: vec3<f32>) -> vec2<f32> {
    let h = hash_block(world - face_normal(face) * 0.1);
    let cell = floor(uv);
    var f = uv - cell;
    if (face == 2u || face == 3u) {
        if ((h & 4u) != 0u) {
            f.x = 1.0 - f.x;
        }
        let r = h & 3u;
        if (r == 1u) {
            f = vec2<f32>(f.y, 1.0 - f.x);
        } else if (r == 2u) {
            f = vec2<f32>(1.0 - f.x, 1.0 - f.y);
        } else if (r == 3u) {
            f = vec2<f32>(1.0 - f.y, f.x);
        }
    } else if ((h & 1u) != 0u) {
        f.x = 1.0 - f.x;
    }
    return cell + f;
}

// One travelling wave: adds its slope (d height / dx, d height / dz) to s.
fn wave(s: ptr<function, vec2<f32>>, p: vec2<f32>, dir: vec2<f32>, wavelength: f32, amp: f32, speed: f32) {
    let k = 6.2831853 / wavelength;
    let c = cos(k * (dot(dir, p) - speed * g.flags.y));
    *s += dir * (amp * k * c);
}

// Water: waves bend the surface normal; the surface shows the deep water colour looking down
// and the sky's reflection at grazing angles (Fresnel), plus the sun's glint. Returns the
// colour and how opaque it is (the translucent pass blends with it; LOD water is opaque).
fn water_shade(world: vec3<f32>, face: u32, light: vec3<f32>) -> vec4<f32> {
    let to_cam = g.cam_pos.xyz - world;
    let v = normalize(to_cam);
    var n = face_normal(face);
    if (face == 2u) {
        var s = vec2<f32>(0.0);
        let p = world.xz;
        wave(&s, p, vec2<f32>(0.8, 0.6), 7.0, 0.09, 1.4);
        wave(&s, p, vec2<f32>(-0.42, 0.91), 4.3, 0.05, 1.1);
        // Small ripples only up close: further out they would only shimmer.
        let near = 1.0 / (1.0 + length(to_cam) / 25.0);
        wave(&s, p, vec2<f32>(0.96, -0.28), 2.1, 0.022 * near, 0.8);
        wave(&s, p, vec2<f32>(-0.71, -0.71), 1.3, 0.011 * near, 0.6);
        // Far away the waves average out: calmer, more mirror-like.
        s *= 1.0 / (1.0 + length(to_cam) / 60.0);
        n = normalize(vec3<f32>(-s.x, 1.0, -s.y));
    }
    let fres = 0.02 + 0.98 * pow(1.0 - clamp(dot(n, v), 0.0, 1.0), 5.0);
    let r = reflect(-v, n);
    let sky = mix(g.fog_color.rgb, vec3<f32>(0.12, 0.28, 0.72), clamp(r.y * 1.4, 0.0, 1.0));
    // The water body's own colour, which also tints whatever is seen through it.
    let deep = vec3<f32>(0.018, 0.11, 0.16) * light;
    let sun = normalize(vec3<f32>(0.4, 0.9, 0.3));
    let glint = pow(max(dot(n, normalize(sun + v)), 0.0), 220.0) * 2.5;
    let col = mix(deep, sky, fres) + vec3<f32>(glint);
    return vec4<f32>(col, mix(0.8, 0.97, fres));
}

@fragment
fn fs_main(f: VsOut) -> @location(0) vec4<f32> {
    // Derivatives first (they need uniform control flow): mip levels for the turned textures.
    let ddx = dpdx(f.uv);
    let ddy = dpdy(f.uv);
    let d = distance(f.world, g.cam_pos.xyz);
    let fog = clamp((d - g.fog_params.x) / (g.fog_params.y - g.fog_params.x), 0.0, 1.0) * g.flags.x;
    if (f.layer == u32(g.flags.z + 0.5)) {
        let w = water_shade(f.world, f.face, f.color);
        return vec4<f32>(mix(w.rgb, g.fog_color.rgb, fog), w.a);
    }
    let tex = textureSampleGrad(blocks, blocks_sampler, block_uv(f.face, f.uv, f.world), f.layer, ddx, ddy);
    let t = tint_at(f.world.xz);
    // Alpha says how much of the biome colour to apply: 0 = all (grass), 1 = none.
    let albedo = mix(tex.rgb * t.rgb, tex.rgb, tex.a) * mix(0.86, 1.14, t.a);
    let col = mix(albedo * f.color, g.fog_color.rgb, fog);
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
