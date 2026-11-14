// The title screen's background: a panorama photographed in the game (assets/title,
// tools/stitch_panorama.py), turning slowly like Minecraft's. The panorama is cylindrical:
// u is the compass direction (yaw / 2pi, the camera's yaw convention), v runs linearly in
// elevation from +PANO_LAT at the top to -PANO_LAT at the bottom.

struct Title {
    // x: yaw (radians), y: tan(half the vertical field of view), z: aspect (width / height),
    // w: brightness.
    view: vec4<f32>,
};

@group(0) @binding(0) var<uniform> t: Title;
@group(0) @binding(1) var pano: texture_2d<f32>;
@group(0) @binding(2) var pano_s: sampler;

const PANO_LAT: f32 = 0.5235988; // 30 degrees

struct TitleOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_title(@builtin(vertex_index) i: u32) -> TitleOut {
    // One triangle covering the screen.
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    var o: TitleOut;
    o.clip = vec4<f32>(p, 0.0, 1.0);
    o.ndc = p;
    return o;
}

@fragment
fn fs_title(f: TitleOut) -> @location(0) vec4<f32> {
    let yaw = t.view.x;
    let fwd = vec3<f32>(-sin(yaw), 0.0, -cos(yaw));
    let right = vec3<f32>(cos(yaw), 0.0, -sin(yaw));
    let d = normalize(fwd + right * (f.ndc.x * t.view.y * t.view.z) + vec3<f32>(0.0, 1.0, 0.0) * (f.ndc.y * t.view.y));
    let u = fract(atan2(-d.x, -d.z) / 6.2831853);
    let v = clamp(0.5 - asin(clamp(d.y, -1.0, 1.0)) / (2.0 * PANO_LAT), 0.0, 1.0);
    // One level, no mipmaps: the picture is only ever magnified, and a mip choice would
    // flicker along the seam where u wraps.
    let c = textureSampleLevel(pano, pano_s, vec2<f32>(u, v), 0.0).rgb;
    // A little darker at the top and bottom, where the logo and the footer sit.
    let edge = 1.0 - 0.28 * smoothstep(0.55, 1.0, abs(f.ndc.y));
    return vec4<f32>(c * t.view.w * edge, 1.0);
}
