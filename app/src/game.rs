//! Player-facing state: the integrated server + client, the character (or a fly camera) and
//! touch controls.
//!
//! Controls (landscape):
//!   left 40% of the screen   drag = virtual stick: walk, jog or sprint by how far it is
//!                            pushed (or fly in the direction you are looking, in FLY mode)
//!   right side               drag = look around
//!   bottom right             JUMP and ROLL (hold to repeat), CROUCH, CRAWL; ACT opens the
//!                            rest of what the character can do
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
use blockforge::lod::LodSettings;
use blockforge::server::{self, ServerConfig, ServerHandle};
use blockforge::worldgen::{GroundInfo, WorldGen, WORLDGEN_VERSION};
use winit::event::TouchPhase;

use crate::hyrig::View;
use crate::player::Player;
use crate::sky::Clock;
use crate::ui::{Action, Ui, LOD_DISTANCES, LOD_QUALITY};

const LOOK_SENS: f32 = 0.0045;
const FLY_SPEED: f32 = 14.0;
const REACH: f32 = 8.0;
/// Walking, the camera's pitch stays inside the character controller's limits.
const WALK_PITCH: (f32, f32) = (-1.4, 1.35);
const TAP_MAX: Duration = Duration::from_millis(300);
const DRAG_PIXELS: f32 = 14.0;
/// Settings and the camera, kept between launches next to the world save.
const STATE_FILE: &str = "state.txt";
/// The world's seed (one world for now).
const SEED: u64 = 1337;
/// A saved camera further out than this is treated as corrupt (f32 loses block precision).
const MAX_SAVED_COORD: f32 = 1.0e6;

#[derive(Clone, Copy, Debug, PartialEq)]
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

    fn to_text(&self) -> String {
        format!("cam={},{},{},{},{}\n", self.pos[0], self.pos[1], self.pos[2], self.yaw, self.pitch)
    }

    /// Reads the `cam=` line written by `to_text`; `None` if it is missing or not sane.
    fn from_text(text: &str) -> Option<CameraView> {
        let line = text.lines().find_map(|l| l.trim().strip_prefix("cam="))?;
        let v: Vec<f32> = line.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        if v.len() != 5 || v.iter().any(|x| !x.is_finite()) {
            return None;
        }
        if v[..3].iter().any(|x| x.abs() > MAX_SAVED_COORD) {
            return None;
        }
        Some(CameraView { pos: [v[0], v[1], v[2]], yaw: v[3], pitch: v[4].clamp(-1.5, 1.5) })
    }
}

/// Biome, climate and landform under the camera, for the stats overlay.
pub fn place_lines(g: &GroundInfo) -> Vec<String> {
    vec![
        format!("{}  {:.0}C  RAIN {:.0}%", g.biome.to_uppercase(), g.climate.temperature, g.climate.rainfall * 100.0),
        format!("{}  GROUND Y {:.0}", g.landform.to_uppercase(), g.height),
    ]
}

/// The world save for the current generator: `world.bfwj` for the first one (so older
/// saves keep working), `world_v2.bfwj` and so on after.
fn world_file() -> String {
    if WORLDGEN_VERSION <= 1 {
        "world.bfwj".to_string()
    } else {
        format!("world_v{WORLDGEN_VERSION}.bfwj")
    }
}

/// The generator version a state file was written with (1 before versions were saved).
fn saved_generator(text: &str) -> u32 {
    text.lines().find_map(|l| l.trim().strip_prefix("gen=")?.parse().ok()).unwrap_or(1)
}

/// The character's saved position (`player=x,y,z`), if it is there and sane.
fn saved_player(text: &str) -> Option<[f64; 3]> {
    let line = text.lines().find_map(|l| l.trim().strip_prefix("player="))?;
    let v: Vec<f64> = line.split(',').filter_map(|p| p.trim().parse().ok()).collect();
    (v.len() == 3 && v.iter().all(|x| x.is_finite() && x.abs() < MAX_SAVED_COORD as f64)).then(|| [v[0], v[1], v[2]])
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
    server: ServerHandle,
    /// Where `state.txt` lives (None on platforms without a data directory).
    state_path: Option<PathBuf>,
    registry: Arc<BlockRegistry>,
    /// The world's generator, for the stats overlay's biome lines.
    gen: Arc<WorldGen>,
    pub cam: CameraView,
    pub size: (u32, u32),
    pub ui: Ui,
    /// The time of day (the sky's sun follows it).
    pub clock: Clock,
    /// Touches that started on a HUD button; they never reach the game controls.
    ui_touches: HashSet<u64>,
    touches: HashMap<u64, TouchInfo>,
    stone: BlockState,
    /// The character (he stands where he was left while the camera flies).
    pub player: Player,
    /// Whether the last frame was in walk mode (to notice the switch).
    walking: bool,
    /// Touches holding JUMP or ROLL down.
    held: HashMap<u64, &'static str>,
    last_update: Instant,
    last_pos_sent: Instant,
}

impl Game {
    pub fn new(save_dir: Option<PathBuf>) -> Game {
        let registry = Arc::new(BlockRegistry::builtin());
        let stone = registry.state("stone").expect("stone block");

        // Pick up where the last session stopped: settings and camera from the state file.
        let state_path = save_dir.as_ref().map(|d| d.join(STATE_FILE));
        let saved = state_path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        let mut ui = Ui::new();
        ui.settings.apply_text(&saved);
        let clock = Clock::from_text(&saved);
        ui.hours = clock.hours();
        // A camera saved in a world from an older generator would be over different ground:
        // start over at the spawn (that world's save is kept, under its own name).
        let same_world = saved_generator(&saved) == WORLDGEN_VERSION;
        let gen = Arc::new(WorldGen::new(SEED, &registry));
        let cam = CameraView::from_text(&saved).filter(|_| same_world).unwrap_or_else(|| {
            let [x, y, z] = gen.find_spawn();
            CameraView { pos: [x, y + 24.0, z], yaw: 0.0, pitch: -0.35 }
        });

        // The character starts where he was left, or at the spawn (a little above the ground:
        // he drops onto it once it has loaded).
        let view = if ui.settings.third_person { View::Follow } else { View::First };
        let player_at = saved_player(&saved).filter(|_| same_world).unwrap_or_else(|| {
            let [x, y, z] = gen.find_spawn();
            [x as f64 + 0.5, y as f64 + 1.5, z as f64 + 0.5]
        });
        let player = Player::new(player_at, view);

        let cfg = ServerConfig {
            seed: SEED,
            worker_threads: 2,
            save_path: save_dir.map(|d| d.join(world_file())),
            ..ServerConfig::default()
        };
        let server = server::spawn(cfg, registry.clone());
        let mut client = Client::connect(server.connect(), 2);
        if ui.settings.greedy {
            client.set_mesher(Mesher::Greedy);
        }
        client.set_view_distance(ui.settings.render_chunks);
        client.set_position(cam.pos);

        let now = Instant::now();
        let ui_walk = ui.settings.walk;
        Game {
            client,
            server,
            state_path,
            registry,
            gen,
            cam,
            size: (1, 1),
            ui,
            clock,
            ui_touches: HashSet::new(),
            touches: HashMap::new(),
            stone,
            player,
            walking: ui_walk,
            held: HashMap::new(),
            last_update: now,
            last_pos_sent: now,
        }
    }

    pub fn registry(&self) -> &Arc<BlockRegistry> {
        &self.registry
    }

    /// Writes the world (if it changed), the settings and the camera to disk. Called when the
    /// app goes to the background, because Android may kill it there without warning.
    pub fn persist(&self) {
        self.server.save_now(Duration::from_millis(500));
        self.save_state();
    }

    fn save_state(&self) {
        let Some(path) = &self.state_path else { return };
        let p = self.player.position();
        let text = format!(
            "{}{}{}player={},{},{}\ngen={WORLDGEN_VERSION}\n",
            self.ui.settings.to_text(),
            self.cam.to_text(),
            self.clock.to_text(),
            p[0],
            p[1],
            p[2]
        );
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }

    /// Far edge of the streamed area, in blocks: real chunks are drawn up to here.
    /// The world's generator (the renderer's biome colours come from it).
    pub fn generator(&self) -> Arc<WorldGen> {
        self.gen.clone()
    }

    /// The stats overlay's lines about the ground under the camera.
    pub fn place(&self) -> Vec<String> {
        place_lines(&self.gen.describe(self.cam.pos[0].floor() as i32, self.cam.pos[2].floor() as i32))
    }

    pub fn view_blocks(&self) -> f32 {
        (self.ui.settings.render_chunks as f32 + 0.5) * CHUNK_SIZE as f32
    }

    /// The level-of-detail terrain the menu asks for.
    pub fn lod_settings(&self) -> LodSettings {
        let s = self.ui.settings;
        let radius = LOD_DISTANCES[(s.lod_level as usize).min(LOD_DISTANCES.len() - 1)] as f32;
        let ring = LOD_QUALITY[(s.lod_quality as usize).min(LOD_QUALITY.len() - 1)].1;
        // The rings start where the real chunks stop being drawn.
        LodSettings::new(radius, ring).with_near_edge(self.view_blocks())
    }

    /// Where fog starts and ends: over the far part of the LOD terrain when it is on (the
    /// horizon fades into the sky), else over the edge of the real chunks.
    pub fn fog_range(&self) -> (f32, f32) {
        let lod = self.lod_settings();
        if lod.enabled() {
            (lod.radius * 0.3, lod.radius * 0.97)
        } else {
            let v = self.view_blocks();
            (v * 0.55, v * 0.92)
        }
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
        self.clock.advance(dt as f64);
        self.ui.hours = self.clock.hours();

        let (strafe, forward) = self.stick();
        let walk = self.ui.settings.walk;
        if walk && !self.walking {
            // Back from flying: he is put down where the camera is and drops to the ground.
            let c = self.cam.pos;
            self.player.place([c[0] as f64, c[1] as f64 - 1.6, c[2] as f64]);
        }
        self.walking = walk;
        let view = if self.ui.settings.third_person { View::Follow } else { View::First };
        if self.player.ctl.view != view {
            self.player.ctl.set_view(view);
        }
        if walk {
            self.cam.pitch = self.cam.pitch.clamp(WALK_PITCH.0, WALK_PITCH.1);
        } else {
            let f = self.cam.forward();
            let r = self.cam.right();
            for i in 0..3 {
                self.cam.pos[i] += (f[i] * forward + r[i] * strafe) * FLY_SPEED * dt;
            }
        }
        // The controller's yaw 0 faces +Z; the camera's faces -Z.
        let look = (self.cam.yaw as f64 + std::f64::consts::PI, self.cam.pitch as f64);
        let aspect = self.size.0 as f64 / self.size.1 as f64;
        self.player.jump = self.held.values().any(|b| *b == "jump");
        self.player.roll = self.held.values().any(|b| *b == "roll");
        let eye = self.player.update(&self.client, dt as f64, (strafe, forward), look, aspect, walk);
        if walk {
            self.cam.pos = eye;
        }

        if now.duration_since(self.last_pos_sent) > Duration::from_millis(250) {
            self.client.set_position(self.cam.pos);
            self.last_pos_sent = now;
        } else {
            self.client.camera = self.cam.pos;
        }
        self.client.lod.set_settings(self.lod_settings()); // a no-op unless the menu changed it
        self.client.update();
    }

    pub fn touch(&mut self, id: u64, phase: TouchPhase, x: f32, y: f32) {
        // HUD first: a touch that starts on a button belongs to the HUD until it ends.
        match phase {
            TouchPhase::Started => {
                if let Some(action) = self.ui.hit(x, y) {
                    self.ui_touches.insert(id);
                    match action {
                        Action::Hold(button) => {
                            self.held.insert(id, button);
                        }
                        Action::Press(name) => self.player.press(name),
                        _ => {}
                    }
                    let greedy_before = self.ui.settings.greedy;
                    let menu_was_open = self.ui.menu_open;
                    if let Action::Time(h) = action {
                        self.clock.shift(h as f64);
                        self.ui.hours = self.clock.hours();
                    }
                    if self.ui.apply(action) {
                        self.client.set_view_distance(self.ui.settings.render_chunks);
                    }
                    if self.ui.settings.greedy != greedy_before {
                        let kind = if self.ui.settings.greedy { Mesher::Greedy } else { Mesher::Simple };
                        self.client.set_mesher(kind);
                    }
                    if menu_was_open && !self.ui.menu_open {
                        self.save_state(); // closing the menu keeps what was chosen in it
                    }
                    return;
                }
            }
            _ => {
                if self.ui_touches.contains(&id) {
                    if matches!(phase, TouchPhase::Ended | TouchPhase::Cancelled) {
                        self.ui_touches.remove(&id);
                        self.held.remove(&id);
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
        // Behind the character, the camera is further from what he can reach.
        let behind = if self.ui.settings.walk && self.ui.settings.third_person { 4.0 } else { 0.0 };
        raycast(self.cam.pos, self.cam.forward(), REACH + behind, |p| self.client.is_solid(p))
    }

    fn break_target(&mut self) {
        if let Some(hit) = self.target() {
            self.client.break_block(hit.pos);
            if self.ui.settings.walk {
                self.player.press("punch");
            }
        }
    }

    fn place_target(&mut self) {
        if let Some(hit) = self.target() {
            let cell = hit.pos.neighbor(hit.face);
            // Don't entomb the camera or the character.
            if cell != BlockPos::from_f32(self.cam.pos) && !self.player.occupies(cell) {
                self.client.place_block(cell, self.stone);
                if self.ui.settings.walk {
                    self.player.press("interact");
                }
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
    fn camera_survives_a_round_trip_through_text() {
        let c = CameraView { pos: [-123.25, 64.5, 9001.75], yaw: 2.5, pitch: -0.75 };
        assert_eq!(CameraView::from_text(&c.to_text()), Some(c));
        // It shares a file with the settings, which must not confuse either parser.
        let both = format!("{}{}", crate::ui::Settings::default().to_text(), c.to_text());
        assert_eq!(CameraView::from_text(&both), Some(c));
    }

    #[test]
    fn worlds_from_older_generators_are_kept_apart() {
        assert_eq!(saved_generator("lod_level=5\ncam=1,2,3,0,0\n"), 1);
        assert_eq!(saved_generator("cam=1,2,3,0,0\ngen=2\n"), 2);
        assert_eq!(world_file(), format!("world_v{WORLDGEN_VERSION}.bfwj"));
    }

    #[test]
    fn a_broken_saved_camera_is_rejected() {
        for bad in ["", "cam=1,2,3", "cam=1,2,3,4,x", "cam=NaN,0,0,0,0", "cam=inf,0,0,0,0", "cam=1e30,0,0,0,0"] {
            assert_eq!(CameraView::from_text(bad), None, "{bad}");
        }
        // A wild pitch is clamped rather than thrown away.
        assert_eq!(CameraView::from_text("cam=0,0,0,0,9").map(|c| c.pitch), Some(1.5));
    }

    #[test]
    fn yaw_zero_looks_down_negative_z() {
        let c = CameraView { pos: [0.0; 3], yaw: 0.0, pitch: 0.0 };
        let f = c.forward();
        assert!(f[2] < -0.99 && f[0].abs() < 1e-5);
    }
}
