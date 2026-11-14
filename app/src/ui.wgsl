// The HUD and the menus: triangles in pixel coordinates (origin top left), each vertex with
// a place in a texture, a colour and how to read the texture:
//   0: the UI atlas's colour, times the vertex colour (frames, the logo, plain rectangles
//      from the atlas's white square);
//   1: the atlas's alpha as a signed distance field (text and icons: crisp at any size);
//   2: the world thumbnails' texture, times the vertex colour.

struct Screen {
    size: vec4<f32>,
};

@group(0) @binding(0) var<uniform> screen: Screen;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var thumbs: texture_2d<f32>;
@group(0) @binding(3) var ui_s: sampler;

struct UiIn {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) mode: u32,
};

struct UiOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) mode: u32,
};

@vertex
fn vs_ui(v: UiIn) -> UiOut {
    var o: UiOut;
    o.clip = vec4<f32>(v.pos.x / screen.size.x * 2.0 - 1.0, 1.0 - v.pos.y / screen.size.y * 2.0, 0.0, 1.0);
    o.uv = v.uv;
    o.color = v.color;
    o.mode = v.mode;
    return o;
}

@fragment
fn fs_ui(f: UiOut) -> @location(0) vec4<f32> {
    // Both textures and the derivative are read for every pixel (WGSL wants them outside
    // branches); the mode only chooses what to keep.
    let a = textureSample(atlas, ui_s, f.uv);
    let t = textureSample(thumbs, ui_s, f.uv);
    let w = max(fwidth(a.a), 1e-4) * 0.7;
    // Colours are authored as sRGB bytes; the surface is sRGB, so go to linear first.
    let c = vec4<f32>(pow(f.color.rgb, vec3<f32>(2.2)), f.color.a);
    var out = c * a;
    if (f.mode == 1u) {
        out = vec4<f32>(c.rgb, c.a * smoothstep(0.5 - w, 0.5 + w, a.a));
    } else if (f.mode == 2u) {
        out = c * t;
    }
    return out;
}
