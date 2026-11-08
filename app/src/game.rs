//! Player-facing state: the integrated server + client, a fly camera and touch controls.
//!
//! Controls (landscape):
//!   left 40% of the screen   drag = virtual stick (fly in the direction you are looking)
//!   right side               drag = look around
//!                            quick tap = break the block under the crosshair
//!                            press-and-hold (no drag) = place stone on the face you look at
//!   top-right button         settings menu (render distance, LOD distance, fog, stats)

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use blockforge::block::BlockState;
use blockforge::client::Client;
use blockforge::coords::{BlockPos, CHUNK_SIZE};
use blockforge::mesh::Mesher;
use blockforge::raycast::{raycast, RayHit};
use blockforge::registry::BlockRegistry;
use blockforge::server::{self, ServerConfig, ServerHandle};
use winit::event::TouchPhase;

use crate::ui::Ui;

const LOOK_SENS: f32 = 0.0045;
const FLY_SPEED: f32 = 14.0;
const REACH: f32 = 8.0;
const TAP_MAX: Duration = Duration::from_millis(300);
const DRAG_PIXELS: f32 = 14.0;

#[derive(Clone, Copy, Debug)]
pub struct CameraView {
    pub pos: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
}

impl CameraView {
    /// yaw 0 looks toward -Z; positive yaw turns left.
    pub fn forward(&self) -> [f32; 3] {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        [-sy * cp, sp, -cy * cp]
    }

    pub fn right(&self) -> [f32; 3] {
        let (sy, cy) = self.yaw.sin_cos();
        [cy, 0.0, -sy]
    }
}

struct TouchInfo {
    start: (f32, f32),
    last: (f32, f32),
    t0: Instant,
    stick: bool,
    moved: bool,
}

pub struct Game {
    // Field order matters: the client is dropped before the server it talks to.
    pub client: Client,
    _server: ServerHandle,
    registry: Arc<BlockRegistry>,
    pub cam: CameraView,
    pub size: (u32, u32),
    pub ui: Ui,
    /// Touches that started on a HUD button; they never reach the game controls.
    ui_touches: HashSet<u64>,
    touches: HashMap<u64, TouchInfo>,
    stone: BlockState,
    last_update: Instant,
    last_pos_sent: Instant,
}

impl Game {
    pub fn new(save_dir: Option<PathBuf>) -> Game {
        let registry = Arc::new(BlockRegistry::builtin());
        let stone = registry.state("stone").expect("stone block");

        let cfg = ServerConfig {
            seed: 1337,
            worker_threads: 2,
            save_path: save_dir.map(|d| d.join("world.bfwj")),
            ..ServerConfig::default()
        };
        let server = server::spawn(cfg, registry.clone());
        let mut client = Client::connect(server.connect(), 2);
        let cam = CameraView { pos: [0.5, 70.0, 0.5], yaw: 0.0, pitch: -0.35 };
        let ui = Ui::new();
        client.set_view_distance(ui.settings.render_chunks);
        client.set_position(cam.pos);

        let now = Instant::now();
        Game {
            client,
            _server: server,
            registry,
            cam,
            size: (1, 1),
            ui,
            ui_touches: HashSet::new(),
            touches: HashMap::new(),
            stone,
            last_update: now,
            last_pos_sent: now,
        }
    }

    pub fn registry(&self) -> &Arc<BlockRegistry> {
        &self.registry
    }

    /// Far edge of the streamed area, in blocks (used for fog).
    pub fn view_blocks(&self) -> f32 {
        (self.ui.settings.render_chunks as f32 + 0.5) * CHUNK_SIZE as f32
    }

    /// Call when the window is (re)created or resized.
    pub fn set_surface(&mut self, w: u32, h: u32, scale_factor: f64) {
        self.size = (w.max(1), h.max(1));
        self.ui.set_surface(w, h, scale_factor);
    }

    /// (strafe, forward), each in -1..1, from the virtual stick.
    fn stick(&self) -> (f32, f32) {
        let radius = 0.12 * (self.size.0.min(self.size.1) as f32).max(1.0);
        for t in self.touches.values() {
            if t.stick {
                let mut x = (t.last.0 - t.start.0) / radius;
                let mut y = (t.last.1 - t.start.1) / radius;
                let len = (x * x + y * y).sqrt();
                if len > 1.0 {
                    x /= len;
                    y /= len;
                }
                return (x, -y);
            }
        }
        (0.0, 0.0)
    }

    /// Once per frame, before rendering.
    pub fn update(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_update).as_secs_f32().min(0.1);
        self.last_update = now;

        let (strafe, forward) = self.stick();
        let f = self.cam.forward();
        let r = self.cam.right();
        for i in 0..3 {
            self.cam.pos[i] += (f[i] * forward + r[i] * strafe) * FLY_SPEED * dt;
        }

        if now.duration_since(self.last_pos_sent) > Duration::from_millis(250) {
            self.client.set_position(self.cam.pos);
            self.last_pos_sent = now;
        } else {
            self.client.camera = self.cam.pos;
        }
        self.client.update();
    }

    pub fn touch(&mut self, id: u64, phase: TouchPhase, x: f32, y: f32) {
        // HUD first: a touch that starts on a button belongs to the HUD until it ends.
        match phase {
            TouchPhase::Started => {
                if let Some(action) = self.ui.hit(x, y) {
                    self.ui_touches.insert(id);
                    let greedy_before = self.ui.settings.greedy;
                    if self.ui.apply(action) {
                        self.client.set_view_distance(self.ui.settings.render_chunks);
                    }
                    if self.ui.settings.greedy != greedy_before {
                        let kind = if self.ui.settings.greedy { Mesher::Greedy } else { Mesher::Simple };
                        self.client.set_mesher(kind);
                    }
                    return;
                }
            }
            _ => {
                if self.ui_touches.contains(&id) {
                    if matches!(phase, TouchPhase::Ended | TouchPhase::Cancelled) {
                        self.ui_touches.remove(&id);
                    }
                    return;
                }
            }
        }

        let width = self.size.0 as f32;
        match phase {
            TouchPhase::Started => {
                self.touches.insert(
                    id,
                    TouchInfo {
                        start: (x, y),
                        last: (x, y),
                        t0: Instant::now(),
                        stick: x < width * 0.4,
                        moved: false,
                    },
                );
            }
            TouchPhase::Moved => {
                if let Some(t) = self.touches.get_mut(&id) {
                    let dx = x - t.last.0;
                    let dy = y - t.last.1;
                    t.last = (x, y);
                    if (x - t.start.0).hypot(y - t.start.1) > DRAG_PIXELS {
                        t.moved = true;
                    }
                    if !t.stick {
                        self.cam.yaw -= dx * LOOK_SENS;
                        self.cam.pitch = (self.cam.pitch - dy * LOOK_SENS).clamp(-1.5, 1.5);
                    }
                }
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                if let Some(t) = self.touches.remove(&id) {
                    if !t.stick && !t.moved && phase == TouchPhase::Ended {
                        if t.t0.elapsed() < TAP_MAX {
                            self.break_target();
                        } else {
                            self.place_target();
                        }
                    }
                }
            }
        }
    }

    fn target(&self) -> Option<RayHit> {
        raycast(self.cam.pos, self.cam.forward(), REACH, |p| self.client.is_solid(p))
    }

    fn break_target(&mut self) {
        if let Some(hit) = self.target() {
            self.client.break_block(hit.pos);
        }
    }

    fn place_target(&mut self) {
        if let Some(hit) = self.target() {
            let cell = hit.pos.neighbor(hit.face);
            // Don't entomb the camera.
            if cell != BlockPos::from_f32(self.cam.pos) {
                self.client.place_block(cell, self.stone);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_and_right_are_orthogonal_unit_vectors() {
        for &(yaw, pitch) in &[(0.0f32, 0.0f32), (1.0, 0.3), (-2.5, -1.0), (3.0, 1.4)] {
            let c = CameraView { pos: [0.0; 3], yaw, pitch };
            let (f, r) = (c.forward(), c.right());
            let dot = f[0] * r[0] + f[1] * r[1] + f[2] * r[2];
            let len = (f[0] * f[0] + f[1] * f[1] + f[2] * f[2]).sqrt();
            assert!(dot.abs() < 1e-5);
            assert!((len - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn yaw_zero_looks_down_negative_z() {
        let c = CameraView { pos: [0.0; 3], yaw: 0.0, pitch: 0.0 };
        let f = c.forward();
        assert!(f[2] < -0.99 && f[0].abs() < 1e-5);
    }
}
