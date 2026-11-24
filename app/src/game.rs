//! Player-facing state: the integrated server + client, the character and touch controls.
//!
//! Controls (landscape; every button can be moved, resized or hidden in the controls editor):
//!   left side         a stick appears where the thumb lands and follows it: walk, jog or
//!                     sprint by how far it is pushed (or fly where you look, in fly mode;
//!                     JUMP rises, DOWN sinks; two quick JUMPs take off or drop out of it)
//!   right side        drag = look around
//!   ATTACK            tap = punch (breaks the block under the crosshair); hold = chop, a
//!                     heavy swing that breaks a block each time it lands, again and again
//!   PLACE             puts the hotbar's block on the face you look at (hold to keep going)
//!   hotbar            nine blocks; the last button opens a picker of every block
//!   JUMP, ROLL, CROUCH, CRAWL, ACT (everything else the character can do), PAUSE

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use blockforge::behavior::BehaviorTable;
use blockforge::block::{BlockState, Shape};
use blockforge::client::Client;
use blockforge::coords::{BlockPos, Face, CHUNK_SIZE};
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
use crate::ui::{items, Action, BlockItem, CamView, Joy, Recipe, Screen, Stack, Ui, JOY_RADII, LOD_DISTANCES, LOD_QUALITY};

const LOOK_SENS: f32 = 0.0045;
const REACH: f32 = 8.0;
/// Walking, the camera's pitch stays inside the character controller's limits.
const WALK_PITCH: (f32, f32) = (-1.4, 1.35);
/// How long a punch, and a chop, keep his hands from a block he carries.
const PUNCH_HANDS: Duration = Duration::from_millis(500);
const CHOP_HANDS: Duration = Duration::from_millis(950);
/// ATTACK held this long is a heavy attack (a chop) rather than a punch.
const HEAVY_AFTER: Duration = Duration::from_millis(300);
/// A chop lands this far into its swing, and swings again this often while held (the
/// character's chop animation is 0.9 s).
const CHOP_LANDS: Duration = Duration::from_millis(450);
const CHOP_EVERY: Duration = Duration::from_millis(900);
/// PLACE held: the first repeat after this, then one every `PLACE_EVERY`.
const PLACE_FIRST: Duration = Duration::from_millis(400);
const PLACE_EVERY: Duration = Duration::from_millis(220);
/// Building by touch: a touch on the world held this long without moving digs; one that
/// lifts sooner without moving is a tap (place, or use). Moving further than `DRAG_PIXELS`
/// dp makes it a look.
const DIG_AFTER: Duration = Duration::from_millis(350);
const DRAG_DP: f32 = 8.0;
/// The one stance button: held this long it crawls; a second tap this soon after the first
/// rolls (so a single tap's crouch waits that long to be sure).
const CRAWL_AFTER: Duration = Duration::from_millis(350);
const DOUBLE_TAP: Duration = Duration::from_millis(280);
const ROLL_PULSE: Duration = Duration::from_millis(200);
/// How long the stick takes to fade out after the thumb lifts.
const JOY_FADE: f32 = 0.25;
/// The left part of the screen where a touch is the stick (elsewhere it looks around).
const STICK_ZONE: f32 = 0.45;
/// The hotbar of a new world.
const DEFAULT_HOTBAR: [&str; 9] = ["stone", "dirt", "grass", "sand", "log", "leaves", "glowstone", "sandstone", "snow"];
/// Settings and the camera, kept between launches next to the world save.
use crate::worlds::STATE_FILE;
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
        format!("{}  {:.0}C  RAIN {:.0}%", g.biome.to_uppercase(), g.temperature, g.rainfall * 100.0),
        format!("{}  GROUND Y {:.0}", g.landform.to_uppercase(), g.height),
    ]
}

/// The world save for the current generator: `world.bfwj` for the first one (so older
/// saves keep working), `world_v2.bfwj` and so on after.
/// Writes `text` to `path` through a temporary file, so a crash never leaves half a file.
pub fn write_atomically(path: &std::path::Path, text: &str) {
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

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
    last: (f32, f32),
    /// For the stick: where its ring is (where the thumb landed).
    base: (f32, f32),
    stick: bool,
    /// Where and when it started, whether it has moved far enough to be a look, and whether
    /// it is digging (building by touch).
    start: (f32, f32),
    t0: Instant,
    moved: bool,
    digging: bool,
}

/// The one stance button held: by which touch, since when, and whether it has already done
/// something (crawled, or rolled on a double tap) so letting go does nothing more.
struct Stance {
    touch: u64,
    since: Instant,
    done: bool,
}

/// ATTACK being held: by which touch, since when, and when the next chop starts.
struct Attack {
    touch: u64,
    since: Instant,
    next_chop: Option<Instant>,
}

/// Digging by touch: which touch is held on the world, the block being hammered at and how
/// many strikes have landed on it.
struct Mining {
    touch: u64,
    block: Option<BlockPos>,
    hits: u32,
}

/// How many strikes of the held mining swing (about three a second) break a block: by its
/// hardness, as in Minecraft (dirt one, stone three, granite four).
fn strikes_to_break(hardness: f32) -> u32 {
    (hardness * 2.0).round().clamp(1.0, 8.0) as u32
}

/// The blocks the hotbar can hold (in `icons::placeable` order, the icons' order).
pub fn block_items(reg: &BlockRegistry) -> Vec<BlockItem> {
    crate::icons::items(reg)
        .into_iter()
        .map(|id| BlockItem { id, name: crate::icons::item_name(reg, id), group: crate::icons::group_of(reg, id), biome: crate::icons::biome_of(reg, id) })
        .collect()
}

/// An item's name in saves: the block's, ":slab" after a slab's.
fn item_save_name(reg: &BlockRegistry, item: u16) -> String {
    let n = reg.def(item).name.clone();
    if crate::icons::is_slab(item) {
        n + ":slab"
    } else {
        n
    }
}

/// The item a save names (`item_save_name`).
fn item_by_name(reg: &BlockRegistry, name: &str) -> Option<u16> {
    let name = name.trim();
    match name.strip_suffix(":slab") {
        Some(block) => reg.id(block).map(crate::icons::slab_item),
        None => reg.id(name),
    }
}

/// The hotbar saved in a world's state (`hotbar=stone*64,dirt*12,...` and `slot=n`; older
/// saves name the blocks alone: full stacks), or the default one; unknown blocks leave their
/// slot empty.
fn saved_hotbar(text: &str, reg: &BlockRegistry, items: &[BlockItem]) -> ([Stack; 9], usize) {
    let id_of = |name: &str| item_by_name(reg, name).filter(|id| items.iter().any(|b| b.id == *id));
    let mut bar = [Stack::EMPTY; 9];
    match text.lines().find_map(|l| l.trim().strip_prefix("hotbar=")) {
        Some(line) => crate::ui::items::from_text(line, &mut bar, id_of),
        None => {
            for (slot, name) in bar.iter_mut().zip(DEFAULT_HOTBAR) {
                *slot = id_of(name).map_or(Stack::EMPTY, |id| Stack::of(id, crate::ui::items::STACK_MAX));
            }
        }
    }
    let slot = text.lines().find_map(|l| l.trim().strip_prefix("slot=")?.parse::<usize>().ok()).unwrap_or(0).min(8);
    (bar, slot)
}

/// A container saved in a world's state under `key=` (as the hotbar is), into `slots`.
fn saved_stacks(text: &str, key: &str, reg: &BlockRegistry, items: &[BlockItem], slots: &mut [Stack]) {
    let id_of = |name: &str| item_by_name(reg, name).filter(|id| items.iter().any(|b| b.id == *id));
    if let Some(line) = text.lines().find_map(|l| l.trim().strip_prefix(key)?.strip_prefix('=')) {
        crate::ui::items::from_text(line, slots, id_of);
    }
}

/// The crafting recipes (`ui::items::RECIPES`) for the blocks this registry has.
pub fn recipes(reg: &BlockRegistry) -> Vec<Recipe> {
    let mut out: Vec<Recipe> = crate::ui::items::RECIPES
        .iter()
        .filter_map(|(inputs, (out, n))| {
            let inputs = inputs.iter().map(|(name, k)| Some((reg.id(name)?, *k))).collect::<Option<Vec<_>>>()?;
            Some(Recipe { inputs, output: Stack::of(reg.id(out)?, *n) })
        })
        .collect();
    let mut add = |inputs: &[(&str, u8)], output: &str, n: u16| {
        let ins = inputs.iter().map(|(name, k)| Some((item_by_name(reg, name)?, *k))).collect::<Option<Vec<_>>>();
        if let (Some(inputs), Some(o)) = (ins, item_by_name(reg, output)) {
            out.push(Recipe { inputs, output: Stack::of(o, n) });
        }
    };
    // Every block of the set by how it is made: logs into planks; a rock into its cobbles,
    // cobbles into bricks, four of the rock into polished blocks; two blocks into four slabs
    // and two slabs back into a block.
    add(&[("log", 1)], "oak_planks", 4);
    for id in 1..reg.len() as u16 {
        let name = reg.def(id).name.as_str();
        if let Some(sp) = name.strip_suffix("_log") {
            add(&[(name, 1)], &format!("{sp}_planks"), 4);
        }
        if let Some(rock) = name.strip_prefix("cobbled_") {
            add(&[(rock, 1)], name, 1);
            add(&[(name, 4)], &format!("{rock}_bricks"), 4);
            add(&[(rock, 4)], &format!("polished_{rock}"), 4);
        }
    }
    add(&[("cobbled_stone", 1), ("moss_block", 1)], "mossy_cobbled_stone", 2);
    add(&[("stone_bricks", 1), ("moss_block", 1)], "mossy_stone_bricks", 2);
    for (from, to) in [("clay", "clay_bricks"), ("mud", "mud_bricks"), ("ice", "ice_bricks"), ("snow", "snow_bricks"), ("petrified_log", "petrified_wood_bricks"), ("dry_grass", "hay_bale")] {
        add(&[(from, 4)], to, 4);
    }
    add(&[("petrified_wood_bricks", 4)], "polished_petrified_wood", 4);
    add(&[("weathered_stone", 1), ("glow_crystal", 1)], "runestone", 1);
    for id in 1..reg.len() as u16 {
        let d = reg.def(id);
        if d.slab {
            let slab = format!("{}:slab", d.name);
            add(&[(d.name.as_str(), 2)], &slab, 4);
            add(&[(slab.as_str(), 2)], &d.name, 1);
        }
    }
    out
}

/// The world a game is opened on.
pub struct WorldSpec {
    /// Its folder (`worlds::dir`), or `None` for a world kept in memory only (tests, and
    /// platforms without a data directory).
    pub dir: Option<PathBuf>,
    pub seed: u64,
    /// Where the app's settings are saved (`worlds::SETTINGS_FILE`), if anywhere.
    pub settings_path: Option<PathBuf>,
}

pub struct Game {
    // Field order matters: the client is dropped before the server it talks to.
    pub client: Client,
    server: ServerHandle,
    /// The world's `state.txt` (player, camera, time of day).
    state_path: Option<PathBuf>,
    /// The app's settings file.
    settings_path: Option<PathBuf>,
    registry: Arc<BlockRegistry>,
    /// The world's generator, for the stats overlay's biome lines.
    gen: Arc<WorldGen>,
    pub cam: CameraView,
    pub size: (u32, u32),
    pub ui: Ui,
    /// The time of day (the sky's sun follows it).
    pub clock: Clock,
    /// Touches that started on a HUD button, and the button; they never reach the game
    /// controls.
    ui_touches: HashMap<u64, Action>,
    touches: HashMap<u64, TouchInfo>,
    /// ATTACK held; a chop on its way down (it lands even if ATTACK is let go); PLACE held
    /// (by which touch, and when it places again); the stick as it last showed, and when
    /// the thumb lifted.
    attack: Option<Attack>,
    chop_lands: Option<Instant>,
    placing: Option<(u64, Instant)>,
    joy_last: Option<(Joy, Instant)>,
    /// The one stance button: held now; a tap's crouch waiting to see if a second tap (a roll)
    /// follows; a roll under way.
    stance: Option<Stance>,
    crouch_at: Option<Instant>,
    roll_until: Option<Instant>,
    /// Which blocks do something when used (a tap uses them instead of building on them).
    behaviors: Option<BehaviorTable>,
    /// The character; the camera goes with him, walking or flying.
    pub player: Player,
    /// Whether the last frame was in walk mode (to notice the switch).
    walking: bool,
    /// His hands are busy (punching, chopping) until then: the block he carries is put away.
    hands_busy: Option<Instant>,
    /// A finger held still on the world while building by touch: digging.
    mining: Option<Mining>,
    /// Touches holding JUMP or ROLL down.
    held: HashMap<u64, &'static str>,
    last_update: Instant,
    last_pos_sent: Instant,
}

impl Game {
    /// Opens `world`, taking over the app's `ui` (with its settings) until `close`.
    pub fn new(world: WorldSpec, mut ui: Ui, registry: Arc<BlockRegistry>) -> Game {
        let behaviors = BehaviorTable::builtin(&registry).ok();

        // Pick up where the last session in this world stopped: camera, player and time.
        let state_path = world.dir.as_ref().map(|d| d.join(STATE_FILE));
        let saved = state_path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        let clock = Clock::from_text(&saved);
        ui.hours = clock.hours();
        if ui.blocks.is_empty() {
            ui.blocks = block_items(&registry);
        }
        (ui.hotbar, ui.slot) = saved_hotbar(&saved, &registry, &ui.blocks);
        ui.inventory = [Stack::EMPTY; items::INV_SLOTS];
        ui.craft = [Stack::EMPTY; items::CRAFT_SLOTS];
        saved_stacks(&saved, "inventory", &registry, &ui.blocks, &mut ui.inventory);
        saved_stacks(&saved, "craft", &registry, &ui.blocks, &mut ui.craft);
        ui.recipes = recipes(&registry);
        // A camera saved in a world from an older generator would be over different ground:
        // start over at the spawn (that world's save is kept, under its own name).
        let same_world = saved_generator(&saved) == WORLDGEN_VERSION;
        let gen = Arc::new(WorldGen::new(world.seed, &registry));
        let cam = CameraView::from_text(&saved).filter(|_| same_world).unwrap_or_else(|| {
            let [x, y, z] = gen.find_spawn();
            CameraView { pos: [x, y + 24.0, z], yaw: 0.0, pitch: -0.35 }
        });

        // The character starts where he was left, or at the spawn (a little above the ground:
        // he drops onto it once it has loaded).
        let view = if ui.settings.view == CamView::First { View::First } else { View::Follow };
        let player_at = saved_player(&saved).filter(|_| same_world).unwrap_or_else(|| {
            let [x, y, z] = gen.find_spawn();
            [x as f64 + 0.5, y as f64 + 1.5, z as f64 + 0.5]
        });
        let mut player = Player::new(player_at, view);
        if !ui.settings.walk {
            player.set_fly(true); // as soon as he is in the world
        }

        let cfg = ServerConfig {
            seed: world.seed,
            worker_threads: 2,
            save_path: world.dir.map(|d| d.join(world_file())),
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
            settings_path: world.settings_path,
            registry,
            gen,
            cam,
            size: (1, 1),
            ui,
            clock,
            ui_touches: HashMap::new(),
            touches: HashMap::new(),
            attack: None,
            chop_lands: None,
            placing: None,
            joy_last: None,
            stance: None,
            crouch_at: None,
            roll_until: None,
            behaviors,
            player,
            walking: ui_walk,
            hands_busy: None,
            mining: None,
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
        if let Some(path) = &self.settings_path {
            write_atomically(path, &self.ui.settings.to_text());
        }
        let Some(path) = &self.state_path else { return };
        write_atomically(path, &self.state_text());
    }

    /// The world's `state.txt`: camera, time of day, the character, the hotbar.
    fn state_text(&self) -> String {
        let p = self.player.position();
        let name = |id: u16| item_save_name(&self.registry, id);
        format!(
            "{}{}player={},{},{}\ngen={WORLDGEN_VERSION}\nhotbar={}\nslot={}\ninventory={}\ncraft={}\n",
            self.cam.to_text(),
            self.clock.to_text(),
            p[0],
            p[1],
            p[2],
            items::to_text(&self.ui.hotbar, name),
            self.ui.slot,
            items::to_text(&self.ui.inventory, name),
            items::to_text(&self.ui.craft, name),
        )
    }

    /// Saves everything and shuts the world down; hands back the app's `ui`.
    pub fn close(mut self) -> Ui {
        self.persist();
        std::mem::take(&mut self.ui)
    }

    /// Whether the world around the player has loaded enough to start (he has landed on
    /// it, or the camera has ground around it in fly mode, or it has taken long enough).
    pub fn ready(&self, waited: Duration) -> bool {
        (!self.player.is_waiting() && self.client.meshes.len() >= 24) || waited > Duration::from_secs(25)
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

    /// The stick's radius in pixels.
    fn stick_radius(&self) -> f32 {
        JOY_RADII[self.ui.settings.joy_size as usize % JOY_RADII.len()] * self.ui.dp_scale()
    }

    /// (strafe, forward), each in -1..1, from the stick.
    fn stick(&self) -> (f32, f32) {
        let r = self.stick_radius();
        for t in self.touches.values() {
            if t.stick {
                let mut x = (t.last.0 - t.base.0) / r;
                let mut y = (t.last.1 - t.base.1) / r;
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

    /// The stick as the HUD draws it: while a thumb is on it, then fading out.
    fn update_joystick(&mut self, now: Instant) {
        let r = self.stick_radius();
        let live = self.touches.values().find(|t| t.stick).map(|t| {
            let (dx, dy) = (t.last.0 - t.base.0, t.last.1 - t.base.1);
            let len = dx.hypot(dy).max(1e-6);
            let k = (r / len).min(1.0);
            Joy { base: [t.base.0, t.base.1], knob: [t.base.0 + dx * k, t.base.1 + dy * k], radius: r, alpha: 1.0 }
        });
        self.ui.joystick = match live {
            Some(j) => {
                self.joy_last = Some((j, now));
                Some(j)
            }
            None => self.joy_last.and_then(|(j, t)| {
                let a = 1.0 - now.duration_since(t).as_secs_f32() / JOY_FADE;
                (a > 0.0).then_some(Joy { knob: j.base, alpha: a, ..j })
            }),
        };
    }

    /// ATTACK and PLACE while held: chops that keep landing, blocks that keep coming.
    fn update_buttons(&mut self, now: Instant) {
        if let Some(a) = &mut self.attack {
            if now.duration_since(a.since) >= HEAVY_AFTER && a.next_chop.map_or(true, |t| now >= t) {
                a.next_chop = Some(now + CHOP_EVERY);
                self.chop_lands = Some(now + CHOP_LANDS);
                self.hands_busy = Some(now + CHOP_HANDS);
                self.player.press("chop");
            }
        }
        if self.chop_lands.is_some_and(|t| now >= t) {
            self.chop_lands = None;
            if let Some(hit) = self.target() {
                self.client.break_block(hit.pos);
            }
        }
        // The one stance button: held long enough, crawl; a tap's crouch once no second tap
        // came.
        if let Some(s) = &mut self.stance {
            if !s.done && now.duration_since(s.since) >= CRAWL_AFTER {
                s.done = true;
                self.player.press("crawl");
            }
        }
        if self.crouch_at.is_some_and(|t| now >= t) {
            self.crouch_at = None;
            self.player.press("crouch");
        }
        if self.roll_until.is_some_and(|t| now >= t) {
            self.roll_until = None;
        }
        // Building by touch: a still finger on the world digs once held long enough, with the
        // mining swing (see `count_strikes`).
        if self.ui.settings.touch_build && self.attack.is_none() && self.mining.is_none() {
            if let Some((&id, t)) = self.touches.iter_mut().find(|(_, t)| !t.stick && !t.moved && !t.digging && now.duration_since(t.t0) >= DIG_AFTER) {
                t.digging = true;
                self.mining = Some(Mining { touch: id, block: None, hits: 0 });
            }
        }
        if let Some((touch, next)) = self.placing {
            if now >= next {
                self.place_target();
                self.placing = Some((touch, now + PLACE_EVERY));
            }
        }
        self.ui.touching = self.ui_touches.values().copied().collect();
    }

    /// A finger left a HUD button.
    fn button_released(&mut self, id: u64) {
        self.ui_touches.remove(&id);
        self.held.remove(&id);
        if self.attack.as_ref().is_some_and(|a| a.touch == id) {
            let a = self.attack.take().unwrap();
            if a.next_chop.is_none() {
                // Let go before it became a heavy attack: a punch.
                self.break_target();
            }
        }
        if self.placing.is_some_and(|(t, _)| t == id) {
            self.placing = None;
        }
        if let Some(s) = self.stance.take_if(|s| s.touch == id) {
            if !s.done {
                // A tap: crouch, unless a second tap comes soon and makes it a roll.
                self.crouch_at = Some(Instant::now() + DOUBLE_TAP);
            }
        }
    }

    /// Digging: each strike of the mining swing that lands counts against the block looked
    /// at; enough of them (by its hardness) break it. Looking at another block starts over.
    fn count_strikes(&mut self) {
        let landed = self.player.last.as_ref().map_or(0, |o| o.events.iter().filter(|e| **e == "mineHit").count() as u32);
        let target = self.target().map(|h| h.pos);
        let Some(m) = self.mining.as_mut() else { return };
        if m.block != target {
            (m.block, m.hits) = (target, 0);
        }
        let Some(pos) = target.filter(|_| landed > 0) else { return };
        m.hits += landed;
        let hardness = self.client.block_at(pos).map_or(1.0, |s| self.registry.def(s.id()).hardness);
        if m.hits >= strikes_to_break(hardness) {
            m.hits = 0;
            self.client.break_block(pos);
        }
    }

    /// A tap on the world while building by touch: use the block if it can be used, else put
    /// the hotbar's block against it.
    fn tap_world(&mut self) {
        let usable = |g: &Game, pos: BlockPos| {
            g.client.block_at(pos).is_some_and(|s| g.behaviors.as_ref().is_some_and(|b| b.usable(s.id())))
        };
        match self.target() {
            Some(hit) if usable(self, hit.pos) => {
                self.client.use_block(hit.pos);
                // With a block in his hands, both reach out.
                self.player.press(if self.player.hold == "two_handed_item" { "interact_both" } else { "interact" });
            }
            Some(_) => self.place_target(),
            None => {}
        }
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
        if walk != self.walking {
            // Walk or Fly picked in the settings: he takes off, or drops to the ground.
            self.player.set_fly(!walk);
        }
        self.walking = walk;
        let cam_view = self.ui.settings.view;
        let view = if cam_view == CamView::First { View::First } else { View::Follow };
        if self.player.ctl.view != view {
            self.player.ctl.set_view(view);
        }
        self.cam.pitch = self.cam.pitch.clamp(WALK_PITCH.0, WALK_PITCH.1);
        // The controller's yaw 0 faces +Z; the camera's faces -Z.
        let look = (self.cam.yaw as f64 + std::f64::consts::PI, self.cam.pitch as f64);
        let aspect = self.size.0 as f64 / self.size.1 as f64;
        self.update_joystick(now);
        self.update_buttons(now);
        self.player.jump = self.held.values().any(|b| *b == "jump");
        self.player.roll = self.held.values().any(|b| *b == "roll") || self.roll_until.is_some_and(|t| now < t);
        self.player.down = self.held.values().any(|b| *b == "down");
        // A block picked in the hotbar is carried in both hands, put away while they are busy.
        let block = self.ui.held_block();
        let busy = self.attack.is_some() || self.chop_lands.is_some() || self.mining.is_some() || self.hands_busy.is_some_and(|t| now < t);
        self.player.mine = self.mining.is_some();
        self.player.hold = if block != 0 && !busy { "two_handed_item" } else { "none" };
        self.player.set_carry((block != 0).then(|| std::array::from_fn(|f| self.registry.def(block).tex[f])));
        let eye = self.player.update(&self.client, dt as f64, (strafe, forward), look, aspect, true, cam_view == CamView::Front);
        self.cam.pos = eye;
        self.count_strikes();
        // Flight can start or end by itself (two quick jumps; flying down onto the ground):
        // the setting follows.
        if !self.player.is_waiting() && self.player.flying() == self.ui.settings.walk {
            self.ui.settings.walk = !self.player.flying();
            self.walking = self.ui.settings.walk;
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

    /// Does what a HUD button asks (`id` is the touch that pressed it).
    fn ui_action(&mut self, id: u64, action: Action) {
        match action {
            Action::Hold("attack") => {
                self.attack = Some(Attack { touch: id, since: Instant::now(), next_chop: None });
            }
            Action::Hold("place") => {
                self.place_target();
                self.placing = Some((id, Instant::now() + PLACE_FIRST));
            }
            Action::Hold("stance") => {
                let now = Instant::now();
                // A second tap soon after a first: roll (and forget the first tap's crouch).
                let rolled = self.crouch_at.take().is_some();
                if rolled {
                    self.roll_until = Some(now + ROLL_PULSE);
                }
                self.stance = Some(Stance { touch: id, since: now, done: rolled });
            }
            Action::Hold(button) => {
                self.held.insert(id, button);
            }
            Action::Press(name) => self.player.press(name),
            _ => {}
        }
        let greedy_before = self.ui.settings.greedy;
        let screen_before = self.ui.screen;
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
        if self.ui.screen != screen_before {
            // Fingers on the old screen don't carry over (no stick left pushed, no button held).
            self.touches.clear();
            self.held.clear();
            self.ui_touches.clear();
            self.attack = None;
            self.placing = None;
            self.stance = None;
            if screen_before == Screen::Settings {
                self.save_state(); // keeps what was applied
            }
        }
    }

    pub fn touch(&mut self, id: u64, phase: TouchPhase, x: f32, y: f32) {
        // Off the game (its menu, settings, loading), touches only press buttons, which act
        // when the finger lifts off them.
        if self.ui.screen != Screen::Playing {
            let action = match phase {
                TouchPhase::Started => self.ui.press(id, x, y),
                TouchPhase::Ended => self.ui.release(id, x, y),
                TouchPhase::Cancelled => self.ui.release(id, f32::NAN, f32::NAN),
                TouchPhase::Moved => {
                    self.ui.moved(id, x, y);
                    None
                }
            };
            if let Some(a) = action {
                self.ui_action(id, a);
            }
            return;
        }
        // HUD first: a touch that starts on a button belongs to the HUD until it ends.
        match phase {
            TouchPhase::Started => {
                if let Some(action) = self.ui.press(id, x, y) {
                    self.ui_touches.insert(id, action);
                    self.ui_action(id, action);
                    return;
                }
            }
            _ => {
                if self.ui_touches.contains_key(&id) {
                    if matches!(phase, TouchPhase::Ended | TouchPhase::Cancelled) {
                        self.button_released(id);
                    }
                    return;
                }
            }
        }

        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        match phase {
            TouchPhase::Started => {
                // One stick at a time; its ring appears where the thumb lands (kept on screen).
                let stick = x < w * STICK_ZONE && !self.touches.values().any(|t| t.stick);
                let r = self.stick_radius();
                let base = if stick { (x.clamp(r, (w - r).max(r)), y.clamp(r, (h - r).max(r))) } else { (x, y) };
                self.touches.insert(id, TouchInfo { last: (x, y), base, stick, start: (x, y), t0: Instant::now(), moved: false, digging: false });
                // A touch elsewhere closes the action grid.
                self.ui.acts_open = false;
            }
            TouchPhase::Moved => {
                let look = LOOK_SENS * (0.25 + 0.15 * self.ui.settings.look_speed as f32);
                let drag = DRAG_DP * self.ui.dp_scale();
                if let Some(t) = self.touches.get_mut(&id) {
                    let dx = x - t.last.0;
                    let dy = y - t.last.1;
                    t.last = (x, y);
                    if (x - t.start.0).hypot(y - t.start.1) > drag {
                        t.moved = true;
                    }
                    // The stick's ring stays where the thumb first landed.
                    if !t.stick {
                        self.cam.yaw -= dx * look;
                        self.cam.pitch = (self.cam.pitch - dy * look).clamp(-1.5, 1.5);
                    }
                }
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                if let Some(t) = self.touches.remove(&id) {
                    if t.digging {
                        // Stop digging (the swing on its way finishes, but breaks nothing more).
                        if self.mining.as_ref().is_some_and(|m| m.touch == id) {
                            self.mining = None;
                        }
                    } else if self.ui.settings.touch_build && !t.stick && !t.moved && phase == TouchPhase::Ended && t.t0.elapsed() < DIG_AFTER {
                        self.tap_world();
                    }
                }
            }
        }
    }

    /// The camera to draw with. It is `cam` (where you look), except in the front view,
    /// where the camera is out in front of the character and looks back the other way.
    pub fn render_cam(&self) -> CameraView {
        if self.ui.settings.view == CamView::Front {
            CameraView { pos: self.cam.pos, yaw: self.cam.yaw + std::f32::consts::PI, pitch: -self.cam.pitch }
        } else {
            self.cam
        }
    }

    fn target(&self) -> Option<RayHit> {
        let s = self.ui.settings;
        if s.view == CamView::Front {
            // The camera is in front of him: reach from his own eyes instead.
            let p = self.player.position();
            let eyes = [p[0] as f32, p[1] as f32 + 1.6, p[2] as f32];
            return raycast(eyes, self.cam.forward(), REACH, |p| self.client.is_targetable(p));
        }
        // Behind the character, the camera is further from what he can reach.
        let behind = if s.view == CamView::Back { 4.0 } else { 0.0 };
        raycast(self.cam.pos, self.cam.forward(), REACH + behind, |p| self.client.is_targetable(p))
    }

    fn break_target(&mut self) {
        if let Some(hit) = self.target() {
            self.client.break_block(hit.pos);
            self.hands_busy = Some(Instant::now() + PUNCH_HANDS);
            self.player.press("punch");
        }
    }

    fn place_target(&mut self) {
        let item = self.ui.held_block();
        if item == 0 {
            return;
        }
        if let Some(hit) = self.target() {
            let block = BlockState(item).id();
            // Building onto a plant puts the block in its place (grass is not a wall).
            let mut cell = if self.client.is_plant(hit.pos) { hit.pos } else { hit.pos.neighbor(hit.face) };
            let mut state = BlockState::full(block);
            if crate::icons::is_slab(item) {
                // A slab onto the top of a block lies at the bottom of the cell above it, under
                // a block at its top; on a side, in the half the cursor points at. A slab onto
                // the open half of the same slab makes the whole block.
                let fill = match (self.client.block_at(hit.pos), hit.face) {
                    (Some(s), Face::PosY) if s.id() == block && s.shape() == Shape::SlabBottom => true,
                    (Some(s), Face::NegY) if s.id() == block && s.shape() == Shape::SlabTop => true,
                    _ => false,
                };
                if fill {
                    cell = hit.pos;
                } else {
                    let y = self.cam.pos[1] + self.cam.forward()[1] * hit.distance;
                    let upper = match hit.face {
                        Face::PosY => false,
                        Face::NegY => true,
                        _ => y - y.floor() > 0.5,
                    };
                    state = BlockState::new(block, if upper { Shape::SlabTop } else { Shape::SlabBottom });
                }
            }
            // Don't entomb the camera or the character.
            if cell != BlockPos::from_f32(self.cam.pos) && !self.player.occupies(cell) {
                self.client.place_block(cell, state);
                // Put down with both hands (it is carried in both).
                self.player.press("interact_both");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_block_set_is_grouped_has_slabs_and_recipes_and_saves_by_name() {
        let reg = BlockRegistry::builtin();
        let items = block_items(&reg);
        // In tab order, each tab's blocks by material: a rock's cobbles, bricks and polished
        // blocks together, each with its slab after it.
        assert!(items.windows(2).all(|w| w[0].group <= w[1].group));
        let at = |name: &str| items.iter().position(|b| b.name == name).unwrap_or_else(|| panic!("no {name}"));
        let c = at("Cobbled Granite");
        assert_eq!([at("Cobbled Granite Slab"), at("Granite Bricks"), at("Granite Bricks Slab"), at("Polished Granite")], [c + 1, c + 2, c + 3, c + 4]);
        assert!(at("Oak Log") < at("Oak Planks") && at("Oak Planks") < at("Redwood Log"));
        // A slab is an item of its own that saves by name.
        let slab = item_by_name(&reg, "slate_bricks:slab").unwrap();
        assert!(crate::icons::is_slab(slab) && reg.def(slab).name == "slate_bricks");
        assert_eq!(item_save_name(&reg, slab), "slate_bricks:slab");
        // Recipes for the set.
        let rs = recipes(&reg);
        let makes = |ins: &str, out: &str| {
            let (i, o) = (item_by_name(&reg, ins).unwrap(), item_by_name(&reg, out).unwrap());
            rs.iter().any(|r| r.inputs.iter().any(|x| x.0 == i) && r.output.id == o)
        };
        assert!(makes("maple_log", "maple_planks") && makes("log", "oak_planks") && makes("basalt", "cobbled_basalt"));
        assert!(makes("cobbled_basalt", "basalt_bricks") && makes("basalt_bricks", "basalt_bricks:slab") && makes("basalt_bricks:slab", "basalt_bricks"));
        // Every texture fits the texture array and the distant terrain's 8 bits.
        assert!(reg.texture_names().len() < 2048);
        // About a hundred blocks for every biome.
        for (n, (key, _)) in crate::icons::BIOMES.iter().enumerate() {
            let k = (1..reg.len() as u16).filter(|&id| crate::icons::biome_of(&reg, id) == n as u8 + 1).count();
            assert!((95..=110).contains(&k), "{key}: {k} blocks");
        }
    }

    #[test]
    fn harder_blocks_take_more_strikes() {
        assert_eq!([0.2, 0.5, 1.5, 1.8, 100.0].map(strikes_to_break), [1, 1, 3, 4, 8]);
    }

    #[test]
    fn worlds_open_on_their_own_seed_and_keep_their_player_and_the_settings() {
        use crate::worlds;
        let d = std::env::temp_dir().join(format!("bf_game_worlds_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        worlds::migrate(&d).unwrap();
        let reg = Arc::new(BlockRegistry::builtin());
        let a = worlds::create(&d, "A", 1337).unwrap();
        let b = worlds::create(&d, "B", 4242).unwrap();
        let spec = |w: &worlds::WorldInfo| WorldSpec {
            dir: Some(worlds::dir(&d, &w.folder)),
            seed: w.seed,
            settings_path: Some(d.join(worlds::SETTINGS_FILE)),
        };
        let play = |g: &mut Game| {
            g.set_surface(2400, 1080, 2.75);
            let t = Instant::now();
            while !g.ready(t.elapsed()) {
                g.update();
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(!g.player.is_waiting(), "he landed (not just a timeout)");
        };
        let mut ui = Ui::new();
        ui.settings.render_chunks = 2;
        ui.settings.fog = false;
        let mut g = Game::new(spec(&a), ui, reg.clone());
        assert_eq!(g.generator().seed(), 1337);
        play(&mut g);
        let in_a = g.player.position();
        let ui = g.close();
        let state = std::fs::read_to_string(worlds::dir(&d, &a.folder).join(STATE_FILE)).unwrap();
        assert!(state.contains("player=") && !state.contains("fog="), "{state}");
        let settings = std::fs::read_to_string(d.join(worlds::SETTINGS_FILE)).unwrap();
        assert!(settings.contains("fog=false"), "{settings}");

        // Another world: another seed, its own spawn.
        let mut g = Game::new(spec(&b), ui, reg.clone());
        assert_eq!(g.generator().seed(), 4242);
        play(&mut g);
        let in_b = g.player.position();
        assert!((in_a[0] - in_b[0]).abs() + (in_a[2] - in_b[2]).abs() > 1.0, "{in_a:?} {in_b:?}");
        let ui = g.close();

        // Back in the first: where he was left, with the settings as they were.
        let g = Game::new(spec(&a), ui, reg);
        let p = g.player.position();
        assert!((0..3).all(|i| (p[i] - in_a[i]).abs() < 0.01), "{p:?} vs {in_a:?}");
        assert!(!g.ui.settings.fog);
        drop(g);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn place_puts_the_hotbars_block_tap_attack_punches_and_held_attack_chops() {
        let reg = Arc::new(BlockRegistry::builtin());
        let mut ui = Ui::new();
        ui.settings.render_chunks = 2;
        ui.settings.walk = false; // flying: he stays up where he is put
        let mut g = Game::new(WorldSpec { dir: None, seed: 1337, settings_path: None }, ui, reg.clone());
        g.set_surface(2400, 1080, 2.75);
        let names: Vec<&str> = g.ui.hotbar.iter().map(|s| reg.def(s.id).name.as_str()).collect();
        assert_eq!(names, DEFAULT_HOTBAR);
        let wait = |g: &mut Game, what: &str, done: &dyn Fn(&Game) -> bool| {
            let t = Instant::now();
            while !done(g) {
                assert!(t.elapsed() < Duration::from_secs(20), "timed out waiting for {what}");
                g.update();
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        // Hover a little above the ground and look down at it.
        let (x, z) = bare_spot(&g);
        hover(&mut g, x, z);
        wait(&mut g, "the ground below", &|g| g.target().is_some());
        let hit = g.target().unwrap();
        let cell = hit.pos.neighbor(hit.face);
        assert!(!g.client.is_solid(cell));
        // PLACE with the hotbar on GLOWSTONE.
        let glow = g.ui.blocks.iter().position(|b| b.name == "Glowstone").unwrap();
        g.ui.apply(Action::Slot(3));
        g.ui.apply(Action::PickBlock(glow as u16));
        g.ui_touches.insert(1, Action::Hold("place"));
        g.ui_action(1, Action::Hold("place"));
        g.button_released(1);
        wait(&mut g, "the placed block", &|g| g.client.is_solid(cell));
        assert_eq!(g.target().map(|h| h.pos), Some(cell));
        // A quick tap on ATTACK: a punch, which breaks it.
        g.ui_touches.insert(2, Action::Hold("attack"));
        g.ui_action(2, Action::Hold("attack"));
        g.button_released(2);
        wait(&mut g, "the punch", &|g| !g.client.is_solid(cell));
        // Held: nothing for a moment, then a chop lands and breaks the ground, then another.
        let first = g.target().unwrap().pos;
        g.ui_touches.insert(3, Action::Hold("attack"));
        g.ui_action(3, Action::Hold("attack"));
        let t = Instant::now();
        wait(&mut g, "the first chop", &|g| !g.client.is_solid(first));
        assert!(t.elapsed() >= HEAVY_AFTER + CHOP_LANDS, "a chop takes its time: {:?}", t.elapsed());
        wait(&mut g, "the ground under it", &|g| g.target().is_some_and(|h| h.pos != first));
        let second = g.target().unwrap().pos;
        wait(&mut g, "the second chop", &|g| !g.client.is_solid(second));
        g.button_released(3);
        assert!(g.attack.is_none());
        // The hotbar is saved with the world's state.
        assert!(g.state_text().contains("hotbar=stone*64,dirt*64,grass*64,glowstone*64,"), "{}", g.state_text());
        // The inventory and the crafting grid are kept with it; older saves (names alone) read
        // as full stacks.
        g.ui.inventory[3] = Stack::of(reg.id("sand").unwrap(), 12);
        let text = g.state_text();
        assert!(text.contains("inventory=,,,sand*12,"), "{text}");
        let mut inv = [Stack::EMPTY; items::INV_SLOTS];
        saved_stacks(&text, "inventory", &reg, &g.ui.blocks, &mut inv);
        assert_eq!(inv, g.ui.inventory);
        assert_eq!(saved_hotbar("hotbar=stone,,dirt\n", &reg, &g.ui.blocks).0[..3], [Stack::of(reg.id("stone").unwrap(), 64), Stack::EMPTY, Stack::of(reg.id("dirt").unwrap(), 64)]);
        let (bar, slot) = saved_hotbar(&g.state_text(), &reg, &g.ui.blocks);
        assert_eq!((bar, slot), (g.ui.hotbar, 3));
    }

    /// He flies over (x, z), three blocks above the ground, looking down at it in first
    /// person.
    fn hover(g: &mut Game, x: i32, z: i32) {
        let ground = g.gen.describe(x, z).height;
        g.ui.settings.view = CamView::First;
        g.ui.settings.walk = false;
        g.player.place([x as f64 + 0.5, ground as f64 + 3.0, z as f64 + 0.5]);
        (g.cam.yaw, g.cam.pitch) = (0.0, WALK_PITCH.0);
    }

    /// A spot near the spawn with nothing growing within a few blocks (and flat ground), so
    /// a block placed there has open sky above it.
    fn bare_spot(g: &Game) -> (i32, i32) {
        let [x, _, z] = g.gen.find_spawn();
        let (x, z) = (x.floor() as i32, z.floor() as i32);
        for (dx, dz) in (6..120).flat_map(|r| [(r, 0), (0, r), (-r, 0), (0, -r), (r, r), (-r, -r)]) {
            let (px, pz) = (x + dx, z + dz);
            let h = g.gen.describe(px, pz).height.round();
            let flat = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().all(|&(a, b)| g.gen.describe(px + a, pz + b).height.round() == h);
            let col = g.gen.column(px.div_euclid(blockforge::coords::CHUNK_SIZE), pz.div_euclid(blockforge::coords::CHUNK_SIZE));
            let clear = col.features.iter().all(|f| {
                let (lo, hi) = f.bounds();
                px < lo[0] - 1 || px > hi[0] + 1 || pz < lo[2] - 1 || pz > hi[2] + 1
            });
            if flat && clear {
                return (px, pz);
            }
        }
        panic!("no bare spot near the spawn");
    }

    /// A world in memory around seed 1337's spawn, loaded, with the HUD laid out.
    fn loaded_game(walk: bool) -> Game {
        let reg = Arc::new(BlockRegistry::builtin());
        let mut ui = Ui::new();
        ui.screen = Screen::Playing;
        ui.settings.render_chunks = 2;
        ui.settings.walk = walk;
        let mut g = Game::new(WorldSpec { dir: None, seed: 1337, settings_path: None }, ui, reg);
        g.set_surface(2400, 1080, 2.75);
        g.ui.build(&crate::perf::Perf::new(), &mut Vec::new());
        g
    }

    fn run_until(g: &mut Game, what: &str, secs: f32, done: &dyn Fn(&Game) -> bool) {
        let t = Instant::now();
        while !done(g) {
            assert!(t.elapsed().as_secs_f32() < secs, "timed out waiting for {what}");
            g.update();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn run_for(g: &mut Game, secs: f32) {
        let t = Instant::now();
        while t.elapsed().as_secs_f32() < secs {
            g.update();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn building_by_touch_taps_to_place_and_holds_to_dig() {
        let mut g = loaded_game(false);
        g.ui.settings.touch_build = true;
        let (x, z) = bare_spot(&g);
        hover(&mut g, x, z);
        run_until(&mut g, "the ground below", 20.0, &|g| g.target().is_some());
        let hit = g.target().unwrap();
        let cell = hit.pos.neighbor(hit.face);
        // A tap on the world (the look side, away from every button) places the hotbar's block.
        let (tx, ty) = (1500.0, 420.0);
        assert!(g.ui.hit(tx, ty).is_none());
        g.touch(1, TouchPhase::Started, tx, ty);
        g.touch(1, TouchPhase::Ended, tx, ty);
        run_until(&mut g, "the placed block", 5.0, &|g| g.client.is_solid(cell));
        // A drag is a look: nothing is built.
        let yaw = g.cam.yaw;
        g.touch(2, TouchPhase::Started, tx, ty);
        g.touch(2, TouchPhase::Moved, tx + 60.0, ty);
        g.touch(2, TouchPhase::Ended, tx + 60.0, ty);
        assert!(g.cam.yaw != yaw);
        g.cam.yaw = yaw;
        run_for(&mut g, 0.3);
        assert_eq!(g.target().map(|h| h.pos), Some(cell), "the drag placed nothing on top");
        // Held still: after a moment it digs, and keeps digging.
        g.touch(3, TouchPhase::Started, tx, ty);
        run_for(&mut g, 0.2);
        assert!(g.client.is_solid(cell), "not yet");
        run_until(&mut g, "the dig", 5.0, &|g| !g.client.is_solid(cell));
        // ... with the mining swing, hands free of the block he carries.
        let swing = g.player.last.as_ref().and_then(|o| o.upper.clone()).map(|u| u.clip);
        assert_eq!(swing.as_deref(), Some("mine"));
        assert!(!g.player.carrying);
        run_until(&mut g, "the next block", 5.0, &|g| g.target().is_some_and(|h| h.pos != cell));
        let below = g.target().unwrap().pos;
        run_until(&mut g, "the second dig", 5.0, &|g| !g.client.is_solid(below));
        g.touch(3, TouchPhase::Ended, tx, ty);
        assert!(g.attack.is_none());
        // Off, a tap does nothing.
        g.ui.settings.touch_build = false;
        let next = g.target().unwrap();
        let above = next.pos.neighbor(next.face);
        g.touch(4, TouchPhase::Started, tx, ty);
        g.touch(4, TouchPhase::Ended, tx, ty);
        run_for(&mut g, 0.4);
        assert!(!g.client.is_solid(above));
    }

    #[test]
    fn he_flies_when_fly_is_picked_or_jump_is_tapped_twice_and_lands_on_the_ground() {
        let mut g = loaded_game(true);
        run_until(&mut g, "the ground under him", 20.0, &|g| g.ready(Duration::ZERO));
        run_for(&mut g, 1.0);
        let state = |g: &Game| g.player.last.as_ref().map(|o| o.state.clone()).unwrap_or_default();
        assert!(!g.player.ctl.is_flying());
        let y0 = g.player.position()[1];
        // Fly, in the settings: he takes off and stays up, the camera with him.
        g.ui.settings.walk = false;
        run_for(&mut g, 1.2);
        assert!(g.player.ctl.is_flying() && state(&g) == "fly");
        let y1 = g.player.position()[1];
        assert!(y1 > y0 + 0.5, "{y0} -> {y1}");
        assert!((g.cam.pos[1] as f64 - y1).abs() < 4.0);
        // The stick flies him along where he looks.
        let x0 = g.player.position();
        g.cam.pitch = 0.0;
        g.touch(1, TouchPhase::Started, 300.0, 700.0);
        g.touch(1, TouchPhase::Moved, 300.0, 500.0);
        run_for(&mut g, 0.8);
        g.touch(1, TouchPhase::Ended, 300.0, 500.0);
        let x1 = g.player.position();
        assert!((x1[0] - x0[0]).hypot(x1[2] - x0[2]) > 1.5, "{x0:?} -> {x1:?}");
        // Walk: he drops back to the ground.
        g.ui.settings.walk = true;
        run_until(&mut g, "the landing", 6.0, &|g| g.player.last.as_ref().is_some_and(|o| o.grounded));
        assert!(!g.player.ctl.is_flying());
        // Two quick taps on JUMP: he takes off, and the setting follows.
        for _ in 0..2 {
            g.held.insert(9, "jump");
            g.update();
            std::thread::sleep(Duration::from_millis(30));
            g.update();
            g.held.remove(&9);
            g.update();
            std::thread::sleep(Duration::from_millis(30));
        }
        g.update();
        assert!(g.player.ctl.is_flying() && !g.ui.settings.walk);
        // DOWN held sinks him onto the ground, which ends the flight; the setting follows.
        g.held.insert(9, "down");
        run_until(&mut g, "the landing", 8.0, &|g| !g.player.ctl.is_flying());
        g.held.remove(&9);
        g.update();
        assert!(g.ui.settings.walk);
    }

    #[test]
    fn the_hotbars_block_is_carried_in_both_hands_and_put_down_with_both() {
        let mut g = loaded_game(true);
        g.ui.settings.view = CamView::First;
        run_until(&mut g, "the ground under him", 20.0, &|g| g.ready(Duration::ZERO));
        run_for(&mut g, 1.0);
        // Stone in the hotbar's first slot: in his hands.
        assert_eq!(g.ui.held_block(), g.registry.id("stone").unwrap());
        assert_eq!(g.player.ctl.hold(), "two_handed_item");
        assert!(g.player.carrying);
        // An empty slot: empty hands.
        g.ui.apply(Action::ClearSlot);
        run_for(&mut g, 0.4);
        assert!(!g.player.carrying && g.player.ctl.hold() == "none");
        g.ui.apply(Action::Slot(1));
        run_until(&mut g, "the dirt in his hands", 2.0, &|g| g.player.carrying);
        // Look at the ground a little ahead.
        g.cam.pitch = -0.8;
        run_until(&mut g, "the ground ahead", 5.0, &|g| g.target().is_some_and(|h| !g.player.occupies(h.pos.neighbor(h.face))));
        // A punch puts it away for a moment; the jab is thrown with a free hand.
        g.ui_touches.insert(2, Action::Hold("attack"));
        g.ui_action(2, Action::Hold("attack"));
        g.button_released(2);
        g.update();
        let jab = g.player.last.as_ref().and_then(|o| o.upper.clone()).map(|u| u.clip);
        assert!(matches!(jab.as_deref(), Some("punch" | "punch_l")), "{jab:?}");
        run_for(&mut g, 0.25);
        assert!(!g.player.carrying);
        run_until(&mut g, "the block back in his hands", 2.0, &|g| g.player.carrying);
        // PLACE: both hands reach out with it.
        let hit = g.target().unwrap();
        let cell = hit.pos.neighbor(hit.face);
        g.ui_touches.insert(3, Action::Hold("place"));
        g.ui_action(3, Action::Hold("place"));
        g.button_released(3);
        g.update();
        let o = g.player.last.as_ref().unwrap();
        assert_eq!(o.upper.as_ref().map(|u| u.clip.as_str()), Some("interact"));
        assert_eq!(o.upper_left.as_ref().map(|u| u.clip.as_str()), Some("interact_l"));
        assert!(g.player.carrying, "the block stays in his hands as he puts it down");
        run_until(&mut g, "the placed block", 5.0, &|g| g.client.is_solid(cell));
    }

    #[test]
    fn one_stance_button_taps_to_crouch_holds_to_crawl_and_double_taps_to_roll() {
        let mut g = loaded_game(true);
        g.ui.settings.one_stance = true;
        run_until(&mut g, "the ground under him", 20.0, &|g| g.ready(Duration::ZERO));
        run_for(&mut g, 2.0);
        let press = |g: &mut Game, id: u64| {
            g.ui_touches.insert(id, Action::Hold("stance"));
            g.ui_action(id, Action::Hold("stance"));
        };
        // A tap: he crouches, once the moment for a second tap has passed.
        press(&mut g, 1);
        g.button_released(1);
        g.update();
        assert!(!g.player.ctl.crouching);
        run_until(&mut g, "the crouch", 2.0, &|g| g.player.ctl.crouching);
        // Another tap stands him up.
        press(&mut g, 2);
        g.button_released(2);
        run_until(&mut g, "standing", 2.0, &|g| !g.player.ctl.crouching);
        run_for(&mut g, 0.4);
        // A double tap: a roll, and no crouch.
        press(&mut g, 3);
        g.button_released(3);
        g.update();
        press(&mut g, 4);
        assert!(g.roll_until.is_some());
        g.update();
        assert!(g.player.roll);
        g.button_released(4);
        run_for(&mut g, 0.6);
        assert!(!g.player.ctl.crouching && g.crouch_at.is_none());
        run_for(&mut g, 1.0);
        // Held: he goes down to crawl, and letting go changes nothing.
        press(&mut g, 5);
        run_until(&mut g, "the crawl", 3.0, &|g| g.player.ctl.prone);
        g.button_released(5);
        run_for(&mut g, 0.6);
        assert!(g.player.ctl.prone && !g.player.ctl.crouching);
    }

    #[test]
    fn the_stick_appears_under_the_thumb_and_stays_put() {
        let reg = Arc::new(BlockRegistry::builtin());
        let mut ui = Ui::new();
        ui.screen = Screen::Playing;
        ui.settings.render_chunks = 2;
        let mut g = Game::new(WorldSpec { dir: None, seed: 7, settings_path: None }, ui, reg);
        g.set_surface(2400, 1080, 2.75);
        g.ui.build(&crate::perf::Perf::new(), &mut Vec::new());
        let r = g.stick_radius();
        assert!((r - 56.0 * 2.75).abs() < 0.01);
        g.update();
        assert_eq!(g.ui.joystick, None, "no stick until a thumb lands");
        g.touch(1, TouchPhase::Started, 500.0, 600.0);
        g.update();
        let j = g.ui.joystick.expect("the stick under the thumb");
        assert_eq!((j.base, j.knob, j.alpha), ([500.0, 600.0], [500.0, 600.0], 1.0));
        // Half way out: half speed, the ring stays.
        g.touch(1, TouchPhase::Moved, 500.0 + r * 0.5, 600.0);
        assert!((g.stick().0 - 0.5).abs() < 0.01);
        // Past the edge: full speed; the ring stays put, the knob stops at its edge.
        g.touch(1, TouchPhase::Moved, 500.0 + r * 3.0, 600.0);
        g.update();
        let j = g.ui.joystick.unwrap();
        assert!((g.stick().0 - 1.0).abs() < 0.01);
        assert!(j.base == [500.0, 600.0] && (j.knob[0] - (500.0 + r)).abs() < 0.5, "{j:?}");
        // Back inside: it follows the thumb again from the same spot.
        g.touch(1, TouchPhase::Moved, 500.0, 600.0 - r * 0.5);
        g.update();
        assert!(g.ui.joystick.unwrap().base == [500.0, 600.0] && (g.stick().1 - 0.5).abs() < 0.01);
        // Let go: it fades, then is gone; the character stops.
        g.touch(1, TouchPhase::Ended, 500.0 + r * 3.0, 600.0);
        g.update();
        assert!(g.ui.joystick.is_some_and(|j| j.alpha < 1.0));
        assert_eq!(g.stick(), (0.0, 0.0));
        std::thread::sleep(Duration::from_secs_f32(JOY_FADE + 0.05));
        g.update();
        assert_eq!(g.ui.joystick, None);
        // Near the edge the ring is kept on screen; on the right a drag looks around instead.
        g.touch(2, TouchPhase::Started, 10.0, 1070.0);
        g.update();
        let j = g.ui.joystick.unwrap();
        assert_eq!(j.base, [r, 1080.0 - r]);
        g.touch(2, TouchPhase::Ended, 10.0, 1070.0);
        let yaw = g.cam.yaw;
        g.touch(3, TouchPhase::Started, 1500.0, 400.0);
        g.touch(3, TouchPhase::Moved, 1600.0, 400.0);
        assert!(g.cam.yaw < yaw);
        assert_eq!(g.stick(), (0.0, 0.0));
    }

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
