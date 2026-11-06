//! wgpu renderer. Mobile-minded choices:
//!   * one static index buffer shared by every chunk (quads carry no indices)
//!   * chunk origin as a per-instance attribute: one draw call per chunk, no per-chunk uniforms
//!   * depth is discarded after the pass (cheap on tile-based GPUs)
//!   * opaque + cutout first, translucent water last, far-to-near
//!
//! TODO (see README): pooled vertex buffers, frustum planes, ASTC texture array, render scale.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use blockforge::client::Client;
use blockforge::coords::{ChunkPos, CHUNK_SIZE};
use blockforge::mesh::{quad_indices, PackedVertex};
use blockforge::registry::BlockRegistry;
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
}

/// Per-frame toggles chosen in the settings menu.
#[derive(Clone, Copy, Debug)]
pub struct RenderOptions {
    pub fog: bool,
}

fn as_bytes<T>(s: &[T]) -> &[u8] {
    // Only used with plain-old-data (f32 arrays, #[repr(C)] vertex/uniform structs).
    unsafe { std::slice::from_raw_parts(s.as_ptr() as *const u8, std::mem::size_of_val(s)) }
}

fn srgb_to_linear(c: f32) -> f32 {
    c.powf(2.2)
}

fn color_for(name: &str) -> [f32; 3] {
    match name {
        "stone" => [0.50, 0.50, 0.52],
        "dirt" => [0.45, 0.32, 0.20],
        "grass_top" => [0.35, 0.62, 0.25],
        "grass_side" => [0.40, 0.50, 0.24],
        "sand" => [0.85, 0.80, 0.55],
        "water" => [0.20, 0.38, 0.75],
        "log_top" => [0.60, 0.48, 0.30],
        "log_side" => [0.40, 0.30, 0.18],
        "leaves" => [0.20, 0.45, 0.18],
        "glowstone" => [1.00, 0.85, 0.45],
        "bedrock" => [0.18, 0.18, 0.20],
        _ => {
            // Unknown texture names still get a stable, distinct colour.
            let mut h: u32 = 2_166_136_261;
            for b in name.bytes() {
                h = (h ^ b as u32).wrapping_mul(16_777_619);
            }
            [
                (h & 255) as f32 / 255.0 * 0.6 + 0.2,
                ((h >> 8) & 255) as f32 / 255.0 * 0.6 + 0.2,
                ((h >> 16) & 255) as f32 / 255.0 * 0.6 + 0.2,
            ]
        }
    }
}

fn build_palette(reg: &BlockRegistry) -> [[f32; 4]; 32] {
    let mut p = [[1.0f32, 0.0, 1.0, 1.0]; 32]; // magenta = missing texture
    for (i, name) in reg.texture_names().iter().enumerate().take(32) {
        let c = color_for(name);
        p[i] = [srgb_to_linear(c[0]), srgb_to_linear(c[1]), srgb_to_linear(c[2]), 1.0];
    }
    p
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

pub struct Gfx {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    depth_view: wgpu::TextureView,
    pipe_solid: wgpu::RenderPipeline,
    pipe_water: wgpu::RenderPipeline,
    pipe_hud: wgpu::RenderPipeline,
    hud_buf: wgpu::Buffer,
    timer: Option<GpuTimer>,
    globals_solid: wgpu::Buffer,
    globals_water: wgpu::Buffer,
    bind_solid: wgpu::BindGroup,
    bind_water: wgpu::BindGroup,
    index_buf: wgpu::Buffer,
    instance_buf: wgpu::Buffer,
    chunks: HashMap<ChunkPos, GpuChunk>,
    palette: [[f32; 4]; 32],
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
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
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
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() }],
            })
        };
        let bind_solid = make_bind("bind solid", &globals_solid);
        let bind_water = make_bind("bind water", &globals_water);

        // Solid terrain is one-sided: cull back faces (the winding is covered by tests).
        let pipe_solid =
            voxel_pipeline(&device, &layout, &shader, format, None, true, Some(wgpu::Face::Back), "solid");
        let pipe_water = voxel_pipeline(
            &device,
            &layout,
            &shader,
            format,
            Some(wgpu::BlendState::ALPHA_BLENDING),
            false,
            None, // double-sided: from underwater the surface is a back face
            "water",
        );
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
            depth_view,
            pipe_solid,
            pipe_water,
            pipe_hud,
            hud_buf,
            timer,
            globals_solid,
            globals_water,
            bind_solid,
            bind_water,
            index_buf,
            instance_buf,
            chunks: HashMap::new(),
            palette: build_palette(reg),
        })
    }

    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
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

    fn upload(&self, v: &[PackedVertex], label: &str) -> Option<GpuLayer> {
        if v.is_empty() {
            return None;
        }
        let buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: as_bytes(v),
            usage: wgpu::BufferUsages::VERTEX,
        });
        Some(GpuLayer { buf, quads: (v.len() / 4) as u32 })
    }

    fn sync_meshes(&mut self, client: &mut Client) {
        for pos in client.take_mesh_changes(UPLOADS_PER_FRAME) {
            match client.meshes.get(&pos) {
                None => {
                    self.chunks.remove(&pos);
                }
                Some(mesh) => {
                    // Cutout (leaves) shares the opaque draw for now.
                    let mut solid = mesh.opaque.vertices.clone();
                    solid.extend_from_slice(&mesh.cutout.vertices);
                    let solid = self.upload(&solid, "chunk solid");
                    let water = self.upload(&mesh.translucent.vertices, "chunk water");
                    self.chunks.insert(pos, GpuChunk { solid, water });
                }
            }
        }
    }

    /// Draws a frame. On success returns how long we waited for the display to hand
    /// us a frame (so callers can exclude it from CPU time).
    pub fn render(
        &mut self,
        client: &mut Client,
        cam: &CameraView,
        view_blocks: f32,
        opts: &RenderOptions,
        hud: &[HudVertex],
    ) -> Result<Duration, wgpu::SurfaceError> {
        self.sync_meshes(client);

        let aspect = self.config.width as f32 / self.config.height as f32;
        let fwd = cam.forward();

        // Visible chunks: drop anything behind the camera or beyond the fog distance.
        let half = CHUNK_SIZE as f32 * 0.5;
        let mut vis: Vec<(f32, ChunkPos)> = Vec::new();
        for pos in self.chunks.keys() {
            let o = pos.origin();
            let c = [
                o.x as f32 + half - cam.pos[0],
                o.y as f32 + half - cam.pos[1],
                o.z as f32 + half - cam.pos[2],
            ];
            let d2 = math::dot(c, c);
            if math::dot(c, fwd) < -half * 1.8 || d2 > view_blocks * view_blocks {
                continue;
            }
            vis.push((d2, *pos));
        }
        vis.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        vis.truncate(MAX_INSTANCES);
        vis.reverse(); // far to near, for the translucent pass

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
        let mut globals = Globals {
            view_proj: math::view_proj(cam.pos, fwd, aspect),
            cam_pos: [cam.pos[0], cam.pos[1], cam.pos[2], 1.0],
            fog_color: [sky[0], sky[1], sky[2], 1.0],
            fog_params: [view_blocks * 0.55, view_blocks * 0.92, 1.0, aspect],
            screen: [self.config.width as f32, self.config.height as f32, 0.0, 0.0],
            flags: [if opts.fog { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0],
            palette: self.palette,
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
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: self.timer.as_ref().map(|t| t.pass_writes()),
                occlusion_query_set: None,
            });

            pass.set_index_buffer(self.index_buf.slice(..), wgpu::IndexFormat::Uint32);
            pass.set_vertex_buffer(1, self.instance_buf.slice(..));

            pass.set_pipeline(&self.pipe_solid);
            pass.set_bind_group(0, &self.bind_solid, &[]);
            for (i, (_, pos)) in vis.iter().enumerate() {
                if let Some(layer) = self.chunks.get(pos).and_then(|c| c.solid.as_ref()) {
                    pass.set_vertex_buffer(0, layer.buf.slice(..));
                    pass.draw_indexed(0..layer.quads.min(MAX_QUADS) * 6, 0, i as u32..i as u32 + 1);
                }
            }

            pass.set_pipeline(&self.pipe_water);
            pass.set_bind_group(0, &self.bind_water, &[]);
            for (i, (_, pos)) in vis.iter().enumerate() {
                if let Some(layer) = self.chunks.get(pos).and_then(|c| c.water.as_ref()) {
                    pass.set_vertex_buffer(0, layer.buf.slice(..));
                    pass.draw_indexed(0..layer.quads.min(MAX_QUADS) * 6, 0, i as u32..i as u32 + 1);
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
        if let Some(t) = self.timer.as_mut() {
            t.encode(&mut encoder);
        }
        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        if let Some(t) = self.timer.as_mut() {
            t.after_submit(&self.device);
        }
        Ok(acquire_wait)
    }
}

fn voxel_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
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
            entry_point: "vs_main",
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
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
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
        // 64 (mat4) + 5 * 16 (vec4s) + 32 * 16 (palette)
        assert_eq!(std::mem::size_of::<Globals>(), 64 + 80 + 512);
        assert_eq!(std::mem::size_of::<HudVertex>(), 12);
    }

    #[test]
    fn every_builtin_texture_gets_a_colour_and_fits_the_palette() {
        let reg = BlockRegistry::builtin();
        assert!(reg.texture_names().len() <= 32);
        let p = build_palette(&reg);
        for i in 0..reg.texture_names().len() {
            assert_ne!(p[i], [1.0, 0.0, 1.0, 1.0], "texture {} fell through", reg.texture_names()[i]);
        }
    }
}
