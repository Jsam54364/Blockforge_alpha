//! GPU time via timestamp queries around the frame's passes: each shadow map, the world and
//! the upscale (see `Slot`), so the stats can show where the GPU's time goes.
//!
//! Not every device supports `TIMESTAMP_QUERY`; when it is missing the caller simply
//! has no timer and the overlay shows "GPU N/A". Results arrive a few frames late:
//! resolved timestamps are copied into a mappable buffer, mapped asynchronously, and read
//! back when ready. While a map is outstanding no new copy is issued, so at most one
//! readback is in flight. Passes not drawn in a frame (a shadow map not redrawn) count 0.

use std::cell::Cell;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

const PENDING: u8 = 0;
const OK: u8 = 1;
const FAILED: u8 = 2;

/// The passes timed.
pub const SHADOW_NEAR: usize = 0;
pub const SHADOW_FAR: usize = 1;
pub const WORLD: usize = 2;
pub const UPSCALE: usize = 3;
pub const SLOTS: usize = 4;

/// One frame's GPU times (ms) per pass (`SHADOW_NEAR` ..), 0 for passes not drawn.
pub type Parts = [f32; SLOTS];

pub struct GpuTimer {
    query_set: wgpu::QuerySet,
    resolve_buf: wgpu::Buffer,
    read_buf: wgpu::Buffer,
    period_ns: f32,
    state: Arc<AtomicU8>,
    mapping: bool,
    copied: bool,
    /// Passes given timestamps this frame (bits), and in the copy being read back.
    used: Cell<u8>,
    in_flight: u8,
    latest: Option<Parts>,
}

const BYTES: u64 = (SLOTS * 2 * 8) as u64;

impl GpuTimer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> GpuTimer {
        let query_set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("gpu timer"),
            ty: wgpu::QueryType::Timestamp,
            count: (SLOTS * 2) as u32,
        });
        let resolve_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu timer resolve"),
            size: BYTES,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let read_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu timer read"),
            size: BYTES,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        GpuTimer {
            query_set,
            resolve_buf,
            read_buf,
            period_ns: queue.get_timestamp_period(),
            state: Arc::new(AtomicU8::new(PENDING)),
            mapping: false,
            copied: false,
            used: Cell::new(0),
            in_flight: 0,
            latest: None,
        }
    }

    /// Plug into `RenderPassDescriptor::timestamp_writes` of the pass `slot` (`WORLD` ..).
    pub fn pass_writes(&self, slot: usize) -> wgpu::RenderPassTimestampWrites<'_> {
        self.used.set(self.used.get() | 1 << slot);
        wgpu::RenderPassTimestampWrites {
            query_set: &self.query_set,
            beginning_of_pass_write_index: Some(slot as u32 * 2),
            end_of_pass_write_index: Some(slot as u32 * 2 + 1),
        }
    }

    /// Call after the passes have ended, before `encoder.finish()`.
    pub fn encode(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let used = self.used.replace(0);
        self.copied = false;
        if used == 0 {
            return;
        }
        encoder.resolve_query_set(&self.query_set, 0..(SLOTS * 2) as u32, &self.resolve_buf, 0);
        if !self.mapping {
            encoder.copy_buffer_to_buffer(&self.resolve_buf, 0, &self.read_buf, 0, BYTES);
            self.copied = true;
            self.in_flight = used;
        }
    }

    /// Call right after `queue.submit`.
    pub fn after_submit(&mut self, device: &wgpu::Device) {
        if self.copied {
            let state = self.state.clone();
            self.read_buf.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                state.store(if r.is_ok() { OK } else { FAILED }, Ordering::SeqCst);
            });
            self.mapping = true;
            self.copied = false;
        }
        device.poll(wgpu::Maintain::Poll);

        if self.mapping {
            match self.state.load(Ordering::SeqCst) {
                OK => {
                    let mut parts = [0.0; SLOTS];
                    {
                        let view = self.read_buf.slice(..).get_mapped_range();
                        for (i, p) in parts.iter_mut().enumerate() {
                            if self.in_flight & (1 << i) == 0 {
                                continue;
                            }
                            let at = |k: usize| u64::from_le_bytes(view[k * 8..k * 8 + 8].try_into().unwrap());
                            let (a, b) = (at(i * 2), at(i * 2 + 1));
                            if b > a {
                                *p = ((b - a) as f64 * self.period_ns as f64 / 1_000_000.0) as f32;
                            }
                        }
                    }
                    self.read_buf.unmap();
                    self.state.store(PENDING, Ordering::SeqCst);
                    self.mapping = false;
                    self.latest = Some(parts);
                }
                FAILED => {
                    self.state.store(PENDING, Ordering::SeqCst);
                    self.mapping = false;
                }
                _ => {}
            }
        }
    }

    /// The newest unread per-pass GPU times, if new ones arrived.
    pub fn take(&mut self) -> Option<Parts> {
        self.latest.take()
    }
}
