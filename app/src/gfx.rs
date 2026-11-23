//! wgpu renderer. Mobile-minded choices:
//!   * one static index buffer shared by every chunk (quads carry no indices)
//!   * chunk origin as a per-instance attribute: one draw call per chunk, no per-chunk uniforms
//!   * depth is discarded after the pass (cheap on tile-based GPUs)
//!   * opaque + cutout first, translucent water last, far-to-near
//!
//!   * solid terrain is drawn nearest chunk first, so the depth test rejects hidden
//!     fragments before they are shaded (the renderer is GPU-bound on phones)
//!   * level-of-detail terrain (see blockforge::lod) is drawn after the real terrain, nearest
//!     section first, so it only shades pixels the real terrain left empty; its vertex
//!     shader drops quads in chunk columns where the real terrain is drawn
//!   * reverse-Z depth with no far plane (math::perspective_reverse_z), for 30 km views
//!   * the sky (see `sky`) is one full-screen triangle drawn after the terrain at the far
//!     depth, so it only shades the pixels the terrain left empty
//!   * clouds (see `clouds`) are one mesh, one draw call, blended far to near
//!   * god rays cost a quarter-resolution pass after the frame (a mask of open sky near the
//!     sun, from the depth buffer) and one full-screen blend with 12 samples of it in the
//!     next frame; both are skipped while the sun is down or behind the camera
//!
//! TODO (see README): pooled vertex buffers, occlusion culling, ASTC texture array, render scale.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use blockforge::client::{Client, LodCoverage};
use blockforge::coords::{ChunkPos, CHUNK_SIZE};
use blockforge::lod::{SectionKey, MASKED_MAX_LEVEL};
use blockforge::mesh::quad_indices;
use blockforge::registry::BlockRegistry;

use crate::clouds::{self, Clouds};
use crate::hyrig::rig;
use crate::sky::{self, Sky};
use crate::texgen;
use crate::textures;
use crate::tint::{self, TintMap};
use blockforge::worldgen::WorldGen;
use wgpu::util::DeviceExt;
use winit::window::Window;

use crate::game::CameraView;
use crate::gpu_timer::GpuTimer;
use crate::math;
use crate::shadows::{self, Shadows};
use crate::pool;
use crate::ui::{HudVertex, MAX_HUD_VERTS};

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// A chunk can show at most 3 * 32^3 faces (3D checkerboard), so this index range is always enough.
const MAX_QUADS: u32 = 100_000;
const MAX_INSTANCES: usize = 4096;
/// Mesh uploads per frame, so a burst of finished meshes doesn't cause a hitch.
const UPLOADS_PER_FRAME: usize = 24;
/// LOD section uploads per frame (each is up to ~100 KB).
const LOD_UPLOADS_PER_FRAME: usize = 8;
const MAX_LOD_INSTANCES: usize = 2048;
const SHADER: &str = include_str!("shader.wgsl");

const VERTEX_ATTRS: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Uint32x2];
const INSTANCE_ATTRS: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![1 => Float32x4];
const HUD_ATTRS: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Unorm8x4, 3 => Uint32];

#[repr(C)]
#[derive(Clone, Copy)]
struct Globals {
    view_proj: [f32; 16],
    cam_pos: [f32; 4],
    fog_params: [f32; 4],
    screen: [f32; 4],
    flags: [f32; 4],
    palette: [[f32; 4]; 32],
    /// x, y: chunk column at bit 0 of `lod_mask` (see `LodCoverage`).
    lod_window: [f32; 4],
    /// One bit per chunk column: set where LOD terrain must not be drawn.
    lod_mask: [[u32; 4]; 64],
    /// The tint map's level centres in blocks: x0, z0, x1, z1 (see `tint`).
    tint_window: [f32; 4],
    /// View rays for the sky pass (`math::sky_basis`).
    sky_basis: [[f32; 4]; 3],
    /// Sun, moon, sky colouring and the light on each face direction (`sky::uniforms`).
    sky: sky::SkyUniforms,
    /// Clouds: x, z of the window's corner in the world, base height, fade distance.
    cloud: [f32; 4],
    /// God rays: the sun on screen (uv), strength.
    rays: [f32; 4],
    /// Shadows and the shader look (`shadows`): the maps' matrices; their half widths,
    /// strength (0 = off) and size; towards the light, soft taps; its colour, foliage sways;
    /// bounce light, grading, warm block light, look on.
    shadow_mat: [[f32; 16]; 2],
    shadow: [f32; 4],
    light: [f32; 4],
    light_color: [f32; 4],
    look: [f32; 4],
    /// Leaves and ferns: one bit per texture layer.
    foliage: [[u32; 4]; 32],
}

/// Per-frame toggles chosen in the settings menu.
#[derive(Clone, Copy, Debug)]
pub struct RenderOptions {
    pub fog: bool,
    /// Skip chunks whose bounding box is outside the camera frustum.
    pub frustum_cull: bool,
    /// Use the back-face-culling pipeline for solid terrain.
    pub backface_cull: bool,
    /// Skip chunks hidden behind terrain (`blockforge::cull`).
    pub occlusion_cull: bool,
    /// true = FIFO (capped at the display rate); false = the fastest uncapped mode available.
    pub vsync: bool,
    /// Where fog starts and where it is complete, in blocks from the camera.
    pub fog_start: f32,
    pub fog_end: f32,
    /// Hours, 0..24: where the sun is.
    pub time_of_day: f64,
    pub clouds: bool,
    pub god_rays: bool,
    /// Shadows and the rest of the shader-pack look.
    pub shaders: shadows::Quality,
    /// Show the shadow maps (with the stats overlay), to see what the GPU drew.
    pub shadow_debug: bool,
    /// A character to draw over the HUD's first `.1` vertices (the inventory's equipment
    /// tab): where he stands; his triangles as given to `set_portrait`.
    pub figure: Option<(crate::ui::Showcase, usize)>,
}

/// `Globals::light.w`: the radius of the 4 soft-edge taps, in texels.
fn shadow_softness(q: shadows::Quality) -> f32 {
    match q {
        shadows::Quality::Fancy => 1.6,
        _ => 0.9,
    }
}

fn as_bytes<T>(s: &[T]) -> &[u8] {
    // Only used with plain-old-data (f32 arrays, #[repr(C)] vertex/uniform structs).
    unsafe { std::slice::from_raw_parts(s.as_ptr() as *const u8, std::mem::size_of_val(s)) }
}


/// Each texture's average colour (linear), indexed by texture id.
fn build_palette(reg: &BlockRegistry) -> [[f32; 4]; 32] {
    let mut p = [[1.0f32, 0.0, 1.0, 1.0]; 32]; // magenta = missing texture
    for (i, name) in reg.texture_names().iter().enumerate().take(32) {
        if textures::known(name) {
            let c = texgen::average_linear(&textures::load(name));
            p[i] = [c[0], c[1], c[2], 1.0];
        }
    }
    p
}

/// The biome tint map's texture (two levels as array layers, see `tint`) and its sampler:
/// bilinear and repeating, so colours blend between texels and wrap like the storage does.
fn tint_texture(device: &wgpu::Device) -> (wgpu::Texture, wgpu::TextureView, wgpu::Sampler) {
    let n = tint::TEXELS as u32;
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("tint map"),
        size: wgpu::Extent3d { width: n, height: n, depth_or_array_layers: 2 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor {
        label: Some("tint map"),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("tint sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    (tex, view, sampler)
}

/// A photographed sky (see `sky`) as an RGB9E5 texture with all its mip levels, and its
/// sampler: wrapping around the horizon, clamped at the top and bottom rows. The shader
/// always picks the mip level itself, so the seam where the angle wraps never shows.
fn sky_texture(device: &wgpu::Device, queue: &wgpu::Queue, map: &sky::SkyMap) -> (wgpu::TextureView, wgpu::Sampler) {
    let top = &map.levels[0];
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("sky"),
        size: wgpu::Extent3d { width: top.width, height: top.height, depth_or_array_layers: 1 },
        mip_level_count: map.levels.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgb9e5Ufloat,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (i, level) in map.levels.iter().enumerate() {
        let data: Vec<u32> = level.texels.iter().map(|&c| sky::to_rgb9e5(c)).collect();
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &tex,
                mip_level: i as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            as_bytes(&data),
            wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(4 * level.width), rows_per_image: Some(level.height) },
            wgpu::Extent3d { width: level.width, height: level.height, depth_or_array_layers: 1 },
        );
    }
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("sky sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    (view, sampler)
}

/// How many layers the block texture array gets for `n` textures. At least two: some GL
/// drivers mistake a one-layer array for a plain 2D texture. Never a multiple of six: on
/// OpenGL ES, wgpu makes a square texture with 6, 12, 18, 24... layers a cube map, and every
/// block then shows the wrong texture (this happened at 24 textures).
pub fn array_layers(n: usize) -> u32 {
    let n = n.max(2) as u32;
    if n % 6 == 0 {
        n + 1
    } else {
        n
    }
}

/// The block texture array's layers on a GPU with room for `max`, and, when there are more
/// textures than that, which texture each draws instead: the one under the limit closest in
/// colour.
fn texture_layers(names: &[String], max: u32) -> (u32, Option<Vec<u16>>) {
    let layers = array_layers(names.len());
    if layers <= max {
        return (layers, None);
    }
    let cap = if max % 6 == 0 { max - 1 } else { max };
    let avg: Vec<[f32; 3]> = names.iter().map(|n| texgen::average_linear(&textures::load(n))).collect();
    let map = (0..names.len())
        .map(|t| {
            if t < cap as usize {
                return t as u16;
            }
            let d = |u: usize| (0..3).map(|i| (avg[u][i] - avg[t][i]).powi(2)).sum::<f32>();
            (1..cap as usize).min_by(|&a, &b| d(a).partial_cmp(&d(b)).unwrap()).unwrap_or(0) as u16
        })
        .collect();
    (cap, Some(map))
}

/// Mip levels of the block textures: 32, 16, 8, 4, 2, 1 texels.
const TEXTURE_MIPS: u32 = 6;

/// All block textures as one array texture (layer = texture id), with mipmaps, and the
/// sampler for it: crisp texels up close, smoothly averaged further away.
fn block_textures(device: &wgpu::Device, queue: &wgpu::Queue, reg: &BlockRegistry) -> (wgpu::TextureView, wgpu::Sampler, Option<Vec<u16>>) {
    let names = reg.texture_names();
    let (layers, remap) = texture_layers(names, device.limits().max_texture_array_layers);
    let size = texgen::SIZE as u32;
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("block textures"),
        size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: layers },
        mip_level_count: TEXTURE_MIPS,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for layer in 0..layers {
        let name = names.get(layer as usize).map(String::as_str).unwrap_or("");
        let mut px = textures::load(name);
        let mut s = texgen::SIZE;
        for mip in 0..TEXTURE_MIPS {
            queue.write_texture(
                wgpu::ImageCopyTexture {
                    texture: &tex,
                    mip_level: mip,
                    origin: wgpu::Origin3d { x: 0, y: 0, z: layer },
                    aspect: wgpu::TextureAspect::All,
                },
                &px,
                wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(4 * s as u32), rows_per_image: Some(s as u32) },
                wgpu::Extent3d { width: s as u32, height: s as u32, depth_or_array_layers: 1 },
            );
            if s > 1 {
                px = texgen::downsample(&px, s);
                s /= 2;
            }
        }
    }
    let view = tex.create_view(&wgpu::TextureViewDescriptor {
        label: Some("block textures"),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("block sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    (view, sampler, remap)
}

/// What god rays need besides the frame itself, remade when the surface changes size: the
/// depth buffer (also sampled, to find open sky), the quarter-resolution mask, and the bind
/// groups that hand the depth buffer to the mask pass and the mask to the rays pass.
struct PostTargets {
    depth_view: wgpu::TextureView,
    mask_view: wgpu::TextureView,
    bind_mask: wgpu::BindGroup,
    bind_rays: wgpu::BindGroup,
}

const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

fn post_layouts(device: &wgpu::Device) -> (wgpu::BindGroupLayout, wgpu::BindGroupLayout) {
    let depth = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("depth for the ray mask"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    });
    let mask = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ray mask"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    (depth, mask)
}

fn post_targets(device: &wgpu::Device, layouts: &(wgpu::BindGroupLayout, wgpu::BindGroupLayout), w: u32, h: u32) -> PostTargets {
    let depth_view = make_depth(device, w, h);
    let mask = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ray mask"),
        size: wgpu::Extent3d { width: (w / 4).max(1), height: (h / 4).max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: MASK_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let mask_view = mask.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("ray mask sampler"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let bind_mask = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("depth for the ray mask"),
        layout: &layouts.0,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&depth_view) }],
    });
    let bind_rays = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("ray mask"),
        layout: &layouts.1,
        entries: &[
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&mask_view) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
        ],
    });
    PostTargets { depth_view, mask_view, bind_mask, bind_rays }
}

/// A full-screen triangle (`vs_sky`) with the given fragment shader, for the god-ray passes.
fn screen_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    fragment: &str,
    target: wgpu::ColorTargetState,
    with_depth: bool,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(fragment),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: "vs_sky",
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: fragment,
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(target)],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: with_depth.then(|| wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Always,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

/// The character's texture, with its bind group (group 1 of `pipe_char`): drawn unfiltered,
/// like the blocks.
fn character_texture(device: &wgpu::Device, queue: &wgpu::Queue) -> (wgpu::BindGroupLayout, wgpu::BindGroup) {
    let (w, h, pixels) = decode_rgba(rig::TEXTURE_PNG).unwrap_or_else(|e| {
        eprintln!("blockforge: hyrig.png: {e}; the character is drawn plain");
        (2, 2, vec![200; 16])
    });
    let size = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("character"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::ImageCopyTexture { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        &pixels,
        wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h) },
        size,
    );
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("character sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        ..Default::default()
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("character texture"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("character texture"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&view) },
            wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&sampler) },
        ],
    });
    (layout, bind)
}

/// An 8-bit RGBA PNG as (width, height, bytes).
fn decode_rgba(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let mut dec = png::Decoder::new(bytes);
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    if info.color_type != png::ColorType::Rgba {
        return Err(format!("{:?}, not RGBA", info.color_type));
    }
    buf.truncate(info.buffer_size());
    Ok((info.width, info.height, buf))
}

/// The character: triangles posed on the CPU (`rig::Vertex`), a cut-out texture, both sides.
fn character_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    const ATTRS: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("character"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: "vs_char",
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<rig::Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &ATTRS,
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: "fs_char",
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Greater,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

/// The most character vertices drawn in a frame (the model has 768).
const MAX_CHARACTER_VERTS: usize = 4096;

/// Clouds: the terrain's vertex layout without instances, blended, both sides drawn.
fn cloud_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("clouds"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: "vs_cloud",
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: 8,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &VERTEX_ATTRS,
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: "fs_cloud",
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        // Faces turned away are skipped: the far walls of a cloud would only be blended over
        // by its near ones (and inside a cloud there is nothing to see of it).
        primitive: wgpu::PrimitiveState { cull_mode: Some(wgpu::Face::Back), ..Default::default() },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Greater,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

fn make_depth(device: &wgpu::Device, w: u32, h: u32) -> wgpu::TextureView {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

/// A mesh's vertices: a range in the vertex pool (`pool.rs`).
struct GpuLayer {
    slot: pool::Slot,
    quads: u32,
}

struct GpuChunk {
    solid: Option<GpuLayer>,
    water: Option<GpuLayer>,
    /// Some face sees the sky. A chunk that is all underground can't shadow anything the sun
    /// reaches (what is behind it from the sun is underground too), so it casts no shadow.
    sky_lit: bool,
}

struct GpuLod {
    layer: Option<GpuLayer>,
    min_y: f32,
    max_y: f32,
}

pub struct Gfx {
    /// When the GPU has room for fewer textures than the blocks have: which texture each
    /// draws instead (`BlockRegistry::remap_textures`; the app meshes with that registry).
    pub texture_remap: Option<Vec<u16>>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    /// Immediate or Mailbox if the surface offers one: used when VSYNC is turned off.
    uncapped_mode: Option<wgpu::PresentMode>,
    post: PostTargets,
    post_layouts: (wgpu::BindGroupLayout, wgpu::BindGroupLayout),
    pipe_cloud: wgpu::RenderPipeline,
    pipe_mask: wgpu::RenderPipeline,
    pipe_rays: wgpu::RenderPipeline,
    /// The character: pipeline, texture, and this frame's triangles (see `set_character`).
    pipe_char: wgpu::RenderPipeline,
    bind_char: wgpu::BindGroup,
    char_buf: wgpu::Buffer,
    /// The character drawn in a menu over the world (`RenderOptions::figure`), with globals of
    /// its own.
    portrait_buf: wgpu::Buffer,
    portrait_verts: u32,
    globals_portrait: wgpu::Buffer,
    bind_portrait: wgpu::BindGroup,
    char_verts: u32,
    /// Clouds around the camera, and the mesh on the GPU (buffer, quads).
    clouds: Clouds,
    cloud_mesh: Option<(wgpu::Buffer, u32)>,
    pipe_solid: wgpu::RenderPipeline,
    pipe_solid_nocull: wgpu::RenderPipeline,
    pipe_water: wgpu::RenderPipeline,
    pipe_lod: wgpu::RenderPipeline,
    pipe_sky: wgpu::RenderPipeline,
    /// The HUD and menus (`ui.wgsl`): pipeline, atlas, thumbnails.
    ui: UiGpu,
    /// Set by `capture_world`: the next `render` draws into a texture and keeps the picture.
    capture_next: bool,
    captured: Option<(Vec<u8>, u32, u32)>,
    /// The title screen's turning panorama (`title.rs`, `title.wgsl`).
    pipe_title: wgpu::RenderPipeline,
    bind_title: wgpu::BindGroup,
    title_uniform: wgpu::Buffer,
    hud_buf: wgpu::Buffer,
    timer: Option<GpuTimer>,
    globals_solid: wgpu::Buffer,
    globals_water: wgpu::Buffer,
    bind_solid: wgpu::BindGroup,
    bind_water: wgpu::BindGroup,
    index_buf: wgpu::Buffer,
    instance_buf: wgpu::Buffer,
    lod_instance_buf: wgpu::Buffer,
    chunks: HashMap<ChunkPos, GpuChunk>,
    lods: HashMap<SectionKey, GpuLod>,
    /// Where the vertices of `chunks` and `lods` live.
    pool: pool::VertexPool,
    last_drawn: usize,
    last_hidden: usize,
    last_quads: u64,
    last_lod_drawn: usize,
    last_lod_quads: u64,
    palette: [[f32; 4]; 32],
    /// Biome colours around the camera (two levels, see `tint`), once the world is known.
    tint: Option<TintMap>,
    tint_tex: wgpu::Texture,
    /// Seconds since start, for animated water.
    start: Instant,
    /// The water texture's layer (the shader shades it as water).
    water_layer: u32,
    /// The sky and the light it gives (see `sky`).
    sky: Sky,
    /// Shadow maps (see `shadows`), and which texture layers are foliage.
    shadows: Shadows,
    shadow_debug: shadows::DebugView,
    foliage: [[u32; 4]; 32],
    /// The GPU, its API and shadow sampling, for the performance overlay.
    gpu_line: String,
}

impl Gfx {
    pub async fn new(window: Arc<Window>, reg: &BlockRegistry) -> Result<Gfx, String> {
        let size = window.inner_size();
        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("could not create surface: {e}"))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .ok_or("no suitable GPU adapter found")?;
        let info = adapter.get_info();
        let gpu_line = format!(
            "GPU {} {:?} SHADOWS R32F",
            info.name.to_uppercase(),
            info.backend,
        )
        .to_uppercase();
        // GPU timing is optional: not every device supports timestamp queries.
        let want_ts = adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY);
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("blockforge"),
                    required_features: if want_ts {
                        wgpu::Features::TIMESTAMP_QUERY
                    } else {
                        wgpu::Features::empty()
                    },
                    // As many texture layers as the GPU has (often 2048; at least 256): one
                    // per block texture.
                    required_limits: wgpu::Limits {
                        max_texture_array_layers: adapter.limits().max_texture_array_layers.max(256),
                        ..wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits())
                    },
                    memory_hints: wgpu::MemoryHints::default(),
                },
                None,
            )
            .await
            .map_err(|e| format!("could not create device: {e}"))?;

        let caps = surface.get_capabilities(&adapter);
        let uncapped_mode = [wgpu::PresentMode::Immediate, wgpu::PresentMode::Mailbox]
            .into_iter()
            .find(|m| caps.present_modes.contains(m));
        let format = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);
        let post_layouts = post_layouts(&device);
        let post = post_targets(&device, &post_layouts, config.width, config.height);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blockforge shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let shadow_entries = Shadows::layout_entries();
        let shadows = Shadows::new(&device, MAX_INSTANCES, std::mem::size_of::<rig::Vertex>() as u64);
        let shadow_debug = shadows::DebugView::new(&device, &shadows, format);
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                shadow_entries[0],
                shadow_entries[1],
            ],
        });
        let (tint_tex, tint_view, tint_sampler) = tint_texture(&device);
        let (block_view, block_sampler, texture_remap) = block_textures(&device, &queue, reg);
        let mut sky = Sky::new();
        let (sky_view, sky_sampler) = sky_texture(&device, &queue, &sky.map);
        sky.map.keep_levels_up_to(64); // the GPU has the rest; the light needs only these
        // The night photo is only needed on the GPU (its sampler is the sky's).
        let (night_view, _) = sky_texture(&device, &queue, &sky::SkyMap::load_night());
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pipeline layout"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });

        let make_globals = |label: &str| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: std::mem::size_of::<Globals>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let globals_solid = make_globals("globals solid");
        let globals_portrait = make_globals("globals portrait");
        let globals_water = make_globals("globals water");
        let sh = shadows.bind_entries();
        let make_bind = |label: &str, buf: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&block_view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&block_sampler) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&tint_view) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&tint_sampler) },
                    wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&sky_view) },
                    wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::Sampler(&sky_sampler) },
                    wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::TextureView(&night_view) },
                    sh[0].clone(),
                    sh[1].clone(),
                ],
            })
        };
        let bind_solid = make_bind("bind solid", &globals_solid);
        let bind_portrait = make_bind("bind portrait", &globals_portrait);
        let bind_water = make_bind("bind water", &globals_water);

        // Solid terrain is one-sided: cull back faces (the winding is covered by tests).
        let pipe_solid =
            voxel_pipeline(&device, &layout, &shader, "vs_main", format, None, true, Some(wgpu::Face::Back), "solid");
        let pipe_solid_nocull =
            voxel_pipeline(&device, &layout, &shader, "vs_main", format, None, true, None, "solid (no culling)");
        let pipe_water = voxel_pipeline(
            &device,
            &layout,
            &shader,
            "vs_main",
            format,
            Some(wgpu::BlendState::ALPHA_BLENDING),
            false,
            None, // double-sided: from underwater the surface is a back face
            "water",
        );
        // LOD terrain: same vertex size and instance slot, its own vertex shader.
        let pipe_lod =
            voxel_pipeline(&device, &layout, &shader, "vs_lod", format, None, true, Some(wgpu::Face::Back), "lod");
        let pipe_sky = sky_pipeline(&device, &layout, &shader, format);
        let pipe_cloud = cloud_pipeline(&device, &layout, &shader, format);
        let (char_layout, bind_char) = character_texture(&device, &queue);
        let pipe_char = character_pipeline(
            &device,
            &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("character"),
                bind_group_layouts: &[&bgl, &char_layout],
                push_constant_ranges: &[],
            }),
            &shader,
            format,
        );
        let portrait_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("portrait"),
            size: (MAX_CHARACTER_VERTS * std::mem::size_of::<rig::Vertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let char_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("character vertices"),
            size: (MAX_CHARACTER_VERTS * std::mem::size_of::<rig::Vertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let with_group1 = |group1: &wgpu::BindGroupLayout| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("god rays"),
                bind_group_layouts: &[&bgl, group1],
                push_constant_ranges: &[],
            })
        };
        let pipe_mask = screen_pipeline(
            &device,
            &with_group1(&post_layouts.0),
            &shader,
            "fs_mask",
            wgpu::ColorTargetState { format: MASK_FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL },
            false,
        );
        // Added on top of the frame.
        let add = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let pipe_rays = screen_pipeline(
            &device,
            &with_group1(&post_layouts.1),
            &shader,
            "fs_rays",
            wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState { color: add, alpha: add }),
                write_mask: wgpu::ColorWrites::COLOR,
            },
            true,
        );
        let ui = ui_pipeline(&device, &queue, format);
        let (pipe_title, bind_title, title_uniform) = title_pipeline(&device, &queue, format);

        let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("shared quad indices"),
            contents: as_bytes(&quad_indices(MAX_QUADS as usize)),
            usage: wgpu::BufferUsages::INDEX,
        });
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chunk origins"),
            size: (MAX_INSTANCES * 16) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let lod_instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lod sections"),
            size: (MAX_LOD_INSTANCES * 16) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let hud_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hud vertices"),
            size: (MAX_HUD_VERTS * std::mem::size_of::<HudVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let timer = if want_ts { Some(GpuTimer::new(&device, &queue)) } else { None };

        let texture_remap_ref = texture_remap.clone();
        Ok(Gfx {
            texture_remap,
            surface,
            device,
            queue,
            config,
            uncapped_mode,
            post,
            post_layouts,
            pipe_cloud,
            pipe_mask,
            pipe_rays,
            pipe_char,
            bind_char,
            char_buf,
            portrait_buf,
            portrait_verts: 0,
            globals_portrait,
            bind_portrait,
            char_verts: 0,
            clouds: Clouds::new(0xC10D),
            cloud_mesh: None,
            pipe_solid,
            pipe_solid_nocull,
            pipe_water,
            pipe_lod,
            pipe_sky,
            ui,
            capture_next: false,
            captured: None,
            pipe_title,
            bind_title,
            title_uniform,
            hud_buf,
            timer,
            globals_solid,
            globals_water,
            bind_solid,
            bind_water,
            index_buf,
            instance_buf,
            lod_instance_buf,
            chunks: HashMap::new(),
            lods: HashMap::new(),
            pool: pool::VertexPool::new(),
            last_drawn: 0,
            last_hidden: 0,
            last_quads: 0,
            last_lod_drawn: 0,
            last_lod_quads: 0,
            palette: build_palette(reg),
            tint: None,
            tint_tex,
            start: Instant::now(),
            water_layer: reg.texture_names().iter().position(|n| n == "water").unwrap_or(usize::MAX) as u32,
            sky,
            foliage: shadows::foliage_mask(reg.texture_names(), texture_remap_ref.as_deref()),
            shadows,
            shadow_debug,
            gpu_line,
        })
    }

    /// Starts the biome tint map once the world's generator is known.
    /// The character's triangles for the next frame, in world space (none = not drawn).
    pub fn set_character(&mut self, verts: &[rig::Vertex]) {
        let n = (verts.len().min(MAX_CHARACTER_VERTS) / 3) * 3;
        if n > 0 {
            self.queue.write_buffer(&self.char_buf, 0, as_bytes(&verts[..n]));
        }
        self.char_verts = n as u32;
    }

    /// The character drawn in a menu (`RenderOptions::figure`): triangles with his feet at
    /// the origin, facing +Z.
    pub fn set_portrait(&mut self, verts: &[rig::Vertex]) {
        let n = (verts.len().min(MAX_CHARACTER_VERTS) / 3) * 3;
        if n > 0 {
            self.queue.write_buffer(&self.portrait_buf, 0, as_bytes(&verts[..n]));
        }
        self.portrait_verts = n as u32;
    }

    /// The open world's generator (biome colours come from it).
    pub fn set_generator(&mut self, gen: Arc<WorldGen>) {
        self.tint = Some(TintMap::new(gen));
    }

    /// Forgets the open world's meshes (it was closed, or another is opening).
    pub fn clear_world(&mut self) {
        for (_, c) in std::mem::take(&mut self.chunks) {
            self.release(c.solid);
            self.release(c.water);
        }
        for (_, l) in std::mem::take(&mut self.lods) {
            self.release(l.layer);
        }
        self.set_character(&[]);
    }

    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Whether the surface offers any mode that isn't locked to the display's refresh.
    pub fn vsync_supported(&self) -> bool {
        self.uncapped_mode.is_some()
    }

    pub fn gpu_line(&self) -> &str {
        &self.gpu_line
    }

    pub fn present_mode_name(&self) -> &'static str {
        match self.config.present_mode {
            wgpu::PresentMode::Fifo => "FIFO",
            wgpu::PresentMode::Immediate => "IMMEDIATE",
            wgpu::PresentMode::Mailbox => "MAILBOX",
            _ => "OTHER",
        }
    }

    fn apply_vsync(&mut self, on: bool) {
        let want = if on {
            wgpu::PresentMode::Fifo
        } else {
            self.uncapped_mode.unwrap_or(wgpu::PresentMode::Fifo)
        };
        if want != self.config.present_mode {
            self.config.present_mode = want;
            self.surface.configure(&self.device, &self.config);
        }
    }

    /// Chunk meshes drawn last frame (after distance and frustum culling).
    pub fn drawn_chunks(&self) -> usize {
        self.last_drawn
    }

    /// Chunk meshes in range and in view skipped last frame as hidden behind terrain.
    pub fn hidden_chunks(&self) -> usize {
        self.last_hidden
    }

    /// Quads in those meshes: what greedy meshing is meant to reduce.
    pub fn drawn_quads(&self) -> u64 {
        self.last_quads
    }

    /// LOD sections and quads drawn last frame.
    pub fn drawn_lod(&self) -> (usize, u64) {
        (self.last_lod_drawn, self.last_lod_quads)
    }

    /// Newest GPU frame time in ms, if the device supports timing and a result arrived.
    pub fn take_gpu_ms(&mut self) -> Option<f32> {
        self.timer.as_mut().and_then(|t| t.take())
    }

    fn write_ui_screen(&self) {
        let size = [self.config.width as f32, self.config.height as f32, 0.0, 0.0];
        self.queue.write_buffer(&self.ui.uniform, 0, as_bytes(&size));
    }

    /// Draws item `item`'s icon into icon slot `slot` of the thumbnails texture
    /// (`icons::icon_slot` hands the slots out).
    pub fn draw_icon(&self, reg: &BlockRegistry, slot: usize, item: u16) {
        let n = crate::icons::ICON;
        let id = item;
        {
            let (x, y) = icon_origin(slot);
            self.queue.write_texture(
                wgpu::ImageCopyTexture { texture: &self.ui.thumbs, mip_level: 0, origin: wgpu::Origin3d { x, y, z: 0 }, aspect: wgpu::TextureAspect::All },
                &crate::icons::render(reg, id),
                wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(4 * n), rows_per_image: Some(n) },
                wgpu::Extent3d { width: n, height: n, depth_or_array_layers: 1 },
            );
        }
    }

    /// The world uniforms for drawing the title screen's character: a camera in front of him
    /// at the height of his chest, framed so he is `sc.height` pixels tall with his feet at
    /// (`sc.x`, `sc.feet`) on screen, in late-afternoon light, no fog.
    fn showcase_globals(&mut self, sc: crate::ui::Showcase, w: f32, h: f32) -> Globals {
        let aspect = w / h.max(1.0);
        let (vp, eye, fwd) = showcase_view_proj(sc, w, h);
        Globals {
            view_proj: vp,
            cam_pos: [eye[0], eye[1], eye[2], 1.0],
            fog_params: [1.0e4, 2.0e4, 1.0, aspect],
            screen: [w, h, 0.0, 0.0],
            flags: [0.0, 0.0, self.water_layer as f32, 0.0],
            palette: self.palette,
            lod_window: [0.0; 4],
            lod_mask: [[0; 4]; 64],
            tint_window: [0.0; 4],
            sky_basis: math::sky_basis(fwd, aspect),
            sky: self.sky.uniforms(16.0),
            cloud: [0.0; 4],
            rays: [0.0; 4],
            shadow_mat: [[0.0; 16]; 2],
            shadow: [0.0; 4],
            light: [0.0, 1.0, 0.0, 1.0],
            light_color: [0.0; 4],
            look: [0.0; 4],
            foliage: [[0; 4]; 32],
        }
    }

    /// Puts a world's picture (`THUMB_W` x `THUMB_H` RGBA, sRGB) in thumbnail slot `slot`.
    pub fn set_thumbnail(&self, slot: usize, rgba: &[u8]) {
        if slot >= THUMB_SLOTS || rgba.len() != (THUMB_W * THUMB_H * 4) as usize {
            return;
        }
        let (x, y) = thumb_origin(slot);
        self.queue.write_texture(
            wgpu::ImageCopyTexture { texture: &self.ui.thumbs, mip_level: 0, origin: wgpu::Origin3d { x, y, z: 0 }, aspect: wgpu::TextureAspect::All },
            rgba,
            wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(4 * THUMB_W), rows_per_image: Some(THUMB_H) },
            wgpu::Extent3d { width: THUMB_W, height: THUMB_H, depth_or_array_layers: 1 },
        );
    }

    /// Draws the world once, without the HUD, into a texture, and returns the picture
    /// (RGBA, sRGB, full window size): a world's thumbnail is made from it.
    pub fn capture_world(
        &mut self,
        client: &mut Client,
        cam: &CameraView,
        view_blocks: f32,
        opts: &RenderOptions,
    ) -> Option<(Vec<u8>, u32, u32)> {
        self.capture_next = true;
        self.captured = None;
        let _ = self.render(client, cam, view_blocks, opts, &[]);
        self.capture_next = false;
        self.captured.take()
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        self.config.width = w;
        self.config.height = h;
        self.surface.configure(&self.device, &self.config);
        self.post = post_targets(&self.device, &self.post_layouts, w, h);
    }

    /// `parts` back to back in the vertex pool (8-byte vertices, 4 per quad).
    fn upload<V>(&mut self, parts: &[&[V]]) -> Option<GpuLayer> {
        let verts: usize = parts.iter().map(|p| p.len()).sum();
        let bytes: Vec<&[u8]> = parts.iter().map(|p| as_bytes(p)).collect();
        let slot = self.pool.upload(&self.device, &self.queue, &bytes)?;
        Some(GpuLayer { slot, quads: (verts / 4) as u32 })
    }

    /// Gives a mesh's range back to the pool.
    fn release(&mut self, layer: Option<GpuLayer>) {
        if let Some(l) = layer {
            self.pool.free(l.slot);
        }
    }

    /// Vertex pool use: (pages, MB reserved, MB used).
    pub fn pool_usage(&self) -> (usize, f32, f32) {
        let (n, size, used) = self.pool.usage();
        (n, size as f32 / 1048576.0, used as f32 / 1048576.0)
    }

    fn sync_meshes(&mut self, client: &mut Client) {
        for pos in client.take_mesh_changes(UPLOADS_PER_FRAME) {
            if let Some(old) = self.chunks.remove(&pos) {
                self.release(old.solid);
                self.release(old.water);
            }
            self.shadows.terrain_changed();
            if let Some(mesh) = client.meshes.get(&pos) {
                // Cutout (leaves) shares the opaque draw for now.
                let solid = self.upload(&[&mesh.opaque.vertices, &mesh.cutout.vertices]);
                let water = self.upload(&[&mesh.translucent.vertices]);
                let sky_lit = mesh.opaque.vertices.iter().chain(&mesh.cutout.vertices).any(|v| v.sky() > 0);
                self.chunks.insert(pos, GpuChunk { solid, water, sky_lit });
            }
        }
        for key in client.lod.take_evicted() {
            if let Some(old) = self.lods.remove(&key) {
                self.release(old.layer);
            }
        }
        for mesh in client.lod.take_built(LOD_UPLOADS_PER_FRAME) {
            if let Some(old) = self.lods.remove(&mesh.key) {
                self.release(old.layer);
            }
            let layer = self.upload(&[&mesh.vertices]);
            self.lods.insert(mesh.key, GpuLod { layer, min_y: mesh.min_y, max_y: mesh.max_y });
        }
    }

    /// LOD sections to draw: uploaded, in the frustum, not entirely under real terrain.
    /// Nearest first. Each entry is (instance data, section).
    fn visible_lods(
        &self,
        client: &Client,
        cam: [f32; 3],
        frustum: &math::Frustum,
        cov: &LodCoverage,
        frustum_cull: bool,
    ) -> Vec<([f32; 4], SectionKey)> {
        let mut out: Vec<(f32, [f32; 4], SectionKey)> = Vec::new();
        let chunk = CHUNK_SIZE;
        for &key in client.lod.render_set() {
            let Some(g) = self.lods.get(&key) else { continue };
            if g.layer.is_none() {
                continue;
            }
            let (ox, oz) = key.origin();
            let w = key.width();
            if frustum_cull
                && !frustum.contains_aabb(
                    [ox as f32, g.min_y, oz as f32],
                    [(ox + w) as f32, g.max_y, (oz + w) as f32],
                )
            {
                continue;
            }
            // Sections that can meet real chunks are masked per chunk; skip those whose every
            // chunk (over the section's height) is drawn by real terrain: nothing would show.
            let (c0x, c0z) = (ox.div_euclid(chunk), oz.div_euclid(chunk));
            let n = (w / chunk).max(1);
            let (cy0, cy1) = ((g.min_y.floor() as i32).div_euclid(chunk), (g.max_y.ceil() as i32 - 1).div_euclid(chunk));
            let in_window = c0x + n > cov.origin.0
                && c0z + n > cov.origin.2
                && c0x < cov.origin.0 + LodCoverage::SIZE
                && c0z < cov.origin.2 + LodCoverage::SIZE;
            let masked = key.level <= MASKED_MAX_LEVEL && in_window;
            if masked && (cy0..=cy1).all(|cy| (0..n).all(|a| (0..n).all(|b| cov.covered(c0x + a, cy, c0z + b)))) {
                continue;
            }
            let d = key.distance_to(cam[0], cam[2]);
            // w: 1 if masked, plus twice the voxel height (for the texture scale).
            let w_bits = masked as i32 + 2 * blockforge::lod::voxel_height(key.cell());
            out.push((d, [ox as f32, oz as f32, key.cell() as f32, w_bits as f32], key));
        }
        out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        out.truncate(MAX_LOD_INSTANCES);
        out.into_iter().map(|(_, i, k)| (i, k)).collect()
    }

    /// Draws a frame. On success returns how long we were blocked on the display (waiting
    /// for a frame to draw into, plus presenting it), so callers can exclude it from CPU time.
    pub fn render(
        &mut self,
        client: &mut Client,
        cam: &CameraView,
        view_blocks: f32,
        opts: &RenderOptions,
        hud: &[HudVertex],
    ) -> Result<Duration, wgpu::SurfaceError> {
        self.apply_vsync(opts.vsync);
        self.sync_meshes(client);

        let aspect = self.config.width as f32 / self.config.height as f32;
        let fwd = cam.forward();

        // Visible chunks: within the view distance and (optionally) inside the camera
        // frustum, tested against each chunk's bounding box.
        let vp = math::view_proj(cam.pos, fwd, aspect);
        let frustum = math::Frustum::from_view_proj(&vp);
        let size = CHUNK_SIZE as f32;
        let half = size * 0.5;
        if opts.occlusion_cull {
            client.update_sight(cam.pos);
        }
        let seen = if opts.occlusion_cull { client.seen_chunks() } else { None };
        let mut hidden = 0;
        let mut vis: Vec<(f32, ChunkPos)> = Vec::new();
        for pos in self.chunks.keys() {
            let o = pos.origin();
            let min = [o.x as f32, o.y as f32, o.z as f32];
            let c = [
                min[0] + half - cam.pos[0],
                min[1] + half - cam.pos[1],
                min[2] + half - cam.pos[2],
            ];
            let d2 = math::dot(c, c);
            if d2 > view_blocks * view_blocks {
                continue;
            }
            if opts.frustum_cull
                && !frustum.contains_aabb(min, [min[0] + size, min[1] + size, min[2] + size])
            {
                continue;
            }
            if seen.is_some_and(|s| !s.contains(pos)) {
                hidden += 1;
                continue;
            }
            vis.push((d2, *pos));
        }
        self.last_hidden = hidden;
        vis.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        vis.truncate(MAX_INSTANCES);
        vis.reverse(); // far to near, for the translucent pass
        self.last_drawn = vis.len();

        let instances: Vec<[f32; 4]> = vis
            .iter()
            .map(|(_, p)| {
                let o = p.origin();
                [o.x as f32, o.y as f32, o.z as f32, 0.0]
            })
            .collect();
        if !instances.is_empty() {
            self.queue.write_buffer(&self.instance_buf, 0, as_bytes(&instances));
        }

        // LOD sections, after this frame's uploads so coverage matches what is on the GPU.
        let lod_on = client.lod.settings().enabled();
        let cov = if lod_on { client.lod_coverage(view_blocks) } else { LodCoverage::empty((0, 0, 0)) };
        let lod_vis = if lod_on { self.visible_lods(client, cam.pos, &frustum, &cov, opts.frustum_cull) } else { Vec::new() };
        let lod_instances: Vec<[f32; 4]> = lod_vis.iter().map(|(i, _)| *i).collect();
        if !lod_instances.is_empty() {
            self.queue.write_buffer(&self.lod_instance_buf, 0, as_bytes(&lod_instances));
        }
        let mut lod_mask = [[0u32; 4]; 64];
        for (i, w) in cov.bits.iter().enumerate() {
            lod_mask[i / 4][i % 4] = *w;
        }

        // Biome colours: upload any level the worker finished.
        let (tint_window, tint_ready) = match self.tint.as_mut() {
            Some(t) => {
                for (level, data) in t.update(cam.pos) {
                    let n = tint::TEXELS as u32;
                    self.queue.write_texture(
                        wgpu::ImageCopyTexture {
                            texture: &self.tint_tex,
                            mip_level: 0,
                            origin: wgpu::Origin3d { x: 0, y: 0, z: level as u32 },
                            aspect: wgpu::TextureAspect::All,
                        },
                        &data,
                        wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(4 * n), rows_per_image: Some(n) },
                        wgpu::Extent3d { width: n, height: n, depth_or_array_layers: 1 },
                    );
                }
                (t.window(), t.ready_bits())
            }
            None => ([0.0; 4], 0),
        };
        let seconds = self.start.elapsed().as_secs_f64();
        let time = (seconds % 3600.0) as f32;

        // Clouds: take a finished window from the worker; the wind moves the one we have.
        if opts.clouds {
            if let Some(w) = self.clouds.update(cam.pos, seconds) {
                let quads = (w.vertices.len() / 4).min(MAX_QUADS as usize);
                self.cloud_mesh = (quads > 0).then(|| {
                    let buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("clouds"),
                        contents: as_bytes(&w.vertices[..quads * 4]),
                        usage: wgpu::BufferUsages::VERTEX,
                    });
                    (buf, quads as u32)
                });
            }
        }
        let cloud_origin = self.clouds.world_origin(seconds).filter(|_| opts.clouds && self.cloud_mesh.is_some());

        // God rays: only with the sun up and in front of the camera.
        let sky_uniforms = self.sky.uniforms(opts.time_of_day);
        let sun = [sky_uniforms.sun[0], sky_uniforms.sun[1], sky_uniforms.sun[2]];
        let rays = ray_params(&vp, cam.pos, fwd, sun, sky_uniforms.sun[3]).filter(|_| opts.god_rays);

        // Shadows: which maps to redraw this frame (they are drawn first, below).
        let (light_dir, light_col, light_strength) = shadows::light(&sky::params(opts.time_of_day));
        let look_on = opts.shaders != shadows::Quality::Off;
        let shadow_jobs = self.shadows.plan(opts.shaders, cam.pos, fwd, light_dir);
        let (shadow_mat, shadow_u) = self.shadows.uniforms(&shadow_jobs, light_strength);

        let mut globals = Globals {
            view_proj: vp,
            cam_pos: [cam.pos[0], cam.pos[1], cam.pos[2], 1.0],
            fog_params: [opts.fog_start, opts.fog_end.max(opts.fog_start + 1.0), 1.0, aspect],
            screen: [self.config.width as f32, self.config.height as f32, 0.0, 0.0],
            // x: fog on, y: seconds (animated water), z: the water texture's layer,
            // w: which tint map levels are ready (bits).
            flags: [if opts.fog { 1.0 } else { 0.0 }, time, self.water_layer as f32, tint_ready as f32],
            palette: self.palette,
            lod_window: [cov.origin.0 as f32, cov.origin.2 as f32, cov.origin.1 as f32, 0.0],
            lod_mask,
            tint_window,
            sky_basis: math::sky_basis(fwd, aspect),
            sky: sky_uniforms,
            cloud: cloud_origin.map_or([0.0; 4], |o| [o[0], o[1], clouds::BASE_Y, clouds::FADE_END]),
            rays: rays.unwrap_or([0.0; 4]),
            shadow_mat,
            shadow: shadow_u,
            // w: soft-edge radius in texels (Fancy wider), negative where the GPU filters the
            // maps itself (its 2x2 filter then does Lite's softening alone).
            light: [light_dir[0], light_dir[1], light_dir[2], shadow_softness(opts.shaders)],
            light_color: [light_col[0], light_col[1], light_col[2], if look_on { 1.0 } else { 0.0 }],
            look: if look_on { [1.0, 1.0, 1.0, 1.0] } else { [0.0; 4] },
            foliage: self.foliage,
        };
        self.queue.write_buffer(&self.globals_solid, 0, as_bytes(std::slice::from_ref(&globals)));
        globals.fog_params[2] = 0.65;
        self.queue.write_buffer(&self.globals_water, 0, as_bytes(std::slice::from_ref(&globals)));

        // A capture (`capture_world`) draws the world alone into a texture of its own.
        let capture = std::mem::take(&mut self.capture_next);
        let hud_verts = if capture { 0 } else { hud.len().min(MAX_HUD_VERTS) };
        if hud_verts > 0 {
            self.queue.write_buffer(&self.hud_buf, 0, as_bytes(&hud[..hud_verts]));
            self.write_ui_screen();
        }
        // A character over the menu: the HUD under him in the main pass, then he and the rest
        // in a pass of their own (the depth buffer cleared for him).
        let figure = opts.figure.filter(|_| hud_verts > 0 && self.portrait_verts > 0);
        if let Some((sc, _)) = figure {
            let (w, h) = (self.config.width as f32, self.config.height as f32);
            let g = self.showcase_globals(sc, w, h);
            self.queue.write_buffer(&self.globals_portrait, 0, as_bytes(std::slice::from_ref(&g)));
        }
        let hud_under = figure.map_or(hud_verts, |(_, u)| u.min(hud_verts));

        let acquire_start = Instant::now();
        let (frame, offscreen) = if capture {
            let t = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("capture"),
                size: wgpu::Extent3d { width: self.config.width, height: self.config.height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.config.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            (None, Some(t))
        } else {
            (Some(self.surface.get_current_texture()?), None)
        };
        let acquire_wait = acquire_start.elapsed();
        let view = match (&frame, &offscreen) {
            (Some(f), _) => f.texture.create_view(&wgpu::TextureViewDescriptor::default()),
            (None, Some(t)) => t.create_view(&wgpu::TextureViewDescriptor::default()),
            _ => unreachable!(),
        };
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        for job in &shadow_jobs {
            let size = CHUNK_SIZE as f32;
            let mut casters: Vec<([f32; 4], wgpu::BufferSlice<'_>, u32)> = Vec::new();
            for (pos, c) in &self.chunks {
                let Some(layer) = c.solid.as_ref().filter(|_| c.sky_lit) else { continue };
                let o = pos.origin();
                let min = [o.x as f32, o.y as f32, o.z as f32];
                if job.holds(&self.shadows, min, size) {
                    casters.push(([min[0], min[1], min[2], 0.0], self.pool.slice(&layer.slot), layer.quads.min(MAX_QUADS)));
                }
            }
            let character = (self.char_verts > 0).then(|| (self.char_buf.slice(..), self.char_verts));
            self.shadows.draw(&self.queue, &mut encoder, &self.index_buf, job, &casters, character);
        }
        let mut quads = 0u64;
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Every pixel gets terrain or sky; clearing is just the cheapest start.
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.post.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(math::DEPTH_CLEAR),
                        // Kept only when the ray mask is made from it after this pass.
                        store: if rays.is_some() { wgpu::StoreOp::Store } else { wgpu::StoreOp::Discard },
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: self.timer.as_ref().map(|t| t.pass_writes()),
                occlusion_query_set: None,
            });

            pass.set_index_buffer(self.index_buf.slice(..), wgpu::IndexFormat::Uint32);
            pass.set_vertex_buffer(1, self.instance_buf.slice(..));

            pass.set_pipeline(if opts.backface_cull { &self.pipe_solid } else { &self.pipe_solid_nocull });
            pass.set_bind_group(0, &self.bind_solid, &[]);
            // `vis` is far to near (what the water pass needs). Solid terrain goes the other
            // way: near chunks fill the depth buffer first and hide most of what is behind them.
            for (i, (_, pos)) in vis.iter().enumerate().rev() {
                if let Some(layer) = self.chunks.get(pos).and_then(|c| c.solid.as_ref()) {
                    pass.set_vertex_buffer(0, self.pool.slice(&layer.slot));
                    pass.draw_indexed(0..layer.quads.min(MAX_QUADS) * 6, 0, i as u32..i as u32 + 1);
                    quads += layer.quads as u64;
                }
            }

            // LOD terrain fills what the real terrain left empty (depth-tested against it).
            let mut lod_quads = 0u64;
            if !lod_vis.is_empty() {
                pass.set_pipeline(&self.pipe_lod);
                pass.set_vertex_buffer(1, self.lod_instance_buf.slice(..));
                for (i, (_, key)) in lod_vis.iter().enumerate() {
                    if let Some(layer) = self.lods.get(key).and_then(|g| g.layer.as_ref()) {
                        pass.set_vertex_buffer(0, self.pool.slice(&layer.slot));
                        pass.draw_indexed(0..layer.quads.min(MAX_QUADS) * 6, 0, i as u32..i as u32 + 1);
                        lod_quads += layer.quads as u64;
                    }
                }
                pass.set_vertex_buffer(1, self.instance_buf.slice(..));
            }
            self.last_lod_drawn = lod_vis.len();
            self.last_lod_quads = lod_quads;

            // The character.
            if self.char_verts > 0 {
                pass.set_pipeline(&self.pipe_char);
                pass.set_bind_group(1, &self.bind_char, &[]);
                pass.set_vertex_buffer(0, self.char_buf.slice(..));
                pass.draw(0..self.char_verts, 0..1);
            }

            // The sky, wherever no terrain was drawn (water, drawn next, blends over it).
            pass.set_pipeline(&self.pipe_sky);
            pass.draw(0..3, 0..1);

            // Clouds, over the sky and behind or in front of the terrain as the depth says.
            if let (Some(_), Some((buf, n))) = (cloud_origin, self.cloud_mesh.as_ref()) {
                pass.set_pipeline(&self.pipe_cloud);
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw_indexed(0..n * 6, 0, 0..1);
            }

            pass.set_pipeline(&self.pipe_water);
            pass.set_bind_group(0, &self.bind_water, &[]);
            for (i, (_, pos)) in vis.iter().enumerate() {
                if let Some(layer) = self.chunks.get(pos).and_then(|c| c.water.as_ref()) {
                    pass.set_vertex_buffer(0, self.pool.slice(&layer.slot));
                    pass.draw_indexed(0..layer.quads.min(MAX_QUADS) * 6, 0, i as u32..i as u32 + 1);
                    quads += layer.quads as u64;
                }
            }

            // God rays, from the mask made after the previous frame.
            if rays.is_some() {
                pass.set_pipeline(&self.pipe_rays);
                pass.set_bind_group(0, &self.bind_solid, &[]);
                pass.set_bind_group(1, &self.post.bind_rays, &[]);
                pass.draw(0..3, 0..1);
            }

            // HUD (crosshair, stats, menu)
            if hud_under > 0 {
                pass.set_pipeline(&self.ui.pipe);
                pass.set_bind_group(0, &self.ui.bind, &[]);
                pass.set_vertex_buffer(0, self.hud_buf.slice(0..(hud_verts * std::mem::size_of::<HudVertex>()) as u64));
                pass.draw(0..hud_under as u32, 0..1);
            }
        }
        self.last_quads = quads;
        // The ray mask for the next frame: where this frame's depth buffer shows open sky.
        if rays.is_some() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ray mask"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.post.mask_view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipe_mask);
            pass.set_bind_group(0, &self.bind_solid, &[]);
            pass.set_bind_group(1, &self.post.bind_mask, &[]);
            pass.draw(0..3, 0..1);
        }
        // (after the ray mask, which reads this frame's depth)
        if figure.is_some() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("portrait"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.post.depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(math::DEPTH_CLEAR), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipe_char);
            pass.set_bind_group(0, &self.bind_portrait, &[]);
            pass.set_bind_group(1, &self.bind_char, &[]);
            pass.set_vertex_buffer(0, self.portrait_buf.slice(..));
            pass.draw(0..self.portrait_verts, 0..1);
            if hud_under < hud_verts {
                pass.set_pipeline(&self.ui.pipe);
                pass.set_bind_group(0, &self.ui.bind, &[]);
                pass.set_vertex_buffer(0, self.hud_buf.slice(0..(hud_verts * std::mem::size_of::<HudVertex>()) as u64));
                pass.draw(hud_under as u32..hud_verts as u32, 0..1);
            }
        }
        if opts.shadow_debug && opts.shaders != shadows::Quality::Off {
            let (w, h) = (self.config.width as f32, self.config.height as f32);
            let side = (h * 0.22).round();
            self.shadow_debug.draw(&mut encoder, &view, (w * 0.012).round(), (h * 0.64).round(), side);
        }
        if let Some(t) = self.timer.as_mut() {
            t.encode(&mut encoder);
        }
        if let Some(t) = &offscreen {
            self.captured = read_back(&self.device, &self.queue, encoder, t, self.config.format);
            return Ok(acquire_wait);
        }
        self.queue.submit(std::iter::once(encoder.finish()));
        let present_start = Instant::now();
        frame.expect("a frame").present();
        let present_wait = present_start.elapsed();
        if let Some(t) = self.timer.as_mut() {
            t.after_submit(&self.device);
        }
        Ok(acquire_wait + present_wait)
    }

    /// Draws the title screen: the turning panorama (camera yaw `yaw`) and the HUD (its
    /// buttons). The world is not drawn. Returns the display wait, as `render` does.
    /// The title screen: the turning panorama, then the first `under` HUD vertices, the
    /// character standing where `figure` says (his triangles as given to `set_character`,
    /// feet at the origin, facing +Z), then the rest of the HUD.
    pub fn render_title(
        &mut self,
        yaw: f32,
        hud: &[HudVertex],
        under: usize,
        figure: Option<crate::ui::Showcase>,
    ) -> Result<Duration, wgpu::SurfaceError> {
        let (w, h) = (self.config.width as f32, self.config.height as f32);
        let view = [yaw, (crate::title::FOVY * 0.5).tan(), w / h.max(1.0), 1.0];
        self.queue.write_buffer(&self.title_uniform, 0, as_bytes(&view));
        let figure = figure.filter(|_| self.char_verts > 0);
        if let Some(sc) = figure {
            let globals = self.showcase_globals(sc, w, h);
            self.queue.write_buffer(&self.globals_solid, 0, as_bytes(std::slice::from_ref(&globals)));
        }
        let under = under.min(hud.len());
        self.write_ui_screen();
        let hud_verts = hud.len().min(MAX_HUD_VERTS);
        if hud_verts > 0 {
            self.queue.write_buffer(&self.hud_buf, 0, as_bytes(&hud[..hud_verts]));
        }
        let acquire_start = Instant::now();
        let frame = self.surface.get_current_texture()?;
        let acquire_wait = acquire_start.elapsed();
        let target = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("title") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("title pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.post.depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(math::DEPTH_CLEAR), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipe_title);
            pass.set_bind_group(0, &self.bind_title, &[]);
            pass.draw(0..3, 0..1);
            let draw_hud = |pass: &mut wgpu::RenderPass<'_>, range: std::ops::Range<usize>| {
                if range.start < range.end {
                    pass.set_pipeline(&self.ui.pipe);
                    pass.set_bind_group(0, &self.ui.bind, &[]);
                    pass.set_vertex_buffer(0, self.hud_buf.slice(0..(hud_verts * std::mem::size_of::<HudVertex>()) as u64));
                    pass.draw(range.start as u32..range.end as u32, 0..1);
                }
            };
            draw_hud(&mut pass, 0..under.min(hud_verts));
            if figure.is_some() {
                pass.set_pipeline(&self.pipe_char);
                pass.set_bind_group(0, &self.bind_solid, &[]);
                pass.set_bind_group(1, &self.bind_char, &[]);
                pass.set_vertex_buffer(0, self.char_buf.slice(..));
                pass.draw(0..self.char_verts, 0..1);
            }
            draw_hud(&mut pass, under.min(hud_verts)..hud_verts);
        }
        self.queue.submit(std::iter::once(encoder.finish()));
        let present_start = Instant::now();
        frame.present();
        Ok(acquire_wait + present_start.elapsed())
    }
}

fn voxel_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    vertex_entry: &str,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
    depth_write: bool,
    cull: Option<wgpu::Face>,
    label: &str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: vertex_entry,
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[
                wgpu::VertexBufferLayout {
                    array_stride: 8,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &VERTEX_ATTRS,
                },
                wgpu::VertexBufferLayout {
                    array_stride: 16,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &INSTANCE_ATTRS,
                },
            ],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: "fs_main",
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: cull,
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: depth_write,
            // Reverse-Z: nearer is larger.
            depth_compare: wgpu::CompareFunction::Greater,
            stencil: wgpu::StencilState::default(),
            // Translucent faces (water) lose depth ties: where one lies in the plane of an
            // opaque face (water beside a slab or other partial block), the opaque face shows
            // steadily instead of the two flickering. Depth is reversed: negative is farther.
            // LOD terrain loses ties too (while chunks are still arriving it can be drawn in
            // the same place as the real terrain for a moment), but by a constant only: a
            // slope-scaled bias pushes flat water seen at a grazing angle back further than the
            // upright sides under it, which then show through every seam as dark lines.
            bias: if !depth_write {
                wgpu::DepthBiasState { constant: -4, slope_scale: -1.5, clamp: 0.0 }
            } else if vertex_entry == "vs_lod" {
                wgpu::DepthBiasState { constant: -2, slope_scale: 0.0, clamp: 0.0 }
            } else {
                wgpu::DepthBiasState::default()
            },
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

/// God-ray settings for a frame: the sun's place on screen (uv, may be off screen) and the
/// rays' strength, or None when there is nothing to draw (sun down, or behind the camera).
pub fn ray_params(vp: &math::Mat4, eye: [f32; 3], forward: [f32; 3], sun: [f32; 3], sun_disc: f32) -> Option<[f32; 4]> {
    let facing = math::dot(math::normalize(forward), sun);
    // Fades out as the sun leaves the view to the side, and as it sets.
    let strength = 0.42 * sun_disc * ((facing - 0.25) / 0.45).clamp(0.0, 1.0);
    if strength < 0.01 {
        return None;
    }
    let c = math::transform(vp, [eye[0] + sun[0] * 1000.0, eye[1] + sun[1] * 1000.0, eye[2] + sun[2] * 1000.0, 1.0]);
    if c[3] <= 0.0 {
        return None;
    }
    Some([c[0] / c[3] * 0.5 + 0.5, 0.5 - c[1] / c[3] * 0.5, strength, 0.0])
}

/// The sky: one triangle covering the screen at the far depth (0 with reverse-Z), passing
/// the depth test only where nothing was drawn. No vertex buffers.
fn sky_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sky"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: "vs_sky",
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: "fs_sky",
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::GreaterEqual,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

/// The title screen's background: pipeline, bind group (panorama and its uniform) and the
/// uniform buffer.
fn title_pipeline(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> (wgpu::RenderPipeline, wgpu::BindGroup, wgpu::Buffer) {
    let max = device.limits().max_texture_dimension_2d.min(4096);
    let (px, w, h) = crate::title::panorama(max).unwrap_or_else(|e| {
        eprintln!("blockforge: {e}");
        (vec![40, 50, 80, 255], 1, 1)
    });
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("title panorama"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::ImageCopyTexture { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        &px,
        wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h) },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("title panorama"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let uniform = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("title uniform"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("title layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("title"),
        layout: &bgl,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
        ],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("title"),
        bind_group_layouts: &[&bgl],
        push_constant_ranges: &[],
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("title shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("title.wgsl").into()),
    });
    let pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("title"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: "vs_title",
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: "fs_title",
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        // Drawn in a pass with the depth buffer (the HUD pipeline needs one).
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Always,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    });
    (pipe, bind, uniform)
}

/// Thumbnails: 256 x 144 pictures, 4 across and 7 down a 1024 x 1024 texture.
pub const THUMB_W: u32 = crate::worlds::THUMB_W;
pub const THUMB_H: u32 = crate::worlds::THUMB_H;
/// The camera for the title screen's character: in front of him at the height of his chest,
/// far enough for 1.8 blocks to fill `sc.height` pixels, with the picture shifted so his
/// feet (the origin) land on (`sc.x`, `sc.feet`). Returns the view-projection, eye and
/// forward direction.
pub fn showcase_view_proj(sc: crate::ui::Showcase, w: f32, h: f32) -> (math::Mat4, [f32; 3], [f32; 3]) {
    const CHEST: f32 = 0.95;
    let aspect = w / h.max(1.0);
    let t = (math::FOVY * 0.5).tan();
    let dist = 1.8 * h / (2.0 * sc.height.max(1.0) * t);
    let (eye, fwd) = ([0.0, CHEST, dist], [0.0, 0.0, -1.0]);
    let mut vp = math::view_proj(eye, fwd, aspect);
    // A shift in clip space (x and y move by w times the offset).
    let want = [2.0 * sc.x / w - 1.0, 1.0 - 2.0 * sc.feet / h];
    let feet_y = -CHEST / (dist * t);
    let (dx, dy) = (want[0], want[1] - feet_y);
    for c in 0..4 {
        vp[c * 4] += dx * vp[c * 4 + 3];
        vp[c * 4 + 1] += dy * vp[c * 4 + 3];
    }
    (vp, eye, fwd)
}

/// World pictures fill the top six rows of the thumbnails texture; the block icons
/// (`icons.rs`) the rows under them.
pub const THUMB_SLOTS: usize = 24;
pub const THUMBS_SIZE: u32 = 1024;
/// The thumbnails texture is twice as tall as wide: room for 288 icons.
pub const THUMBS_HEIGHT: u32 = 2 * THUMBS_SIZE;
const ICONS_Y: u32 = 6 * THUMB_H;
const ICONS_PER_ROW: u32 = THUMBS_SIZE / crate::icons::ICON;
pub const ICON_SLOTS: usize = (ICONS_PER_ROW * ((THUMBS_HEIGHT - ICONS_Y) / crate::icons::ICON)) as usize;

/// Where block icon `i` sits in the thumbnails texture.
pub fn icon_origin(i: usize) -> (u32, u32) {
    let i = i as u32;
    ((i % ICONS_PER_ROW) * crate::icons::ICON, ICONS_Y + (i / ICONS_PER_ROW) * crate::icons::ICON)
}

/// Block icon `i`'s texture coordinates: u0, v0, u1, v1.
pub fn icon_uv(i: usize) -> [f32; 4] {
    let (x, y) = icon_origin(i);
    let (s, t) = (THUMBS_SIZE as f32, THUMBS_HEIGHT as f32);
    let n = crate::icons::ICON;
    [x as f32 / s, y as f32 / t, (x + n) as f32 / s, (y + n) as f32 / t]
}

/// Where thumbnail slot `slot` starts in the thumbnails' texture, in pixels.
pub fn thumb_origin(slot: usize) -> (u32, u32) {
    ((slot % 4) as u32 * THUMB_W, (slot / 4) as u32 * THUMB_H)
}

/// The thumbnails' texture coordinates of slot `slot` (u0, v0, u1, v1).
pub fn thumb_uv(slot: usize) -> [f32; 4] {
    let (x, y) = thumb_origin(slot);
    let (s, t) = (THUMBS_SIZE as f32, THUMBS_HEIGHT as f32);
    [x as f32 / s, y as f32 / t, (x + THUMB_W) as f32 / s, (y + THUMB_H) as f32 / t]
}

struct UiGpu {
    pipe: wgpu::RenderPipeline,
    bind: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    thumbs: wgpu::Texture,
}

/// Copies `t` into a buffer, waits for it, and returns its pixels as RGBA.
fn read_back(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    mut encoder: wgpu::CommandEncoder,
    t: &wgpu::Texture,
    format: wgpu::TextureFormat,
) -> Option<(Vec<u8>, u32, u32)> {
    let (w, h) = (t.width(), t.height());
    let row = (4 * w).div_ceil(256) * 256;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("capture read back"),
        size: (row * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::ImageCopyTexture { texture: t, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::ImageCopyBuffer { buffer: &buf, layout: wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit(std::iter::once(encoder.finish()));
    let slice = buf.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device.poll(wgpu::Maintain::Wait);
    rx.recv().ok()?.ok()?;
    let data = slice.get_mapped_range();
    let bgra = matches!(format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        let r = &data[(y * row) as usize..(y * row + 4 * w) as usize];
        for p in r.chunks_exact(4) {
            if bgra {
                out.extend_from_slice(&[p[2], p[1], p[0], 255]);
            } else {
                out.extend_from_slice(&[p[0], p[1], p[2], 255]);
            }
        }
    }
    drop(data);
    buf.unmap();
    Some((out, w, h))
}

/// The HUD's pipeline: `ui.wgsl` with the UI atlas and the thumbnails' texture.
fn ui_pipeline(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> UiGpu {
    let texture = |label: &str, w: u32, h: u32| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    };
    // The atlas.
    let dec = png::Decoder::new(crate::atlas::ATLAS_PNG);
    let mut reader = dec.read_info().expect("UI atlas PNG");
    let mut px = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut px).expect("UI atlas pixels");
    let (aw, ah) = (info.width, info.height);
    let atlas = texture("ui atlas", aw, ah);
    queue.write_texture(
        wgpu::ImageCopyTexture { texture: &atlas, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        &px[..(aw * ah * 4) as usize],
        wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(4 * aw), rows_per_image: Some(ah) },
        wgpu::Extent3d { width: aw, height: ah, depth_or_array_layers: 1 },
    );
    let thumbs = texture("world thumbnails", THUMBS_SIZE, THUMBS_HEIGHT);
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("ui"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let uniform = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ui screen"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ui layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
            tex_entry(1),
            tex_entry(2),
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let atlas_view = atlas.create_view(&wgpu::TextureViewDescriptor::default());
    let thumbs_view = thumbs.create_view(&wgpu::TextureViewDescriptor::default());
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("ui"),
        layout: &bgl,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&atlas_view) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&thumbs_view) },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&sampler) },
        ],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("ui"), bind_group_layouts: &[&bgl], push_constant_ranges: &[] });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("ui shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("ui.wgsl").into()),
    });
    let pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("ui"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: "vs_ui",
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<HudVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &HUD_ATTRS,
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: "fs_ui",
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState { format, blend: Some(wgpu::BlendState::ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        // The passes have a depth attachment, so every pipeline in them declares depth state.
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Always,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    });
    UiGpu { pipe, bind, uniform, thumbs }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_character_lands_where_the_menu_puts_him() {
        let (w, h) = (2400.0, 1080.0);
        let sc = crate::ui::Showcase { x: 1800.0, feet: 800.0, height: 500.0, yaw: 0.0 };
        let (vp, ..) = showcase_view_proj(sc, w, h);
        let screen = |p: [f32; 3]| {
            let c = [0, 1, 3].map(|r| vp[r] * p[0] + vp[4 + r] * p[1] + vp[8 + r] * p[2] + vp[12 + r]);
            ((c[0] / c[2] + 1.0) * 0.5 * w, (1.0 - c[1] / c[2]) * 0.5 * h)
        };
        let (fx, fy) = screen([0.0, 0.0, 0.0]);
        let (hx, hy) = screen([0.0, 1.8, 0.0]);
        assert!((fx - 1800.0).abs() < 1.0 && (fy - 800.0).abs() < 1.0, "feet at {fx},{fy}");
        assert!((hx - 1800.0).abs() < 1.0 && (fy - hy - 500.0).abs() < 2.0, "head at {hx},{hy}");
    }
    use blockforge::mesh::PackedVertex;

    #[test]
    fn every_face_is_front_facing_when_seen_from_outside() {
        // Solid geometry is back-face culled, so a face wound the wrong way would vanish.
        // Project each face of a block from a camera in front of it and require
        // counter-clockwise triangles in NDC (wgpu's front face).
        use blockforge::block::AIR;
        use blockforge::coords::Face;
        use blockforge::mesh::{mesh_chunk, PaddedChunk};

        let reg = BlockRegistry::builtin();
        let mut pc = PaddedChunk::filled(AIR);
        pc.set(5, 5, 5, reg.state("stone").unwrap());
        let mesh = mesh_chunk(&reg, &pc);
        let center = [5.5f32, 5.5, 5.5];
        for face in Face::ALL {
            let (nx, ny, nz) = face.normal();
            let n = [nx as f32, ny as f32, nz as f32];
            // A small offset keeps the view direction off the vertical axis for +Y/-Y faces.
            let eye = [
                center[0] + n[0] * 6.0 + 0.3,
                center[1] + n[1] * 6.0 + 0.2,
                center[2] + n[2] * 6.0 + 0.1,
            ];
            let fwd = [center[0] - eye[0], center[1] - eye[1], center[2] - eye[2]];
            let vp = math::view_proj(eye, fwd, 1.0);
            let verts: Vec<PackedVertex> = mesh
                .opaque
                .vertices
                .iter()
                .copied()
                .filter(|v| v.face_index() == face as u32)
                .collect();
            assert_eq!(verts.len(), 4, "{face:?}");
            let p = |i: usize| {
                let v = verts[i];
                let c = math::transform(
                    &vp,
                    [v.x() as f32 / 4.0, v.y() as f32 / 4.0, v.z() as f32 / 4.0, 1.0],
                );
                assert!(c[3] > 0.0, "vertex behind the camera for {face:?}");
                [c[0] / c[3], c[1] / c[3]]
            };
            for tri in [[0usize, 1, 2], [0, 2, 3]] {
                let (a, b, c) = (p(tri[0]), p(tri[1]), p(tri[2]));
                let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
                assert!(area > 0.0, "{face:?} triangle {tri:?} is back-facing (area {area})");
            }
        }
    }

    #[test]
    fn globals_layout_matches_the_wgsl_struct() {
        // 64 (mat4) + 4 * 16 (vec4s) + 32 * 16 (palette) + 16 (lod window) + 32 * 16 (lod mask)
        // + 16 (tint window) + 3 * 16 (sky basis) + sky: 4 * 16 + 4 * 16 (glow) + 6 * 16 (faces)
        // + 3 * 16 (night rotation) + 3 * 16 + 16 (clouds) + 16 (rays)
        // + 2 * 64 (shadow matrices) + 4 * 16 (shadow, light, light colour, look) + 32 * 16 (foliage)
        assert_eq!(
            std::mem::size_of::<Globals>(),
            64 + 64 + 512 + 16 + 1024 + 16 + 48 + 64 + 64 + 96 + 48 + 48 + 32 + 128 + 64 + 512
        );
    }

    #[test]
    fn the_block_texture_array_is_never_taken_for_a_cube_map() {
        assert_eq!(array_layers(1), 2);
        assert_eq!(array_layers(20), 20);
        assert_eq!(array_layers(24), 25);
        assert!((1..200).all(|n| array_layers(n) % 6 != 0 && array_layers(n) as usize >= n));
        // The tint map's two layers are safe too.
        assert_ne!(2 % 6, 0);
    }

    #[test]
    fn god_rays_aim_at_the_sun_and_switch_off_when_it_is_out_of_view() {
        let eye = [0.0, 50.0, 0.0];
        let sun = math::normalize([0.3, 0.5, -0.8]);
        let vp = math::view_proj(eye, sun, 2.0);
        let r = ray_params(&vp, eye, sun, sun, 1.0).expect("looking at the sun");
        assert!((r[0] - 0.5).abs() < 1e-3 && (r[1] - 0.5).abs() < 1e-3 && r[2] > 0.3, "{r:?}");
        // The sun up and to the right of the view centre lands up and to the right on screen.
        let fwd = math::normalize([0.1, 0.3, -0.9]);
        let r = ray_params(&math::view_proj(eye, fwd, 2.0), eye, fwd, sun, 1.0).unwrap();
        assert!(r[0] > 0.5 && r[1] < 0.5, "{r:?}");
        assert!(ray_params(&vp, eye, sun, sun, 0.0).is_none(), "sun below the horizon");
        let away = [-sun[0], -sun[1], -sun[2]];
        assert!(ray_params(&math::view_proj(eye, away, 2.0), eye, away, sun, 1.0).is_none(), "sun behind");
        assert_eq!(std::mem::size_of::<HudVertex>(), 24);
    }

    #[test]
    fn textures_past_the_gpu_limit_are_drawn_as_similar_ones_under_it() {
        let reg = BlockRegistry::builtin();
        let names = reg.texture_names();
        assert_eq!(texture_layers(names, 4096), (array_layers(names.len()), None));
        let (layers, map) = texture_layers(names, 256);
        let map = map.expect("a remap");
        assert_eq!(layers, 256);
        assert!(map.iter().all(|&t| (t as u32) < layers));
        assert!(map.iter().enumerate().take(255).all(|(i, &t)| i == t as usize));
        let r = reg.remap_textures(&map);
        assert!((1..r.len() as u16).all(|id| r.def(id).tex.iter().all(|&t| (t as u32) < layers)));
        assert_eq!(r.id("mossrock_herringbone"), reg.id("mossrock_herringbone"));
    }

    #[test]
    fn every_builtin_texture_gets_a_colour_and_fits_the_palette() {
        let reg = BlockRegistry::builtin();
        // (the distant terrain keeps a texture in 8 bits, the texture array a layer each)
        assert!(reg.texture_names().len() < 2048, "{} textures", reg.texture_names().len());
        let p = build_palette(&reg);
        for (i, name) in reg.texture_names().iter().enumerate().take(32) {
            if name == "air" {
                continue; // never drawn
            }
            assert_ne!(p[i], [1.0, 0.0, 1.0, 1.0], "texture {name} fell through");
        }
    }
}
