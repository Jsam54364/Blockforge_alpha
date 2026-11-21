//! Real-time shadows from the sun (or the moon at night), done as cheaply as looks right.
//!
//! Two shadow maps ("cascades") follow the camera, each a square seen straight down the
//! light's direction with an orthographic projection:
//!   * the near one, `NEAR_HALF` blocks either side, is sharp (about 18 texels a block) and
//!     redrawn every frame (every other frame on Lite): it carries the character's shadow
//!     and everything close enough to see detail in;
//!   * the far one, `FAR_HALF` either side (about the whole real-terrain draw distance), is
//!     coarse and only redrawn when it has to be: when the camera has moved a good way, the
//!     sun has moved, terrain has changed, or after a few seconds anyway. Most frames it
//!     costs nothing.
//! Corners cut, the way shader packs on phones do it:
//!   * only solid terrain casts (water, LOD terrain, clouds don't); LOD terrain is past the
//!     far map and keeps its plain lighting;
//!   * each map stores the matrix it was drawn with, so a map drawn a few frames ago is still
//!     right where it covers (it is in world space); only its coverage lags;
//!   * the squares snap to whole texels in light space, so shadow edges don't crawl as the
//!     camera moves;
//!   * the fragment shader skips the lookup for faces turned away from the light (they get no
//!     sunlight to take away) and in caves (no sky light);
//!   * soft edges come from the hardware's 2x2 comparison filter, plus 4 rotated taps on Fancy.

use crate::math::{self, Mat4};

pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Texels on a side of each map.
pub const SIZE: u32 = 2048;
/// Half the width of the near and far maps, in blocks.
pub const NEAR_HALF: f32 = 56.0;
pub const FAR_HALF: f32 = 224.0;
/// How far along the light each map reaches either side of its centre, in blocks (casters
/// up to this far above the camera, toward the sun, still cast).
const DEPTH_RANGE: f32 = 320.0;
/// Bindings in the main shader's group 0.
pub const BIND_NEAR: u32 = 8;
pub const BIND_FAR: u32 = 9;
pub const BIND_SAMPLER: u32 = 10;

/// The "Shaders" setting: how much of the shader-pack look is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Off,
    Lite,
    Fancy,
}

impl Quality {
    pub fn from_index(i: u8) -> Quality {
        match i {
            1 => Quality::Lite,
            2 => Quality::Fancy,
            _ => Quality::Off,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Quality::Off => "Off",
            Quality::Lite => "Lite",
            Quality::Fancy => "Fancy",
        }
    }
}

/// The light that casts the shadows: the sun while it is up, else the moon (towards it, unit;
/// its colour), and how strong the shadows are (they fade out as it nears the horizon, where
/// they would stretch to the edge of the map and its light is gone anyway).
pub fn light(p: &crate::sky::SkyParams) -> ([f32; 3], [f32; 3], f32) {
    if p.sun[1] > -0.02 {
        (p.sun, p.sun_light, smoothstep(0.0, 0.08, p.sun[1]))
    } else {
        (p.moon, p.moon_light, smoothstep(0.0, 0.08, p.moon[1]) * 0.8)
    }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Foliage (leaves, ferns): one bit per texture layer. They sway in the wind and let light
/// through. `remap` is the GPU's texture-to-layer map when it has fewer layers than textures.
pub fn foliage_mask(names: &[String], remap: Option<&[u16]>) -> [[u32; 4]; 32] {
    let mut mask = [[0u32; 4]; 32];
    for (i, n) in names.iter().enumerate() {
        let leafy = n == "leaves" || n.ends_with("_leaves") || n == "fern";
        if !leafy {
            continue;
        }
        let layer = remap.and_then(|m| m.get(i).copied()).map_or(i, |l| l as usize);
        if layer < 4096 {
            mask[layer / 128][(layer / 32) % 4] |= 1 << (layer % 32);
        }
    }
    mask
}

/// The light-space frame: right and up across the map, `l` towards the light. It turns with
/// the sun around the sky's pole, smoothly, so the map never flips.
fn basis(l: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let pole = crate::sky::pole();
    let mut r = math::cross(pole, l);
    if math::dot(r, r) < 1e-6 {
        r = math::cross([1.0, 0.0, 0.0], l);
    }
    let r = math::normalize(r);
    let u = math::cross(l, r);
    (r, u)
}

/// Orthographic projection for a map: x, y across it (-1..1 over `half` blocks either side
/// of the centre), depth 0 nearest the light .. 1 farthest. The centre is snapped to whole
/// texels across the map. Column-major, like `math`.
fn projection(center: [f32; 3], l: [f32; 3], half: f32) -> Mat4 {
    let (r, u) = basis(l);
    let texel = 2.0 * half / SIZE as f32;
    let cr = (math::dot(r, center) / texel).round() * texel;
    let cu = (math::dot(u, center) / texel).round() * texel;
    let cl = math::dot(l, center);
    let (k, z) = (1.0 / half, 0.5 / DEPTH_RANGE);
    [
        r[0] * k, u[0] * k, -l[0] * z, 0.0,
        r[1] * k, u[1] * k, -l[1] * z, 0.0,
        r[2] * k, u[2] * k, -l[2] * z, 0.0,
        -cr * k, -cu * k, 0.5 + cl * z, 1.0,
    ]
}

struct Cascade {
    half: f32,
    view: wgpu::TextureView,
    uniform: wgpu::Buffer,
    bind: wgpu::BindGroup,
    instances: wgpu::Buffer,
    /// What it was last drawn with (None: never, or since being turned off).
    matrix: Option<Mat4>,
    center: [f32; 3],
    light: [f32; 3],
    drawn_frame: u64,
}

impl Cascade {
    /// Whether a box (a chunk) is inside the map's volume, for a map centred on `center`.
    fn holds(half: f32, center: [f32; 3], l: [f32; 3], min: [f32; 3], size: f32) -> bool {
        let (r, u) = basis(l);
        let h = size * 0.5;
        let c = [min[0] + h - center[0], min[1] + h - center[1], min[2] + h - center[2]];
        let reach = h * 1.74; // the box's half-diagonal
        math::dot(c, r).abs() <= half + reach
            && math::dot(c, u).abs() <= half + reach
            && math::dot(c, l).abs() <= DEPTH_RANGE - reach
    }
}

/// What to draw into one map this frame.
pub struct Job {
    pub cascade: usize,
    center: [f32; 3],
    light: [f32; 3],
    matrix: Mat4,
}

impl Job {
    /// Whether a chunk (corner `min`, `size` blocks on a side) casts into this map.
    pub fn holds(&self, s: &Shadows, min: [f32; 3], size: f32) -> bool {
        Cascade::holds(s.cascades[self.cascade].half, self.center, self.light, min, size)
    }
}

pub struct Shadows {
    cascades: [Cascade; 2],
    pipe_chunk: wgpu::RenderPipeline,
    pipe_char: wgpu::RenderPipeline,
    pub sampler: wgpu::Sampler,
    frame: u64,
    /// Terrain changed since the far map was drawn.
    far_stale: bool,
    pub max_instances: usize,
}

impl Shadows {
    pub fn new(device: &wgpu::Device, max_instances: usize, char_stride: u64) -> Shadows {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shadow shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shadow.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow view"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shadow"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        const CHUNK_ATTRS: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Uint32x2];
        const INSTANCE_ATTRS: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![1 => Float32x4];
        const CHAR_ATTRS: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];
        let pipeline = |label: &str, entry: &str, buffers: &[wgpu::VertexBufferLayout]| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: entry,
                    compilation_options: Default::default(),
                    buffers,
                },
                fragment: None,
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    // Both sides: a wall's back face casts as well as its front, and thin
                    // things (slabs) don't leak light.
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: FORMAT,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: Default::default(),
                    // Pushed away from the light a little: no "acne" of surfaces shadowing
                    // themselves (the shader also looks up a little off the surface).
                    bias: wgpu::DepthBiasState { constant: 2, slope_scale: 2.0, clamp: 0.0 },
                }),
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        };
        let pipe_chunk = pipeline(
            "shadow terrain",
            "vs_chunk",
            &[
                wgpu::VertexBufferLayout { array_stride: 8, step_mode: wgpu::VertexStepMode::Vertex, attributes: &CHUNK_ATTRS },
                wgpu::VertexBufferLayout { array_stride: 16, step_mode: wgpu::VertexStepMode::Instance, attributes: &INSTANCE_ATTRS },
            ],
        );
        let pipe_char = pipeline(
            "shadow character",
            "vs_char",
            &[wgpu::VertexBufferLayout { array_stride: char_stride, step_mode: wgpu::VertexStepMode::Vertex, attributes: &CHAR_ATTRS }],
        );
        let cascade = |half: f32, label: &str| {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: 64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }],
            });
            let instances = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: (max_instances * 16) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            Cascade {
                half,
                view: tex.create_view(&wgpu::TextureViewDescriptor::default()),
                uniform,
                bind,
                instances,
                matrix: None,
                center: [0.0; 3],
                light: [0.0, 1.0, 0.0],
                drawn_frame: 0,
            }
        };
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow compare"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        Shadows {
            cascades: [cascade(NEAR_HALF, "shadow map near"), cascade(FAR_HALF, "shadow map far")],
            pipe_chunk,
            pipe_char,
            sampler,
            frame: 0,
            far_stale: true,
            max_instances,
        }
    }

    /// The main shader's entries for the maps and their sampler (group 0).
    pub fn layout_entries() -> [wgpu::BindGroupLayoutEntry; 3] {
        let map = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        [
            map(BIND_NEAR),
            map(BIND_FAR),
            wgpu::BindGroupLayoutEntry {
                binding: BIND_SAMPLER,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                count: None,
            },
        ]
    }

    pub fn bind_entries(&self) -> [wgpu::BindGroupEntry<'_>; 3] {
        [
            wgpu::BindGroupEntry { binding: BIND_NEAR, resource: wgpu::BindingResource::TextureView(&self.cascades[0].view) },
            wgpu::BindGroupEntry { binding: BIND_FAR, resource: wgpu::BindingResource::TextureView(&self.cascades[1].view) },
            wgpu::BindGroupEntry { binding: BIND_SAMPLER, resource: wgpu::BindingResource::Sampler(&self.sampler) },
        ]
    }

    /// Terrain changed (chunks uploaded or rebuilt): the far map needs redrawing soon.
    pub fn terrain_changed(&mut self) {
        self.far_stale = true;
    }

    /// Decides which maps to redraw this frame (none with shaders off) and where. `cam` is the
    /// camera, `fwd` where it looks: the maps sit a little ahead of it, where most of what is
    /// seen is.
    pub fn plan(&mut self, quality: Quality, cam: [f32; 3], fwd: [f32; 3], l: [f32; 3]) -> Vec<Job> {
        self.frame += 1;
        if quality == Quality::Off {
            for c in &mut self.cascades {
                c.matrix = None;
            }
            self.far_stale = true;
            return Vec::new();
        }
        let flat = math::normalize([fwd[0], 0.0, fwd[2]]);
        let mut jobs = Vec::new();
        let turned = |c: &Cascade, deg: f32| math::dot(c.light, l) < (deg.to_radians()).cos();
        let frame = self.frame;
        let mut far_drawn = false;
        for (i, c) in self.cascades.iter_mut().enumerate() {
            let ahead = c.half * 0.3;
            let center = [cam[0] + flat[0] * ahead, cam[1], cam[2] + flat[2] * ahead];
            let age = self.frame - c.drawn_frame;
            let moved = {
                let d = [center[0] - c.center[0], center[1] - c.center[1], center[2] - c.center[2]];
                math::dot(d, d).sqrt()
            };
            let redraw = c.matrix.is_none()
                // the light jumped (sun to moon): both now, or they would disagree
                || turned(c, 3.0)
                || if i == 0 {
                    quality == Quality::Fancy || age >= 2
                } else {
                    moved > c.half * 0.2
                        || turned(c, 0.3)
                        || (self.far_stale && age >= 20)
                        || age >= 240
                };
            if redraw {
                // Recorded now (it is drawn this frame), so this frame's uniforms carry it.
                let matrix = projection(center, l, c.half);
                c.matrix = Some(matrix);
                c.center = center;
                c.light = l;
                c.drawn_frame = frame;
                far_drawn |= i == 1;
                jobs.push(Job { cascade: i, center, light: l, matrix });
            }
        }
        if far_drawn {
            self.far_stale = false;
        }
        jobs
    }

    /// Draws one map: `chunks` are the instance origins and vertex ranges (quads) of the solid
    /// terrain inside it (see `Job::holds`), `character` the character's triangles if any.
    pub fn draw(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        index_buf: &wgpu::Buffer,
        job: &Job,
        chunks: &[([f32; 4], wgpu::BufferSlice<'_>, u32)],
        character: Option<(wgpu::BufferSlice<'_>, u32)>,
    ) {
        let n = chunks.len().min(self.max_instances);
        let c = &self.cascades[job.cascade];
        queue.write_buffer(&c.uniform, 0, as_bytes(&job.matrix));
        if n > 0 {
            let origins: Vec<[f32; 4]> = chunks[..n].iter().map(|c| c.0).collect();
            queue.write_buffer(&c.instances, 0, as_bytes(&origins));
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow map"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &c.view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_bind_group(0, &c.bind, &[]);
            if n > 0 {
                pass.set_pipeline(&self.pipe_chunk);
                pass.set_index_buffer(index_buf.slice(..), wgpu::IndexFormat::Uint32);
                pass.set_vertex_buffer(1, c.instances.slice(..));
                for (i, (_, slice, quads)) in chunks[..n].iter().enumerate() {
                    pass.set_vertex_buffer(0, *slice);
                    pass.draw_indexed(0..quads * 6, 0, i as u32..i as u32 + 1);
                }
            }
            if let Some((slice, verts)) = character {
                pass.set_pipeline(&self.pipe_char);
                pass.set_vertex_buffer(0, slice);
                pass.draw(0..verts, 0..1);
            }
        }
    }

    /// For the main shader: each map's matrix, and (half widths, strength, map size). With no
    /// map drawn yet (or shaders off) the strength is 0 and nothing is looked up.
    pub fn uniforms(&self, strength: f32) -> ([Mat4; 2], [f32; 4]) {
        let ready = self.cascades.iter().all(|c| c.matrix.is_some());
        let mats = [self.cascades[0].matrix.unwrap_or([0.0; 16]), self.cascades[1].matrix.unwrap_or([0.0; 16])];
        (mats, [NEAR_HALF, FAR_HALF, if ready { strength } else { 0.0 }, SIZE as f32])
    }
}

fn as_bytes<T>(s: &[T]) -> &[u8] {
    // SAFETY: plain-old-data (f32 arrays) viewed as bytes for upload.
    unsafe { std::slice::from_raw_parts(s.as_ptr() as *const u8, std::mem::size_of_val(s)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_map_puts_its_centre_in_the_middle_and_the_light_side_nearest() {
        let l = math::normalize([0.3, 0.8, -0.4]);
        let c = [1000.0, 70.0, -2000.0];
        let m = projection(c, l, NEAR_HALF);
        let p = math::transform(&m, [c[0], c[1], c[2], 1.0]);
        let texel = 2.0 * NEAR_HALF / SIZE as f32;
        assert!(p[0].abs() < texel / NEAR_HALF && p[1].abs() < texel / NEAR_HALF, "{p:?}");
        assert!((p[2] - 0.5).abs() < 1e-3);
        // Something above (towards the light) is nearer: smaller depth.
        let up = math::transform(&m, [c[0] + l[0] * 10.0, c[1] + l[1] * 10.0, c[2] + l[2] * 10.0, 1.0]);
        assert!(up[2] < p[2]);
        // The map's edge is NEAR_HALF blocks out along its right axis.
        let (r, _) = basis(l);
        let e = math::transform(&m, [c[0] + r[0] * NEAR_HALF, c[1] + r[1] * NEAR_HALF, c[2] + r[2] * NEAR_HALF, 1.0]);
        assert!((e[0] - 1.0).abs() < 0.01, "{e:?}");
    }

    #[test]
    fn foliage_is_leaves_and_ferns() {
        let names: Vec<String> = ["stone", "oak_leaves", "leaves", "leaf_litter", "fern", "meadow"].iter().map(|s| s.to_string()).collect();
        let m = foliage_mask(&names, None);
        assert_eq!(m[0][0], 0b10110);
    }
}
