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

use crate::texgen;
use crate::textures;
use crate::tint::{self, TintMap};
use blockforge::worldgen::WorldGen;
use wgpu::util::DeviceExt;
use winit::window::Window;

use crate::game::CameraView;
use crate::gpu_timer::GpuTimer;
use crate::math;
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
const SKY_SRGB: [f32; 3] = [0.55, 0.74, 0.95];

const VERTEX_ATTRS: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Uint32x2];
const INSTANCE_ATTRS: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![1 => Float32x4];
const HUD_ATTRS: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![0 => Float32x2, 1 => Unorm8x4];

#[repr(C)]
#[derive(Clone, Copy)]
struct Globals {
    view_proj: [f32; 16],
    cam_pos: [f32; 4],
    fog_color: [f32; 4],
    fog_params: [f32; 4],
    screen: [f32; 4],
    flags: [f32; 4],
    palette: [[f32; 4]; 32],
    /// x, y: chunk column at bit 0 of `lod_mask` (see `LodCoverage`).
    lod_window: [f32; 4],
    /// One bit per chunk column: set where LOD terrain must not be drawn.
    lod_mask: [[u32; 4]; 32],
    /// The tint map's level centres in blocks: x0, z0, x1, z1 (see `tint`).
    tint_window: [f32; 4],
}

/// Per-frame toggles chosen in the settings menu.
#[derive(Clone, Copy, Debug)]
pub struct RenderOptions {
    pub fog: bool,
    /// Skip chunks whose bounding box is outside the camera frustum.
    pub frustum_cull: bool,
    /// Use the back-face-culling pipeline for solid terrain.
    pub backface_cull: bool,
    /// true = FIFO (capped at the display rate); false = the fastest uncapped mode available.
    pub vsync: bool,
    /// Where fog starts and where it is complete, in blocks from the camera.
    pub fog_start: f32,
    pub fog_end: f32,
}

fn as_bytes<T>(s: &[T]) -> &[u8] {
    // Only used with plain-old-data (f32 arrays, #[repr(C)] vertex/uniform structs).
    unsafe { std::slice::from_raw_parts(s.as_ptr() as *const u8, std::mem::size_of_val(s)) }
}

fn srgb_to_linear(c: f32) -> f32 {
    c.powf(2.2)
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

/// Mip levels of the block textures: 32, 16, 8, 4, 2, 1 texels.
const TEXTURE_MIPS: u32 = 6;

/// All block textures as one array texture (layer = texture id), with mipmaps, and the
/// sampler for it: crisp texels up close, smoothly averaged further away.
fn block_textures(device: &wgpu::Device, queue: &wgpu::Queue, reg: &BlockRegistry) -> (wgpu::TextureView, wgpu::Sampler) {
    let names = reg.texture_names();
    // At least two layers: some GL drivers mistake a one-layer array for a plain 2D texture.
    let layers = names.len().max(2) as u32;
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
    (view, sampler)
}

fn make_depth(device: &wgpu::Device, w: u32, h: u32) -> wgpu::TextureView {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

struct GpuLayer {
    buf: wgpu::Buffer,
    quads: u32,
}

struct GpuChunk {
    solid: Option<GpuLayer>,
    water: Option<GpuLayer>,
}

struct GpuLod {
    layer: Option<GpuLayer>,
    min_y: f32,
    max_y: f32,
}

pub struct Gfx {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    /// Immediate or Mailbox if the surface offers one: used when VSYNC is turned off.
    uncapped_mode: Option<wgpu::PresentMode>,
    depth_view: wgpu::TextureView,
    pipe_solid: wgpu::RenderPipeline,
    pipe_solid_nocull: wgpu::RenderPipeline,
    pipe_water: wgpu::RenderPipeline,
    pipe_lod: wgpu::RenderPipeline,
    pipe_hud: wgpu::RenderPipeline,
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
    last_drawn: usize,
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
                    required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
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
        let depth_view = make_depth(&device, config.width, config.height);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blockforge shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

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
            ],
        });
        let (tint_tex, tint_view, tint_sampler) = tint_texture(&device);
        let (block_view, block_sampler) = block_textures(&device, &queue, reg);
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
        let globals_water = make_globals("globals water");
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
                ],
            })
        };
        let bind_solid = make_bind("bind solid", &globals_solid);
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
        let pipe_hud = hud_pipeline(&device, &layout, &shader, format);

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

        Ok(Gfx {
            surface,
            device,
            queue,
            config,
            uncapped_mode,
            depth_view,
            pipe_solid,
            pipe_solid_nocull,
            pipe_water,
            pipe_lod,
            pipe_hud,
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
            last_drawn: 0,
            last_quads: 0,
            last_lod_drawn: 0,
            last_lod_quads: 0,
            palette: build_palette(reg),
            tint: None,
            tint_tex,
            start: Instant::now(),
            water_layer: reg.texture_names().iter().position(|n| n == "water").unwrap_or(usize::MAX) as u32,
        })
    }

    /// Starts the biome tint map once the world's generator is known.
    pub fn set_generator(&mut self, gen: Arc<WorldGen>) {
        if self.tint.is_none() {
            self.tint = Some(TintMap::new(gen));
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Whether the surface offers any mode that isn't locked to the display's refresh.
    pub fn vsync_supported(&self) -> bool {
        self.uncapped_mode.is_some()
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

    pub fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        self.config.width = w;
        self.config.height = h;
        self.surface.configure(&self.device, &self.config);
        self.depth_view = make_depth(&self.device, w, h);
    }

    /// One vertex buffer holding `parts` back to back, written straight into GPU memory
    /// (no temporary copy on the CPU side).
    fn upload<V>(&self, parts: &[&[V]], label: &str) -> Option<GpuLayer> {
        let verts: usize = parts.iter().map(|p| p.len()).sum();
        if verts == 0 {
            return None;
        }
        // 8-byte vertices, so the size is always a multiple of wgpu's 4-byte alignment.
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (verts * std::mem::size_of::<V>()) as u64,
            usage: wgpu::BufferUsages::VERTEX,
            mapped_at_creation: true,
        });
        {
            let mut view = buf.slice(..).get_mapped_range_mut();
            let mut at = 0;
            for p in parts {
                let bytes = as_bytes(p);
                view[at..at + bytes.len()].copy_from_slice(bytes);
                at += bytes.len();
            }
        }
        buf.unmap();
        Some(GpuLayer { buf, quads: (verts / 4) as u32 })
    }

    fn sync_meshes(&mut self, client: &mut Client) {
        for pos in client.take_mesh_changes(UPLOADS_PER_FRAME) {
            match client.meshes.get(&pos) {
                None => {
                    self.chunks.remove(&pos);
                }
                Some(mesh) => {
                    // Cutout (leaves) shares the opaque draw for now.
                    let solid = self.upload(&[&mesh.opaque.vertices, &mesh.cutout.vertices], "chunk solid");
                    let water = self.upload(&[&mesh.translucent.vertices], "chunk water");
                    self.chunks.insert(pos, GpuChunk { solid, water });
                }
            }
        }
        for key in client.lod.take_evicted() {
            self.lods.remove(&key);
        }
        for mesh in client.lod.take_built(LOD_UPLOADS_PER_FRAME) {
            let layer = self.upload(&[&mesh.vertices], "lod section");
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
            // Sections that can meet real chunks are masked per chunk column; skip those
            // whose every column is drawn by real terrain (nothing of them would show).
            let (c0x, c0z) = (ox.div_euclid(chunk), oz.div_euclid(chunk));
            let n = (w / chunk).max(1);
            let in_window = c0x + n > cov.origin.0
                && c0z + n > cov.origin.1
                && c0x < cov.origin.0 + LodCoverage::SIZE
                && c0z < cov.origin.1 + LodCoverage::SIZE;
            let masked = key.level <= MASKED_MAX_LEVEL && in_window;
            if masked && (0..n).all(|a| (0..n).all(|b| cov.covered(c0x + a, c0z + b))) {
                continue;
            }
            let d = key.distance_to(cam[0], cam[2]);
            out.push((d, [ox as f32, oz as f32, key.cell() as f32, if masked { 1.0 } else { 0.0 }], key));
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
            vis.push((d2, *pos));
        }
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

        let sky = [
            srgb_to_linear(SKY_SRGB[0]),
            srgb_to_linear(SKY_SRGB[1]),
            srgb_to_linear(SKY_SRGB[2]),
        ];
        // LOD sections, after this frame's uploads so coverage matches what is on the GPU.
        let lod_on = client.lod.settings().enabled();
        let cov = if lod_on { client.lod_coverage(view_blocks) } else { LodCoverage::empty((0, 0)) };
        let lod_vis = if lod_on { self.visible_lods(client, cam.pos, &frustum, &cov, opts.frustum_cull) } else { Vec::new() };
        let lod_instances: Vec<[f32; 4]> = lod_vis.iter().map(|(i, _)| *i).collect();
        if !lod_instances.is_empty() {
            self.queue.write_buffer(&self.lod_instance_buf, 0, as_bytes(&lod_instances));
        }
        let mut lod_mask = [[0u32; 4]; 32];
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
        let time = self.start.elapsed().as_secs_f32() % 3600.0;

        let mut globals = Globals {
            view_proj: vp,
            cam_pos: [cam.pos[0], cam.pos[1], cam.pos[2], 1.0],
            fog_color: [sky[0], sky[1], sky[2], 1.0],
            fog_params: [opts.fog_start, opts.fog_end.max(opts.fog_start + 1.0), 1.0, aspect],
            screen: [self.config.width as f32, self.config.height as f32, 0.0, 0.0],
            // x: fog on, y: seconds (animated water), z: the water texture's layer,
            // w: which tint map levels are ready (bits).
            flags: [if opts.fog { 1.0 } else { 0.0 }, time, self.water_layer as f32, tint_ready as f32],
            palette: self.palette,
            lod_window: [cov.origin.0 as f32, cov.origin.1 as f32, 0.0, 0.0],
            lod_mask,
            tint_window,
        };
        self.queue.write_buffer(&self.globals_solid, 0, as_bytes(std::slice::from_ref(&globals)));
        globals.fog_params[2] = 0.65;
        self.queue.write_buffer(&self.globals_water, 0, as_bytes(std::slice::from_ref(&globals)));

        let hud_verts = hud.len().min(MAX_HUD_VERTS);
        if hud_verts > 0 {
            self.queue.write_buffer(&self.hud_buf, 0, as_bytes(&hud[..hud_verts]));
        }

        let acquire_start = Instant::now();
        let frame = self.surface.get_current_texture()?;
        let acquire_wait = acquire_start.elapsed();
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        let mut quads = 0u64;
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: sky[0] as f64,
                            g: sky[1] as f64,
                            b: sky[2] as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(math::DEPTH_CLEAR),
                        store: wgpu::StoreOp::Discard,
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
                    pass.set_vertex_buffer(0, layer.buf.slice(..));
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
                        pass.set_vertex_buffer(0, layer.buf.slice(..));
                        pass.draw_indexed(0..layer.quads.min(MAX_QUADS) * 6, 0, i as u32..i as u32 + 1);
                        lod_quads += layer.quads as u64;
                    }
                }
                pass.set_vertex_buffer(1, self.instance_buf.slice(..));
            }
            self.last_lod_drawn = lod_vis.len();
            self.last_lod_quads = lod_quads;

            pass.set_pipeline(&self.pipe_water);
            pass.set_bind_group(0, &self.bind_water, &[]);
            for (i, (_, pos)) in vis.iter().enumerate() {
                if let Some(layer) = self.chunks.get(pos).and_then(|c| c.water.as_ref()) {
                    pass.set_vertex_buffer(0, layer.buf.slice(..));
                    pass.draw_indexed(0..layer.quads.min(MAX_QUADS) * 6, 0, i as u32..i as u32 + 1);
                    quads += layer.quads as u64;
                }
            }

            // HUD (crosshair, stats, menu)
            if hud_verts > 0 {
                pass.set_pipeline(&self.pipe_hud);
                pass.set_bind_group(0, &self.bind_solid, &[]);
                pass.set_vertex_buffer(0, self.hud_buf.slice(0..(hud_verts * std::mem::size_of::<HudVertex>()) as u64));
                pass.draw(0..hud_verts as u32, 0..1);
            }
        }
        self.last_quads = quads;
        if let Some(t) = self.timer.as_mut() {
            t.encode(&mut encoder);
        }
        self.queue.submit(std::iter::once(encoder.finish()));
        let present_start = Instant::now();
        frame.present();
        let present_wait = present_start.elapsed();
        if let Some(t) = self.timer.as_mut() {
            t.after_submit(&self.device);
        }
        Ok(acquire_wait + present_wait)
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
            bias: if depth_write {
                wgpu::DepthBiasState::default()
            } else {
                wgpu::DepthBiasState { constant: -4, slope_scale: -1.5, clamp: 0.0 }
            },
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

fn hud_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("hud"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: "vs_hud",
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<HudVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &HUD_ATTRS,
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: "fs_hud",
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        // The pass has a depth attachment, so every pipeline in it must declare depth state.
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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
        // 64 (mat4) + 5 * 16 (vec4s) + 32 * 16 (palette) + 16 (lod window) + 32 * 16 (lod mask)
        // + 16 (tint window)
        assert_eq!(std::mem::size_of::<Globals>(), 64 + 80 + 512 + 16 + 512 + 16);
        assert_eq!(std::mem::size_of::<HudVertex>(), 12);
    }

    #[test]
    fn every_builtin_texture_gets_a_colour_and_fits_the_palette() {
        let reg = BlockRegistry::builtin();
        assert!(reg.texture_names().len() <= 32);
        let p = build_palette(&reg);
        for (i, name) in reg.texture_names().iter().enumerate() {
            if name == "air" {
                continue; // never drawn
            }
            assert_ne!(p[i], [1.0, 0.0, 1.0, 1.0], "texture {name} fell through");
        }
    }
}
