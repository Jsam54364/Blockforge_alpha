// Voxel shader. Vertices are the engine's 8-byte packed format (see blockforge::mesh):
//   x: pos_x(8) | pos_y(8)<<8 | pos_z(8)<<16 | face(3)<<24 | ao(2)<<27 | liquid_top(1)<<29
//      (pos in quarter blocks; liquid_top: lower this corner by 1/8 block, a water surface)
//   y: texture(16) | sky(4)<<16 | block_light(4)<<20 | grow(3)<<24 | tex_top(2)<<27
// The chunk origin comes from a per-instance attribute, so one static index buffer and
// one draw call per chunk is all the CPU has to do.

struct Globals {
    view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,
    fog_params: vec4<f32>,   // x = fog start, y = fog end, z = alpha, w = aspect ratio
    screen: vec4<f32>,       // x = width px, y = height px
    flags: vec4<f32>,        // x = fog enabled, y = seconds, z = water texture layer, w = tint levels ready (bits)
    palette: array<vec4<f32>, 32>,
    lod_window: vec4<f32>,   // xy = chunk column at bit 0 of lod_mask
    lod_mask: array<vec4<u32>, 32>, // 64 x 64 bits: chunk columns drawn by real terrain
    tint_window: vec4<f32>,  // tint map level centres in blocks: x0, z0, x1, z1
    // ---- The sky (see app/src/sky.rs) ----
    sky_basis: array<vec4<f32>, 3>, // view rays: right, up (scaled to the field of view), forward
    sun: vec4<f32>,          // xyz: towards the sun; w: how much of its disc shows
    sun_color: vec4<f32>,    // rgb: direct sunlight; w: daylight (0 night .. 1 day)
    moon: vec4<f32>,         // xyz: towards the moon; w: how much of its disc shows
    sky_tint: vec4<f32>,     // x: the photographed sky's brightness; y: wide glow; z: glare; w: sunset colouring
    glow: array<vec4<f32>, 4>, // glow around the sun: (a, w1), (c1, w2), (c2, -), (e, -)
    face_light: array<vec4<f32>, 6>, // light on each face direction: sky + sun + moon
    night_rot: array<vec4<f32>, 3>, // rows: view direction -> the night photo's frame (turns with the sky)
    night_zenith: vec4<f32>, // night background straight up (rgb)
    night_horizon: vec4<f32>, // ... and at the horizon
    night: vec4<f32>,        // x: how much of the night photo (stars, moon) shows
    cloud: vec4<f32>,        // clouds: x, z of the window's corner in the world, base height, fade distance
    rays: vec4<f32>,         // god rays: xy = the sun on screen (uv), z = strength
};

@group(0) @binding(0) var<uniform> g: Globals;
// Block textures: one layer per texture id, mipmapped, repeating (see gfx::block_textures).
@group(0) @binding(1) var blocks: texture_2d_array<f32>;
@group(0) @binding(2) var blocks_sampler: sampler;
// Biome colours (rgb) and brightness variation (a) around the camera; see gfx/tint.rs.
@group(0) @binding(3) var tint_map: texture_2d_array<f32>;
@group(0) @binding(4) var tint_sampler: sampler;
// The photographed sky without its sun: upper half of an equirectangular image, mipmapped.
@group(0) @binding(5) var sky_map: texture_2d<f32>;
@group(0) @binding(6) var sky_sampler: sampler;
// The night sky photo: stars, Milky Way and moon (background taken out), the whole sphere.
@group(0) @binding(7) var night_map: texture_2d<f32>;

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

// How far a water surface sits below the top of its block (like Minecraft's): 4 of 32 pixels.
const WATER_SINK: f32 = 0.125;

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
    let sink = f32((a >> 29u) & 1u) * WATER_SINK;
    let local = vec3<f32>(f32(a & 255u), f32((a >> 8u) & 255u), f32((a >> 16u) & 255u)) * 0.25 + grow
        - vec3<f32>(0.0, sink, 0.0);
    let ao = (a >> 27u) & 3u;
    let tex = b & 65535u;
    let sky = f32((b >> 16u) & 15u) / 15.0;
    let blk = f32((b >> 20u) & 15u) / 15.0;

    let world = v.origin.xyz + local;
    let ao_f = 0.55 + 0.15 * f32(ao);

    var o: VsOut;
    o.clip = g.view_proj * vec4<f32>(world, 1.0);
    // Sky, sun and moon light for this face's direction (sky::face_light), dimmed by ambient
    // occlusion and by how much sky the block sees; block light (glowstone) on top.
    o.color = g.face_light[min(face, 5u)].rgb * (ao_f * max(sky, 0.25)) + vec3<f32>(blk);
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
    var world = base + grow;
    if (tex == u32(g.flags.z + 0.5) && (face == 2u || top > 0.5)) {
        world.y -= WATER_SINK; // LOD water (its surface and the tops of its sides) sits as low as real water
    }

    o.clip = g.view_proj * vec4<f32>(world, 1.0);
    o.color = g.face_light[min(face, 5u)].rgb;
    o.world = world;
    o.uv = face_uv(face, world - vec3<f32>(v.origin.x, 0.0, v.origin.y));
    o.layer = tex;
    o.face = face;
    return o;
}

// ---- Sky ----
const PI: f32 = 3.14159265;
// The glow was measured outside this angle from the sun (4 degrees); inside, the sun is drawn.
const GLOW_FROM: f32 = 0.0698;
// Sunset colouring (low toward the sun / everywhere else).
const DUSK_WARM = vec3<f32>(1.55, 0.80, 0.42);
const DUSK_COOL = vec3<f32>(0.80, 0.82, 1.0);

// The photographed sky along a unit direction; below the horizon, the horizon. The caller
// picks the mip level (an automatic one would jump where the angle wraps around).
fn sky_base(d: vec3<f32>, lod: f32) -> vec3<f32> {
    let u = atan2(d.x, d.z) * (0.5 / PI);
    let v = 1.0 - asin(clamp(d.y, 0.0, 1.0)) * (2.0 / PI);
    return textureSampleLevel(sky_map, sky_sampler, vec2<f32>(u, v), lod).rgb;
}

// Stars, Milky Way and moon from the night photo, turned with the sky (its moon is the game's
// moon), fading out toward the horizon and gone below it.
fn night_photo(d: vec3<f32>, lod: f32) -> vec3<f32> {
    let p = vec3<f32>(dot(g.night_rot[0].xyz, d), dot(g.night_rot[1].xyz, d), dot(g.night_rot[2].xyz, d));
    let u = atan2(p.x, p.z) * (0.5 / PI);
    let v = 0.5 - asin(clamp(p.y, -1.0, 1.0)) / PI;
    return textureSampleLevel(night_map, sky_sampler, vec2<f32>(u, v), lod).rgb * smoothstep(-0.02, 0.12, d.y);
}

// The sky's colour along a unit direction, without the sun's disc: the day photo times the
// glow around the sun, sunset colouring, plus the night (its background is the same as in
// sky::radiance; the stars and moon only exist here).
fn sky_radiance(d: vec3<f32>, lod: f32) -> vec3<f32> {
    let base = sky_base(d, lod);
    let cg = dot(d, g.sun.xyz);
    let g0 = acos(clamp(cg, -1.0, 1.0));
    let gam = sqrt(g0 * g0 + GLOW_FROM * GLOW_FROM); // eased off inside GLOW_FROM, no ring
    let glow = g.glow[0].rgb + g.glow[1].rgb * (exp(-gam / g.glow[0].w) * g.sky_tint.z)
        + g.glow[2].rgb * (exp(-gam / g.glow[1].w) * g.sky_tint.y) + g.glow[3].rgb * (cg * cg);
    let hz = pow(1.0 - clamp(d.y, 0.0, 1.0), 3.0);
    let hl = max(dot(d.xz, d.xz) * dot(g.sun.xz, g.sun.xz), 1e-8);
    let toward = max(dot(d.xz, g.sun.xz) * inverseSqrt(hl), 0.0);
    let tint = mix(DUSK_COOL, DUSK_WARM, hz * (0.25 + 0.75 * toward * toward));
    let dusk = mix(vec3<f32>(1.0), tint, g.sky_tint.w);
    var night = mix(g.night_zenith.rgb, g.night_horizon.rgb, hz) * (1.0 - g.sun_color.w);
    if (g.night.x > 0.001) {
        night += night_photo(d, lod) * g.night.x;
    }
    return base * glow * dusk * g.sky_tint.x + night;
}

// What goes to the screen: unchanged up to 0.8, then a soft shoulder up to 1 that keeps the
// hue (an orange sunset stays orange instead of clipping to yellow or white); only very bright
// light (the sun, the sky right around it) washes out toward white.
fn display(c: vec3<f32>) -> vec3<f32> {
    let k = 0.8;
    let m = max(c.r, max(c.g, c.b));
    if (m <= k) {
        return c;
    }
    let top = k + (1.0 - k) * (1.0 - exp(-(m - k) / (1.0 - k)));
    return mix(c * (top / m), vec3<f32>(top), smoothstep(1.5, 6.0, m) * 0.7);
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

// Smooth random values (0..1) over the plane, one per unit square, blended between.
fn hash21(p: vec2<f32>) -> f32 {
    var q = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    q += dot(q, q.yzx + 33.33);
    return fract((q.x + q.y) * q.z);
}

fn vnoise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = p - i;
    let u = f * f * (3.0 - 2.0 * f);
    return mix(
        mix(hash21(i), hash21(i + vec2<f32>(1.0, 0.0)), u.x),
        mix(hash21(i + vec2<f32>(0.0, 1.0)), hash21(i + vec2<f32>(1.0, 1.0)), u.x),
        u.y
    );
}

// One travelling wave: adds its slope (d height / dx, d height / dz) to s. `footprint` is how
// much water one screen pixel covers, in blocks: a wave shorter than a few pixels would only
// shimmer and draw moire rings, so it fades out before that.
fn wave(s: ptr<function, vec2<f32>>, p: vec2<f32>, dir: vec2<f32>, wavelength: f32, amp: f32, speed: f32, footprint: f32) {
    let k = 6.2831853 / wavelength;
    let c = cos(k * (dot(dir, p) - speed * g.flags.y));
    *s += dir * (amp * k * c * smoothstep(2.5, 7.0, wavelength / footprint));
}

// Water, after open sea under a bright sky: a deep navy body, mostly opaque; irregular waves
// bend the surface normal, so facets alternate between the dark body and reflected sky;
// reflections take over toward the horizon (Fresnel), with sun and moon glints. The waves are
// travelling sine waves in many directions, made irregular three ways: the surface they run
// over is warped by slowly drifting noise (wave fronts bend and wander), each wave's height
// comes and goes in patches, and wind gusts roughen some areas more than others. Returns the
// colour and how opaque it is (the translucent pass blends with it; LOD water is drawn opaque).
fn water_shade(world: vec3<f32>, face: u32, light: vec3<f32>) -> vec4<f32> {
    let to_cam = g.cam_pos.xyz - world;
    let dist = length(to_cam);
    let v = to_cam / max(dist, 1e-4);
    var n = face_normal(face);
    if (face == 2u) {
        var s = vec2<f32>(0.0);
        let t = g.flags.y;
        // One pixel's footprint on the water, stretched at grazing angles.
        let fp = dist * (1.1 / g.screen.y) / max(abs(v.y), 0.04);
        // Warp: every point is pushed around by noise that drifts with time.
        let q = world.xz * 0.043;
        let warp = vec2<f32>(vnoise(q + vec2<f32>(t * 0.021, -t * 0.013)), vnoise(q + vec2<f32>(37.2 - t * 0.017, 11.7 + t * 0.019))) - 0.5;
        let p = world.xz + warp * 9.0;
        // Patches where each family of waves is high or low, and gusts.
        let m = world.xz * 0.019;
        let a1 = 0.35 + 1.1 * vnoise(m + vec2<f32>(t * 0.011, 0.0));
        let a2 = 0.35 + 1.1 * vnoise(m * 1.7 + vec2<f32>(5.3, 9.1 - t * 0.014));
        let a3 = 0.35 + 1.1 * vnoise(m * 2.9 + vec2<f32>(-t * 0.02, 3.3));
        let gust = 0.4 + 1.2 * vnoise(world.xz * 0.007 + vec2<f32>(t * 0.006, t * 0.004));
        // Swell, then chop, then ripples.
        wave(&s, p, vec2<f32>(0.8, 0.6), 37.0, 0.22 * a2, 2.6, fp);
        wave(&s, p, vec2<f32>(-0.28, 0.96), 21.0, 0.15 * a3, 2.1, fp);
        wave(&s, p, vec2<f32>(0.8, 0.6), 12.3, 0.13 * a1, 1.7, fp);
        wave(&s, p, vec2<f32>(-0.45, 0.89), 7.1, 0.10 * a2, 1.3, fp);
        wave(&s, p, vec2<f32>(0.97, -0.24), 4.3, 0.07 * a3 * gust, 1.05, fp);
        wave(&s, p, vec2<f32>(0.31, 0.95), 5.6, 0.06 * a1 * gust, 1.2, fp);
        wave(&s, p, vec2<f32>(-0.83, -0.56), 2.7, 0.05 * a2 * gust, 0.85, fp);
        wave(&s, p, vec2<f32>(0.2, -0.98), 1.9, 0.035 * gust, 0.7, fp);
        wave(&s, p, vec2<f32>(-0.96, 0.27), 1.4, 0.022 * a1 * gust, 0.6, fp);
        wave(&s, p, vec2<f32>(0.66, 0.75), 1.05, 0.015 * gust, 0.5, fp);
        n = normalize(vec3<f32>(-s.x, 1.0, -s.y));
    }
    let cos_v = clamp(dot(n, v), 0.0, 1.0);
    let fres = 0.03 + 0.97 * pow(1.0 - cos_v, 5.0);
    let r = reflect(-v, n);
    // The sky in the reflection (a little blurred: the surface is never quite flat).
    let sky = sky_radiance(r, 1.5);
    // Deep navy, a touch greener where the surface faces the viewer (light from inside).
    let body = mix(vec3<f32>(0.004, 0.016, 0.040), vec3<f32>(0.010, 0.040, 0.070), cos_v * 0.6) * light;
    // Sun and moon glints.
    let glint = pow(max(dot(n, normalize(g.sun.xyz + v)), 0.0), 300.0) * 2.2 * g.sun.w * g.sun_color.rgb
        + pow(max(dot(n, normalize(g.moon.xyz + v)), 0.0), 400.0) * 0.5 * g.moon.w * vec3<f32>(0.80, 0.85, 0.95);
    let col = mix(body, sky, fres) + glint;
    return vec4<f32>(col, mix(0.95, 0.99, fres));
}

@fragment
fn fs_main(f: VsOut) -> @location(0) vec4<f32> {
    // Derivatives first (they need uniform control flow): mip levels for the turned textures.
    let ddx = dpdx(f.uv);
    let ddy = dpdy(f.uv);
    let tex = textureSampleGrad(blocks, blocks_sampler, block_uv(f.face, f.uv, f.world), f.layer, ddx, ddy);
    let d = distance(f.world, g.cam_pos.xyz);
    let fog = clamp((d - g.fog_params.x) / (g.fog_params.y - g.fog_params.x), 0.0, 1.0) * g.flags.x;
    var col: vec3<f32>;
    var alpha = g.fog_params.z;
    if (f.layer == u32(g.flags.z + 0.5)) {
        let w = water_shade(f.world, f.face, f.color);
        col = w.rgb;
        alpha = w.a;
    } else {
        let t = tint_at(f.world.xz);
        // Alpha says how much of the biome colour to apply: 0 = all (grass), 1 = none.
        let albedo = mix(tex.rgb * t.rgb, tex.rgb, tex.a) * mix(0.86, 1.14, t.a);
        col = albedo * f.color;
    }
    // Fog takes on the sky's colour in that direction, so far terrain melts into the sky
    // behind it (bright toward the sun, orange at sunset, dark at night).
    if (fog > 0.0) {
        col = mix(col, sky_radiance((f.world - g.cam_pos.xyz) / max(d, 1e-3), 2.0), fog);
    }
    return vec4<f32>(display(col), alpha);
}

// ---- The sky pass: one triangle over the whole screen, behind everything ----
struct SkyOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_sky(@builtin(vertex_index) i: u32) -> SkyOut {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    var o: SkyOut;
    o.clip = vec4<f32>(p, 0.0, 1.0); // depth 0: infinitely far with reverse-Z
    o.ndc = p;
    return o;
}

// The sun's disc, with glare: drawn bigger than the real one (about five times), as games do;
// the ground hides it once it has set. (The moon is in the night photo.)
fn sun_disc(d: vec3<f32>) -> vec3<f32> {
    let above = smoothstep(-0.004, 0.004, d.y);
    let warm = smoothstep(0.0, 0.5, g.sun.y);
    let tint = g.sun_color.rgb / max(g.sun_color.r, 1e-4);
    let a = acos(clamp(dot(d, g.sun.xyz), -1.0, 1.0));
    let r = 0.023;
    let disc = (1.0 - smoothstep(r * 0.85, r, a)) * (1.0 - 0.25 * (a / r) * (a / r));
    return tint * (disc * mix(20.0, 30.0, warm) + exp(-a / 0.018) * mix(1.5, 2.5, warm)) * (g.sun.w * above);
}

@fragment
fn fs_sky(f: SkyOut) -> @location(0) vec4<f32> {
    let d = normalize(g.sky_basis[2].xyz + f.ndc.x * g.sky_basis[0].xyz + f.ndc.y * g.sky_basis[1].xyz);
    return vec4<f32>(display(sky_radiance(d, 0.0) + sun_disc(d)), 1.0);
}

// ---- Clouds (see app/src/clouds.rs): boxes on a grid of 32-block tiles ----
// Vertex: a = x(8) | z(8)<<8 | y(10)<<16 | face(3)<<26 (x, z in tiles, y in blocks above the
//         cloud base), b = opacity(8)
struct CloudOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) light: vec3<f32>,
    @location(2) alpha: f32,
};

@vertex
fn vs_cloud(@location(0) packed: vec2<u32>) -> CloudOut {
    let a = packed.x;
    let face = min((a >> 26u) & 7u, 5u);
    let world = vec3<f32>(
        g.cloud.x + f32(a & 255u) * 32.0,
        g.cloud.z + f32((a >> 16u) & 1023u),
        g.cloud.y + f32((a >> 8u) & 255u) * 32.0
    );
    // Clouds scatter light inside themselves: every side gets half of what the top gets, plus
    // its own share of sky and sun. Dimmer at night than the moonlit ground.
    let n = face_normal(face);
    let soft = mix(g.face_light[2].rgb, g.face_light[face].rgb, 0.5);
    let sun = g.sun_color.rgb * (0.25 + 0.35 * max(dot(n, g.sun.xyz), 0.0));
    var o: CloudOut;
    o.clip = g.view_proj * vec4<f32>(world, 1.0);
    o.world = world;
    o.light = (soft + sun) * (0.4 + 0.6 * g.sun_color.w);
    o.alpha = f32(packed.y & 255u) / 255.0;
    return o;
}

@fragment
fn fs_cloud(f: CloudOut) -> @location(0) vec4<f32> {
    let to = f.world - g.cam_pos.xyz;
    let d = max(length(to), 1e-3);
    let dir = to / d;
    // Looking toward the sun, clouds light up (silver linings by day, fire at sunset).
    let through = pow(max(dot(dir, g.sun.xyz), 0.0), 6.0) * g.sun.w;
    var col = f.light * 0.95 + g.sun_color.rgb * (through * 0.9);
    // Far clouds sink into the haze of the sky behind them and thin out to nothing.
    let fade = smoothstep(g.cloud.w * 0.3, g.cloud.w, length(to.xz));
    col = mix(col, sky_radiance(dir, 2.0), fade * 0.8);
    return vec4<f32>(display(col), f.alpha * (1.0 - fade));
}

// ---- God rays: light shafts from the sun, in screen space ----
// After each frame, `fs_mask` writes a small picture of where open sky near the sun is (from
// the depth buffer: terrain and clouds block it). In the next frame `fs_rays` smears that
// picture away from the sun's place on screen and adds it on top, which looks like shafts of
// light coming past whatever hides the sun. (The technique of GPU Gems 3, chapter 13.)
// The depth buffer, read as plain numbers (texel fetches from a depth-typed texture are not
// available on OpenGL ES).
@group(1) @binding(0) var depth_tex: texture_2d<f32>;
@group(1) @binding(1) var mask_tex: texture_2d<f32>;
@group(1) @binding(2) var mask_sampler: sampler;

@fragment
fn fs_mask(f: SkyOut) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(depth_tex));
    let uv = vec2<f32>(f.ndc.x * 0.5 + 0.5, 0.5 - f.ndc.y * 0.5);
    let depth = textureLoad(depth_tex, vec2<i32>(min(uv * size, size - vec2<f32>(1.0))), 0).r;
    let d = normalize(g.sky_basis[2].xyz + f.ndc.x * g.sky_basis[0].xyz + f.ndc.y * g.sky_basis[1].xyz);
    let near_sun = pow(max(dot(d, g.sun.xyz), 0.0), 10.0);
    let open = select(0.0, 1.0, depth == 0.0); // nothing drawn there: sky
    return vec4<f32>(vec3<f32>(open * near_sun), 1.0);
}

const RAY_STEPS: i32 = 12;

@fragment
fn fs_rays(f: SkyOut) -> @location(0) vec4<f32> {
    let uv = vec2<f32>(f.ndc.x * 0.5 + 0.5, 0.5 - f.ndc.y * 0.5);
    let step = (g.rays.xy - uv) * (0.9 / f32(RAY_STEPS));
    // Start each pixel's walk at a different point along its first step: noise instead of bands.
    let jitter = fract(52.9829189 * fract(dot(f.clip.xy, vec2<f32>(0.06711056, 0.00583715))));
    var p = uv + step * jitter;
    var sum = 0.0;
    var weight = 1.0;
    for (var i = 0; i < RAY_STEPS; i++) {
        sum += textureSampleLevel(mask_tex, mask_sampler, p, 0.0).r * weight;
        weight *= 0.93;
        p += step;
    }
    let tint = g.sun_color.rgb / max(max(g.sun_color.r, max(g.sun_color.g, g.sun_color.b)), 1e-3);
    return vec4<f32>(tint * (sum * g.rays.z / f32(RAY_STEPS)), 1.0);
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
