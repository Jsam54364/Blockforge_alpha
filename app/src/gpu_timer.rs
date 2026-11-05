//! GPU frame time via timestamp queries around the main render pass.
//!
//! Not every device supports `TIMESTAMP_QUERY`; when it is missing the caller simply
//! has no timer and the overlay shows "GPU N/A". Results arrive a few frames late:
//! a resolved pair of timestamps is copied into a mappable buffer, mapped
//! asynchronously, and read back when ready. While a map is outstanding no new copy
//! is issued, so at most one readback is in flight.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

const PENDING: u8 = 0;
const OK: u8 = 1;
const FAILED: u8 = 2;

pub struct GpuTimer {
    query_set: wgpu::QuerySet,
    resolve_buf: wgpu::Buffer,
    read_buf: wgpu::Buffer,
    period_ns: f32,
    state: Arc<AtomicU8>,
    mapping: bool,
    copied: bool,
    latest_ms: Option<f32>,
}

impl GpuTimer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> GpuTimer {
        let query_set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("gpu timer"),
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        });
        let resolve_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu timer resolve"),
            size: 16,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let read_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu timer read"),
            size: 16,
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
            latest_ms: None,
        }
    }

    /// Plug into `RenderPassDescriptor::timestamp_writes`.
    pub fn pass_writes(&self) -> wgpu::RenderPassTimestampWrites<'_> {
        wgpu::RenderPassTimestampWrites {
            query_set: &self.query_set,
            beginning_of_pass_write_index: Some(0),
            end_of_pass_write_index: Some(1),
        }
    }

    /// Call after the pass has ended, before `encoder.finish()`.
    pub fn encode(&mut self, encoder: &mut wgpu::CommandEncoder) {
        encoder.resolve_query_set(&self.query_set, 0..2, &self.resolve_buf, 0);
        self.copied = false;
        if !self.mapping {
            encoder.copy_buffer_to_buffer(&self.resolve_buf, 0, &self.read_buf, 0, 16);
            self.copied = true;
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
                    let (a, b) = {
                        let view = self.read_buf.slice(..).get_mapped_range();
                        let a = u64::from_le_bytes(view[0..8].try_into().unwrap());
                        let b = u64::from_le_bytes(view[8..16].try_into().unwrap());
                        (a, b)
                    };
                    self.read_buf.unmap();
                    self.state.store(PENDING, Ordering::SeqCst);
                    self.mapping = false;
                    if b > a {
                        self.latest_ms = Some(((b - a) as f64 * self.period_ns as f64 / 1_000_000.0) as f32);
                    }
                }
                FAILED => {
                    self.state.store(PENDING, Ordering::SeqCst);
                    self.mapping = false;
                }
                _ => {}
            }
        }
    }

    /// The newest unread GPU time in milliseconds, if a new one arrived.
    pub fn take(&mut self) -> Option<f32> {
        self.latest_ms.take()
    }
}
