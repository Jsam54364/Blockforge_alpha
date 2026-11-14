//! Immediate-mode HUD: stats overlay, crosshair, and a small settings menu.
//!
//! Everything is built each frame into a flat list of coloured triangles (`HudVertex`),
//! including text (see `font`). Touch hit-testing uses the button list from the same
//! pass, so what you see is exactly what you can tap. Sizes are in dp (density-
//! independent pixels) times the window's scale factor, so buttons stay finger-sized.

use crate::font;
use crate::perf::Perf;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct HudVertex {
    pub pos: [f32; 2],
    pub color: [u8; 4],
}

/// Multiple of 6 (two triangles per rectangle).
pub const MAX_HUD_VERTS: usize = 30_000;

pub const RENDER_MIN: u8 = 2;
pub const RENDER_MAX: u8 = 12;
/// LOD view distances in blocks, as menu steps; 0 = LODs off.
pub const LOD_DISTANCES: [u32; 7] = [0, 1024, 2048, 4096, 8192, 16384, 32768];
/// LOD quality steps: names and first-ring widths in blocks (see blockforge::lod).
pub const LOD_QUALITY: [(&str, f32); 3] = blockforge::lod::LodSettings::QUALITY;

/// Margins for when the system does not say where its bars are (an APK without the Java
/// activity, or before the first report): Android draws the status bar over the top edge and
/// the navigation bar over the right edge in landscape, about 24dp and 51dp on a real phone.
/// Normally the bars are hidden (immersive mode) and the HUD keeps clear of what the
/// activity reports instead (`Ui::set_safe_area`).
pub const SAFE_TOP_DP: f32 = 28.0;
pub const SAFE_RIGHT_DP: f32 = 56.0;

/// Which culling is active. Useful for measuring what each one buys on a real device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CullMode {
    /// Frustum, back faces and occlusion (chunks hidden behind terrain, `blockforge::cull`).
    All,
    /// Frustum and back faces, no occlusion.
    NoCaves,
    Frustum,
    Back,
    Off,
}

impl CullMode {
    pub fn frustum(self) -> bool {
        matches!(self, CullMode::All | CullMode::NoCaves | CullMode::Frustum)
    }

    pub fn backface(self) -> bool {
        matches!(self, CullMode::All | CullMode::NoCaves | CullMode::Back)
    }

    pub fn occlusion(self) -> bool {
        self == CullMode::All
    }

    pub fn next(self) -> CullMode {
        match self {
            CullMode::All => CullMode::NoCaves,
            CullMode::NoCaves => CullMode::Frustum,
            CullMode::Frustum => CullMode::Back,
            CullMode::Back => CullMode::Off,
            CullMode::Off => CullMode::All,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CullMode::All => "ALL",
            CullMode::NoCaves => "NOCAVE",
            CullMode::Frustum => "FRUST",
            CullMode::Back => "BACK",
            CullMode::Off => "OFF",
        }
    }

    fn from_label(s: &str) -> Option<CullMode> {
        [CullMode::All, CullMode::NoCaves, CullMode::Frustum, CullMode::Back, CullMode::Off].into_iter().find(|m| m.label() == s)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Chunks streamed and drawn around the player.
    pub render_chunks: u8,
    /// Index into `LOD_DISTANCES`: how far level-of-detail terrain reaches.
    pub lod_level: u8,
    /// Index into `LOD_QUALITY`.
    pub lod_quality: u8,
    pub fog: bool,
    pub cull: CullMode,
    /// true = FIFO (capped at the display refresh); false = uncapped, for measuring.
    pub vsync: bool,
    /// true = bit-mask greedy mesher for full opaque cubes; false = one quad per face.
    pub greedy: bool,
    pub show_stats: bool,
    pub clouds: bool,
    pub god_rays: bool,
    /// Walking as the character (true) or the free-flying camera.
    pub walk: bool,
    /// Where the camera is while walking.
    pub view: CamView,
}

/// The walking camera, cycled like Minecraft's F5: behind the character, in front of him
/// looking back, or in his head.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CamView {
    First,
    Back,
    Front,
}

impl CamView {
    pub fn next(self) -> CamView {
        match self {
            CamView::Back => CamView::Front,
            CamView::Front => CamView::First,
            CamView::First => CamView::Back,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CamView::First => "1ST",
            CamView::Back => "3RD",
            CamView::Front => "FRONT",
        }
    }

    fn key(self) -> &'static str {
        match self {
            CamView::First => "first",
            CamView::Back => "back",
            CamView::Front => "front",
        }
    }

    fn from_key(s: &str) -> Option<CamView> {
        [CamView::First, CamView::Back, CamView::Front].into_iter().find(|v| v.key() == s)
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            render_chunks: 6,
            lod_level: 5, // 16 km
            lod_quality: 1,
            fog: true,
            cull: CullMode::All,
            vsync: true,
            greedy: false,
            show_stats: true,
            clouds: true,
            god_rays: true,
            walk: true,
            view: CamView::Back,
        }
    }
}

impl Settings {
    /// `key=value` lines, one per setting, for the settings file kept between launches.
    pub fn to_text(&self) -> String {
        format!(
            "render_chunks={}\nlod_level={}\nlod_quality={}\nfog={}\ncull={}\nvsync={}\ngreedy={}\nshow_stats={}\nclouds={}\ngod_rays={}\nwalk={}\nview={}\n",
            self.render_chunks,
            self.lod_level,
            self.lod_quality,
            self.fog,
            self.cull.label(),
            self.vsync,
            self.greedy,
            self.show_stats,
            self.clouds,
            self.god_rays,
            self.walk,
            self.view.key()
        )
    }

    /// Reads what `to_text` wrote. Unknown keys and bad values are ignored and numbers are
    /// clamped, so a file from an older or newer version can never produce invalid settings.
    pub fn apply_text(&mut self, text: &str) {
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let (key, value) = (key.trim(), value.trim());
            match key {
                "render_chunks" => {
                    if let Ok(v) = value.parse::<u8>() {
                        self.render_chunks = v.clamp(RENDER_MIN, RENDER_MAX);
                    }
                }
                "view" => {
                    if let Some(v) = CamView::from_key(value) {
                        self.view = v;
                    }
                }
                "lod_level" => {
                    if let Ok(v) = value.parse::<u8>() {
                        self.lod_level = v.min(LOD_DISTANCES.len() as u8 - 1);
                    }
                }
                "lod_quality" => {
                    if let Ok(v) = value.parse::<u8>() {
                        self.lod_quality = v.min(LOD_QUALITY.len() as u8 - 1);
                    }
                }
                "cull" => {
                    if let Some(m) = CullMode::from_label(value) {
                        self.cull = m;
                    }
                }
                "fog" | "vsync" | "greedy" | "show_stats" | "clouds" | "god_rays" | "walk" | "third_person" => {
                    let Ok(v) = value.parse::<bool>() else { continue };
                    match key {
                        "fog" => self.fog = v,
                        "clouds" => self.clouds = v,
                        "walk" => self.walk = v,
                        // (what the view was saved as before there were three)
                        "third_person" => self.view = if v { CamView::Back } else { CamView::First },
                        "god_rays" => self.god_rays = v,
                        "vsync" => self.vsync = v,
                        "greedy" => self.greedy = v,
                        _ => self.show_stats = v,
                    }
                }
                _ => {}
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// A region that swallows touches (the menu panel) without doing anything.
    Nothing,
    /// Goes to a screen: the title screen and the pages it opens, or the game itself.
    Open(Screen),
    /// World list: picks a world (tapping the picked one again plays it).
    SelectWorld(usize),
    /// World list: scrolls by this many rows.
    ScrollWorlds(i8),
    PlayWorld,
    /// Opens the create page empty (`NewWorld`) or filled in from the picked world.
    NewWorld,
    RecreateWorld,
    EditWorld,
    AskDelete,
    ConfirmDelete,
    /// Create page: make the world and play it.
    CreateWorld,
    /// Edit page: keep the new name.
    SaveWorld,
    /// Text fields and the on-screen keyboard.
    Focus(u8),
    Key(char),
    Backspace,
    KeysDone,
    ToggleMenu,
    RenderDist(i8),
    LodDist(i8),
    CycleLodQuality,
    ToggleFog,
    CycleCull,
    ToggleVsync,
    ToggleMesher,
    ToggleStats,
    ToggleClouds,
    ToggleRays,
    /// Walk as the character, or fly the camera.
    ToggleWalk,
    /// Third person or first person.
    ToggleView,
    /// Opens and closes the grid of character actions.
    ToggleActs,
    /// A character button that works while held: "jump" or "roll" (the game handles it).
    Hold(&'static str),
    /// A character action, by the controller's name for it (the game handles it).
    Press(&'static str),
    /// Moves the time of day by this many hours (the game owns the clock and does it).
    Time(i8),
}

/// Where the player is: the title screen (Minecraft-style main menu over a turning
/// panorama), one of the pages it opens, or the game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Title,
    Settings,
    Multiplayer,
    Mods,
    Cosmetics,
    /// SINGLEPLAYER: the saved worlds.
    WorldList,
    CreateWorld,
    EditWorld,
    DeleteWorld,
    /// A world is opening (its terrain loading).
    Loading,
    Playing,
}

/// One saved world as the world list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldEntry {
    pub folder: String,
    pub name: String,
    /// The second line: when it was last played, and its seed.
    pub detail: String,
    /// The seed as typed back into the create page by RE-CREATE.
    pub seed: String,
}

/// What the app has to do for the world screens (they only draw and collect input).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Play(String),
    Create { name: String, seed: String },
    Rename { folder: String, name: String },
    Delete(String),
}

struct Button {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    action: Action,
}

pub struct Ui {
    pub settings: Settings,
    /// Which screen shows. The app starts on the title screen.
    pub screen: Screen,
    /// Seconds since the app started (animates the title screen's splash line).
    pub seconds: f32,
    /// Which of `title::SPLASHES` the title screen shows.
    pub splash: usize,
    pub menu_open: bool,
    /// Whether the grid of character actions is showing.
    pub acts_open: bool,
    /// False when the surface offers no uncapped present mode (the toggle then shows N/A).
    pub vsync_supported: bool,
    /// The time of day shown in the menu (the game keeps it up to date).
    pub hours: f64,
    size: (f32, f32),
    dp: f32,
    /// How far the system bars and cut-out reach into each edge (left, top, right, bottom),
    /// in pixels, as the system reports it; `None` = unknown (fixed margins, `SAFE_*_DP`).
    safe: Option<[f32; 4]>,
    buttons: Vec<Button>,
    /// The saved worlds (most recently played first), the picked one, and the list's scroll.
    pub worlds: Vec<WorldEntry>,
    pub selected: Option<usize>,
    scroll: usize,
    /// The world screens' text fields (name, seed) and which one the keyboard types into.
    pub fields: [String; 2],
    pub focus: Option<usize>,
    /// While a world loads: chunks with geometry so far.
    pub loading_chunks: usize,
    requests: Vec<Request>,
}

const WHITE: [u8; 4] = [255, 255, 255, 255];
const PANEL: [u8; 4] = [8, 10, 14, 205];
const BTN: [u8; 4] = [255, 255, 255, 48];
const XHAIR: [u8; 4] = [255, 255, 255, 215];

fn rect(out: &mut Vec<HudVertex>, x: f32, y: f32, w: f32, h: f32, c: [u8; 4]) {
    let (x1, y1) = (x + w, y + h);
    for p in [[x, y], [x1, y], [x1, y1], [x, y], [x1, y1], [x, y1]] {
        out.push(HudVertex { pos: p, color: c });
    }
}

pub fn text_width(px: f32, s: &str) -> f32 {
    let n = s.chars().count() as f32;
    if n == 0.0 {
        0.0
    } else {
        n * 6.0 * px - px
    }
}

/// `px` is the size of one font pixel on screen; a glyph is 5x7 of them.
fn text(out: &mut Vec<HudVertex>, x: f32, y: f32, px: f32, s: &str, c: [u8; 4]) {
    let mut cx = x;
    for ch in s.chars() {
        let rows = font::glyph(ch);
        for (r, row) in rows.iter().enumerate() {
            let bits = *row;
            let mut col = 0u32;
            while col < 5 {
                if bits & (0x10u8 >> col) != 0 {
                    let start = col;
                    while col < 5 && bits & (0x10u8 >> col) != 0 {
                        col += 1;
                    }
                    rect(out, cx + start as f32 * px, y + r as f32 * px, (col - start) as f32 * px, px, c);
                } else {
                    col += 1;
                }
            }
        }
        cx += 6.0 * px;
    }
}

fn text_centered(out: &mut Vec<HudVertex>, cx: f32, cy: f32, px: f32, s: &str, c: [u8; 4]) {
    let w = text_width(px, s);
    text(out, cx - w * 0.5, cy - 3.5 * px, px, s, c);
}

/// "OFF", "1K" ... "32K": the LOD distance in thousands of blocks.
pub fn lod_label(level: u8) -> String {
    match LOD_DISTANCES[(level as usize).min(LOD_DISTANCES.len() - 1)] {
        0 => "OFF".to_string(),
        d => format!("{}K", d / 1024),
    }
}

fn step(v: u8, d: i8, lo: u8, hi: u8) -> u8 {
    (v as i16 + d as i16).clamp(lo as i16, hi as i16) as u8
}

pub fn stats_lines(perf: &Perf) -> Vec<String> {
    let mut v = Vec::new();
    v.push(format!("FPS {:.0}  ({:.1}MS)", perf.fps, perf.frame_ms));
    match perf.cpu_cores {
        Some(c) => v.push(format!(
            "CPU {:.0}%  {:.1}/{} CORES",
            c / perf.n_cores.max(1) as f32 * 100.0,
            c,
            perf.n_cores
        )),
        None => v.push("CPU N/A".to_string()),
    }
    v.push(format!("CPU FRAME {:.1}MS", perf.cpu_frame_ms));
    if let Some(m) = perf.main_thread {
        v.push(format!("MAIN THREAD {:.0}%", m * 100.0));
    }
    match perf.gpu_ms {
        Some(g) => v.push(format!("GPU {:.1}MS  ({:.0}%)", g, (g * perf.fps / 10.0).min(999.0))),
        None => v.push("GPU N/A".to_string()),
    }
    if let Some(m) = perf.mem_mb {
        v.push(format!("MEM {:.0}MB", m));
    }
    v.push(format!("CHUNKS {}", perf.chunks));
    v.push(format!("DRAWN {}/{}  HIDDEN {}", perf.drawn, perf.meshes, perf.hidden));
    v.push(format!("QUADS {}", perf.quads));
    v.push(format!("VERTS {:.0}/{:.0}MB IN {} PAGES", perf.pool.2, perf.pool.1, perf.pool.0));
    v.push(format!("LOD {} SECT {}K QUADS", perf.lod_drawn, perf.lod_quads / 1000));
    if perf.lod_pending > 0 {
        v.push(format!("LOD BUILDING {}", perf.lod_pending));
    }
    v.push(format!("PRESENT {}", perf.present_mode));
    v.extend(perf.place.iter().cloned());
    v
}

impl Default for Ui {
    fn default() -> Self {
        Self::new()
    }
}

impl Ui {
    pub fn new() -> Ui {
        Ui {
            settings: Settings::default(),
            screen: Screen::Title,
            seconds: 0.0,
            splash: 0,
            menu_open: false,
            acts_open: false,
            vsync_supported: true,
            hours: crate::sky::START_HOUR,
            size: (1.0, 1.0),
            dp: 1.0,
            safe: None,
            buttons: Vec::new(),
            worlds: Vec::new(),
            selected: None,
            scroll: 0,
            fields: [String::new(), String::new()],
            focus: None,
            loading_chunks: 0,
            requests: Vec::new(),
        }
    }

    /// The next thing the app has to do for the world screens.
    pub fn take_request(&mut self) -> Option<Request> {
        (!self.requests.is_empty()).then(|| self.requests.remove(0))
    }

    /// Replaces the world list (after a world was made, renamed or deleted); keeps the
    /// pick on the same world if it is still there.
    pub fn set_worlds(&mut self, worlds: Vec<WorldEntry>) {
        let picked = self.selected.and_then(|i| self.worlds.get(i)).map(|w| w.folder.clone());
        self.worlds = worlds;
        self.selected = picked.and_then(|f| self.worlds.iter().position(|w| w.folder == f));
        self.scroll = self.scroll.min(self.worlds.len().saturating_sub(1));
    }

    /// Typing from a hardware keyboard: letters (as capitals), digits, space and `-`.
    pub fn type_text(&mut self, s: &str) {
        for c in s.chars() {
            match c {
                '\u{8}' | '\u{7f}' => {
                    self.apply(Action::Backspace);
                }
                '\r' | '\n' => {
                    self.apply(Action::KeysDone);
                }
                c => {
                    self.apply(Action::Key(c));
                }
            }
        }
    }

    fn picked(&self) -> Option<&WorldEntry> {
        self.selected.and_then(|i| self.worlds.get(i))
    }

    /// The safe area from the system (see `safe`), or `None` to keep the fixed margins.
    pub fn set_safe_area(&mut self, px: Option<[f32; 4]>) {
        self.safe = px;
    }

    /// Left, top, right and bottom margins to keep the HUD in, in pixels.
    fn margins(&self) -> [f32; 4] {
        self.safe.unwrap_or([0.0, SAFE_TOP_DP * self.dp, SAFE_RIGHT_DP * self.dp, 0.0])
    }

    pub fn set_surface(&mut self, w: u32, h: u32, scale_factor: f64) {
        self.size = (w.max(1) as f32, h.max(1) as f32);
        self.dp = (scale_factor as f32).max(1.0);
    }

    /// The action under a touch, using the layout from the most recent `build`.
    pub fn hit(&self, x: f32, y: f32) -> Option<Action> {
        self.buttons
            .iter()
            .rev()
            .find(|b| x >= b.x && x <= b.x + b.w && y >= b.y && y <= b.y + b.h)
            .map(|b| b.action)
    }

    /// Applies an action. Returns true if the render distance changed (the caller must
    /// tell the server).
    pub fn apply(&mut self, a: Action) -> bool {
        match a {
            Action::Nothing => false,
            Action::Open(s) => {
                self.screen = s;
                self.menu_open = false;
                self.acts_open = false;
                self.focus = None;
                false
            }
            Action::SelectWorld(i) => {
                if i < self.worlds.len() {
                    if self.selected == Some(i) {
                        self.apply(Action::PlayWorld);
                    } else {
                        self.selected = Some(i);
                    }
                }
                false
            }
            Action::ScrollWorlds(d) => {
                let max = self.worlds.len().saturating_sub(1);
                self.scroll = (self.scroll as i64 + d as i64).clamp(0, max as i64) as usize;
                false
            }
            Action::PlayWorld => {
                if let Some(w) = self.picked() {
                    self.requests.push(Request::Play(w.folder.clone()));
                }
                false
            }
            Action::NewWorld | Action::RecreateWorld => {
                self.fields = match (a, self.picked()) {
                    (Action::RecreateWorld, Some(w)) => [w.name.clone(), w.seed.clone()],
                    _ => ["NEW WORLD".to_string(), String::new()],
                };
                self.focus = None;
                self.screen = Screen::CreateWorld;
                false
            }
            Action::EditWorld => {
                if let Some(w) = self.picked() {
                    self.fields[0] = w.name.clone();
                    self.focus = None;
                    self.screen = Screen::EditWorld;
                }
                false
            }
            Action::AskDelete => {
                if self.picked().is_some() {
                    self.screen = Screen::DeleteWorld;
                }
                false
            }
            Action::ConfirmDelete => {
                if let Some(w) = self.picked() {
                    self.requests.push(Request::Delete(w.folder.clone()));
                }
                self.selected = None;
                self.screen = Screen::WorldList;
                false
            }
            Action::CreateWorld => {
                self.requests.push(Request::Create { name: self.fields[0].trim().to_string(), seed: self.fields[1].trim().to_string() });
                self.focus = None;
                false
            }
            Action::SaveWorld => {
                if let Some(w) = self.picked() {
                    self.requests.push(Request::Rename { folder: w.folder.clone(), name: self.fields[0].trim().to_string() });
                }
                self.focus = None;
                self.screen = Screen::WorldList;
                false
            }
            Action::Focus(i) => {
                self.focus = Some(i as usize % 2);
                false
            }
            Action::Key(c) => {
                if let Some(f) = self.focus {
                    let c = c.to_ascii_uppercase();
                    let max = if f == 0 { crate::worlds::MAX_NAME } else { crate::worlds::MAX_SEED };
                    if (c.is_ascii_alphanumeric() || c == ' ' || c == '-') && self.fields[f].chars().count() < max {
                        self.fields[f].push(c);
                    }
                }
                false
            }
            Action::Backspace => {
                if let Some(f) = self.focus {
                    self.fields[f].pop();
                }
                false
            }
            Action::KeysDone => {
                self.focus = None;
                false
            }
            Action::ToggleMenu => {
                self.menu_open = !self.menu_open;
                false
            }
            Action::RenderDist(d) => {
                let old = self.settings.render_chunks;
                self.settings.render_chunks = step(old, d, RENDER_MIN, RENDER_MAX);
                self.settings.render_chunks != old
            }
            Action::LodDist(d) => {
                self.settings.lod_level = step(self.settings.lod_level, d, 0, LOD_DISTANCES.len() as u8 - 1);
                false
            }
            Action::CycleLodQuality => {
                self.settings.lod_quality = (self.settings.lod_quality + 1) % LOD_QUALITY.len() as u8;
                false
            }
            Action::ToggleFog => {
                self.settings.fog = !self.settings.fog;
                false
            }
            Action::CycleCull => {
                self.settings.cull = self.settings.cull.next();
                false
            }
            Action::ToggleVsync => {
                self.settings.vsync = !self.settings.vsync;
                false
            }
            Action::ToggleMesher => {
                self.settings.greedy = !self.settings.greedy;
                false
            }
            Action::ToggleStats => {
                self.settings.show_stats = !self.settings.show_stats;
                false
            }
            Action::ToggleClouds => {
                self.settings.clouds = !self.settings.clouds;
                false
            }
            Action::ToggleRays => {
                self.settings.god_rays = !self.settings.god_rays;
                false
            }
            Action::ToggleWalk => {
                self.settings.walk = !self.settings.walk;
                false
            }
            Action::ToggleView => {
                self.settings.view = self.settings.view.next();
                false
            }
            Action::ToggleActs => {
                self.acts_open = !self.acts_open;
                false
            }
            Action::Time(_) | Action::Hold(_) | Action::Press(_) => false,
        }
    }

    /// Rebuilds the vertex list and the button list for this frame.
    pub fn build(&mut self, perf: &Perf, out: &mut Vec<HudVertex>) {
        out.clear();
        self.buttons.clear();
        if self.screen != Screen::Playing {
            self.draw_screen(out);
            out.truncate(MAX_HUD_VERTS);
            return;
        }
        let (w, h) = self.size;
        let dp = self.dp;

        // Crosshair
        let (cx, cy) = (w * 0.5, h * 0.5);
        let len = 10.0 * dp;
        let th = (1.5 * dp).max(2.0);
        rect(out, cx - len, cy - th * 0.5, len * 2.0, th, XHAIR);
        rect(out, cx - th * 0.5, cy - len, th, len * 2.0, XHAIR);

        let margin = 8.0 * dp;
        let [safe_left, safe_top, safe_right, _] = self.margins();
        if self.settings.show_stats {
            self.draw_stats(perf, out, safe_left + margin, safe_top + 4.0 * dp);
        }

        // Menu button (three bars), kept clear of the system bars
        let bs = 48.0 * dp;
        let (bx, by) = (w - safe_right - margin - bs, safe_top + 4.0 * dp);
        rect(out, bx, by, bs, bs, PANEL);
        for i in 0..3 {
            rect(out, bx + bs * 0.25, by + bs * 0.29 + bs * 0.17 * i as f32, bs * 0.5, bs * 0.07, WHITE);
        }
        self.buttons.push(Button { x: bx, y: by, w: bs, h: bs, action: Action::ToggleMenu });

        if self.menu_open {
            // Beside the menu button (not under it) so six rows still fit a 360dp-tall screen.
            self.draw_menu(out, bx - 4.0 * dp, by);
        } else if self.settings.walk {
            self.draw_character_buttons(out, bx, by, bs);
        }

        if out.len() > MAX_HUD_VERTS {
            out.truncate(MAX_HUD_VERTS);
        }
    }

    fn draw_stats(&self, perf: &Perf, out: &mut Vec<HudVertex>, x: f32, y: f32) {
        let px = (2.0 * self.dp).round().max(2.0);
        let lines = stats_lines(perf);
        let pad = 4.0 * self.dp;
        let line_h = 9.0 * px;
        let width = lines.iter().map(|l| text_width(px, l)).fold(0.0f32, f32::max);
        let height = line_h * lines.len() as f32 - 2.0 * px;
        rect(out, x, y, width + 2.0 * pad, height + 2.0 * pad, PANEL);
        for (i, l) in lines.iter().enumerate() {
            text(out, x + pad, y + pad + line_h * i as f32, px, l, WHITE);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn button(&mut self, out: &mut Vec<HudVertex>, x: f32, y: f32, w: f32, h: f32, label: &str, px: f32, action: Action) {
        if self.screen != Screen::Playing {
            // The settings page off the title screen: its menu's style, a little apart.
            let m = 2.0 * self.dp;
            self.stone_button(out, x + m, y + m, w - 2.0 * m, h - 2.0 * m, label, px, action);
            return;
        }
        rect(out, x, y, w, h, BTN);
        text_centered(out, x + w * 0.5, y + h * 0.5, px, label, WHITE);
        self.buttons.push(Button { x, y, w, h, action });
    }

    /// The character's controls while walking: jump, roll, crouch and crawl at the bottom
    /// right (under the right thumb), and an ACT button beside the menu button that opens a
    /// grid of everything else he can do.
    fn draw_character_buttons(&mut self, out: &mut Vec<HudVertex>, menu_x: f32, menu_y: f32, bs: f32) {
        let dp = self.dp;
        let (w, h) = self.size;
        let px = (2.0 * dp).round().max(2.0);
        let gap = 6.0 * dp;
        let right = menu_x + bs;
        // Bottom right: two rows of two.
        let (cw, ch) = (76.0 * dp, 52.0 * dp);
        let bottom = h - self.margins()[3] - 10.0 * dp;
        let pad: [[(&str, Action); 2]; 2] = [
            [("CROUCH", Action::Press("crouch")), ("CRAWL", Action::Press("crawl"))],
            [("ROLL", Action::Hold("roll")), ("JUMP", Action::Hold("jump"))],
        ];
        for (r, row) in pad.iter().enumerate() {
            for (c, (label, action)) in row.iter().enumerate() {
                let x = right - (2 - c) as f32 * cw - (1 - c) as f32 * gap;
                let y = bottom - (2 - r) as f32 * ch - (1 - r) as f32 * gap;
                self.button(out, x, y, cw, ch, label, px, *action);
            }
        }
        // ACT, left of the menu button.
        let ax = menu_x - gap - bs * 1.3;
        self.button(out, ax, menu_y, bs * 1.3, bs, "ACT", px, Action::ToggleActs);
        if !self.acts_open {
            return;
        }
        // Four columns: attacks, using things, states, and what is carried.
        let grid: [[(&str, Action); 4]; 4] = [
            [("PUNCH", Action::Press("punch")), ("USE", Action::Press("interact")), ("SIT", Action::Press("sit")), ("1 HAND", Action::Press("one_handed"))],
            [("SWING", Action::Press("swing")), ("USE L", Action::Press("interact_left")), ("WAVE", Action::Press("wave")), ("1 HAND L", Action::Press("one_handed_left"))],
            [("CHOP", Action::Press("chop")), ("EAT", Action::Press("eat")), ("DIE", Action::Press("die")), ("2 HANDS", Action::Press("two_handed"))],
            [("HIT", Action::Press("hit")), ("VIEW", Action::ToggleView), ("REVIVE", Action::Press("revive")), ("CARRY", Action::Press("two_handed_item"))],
        ];
        let top = menu_y + bs + gap;
        let space = (bottom - 2.0 * ch - 2.0 * gap) - top;
        let gh = (44.0 * dp).min((space - 3.0 * gap) / 4.0).max(30.0 * dp);
        let gw = (84.0 * dp).min((w - 16.0 * dp - 3.0 * gap) / 4.0);
        let left = right - 4.0 * gw - 3.0 * gap;
        rect(out, left - gap, top - gap, 4.0 * gw + 5.0 * gap, 4.0 * gh + 5.0 * gap, PANEL);
        self.buttons.push(Button { x: left - gap, y: top - gap, w: 4.0 * gw + 5.0 * gap, h: 4.0 * gh + 5.0 * gap, action: Action::Nothing });
        for (r, row) in grid.iter().enumerate() {
            for (c, (label, action)) in row.iter().enumerate() {
                let mut tpx = px;
                while tpx > 2.0 && text_width(tpx, label) > gw * 0.94 {
                    tpx -= 1.0;
                }
                self.button(out, left + c as f32 * (gw + gap), top + r as f32 * (gh + gap), gw, gh, label, tpx, *action);
            }
        }
    }

    fn draw_menu(&mut self, out: &mut Vec<HudVertex>, right: f32, top: f32) {
        let dp = self.dp;
        let px = (2.5 * dp).round().max(2.0);
        // Never wider than the space left of the right edge (matters on narrow screens).
        let panel_w = (376.0 * dp).min((right - 8.0 * dp).max(0.0));
        let pad = 8.0 * dp;
        let bw = 48.0 * dp;
        let gap = 6.0 * dp;
        let rows = 7usize; // three stepper rows + four rows of three toggle buttons
        // Rows shrink on short screens so the panel stays visible (buttons with them, a little
        // below finger size on the very shortest).
        let avail = self.size.1 - top - 8.0 * dp;
        let row_h = (56.0 * dp).min((avail - pad * 2.0) / rows as f32).max(40.0 * dp);
        let bh = bw.min(row_h);
        let panel_h = pad * 2.0 + row_h * rows as f32;
        let x0 = right - panel_w;

        rect(out, x0, top, panel_w, panel_h, PANEL);
        // Registered first so every button on top of it wins the hit test.
        self.buttons.push(Button { x: x0, y: top, w: panel_w, h: panel_h, action: Action::Nothing });

        let s = self.settings;
        let label_x = x0 + pad;
        let value_x = x0 + 104.0 * dp;
        let minus_x = x0 + panel_w - pad - 2.0 * bw - gap;
        let plus_x = x0 + panel_w - pad - bw;

        // Rows 0-2: label, value and -/+ buttons.
        for i in 0..3usize {
            let ry = top + pad + row_h * i as f32;
            let by = ry + (row_h - bh) * 0.5;
            let ty = ry + (row_h - 7.0 * px) * 0.5;
            if i == 0 {
                text(out, label_x, ty, px, "REND", WHITE);
                text(out, value_x, ty, px, &s.render_chunks.to_string(), WHITE);
                self.button(out, minus_x, by, bw, bh, "-", px, Action::RenderDist(-1));
                self.button(out, plus_x, by, bw, bh, "+", px, Action::RenderDist(1));
            } else if i == 1 {
                text(out, label_x, ty, px, "LOD", WHITE);
                text(out, value_x, ty, px, &lod_label(s.lod_level), WHITE);
                self.button(out, minus_x, by, bw, bh, "-", px, Action::LodDist(-1));
                self.button(out, plus_x, by, bw, bh, "+", px, Action::LodDist(1));
            } else if self.screen == Screen::Playing {
                text(out, label_x, ty, px, "TIME", WHITE);
                text(out, value_x, ty, px, &crate::sky::time_label(self.hours), WHITE);
                self.button(out, minus_x, by, bw, bh, "-", px, Action::Time(-1));
                self.button(out, plus_x, by, bw, bh, "+", px, Action::Time(1));
            } else {
                // Off the game: the time of day belongs to each world.
                text(out, label_x, ty, px, "TIME", WHITE);
                text(out, value_x, ty, px, "IN EACH WORLD", [190, 190, 190, 255]);
            }
        }

        // Rows 3-6: toggles in three columns; each button carries its own label.
        let on_off = |b: bool| if b { "ON" } else { "OFF" };
        let vsync_label = if !self.vsync_supported { "N/A" } else { on_off(s.vsync) };
        let toggles: [(Action, String); 11] = [
            (Action::ToggleFog, format!("FOG {}", on_off(s.fog))),
            (Action::CycleCull, format!("CULL {}", s.cull.label())),
            (Action::ToggleVsync, format!("VSYNC {}", vsync_label)),
            (Action::ToggleMesher, format!("MESH {}", if s.greedy { "GREEDY" } else { "SIMPLE" })),
            (Action::ToggleStats, format!("STATS {}", on_off(s.show_stats))),
            (Action::CycleLodQuality, format!("LODQ {}", LOD_QUALITY[s.lod_quality as usize % LOD_QUALITY.len()].0)),
            (Action::ToggleClouds, format!("CLOUDS {}", on_off(s.clouds))),
            (Action::ToggleRays, format!("RAYS {}", on_off(s.god_rays))),
            (Action::ToggleWalk, format!("MODE {}", if s.walk { "WALK" } else { "FLY" })),
            (Action::ToggleView, format!("VIEW {}", s.view.label())),
            // In the game it leads back to the title screen; there it would lead nowhere.
            if self.screen == Screen::Playing {
                (Action::Open(Screen::Title), "MAIN MENU".to_string())
            } else {
                (Action::Nothing, String::new())
            },
        ];
        let cell_w = (panel_w - pad * 4.0) / 3.0;
        for (i, (action, label)) in toggles.iter().enumerate() {
            if label.is_empty() {
                continue;
            }
            let row = 3 + i / 3;
            let col = i % 3;
            let x = x0 + pad + col as f32 * (cell_w + pad);
            let ry = top + pad + row_h * row as f32;
            let by = ry + (row_h - bh) * 0.5;
            // Shrink the text (in whole font pixels) if the label wouldn't fit its button.
            let mut tpx = px;
            while tpx > 2.0 && text_width(tpx, label) > cell_w * 0.94 {
                tpx -= 1.0;
            }
            self.button(out, x, by, cell_w, bh, label, tpx, *action);
        }
    }
}

// ---- The title screen and its pages ---------------------------------------------------

const STONE_FACE: [u8; 4] = [111, 111, 111, 245];
const STONE_LIGHT: [u8; 4] = [168, 168, 168, 245];
const STONE_DARK: [u8; 4] = [62, 62, 62, 245];
const OUTLINE: [u8; 4] = [0, 0, 0, 255];
const TEXT_LIGHT: [u8; 4] = [236, 236, 236, 255];
const TEXT_SHADOW: [u8; 4] = [56, 56, 56, 255];
const SPLASH_YELLOW: [u8; 4] = [255, 255, 0, 255];
const SPLASH_SHADOW: [u8; 4] = [63, 63, 0, 255];

/// Text with a shadow one font pixel down and right, centred on (cx, cy).
fn shadow_text_centered(out: &mut Vec<HudVertex>, cx: f32, cy: f32, px: f32, s: &str, c: [u8; 4], shadow: [u8; 4]) {
    text_centered(out, cx + px, cy + px, px, s, shadow);
    text_centered(out, cx, cy, px, s, c);
}

/// The largest whole font pixel (at most `px`, at least 2) at which `s` fits in `width`.
fn fit_px(px: f32, s: &str, width: f32) -> f32 {
    let mut p = px.round().max(2.0);
    while p > 2.0 && text_width(p, s) > width {
        p -= 1.0;
    }
    p
}

/// The heading and lines of the title screen's information pages.
fn page_text(s: Screen) -> (&'static str, &'static [&'static str]) {
    match s {
        Screen::Multiplayer => (
            "MULTIPLAYER",
            &["NO SERVERS YET.", "THE GAME ALREADY RUNS AS A CLIENT AND A SERVER,", "EVEN ON ITS OWN. PLAYING TOGETHER OVER", "WI-FI IS STILL TO COME."],
        ),
        Screen::Mods => (
            "MODS",
            &["NO MODS INSTALLED.", "BLOCKS COME FROM ASSETS/BLOCKS.TOML FOR NOW.", "MODS WILL ADD THEIR OWN BLOCKS, ITEMS", "AND BEHAVIOURS."],
        ),
        Screen::Cosmetics => ("COSMETICS", &["SKINS AND OUTFITS FOR YOUR CHARACTER.", "COMING SOON."]),
        _ => ("", &[]),
    }
}

impl Ui {
    /// A button in the style of Minecraft's menus: black outline, grey face with a light top
    /// edge and a dark bottom edge, light text with a shadow.
    #[allow(clippy::too_many_arguments)]
    fn stone_button(&mut self, out: &mut Vec<HudVertex>, x: f32, y: f32, w: f32, h: f32, label: &str, px: f32, action: Action) {
        let b = self.dp.round().max(2.0);
        rect(out, x, y, w, h, OUTLINE);
        rect(out, x + b, y + b, w - 2.0 * b, h - 2.0 * b, STONE_FACE);
        rect(out, x + b, y + b, w - 2.0 * b, b, STONE_LIGHT);
        rect(out, x + b, y + h - 3.0 * b, w - 2.0 * b, 2.0 * b, STONE_DARK);
        let px = fit_px(px, label, w * 0.9);
        shadow_text_centered(out, x + w * 0.5, y + h * 0.5 - b * 0.5, px, label, TEXT_LIGHT, TEXT_SHADOW);
        self.buttons.push(Button { x, y, w, h, action });
    }

    fn draw_screen(&mut self, out: &mut Vec<HudVertex>) {
        match self.screen {
            Screen::Title => self.draw_title(out),
            Screen::Settings => self.draw_settings_page(out),
            Screen::WorldList => self.draw_world_list(out),
            Screen::CreateWorld => self.draw_world_form(out, true),
            Screen::EditWorld => self.draw_world_form(out, false),
            Screen::DeleteWorld => self.draw_delete(out),
            Screen::Loading => self.draw_loading(out),
            Screen::Playing => {}
            s => {
                let (heading, lines) = page_text(s);
                self.draw_page(out, heading, lines);
            }
        }
    }

    /// The main menu: logo, a pulsing yellow splash line, and the buttons, over the
    /// panorama the renderer draws behind it.
    fn draw_title(&mut self, out: &mut Vec<HudVertex>) {
        let (w, h) = self.size;
        let dp = self.dp;
        let [sl, st, sr, sb] = self.margins();
        let (x0, x1) = (sl, w - sr);
        let cx = (x0 + x1) * 0.5;

        // Logo, with a dark extrusion under it.
        let logo = "BLOCKFORGE";
        let lpx = ((x1 - x0) * 0.62 / text_width(1.0, logo)).min(9.0 * dp).max(2.0).floor();
        let ly = st + 14.0 * dp;
        let depth = (lpx * 0.45).max(1.0);
        for k in (1..=3).rev() {
            text_centered(out, cx, ly + 3.5 * lpx + k as f32 * depth, lpx, logo, [40, 40, 40, 255]);
        }
        text_centered(out, cx, ly + 3.5 * lpx, lpx, logo, TEXT_LIGHT);
        let logo_bottom = ly + 7.0 * lpx + 3.0 * depth;

        // Buttons: three full width, then two side by side.
        let footer = 7.0 * (1.5 * dp).round().max(2.0) + 12.0 * dp;
        let gap = 6.0 * dp;
        let (top, bottom) = (logo_bottom + 18.0 * dp, h - sb - footer);
        let bh = ((bottom - top - 3.0 * gap) / 4.0).clamp(36.0 * dp, 48.0 * dp);
        let total = 4.0 * bh + 3.0 * gap;
        let by = top + ((bottom - top - total) * 0.5).max(0.0);
        let bw = (300.0 * dp).min((x1 - x0) * 0.8);
        let bx = cx - bw * 0.5;
        let px = (2.5 * dp).round().max(2.0);
        let rows: [(&str, Screen); 3] = [("SINGLEPLAYER", Screen::WorldList), ("MULTIPLAYER", Screen::Multiplayer), ("MODS", Screen::Mods)];
        for (i, (label, screen)) in rows.iter().enumerate() {
            self.stone_button(out, bx, by + i as f32 * (bh + gap), bw, bh, label, px, Action::Open(*screen));
        }
        let half_w = (bw - gap) * 0.5;
        let y3 = by + 3.0 * (bh + gap);
        self.stone_button(out, bx, y3, half_w, bh, "SETTINGS", px, Action::Open(Screen::Settings));
        self.stone_button(out, bx + half_w + gap, y3, half_w, bh, "COSMETICS", px, Action::Open(Screen::Cosmetics));

        // Splash: tilted and pulsing over the logo's lower right corner, as in Minecraft
        // (drawn last, on top; kept on screen and clear of the buttons).
        let splash = crate::title::SPLASHES[self.splash % crate::title::SPLASHES.len()];
        let spx = (lpx * 0.3).max(2.0);
        let pulse = 1.0 + 0.08 * (self.seconds * std::f32::consts::TAU).sin().abs();
        let (sin_a, cos_a) = (-0.35f32).sin_cos();
        let half = text_width(spx, splash) * 0.5 * pulse;
        let logo_right = cx + text_width(lpx, logo) * 0.5;
        let ax = (logo_right - 0.04 * text_width(lpx, logo)).min(x1 - half * cos_a - 6.0 * dp).max(bx + bw + half * cos_a * 0.0);
        let ay = ly + 6.0 * lpx;
        let mut tmp = Vec::new();
        text_centered(&mut tmp, spx, spx, spx, splash, SPLASH_SHADOW);
        text_centered(&mut tmp, 0.0, 0.0, spx, splash, SPLASH_YELLOW);
        for v in tmp {
            let (x, y) = (v.pos[0] * pulse, v.pos[1] * pulse);
            out.push(HudVertex { pos: [ax + x * cos_a - y * sin_a, ay + x * sin_a + y * cos_a], color: v.color });
        }

        // Footer.
        let fpx = (1.5 * dp).round().max(2.0);
        let version = format!("BLOCKFORGE ALPHA {}", env!("CARGO_PKG_VERSION"));
        text(out, x0 + 6.0 * dp + fpx, h - sb - 6.0 * dp - 7.0 * fpx + fpx, fpx, &version, TEXT_SHADOW);
        text(out, x0 + 6.0 * dp, h - sb - 6.0 * dp - 7.0 * fpx, fpx, &version, WHITE);
    }

    /// A page with a heading, a few lines in a dark panel, and DONE back to the title.
    fn draw_page(&mut self, out: &mut Vec<HudVertex>, heading: &str, lines: &[&str]) {
        let (w, h) = self.size;
        let dp = self.dp;
        let [sl, st, sr, sb] = self.margins();
        let (x0, x1) = (sl, w - sr);
        let cx = (x0 + x1) * 0.5;
        let hpx = fit_px(3.5 * dp, heading, (x1 - x0) * 0.9);
        let hy = st + 12.0 * dp;
        shadow_text_centered(out, cx, hy + 3.5 * hpx, hpx, heading, TEXT_LIGHT, TEXT_SHADOW);

        let bh = 48.0 * dp;
        let done_y = h - sb - 12.0 * dp - bh;
        let pw = (560.0 * dp).min((x1 - x0) * 0.92);
        let (py0, py1) = (hy + 7.0 * hpx + 12.0 * dp, done_y - 12.0 * dp);
        rect(out, cx - pw * 0.5, py0, pw, (py1 - py0).max(0.0), [0, 0, 0, 150]);
        let widest = lines.iter().copied().max_by(|a, b| text_width(1.0, a).total_cmp(&text_width(1.0, b))).unwrap_or("");
        let px = fit_px(2.5 * dp, widest, pw - 32.0 * dp);
        let line_h = 11.0 * px;
        let ty = (py0 + py1) * 0.5 - line_h * lines.len() as f32 * 0.5 + 2.0 * px;
        for (i, l) in lines.iter().enumerate() {
            text_centered(out, cx, ty + line_h * i as f32 + 3.5 * px, px, l, if i == 0 { TEXT_LIGHT } else { [200, 200, 200, 255] });
        }

        let bw = (200.0 * dp).min((x1 - x0) * 0.6);
        self.stone_button(out, cx - bw * 0.5, done_y, bw, bh, "DONE", (2.5 * dp).round(), Action::Open(Screen::Title));
    }

    /// A centred heading with a shadow; returns where the space under it starts.
    fn heading(&self, out: &mut Vec<HudVertex>, s: &str) -> f32 {
        let dp = self.dp;
        let [sl, st, sr, _] = self.margins();
        let cx = (sl + self.size.0 - sr) * 0.5;
        let px = fit_px(3.5 * dp, s, (self.size.0 - sl - sr) * 0.9);
        let y = st + 10.0 * dp;
        shadow_text_centered(out, cx, y + 3.5 * px, px, s, TEXT_LIGHT, TEXT_SHADOW);
        y + 7.0 * px + 10.0 * dp
    }

    /// A stone button that does nothing and looks it (no world picked yet).
    #[allow(clippy::too_many_arguments)]
    fn stone_button_if(&mut self, out: &mut Vec<HudVertex>, on: bool, x: f32, y: f32, w: f32, h: f32, label: &str, px: f32, action: Action) {
        if on {
            self.stone_button(out, x, y, w, h, label, px, action);
            return;
        }
        let b = self.dp.round().max(2.0);
        rect(out, x, y, w, h, OUTLINE);
        rect(out, x + b, y + b, w - 2.0 * b, h - 2.0 * b, [44, 44, 44, 235]);
        let px = fit_px(px, label, w * 0.9);
        text_centered(out, x + w * 0.5, y + h * 0.5 - b * 0.5, px, label, [140, 140, 140, 255]);
        self.buttons.push(Button { x, y, w, h, action: Action::Nothing });
    }

    /// A row of stone buttons sharing `width` (all on, or as `on` says).
    #[allow(clippy::too_many_arguments)]
    fn button_row(&mut self, out: &mut Vec<HudVertex>, cx: f32, y: f32, width: f32, h: f32, items: &[(&str, Action, bool)]) {
        let gap = 6.0 * self.dp;
        let n = items.len() as f32;
        let w = (width - gap * (n - 1.0)) / n;
        let px = (2.5 * self.dp).round().max(2.0);
        let x0 = cx - width * 0.5;
        for (i, (label, action, on)) in items.iter().enumerate() {
            self.stone_button_if(out, *on, x0 + i as f32 * (w + gap), y, w, h, label, px, *action);
        }
    }

    /// SINGLEPLAYER: the saved worlds, newest played first, and what to do with them.
    fn draw_world_list(&mut self, out: &mut Vec<HudVertex>) {
        let (w, h) = self.size;
        let dp = self.dp;
        let [sl, _, sr, sb] = self.margins();
        let cx = (sl + w - sr) * 0.5;
        let top = self.heading(out, "SELECT WORLD");

        let bh = 44.0 * dp;
        let gap = 6.0 * dp;
        let row2 = h - sb - 8.0 * dp - bh;
        let row1 = row2 - gap - bh;
        let lw = (600.0 * dp).min((w - sl - sr) * 0.86);
        let (lx, ly, lh) = (cx - lw * 0.5, top, (row1 - 10.0 * dp - top).max(0.0));
        rect(out, lx, ly, lw, lh, [0, 0, 0, 150]);
        self.buttons.push(Button { x: lx, y: ly, w: lw, h: lh, action: Action::Nothing });

        let row_h = 54.0 * dp;
        let fits = ((lh - 8.0 * dp) / row_h).floor().max(1.0) as usize;
        let n = self.worlds.len();
        self.scroll = self.scroll.min(n.saturating_sub(fits));
        if n == 0 {
            let px = fit_px(2.5 * dp, "NO WORLDS YET. CREATE ONE!", lw * 0.9);
            text_centered(out, cx, ly + lh * 0.5, px, "NO WORLDS YET. CREATE ONE!", [200, 200, 200, 255]);
        }
        let npx = (2.5 * dp).round().max(2.0);
        let dpx = (1.6 * dp).round().max(2.0);
        let scrollbar = n > fits;
        let row_w = lw - 8.0 * dp - if scrollbar { 52.0 * dp } else { 0.0 };
        for (k, i) in (self.scroll..n.min(self.scroll + fits)).enumerate() {
            let (rx, ry) = (lx + 4.0 * dp, ly + 4.0 * dp + k as f32 * row_h);
            let picked = self.selected == Some(i);
            if picked {
                let b = self.dp.round().max(2.0);
                rect(out, rx, ry, row_w, row_h - 4.0 * dp, [160, 160, 160, 255]);
                rect(out, rx + b, ry + b, row_w - 2.0 * b, row_h - 4.0 * dp - 2.0 * b, [12, 12, 12, 255]);
            }
            let e = &self.worlds[i];
            let name = e.name.clone();
            let detail = e.detail.clone();
            text(out, rx + 10.0 * dp, ry + 8.0 * dp, npx, &name, WHITE);
            text(out, rx + 10.0 * dp, ry + 8.0 * dp + 7.0 * npx + 6.0 * dp, dpx, &detail, [150, 150, 150, 255]);
            self.buttons.push(Button { x: rx, y: ry, w: row_w, h: row_h - 4.0 * dp, action: Action::SelectWorld(i) });
        }
        if scrollbar {
            let (sx, sw) = (lx + lw - 52.0 * dp, 48.0 * dp);
            let sh = ((lh - 12.0 * dp) * 0.5).min(48.0 * dp);
            let up_on = self.scroll > 0;
            let down_on = self.scroll + fits < n;
            self.stone_button_if(out, up_on, sx, ly + 4.0 * dp, sw, sh, "UP", (2.0 * dp).round(), Action::ScrollWorlds(-1));
            self.stone_button_if(out, down_on, sx, ly + lh - 4.0 * dp - sh, sw, sh, "DOWN", (2.0 * dp).round(), Action::ScrollWorlds(1));
        }

        let some = self.picked().is_some();
        let bw = (620.0 * dp).min((w - sl - sr) * 0.95);
        self.button_row(out, cx, row1, bw, bh, &[("PLAY SELECTED WORLD", Action::PlayWorld, some), ("CREATE NEW WORLD", Action::NewWorld, true)]);
        self.button_row(
            out,
            cx,
            row2,
            bw,
            bh,
            &[
                ("EDIT", Action::EditWorld, some),
                ("DELETE", Action::AskDelete, some),
                ("RE-CREATE", Action::RecreateWorld, some),
                ("CANCEL", Action::Open(Screen::Title), true),
            ],
        );
    }

    /// A text field: dark box, light outline (white while typing into it), its text with a
    /// cursor while focused, or a grey hint while empty.
    #[allow(clippy::too_many_arguments)]
    fn text_field(&mut self, out: &mut Vec<HudVertex>, x: f32, y: f32, w: f32, h: f32, field: usize, hint: &str) {
        let b = self.dp.round().max(2.0);
        let focused = self.focus == Some(field);
        rect(out, x, y, w, h, if focused { [255, 255, 255, 255] } else { [160, 160, 160, 255] });
        rect(out, x + b, y + b, w - 2.0 * b, h - 2.0 * b, [0, 0, 0, 255]);
        let px = (2.5 * self.dp).round().max(2.0);
        let ty = y + (h - 7.0 * px) * 0.5;
        let value = self.fields[field].clone();
        if value.is_empty() && !focused {
            let hp = fit_px(px, hint, w - 16.0 * self.dp);
            text(out, x + 8.0 * self.dp, y + (h - 7.0 * hp) * 0.5, hp, hint, [110, 110, 110, 255]);
        } else {
            let blink = focused && (self.seconds * 2.0).fract() < 0.5;
            let shown = if blink { format!("{value}_") } else { value };
            let fp = fit_px(px, &format!("{}_", self.fields[field]), w - 16.0 * self.dp);
            text(out, x + 8.0 * self.dp, y + (h - 7.0 * fp) * 0.5, fp, &shown, TEXT_LIGHT);
            let _ = ty;
        }
        self.buttons.push(Button { x, y, w, h, action: Action::Focus(field as u8) });
    }

    /// The create page (`create`) or the edit page: name (and seed) fields, and the keyboard
    /// while one of them is being typed into.
    fn draw_world_form(&mut self, out: &mut Vec<HudVertex>, create: bool) {
        let (w, h) = self.size;
        let dp = self.dp;
        let [sl, _, sr, sb] = self.margins();
        let cx = (sl + w - sr) * 0.5;
        let typing = self.focus.is_some();
        let top = self.heading(out, if create { "CREATE NEW WORLD" } else { "EDIT WORLD" });
        let fw = (420.0 * dp).min((w - sl - sr) * 0.9);
        let fx = cx - fw * 0.5;
        let lpx = (1.75 * dp).round().max(2.0);
        let fh = 38.0 * dp;
        let label_gap = 7.0 * lpx + 5.0 * dp;
        // With the keyboard up, the fields sit side by side above it if they don't fit stacked.
        let kb_top = h - sb - self.keyboard_height();
        let stacked = 2.0 * (label_gap + fh) + 8.0 * dp;
        let side_by_side = create && typing && top + stacked > kb_top - 4.0 * dp;
        let bh = 44.0 * dp;
        let by = h - sb - 10.0 * dp - bh;
        // Without the keyboard, the fields sit in the middle of the space above the buttons.
        let room = by - 10.0 * dp - top;
        let used = if create { stacked } else { stacked * 0.5 + 12.0 * dp };
        let mut y = if typing { top } else { top + ((room - used) * 0.5).max(0.0) };
        let fields: &[(&str, &str)] = if create {
            &[("WORLD NAME", "NEW WORLD"), ("SEED FOR THE WORLD GENERATOR", "LEAVE BLANK FOR A RANDOM SEED")]
        } else {
            &[("WORLD NAME", "NEW WORLD")]
        };
        let n = fields.len() as f32;
        let total = (fw * 2.0).min((w - sl - sr) * 0.95);
        let each = if side_by_side { (total - 12.0 * dp * (n - 1.0)) / n } else { fw };
        let x_start = if side_by_side { cx - total * 0.5 } else { fx };
        for (i, (label, hint)) in fields.iter().enumerate() {
            let (x, yy) = if side_by_side { (x_start + i as f32 * (each + 12.0 * dp), y) } else { (fx, y) };
            text(out, x, yy, lpx, label, [190, 190, 190, 255]);
            self.text_field(out, x, yy + label_gap, each, fh, i, hint);
            if !side_by_side {
                y += label_gap + fh + 8.0 * dp;
            }
        }
        if !create {
            if let Some(e) = self.picked() {
                let seed = format!("SEED {}", e.seed);
                text(out, fx, y + 2.0 * dp, lpx, &seed, [150, 150, 150, 255]);
            }
        }
        if typing {
            self.draw_keyboard(out);
            return;
        }
        let bw = (520.0 * dp).min((w - sl - sr) * 0.95);
        let named = !self.fields[0].trim().is_empty();
        if create {
            self.button_row(out, cx, by, bw, bh, &[("CREATE NEW WORLD", Action::CreateWorld, named), ("CANCEL", Action::Open(Screen::WorldList), true)]);
        } else {
            self.button_row(out, cx, by, bw, bh, &[("SAVE", Action::SaveWorld, named), ("CANCEL", Action::Open(Screen::WorldList), true)]);
        }
    }

    fn keyboard_height(&self) -> f32 {
        let dp = self.dp;
        5.0 * self.key_h() + 4.0 * 5.0 * dp + 12.0 * dp
    }

    fn key_h(&self) -> f32 {
        // Shorter keys on short screens, so the fields above still show.
        (36.0 * self.dp).min(self.size.1 * 0.085).max(28.0 * self.dp)
    }

    /// The on-screen keyboard: digits, letters, a dash, space, delete and done. (The system's
    /// keyboard doesn't reach a NativeActivity reliably, and the font has capitals only.)
    fn draw_keyboard(&mut self, out: &mut Vec<HudVertex>) {
        let (w, h) = self.size;
        let dp = self.dp;
        let [sl, _, sr, sb] = self.margins();
        let cx = (sl + w - sr) * 0.5;
        let kh = self.key_h();
        let gap = 5.0 * dp;
        let total_h = self.keyboard_height();
        let y0 = h - sb - total_h;
        let kw = ((w - sl - sr - 24.0 * dp - 9.0 * gap) / 10.0).min(64.0 * dp);
        let width = 10.0 * kw + 9.0 * gap;
        rect(out, cx - width * 0.5 - 6.0 * dp, y0, width + 12.0 * dp, total_h, [0, 0, 0, 170]);
        self.buttons.push(Button { x: cx - width * 0.5 - 6.0 * dp, y: y0, w: width + 12.0 * dp, h: total_h, action: Action::Nothing });
        let px = (2.25 * dp).round().max(2.0);
        let rows = ["1234567890", "QWERTYUIOP", "ASDFGHJKL", "ZXCVBNM-"];
        for (r, keys) in rows.iter().enumerate() {
            let n = keys.chars().count() as f32;
            let rw = n * kw + (n - 1.0) * gap;
            let ry = y0 + 6.0 * dp + r as f32 * (kh + gap);
            for (i, c) in keys.chars().enumerate() {
                let x = cx - rw * 0.5 + i as f32 * (kw + gap);
                self.stone_button(out, x, ry, kw, kh, &c.to_string(), px, Action::Key(c));
            }
        }
        let ry = y0 + 6.0 * dp + 4.0 * (kh + gap);
        let x0 = cx - width * 0.5;
        let unit = kw + gap;
        self.stone_button(out, x0, ry, 2.0 * unit - gap, kh, "DEL", px, Action::Backspace);
        self.stone_button(out, x0 + 2.0 * unit, ry, 6.0 * unit - gap, kh, "SPACE", px, Action::Key(' '));
        self.stone_button(out, x0 + 8.0 * unit, ry, 2.0 * unit - gap, kh, "DONE", px, Action::KeysDone);
    }

    fn draw_delete(&mut self, out: &mut Vec<HudVertex>) {
        let (w, h) = self.size;
        let dp = self.dp;
        let [sl, _, sr, sb] = self.margins();
        let cx = (sl + w - sr) * 0.5;
        rect(out, 0.0, 0.0, w, h, [60, 0, 0, 130]);
        let top = self.heading(out, "DELETE WORLD?");
        let name = self.picked().map(|e| e.name.clone()).unwrap_or_default();
        let lines = ["ARE YOU SURE YOU WANT TO DELETE THIS WORLD?".to_string(), format!("'{name}' WILL BE LOST FOREVER!"), "(A LONG TIME!)".to_string()];
        let px = fit_px(2.5 * dp, &lines[0], (w - sl - sr) * 0.9);
        for (i, l) in lines.iter().enumerate() {
            text_centered(out, cx, top + 20.0 * dp + i as f32 * 11.0 * px, px, l, if i == 1 { TEXT_LIGHT } else { [210, 210, 210, 255] });
        }
        let bh = 44.0 * dp;
        let bw = (420.0 * dp).min((w - sl - sr) * 0.95);
        self.button_row(out, cx, h - sb - 10.0 * dp - bh, bw, bh, &[("DELETE", Action::ConfirmDelete, true), ("CANCEL", Action::Open(Screen::WorldList), true)]);
    }

    fn draw_loading(&mut self, out: &mut Vec<HudVertex>) {
        let (w, h) = self.size;
        let dp = self.dp;
        let [sl, _, sr, _] = self.margins();
        let cx = (sl + w - sr) * 0.5;
        let px = (3.0 * dp).round().max(2.0);
        shadow_text_centered(out, cx, h * 0.42, px, "LOADING TERRAIN", TEXT_LIGHT, TEXT_SHADOW);
        // A bar that fills as the first chunks get their geometry (the world opens once the
        // player has ground under him, usually before it is full).
        let bw = (300.0 * dp).min((w - sl - sr) * 0.8);
        let bh = 6.0 * dp;
        let by = h * 0.42 + 7.0 * px + 4.0 * dp;
        let f = (self.loading_chunks as f32 / 120.0).min(1.0);
        rect(out, cx - bw * 0.5, by, bw, bh, [40, 40, 40, 220]);
        rect(out, cx - bw * 0.5, by, bw * f, bh, [128, 255, 128, 255]);
        let n = format!("{} CHUNKS", self.loading_chunks);
        let spx = (1.75 * dp).round().max(2.0);
        text_centered(out, cx, by + bh + 8.0 * dp + 3.5 * spx, spx, &n, [190, 190, 190, 255]);
    }

    /// The settings page: the game's settings panel, with a heading and DONE beside it.
    fn draw_settings_page(&mut self, out: &mut Vec<HudVertex>) {
        let (w, h) = self.size;
        let dp = self.dp;
        let [sl, st, sr, sb] = self.margins();
        let (x0, x1) = (sl + 8.0 * dp, w - sr - 8.0 * dp);
        let side = 180.0 * dp;
        let gap = 16.0 * dp;
        let panel = (376.0 * dp).min(x1 - x0 - side - gap);
        let left = (((x0 + x1) - (side + gap + panel)) * 0.5).max(x0);
        let hpx = fit_px(3.5 * dp, "SETTINGS", side);
        let hy = st + 12.0 * dp;
        shadow_text_centered(out, left + side * 0.5, hy + 3.5 * hpx, hpx, "SETTINGS", TEXT_LIGHT, TEXT_SHADOW);
        let bh = 48.0 * dp;
        self.stone_button(out, left, h - sb - 12.0 * dp - bh, side, bh, "DONE", (2.5 * dp).round(), Action::Open(Screen::Title));
        self.draw_menu(out, left + side + gap + panel, st + 8.0 * dp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In the game (the HUD tests); `title()` for the title screen.
    fn ui() -> Ui {
        let mut u = Ui::new();
        u.set_surface(2400, 1080, 2.75);
        u.screen = Screen::Playing;
        u
    }

    fn title() -> Ui {
        let mut u = Ui::new();
        u.set_surface(2400, 1080, 2.75);
        assert_eq!(u.screen, Screen::Title, "the app opens on the title screen");
        u
    }

    fn find(u: &Ui, a: Action) -> &Button {
        u.buttons.iter().find(|b| b.action == a).unwrap_or_else(|| panic!("no {a:?} button"))
    }

    #[test]
    fn the_title_screen_has_its_five_buttons_and_each_page_leads_back() {
        for (w, h) in [(2400, 1080), (1920, 1080), (2800, 1260), (1080, 2400)] {
            let mut u = title();
            u.set_surface(w, h, 2.75);
            for splash in 0..crate::title::SPLASHES.len() {
                u.splash = splash;
                u.seconds = splash as f32 * 0.37;
                let out = build(&mut u);
                // Everything drawn is on screen (the logo and the turning splash too).
                for v in &out {
                    assert!(v.pos[0] >= -0.5 && v.pos[0] <= w as f32 + 0.5 && v.pos[1] >= -0.5 && v.pos[1] <= h as f32 + 0.5, "{:?} off a {w}x{h} screen", v.pos);
                }
            }
            let targets = [Screen::WorldList, Screen::Multiplayer, Screen::Mods, Screen::Settings, Screen::Cosmetics];
            for s in targets {
                let b = find(&u, Action::Open(s));
                assert!(b.h >= 36.0 * 2.75 - 0.5 && b.w >= 100.0 * 2.75, "{s:?}: {}x{}", b.w, b.h);
            }
            // No two buttons overlap.
            for (i, a) in u.buttons.iter().enumerate() {
                for b in &u.buttons[i + 1..] {
                    assert!(a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y);
                }
            }
            for s in targets {
                let mut p = title();
                p.set_surface(w, h, 2.75);
                build(&mut p);
                let b = find(&p, Action::Open(s));
                let (x, y) = (b.x + b.w * 0.5, b.y + b.h * 0.5);
                let a = p.hit(x, y).unwrap();
                p.apply(a);
                assert_eq!(p.screen, s);
                build(&mut p);
                let done = find(&p, Action::Open(Screen::Title));
                assert!(done.y + done.h <= h as f32 && done.x >= 0.0 && done.x + done.w <= w as f32);
                p.apply(Action::Open(Screen::Title));
                assert_eq!(p.screen, Screen::Title);
            }
        }
    }

    fn worlds(n: usize) -> Vec<WorldEntry> {
        (0..n)
            .map(|i| WorldEntry { folder: format!("W{i}"), name: format!("WORLD {i}"), detail: format!("14 NOV 2026   SEED {i}"), seed: i.to_string() })
            .collect()
    }

    /// Taps the middle of the first button doing `a`.
    fn tap(u: &mut Ui, a: Action) {
        let b = find(u, a);
        let (x, y) = (b.x + b.w * 0.5, b.y + b.h * 0.5);
        let hit = u.hit(x, y).unwrap();
        assert_eq!(hit, a, "something else is on top of {a:?}");
        u.apply(hit);
        build(u);
    }

    /// Every real button on screen and none on top of another (the panels behind them
    /// do nothing and are meant to be under them).
    fn check_layout(u: &Ui, w: f32, h: f32) {
        let real: Vec<&Button> = u.buttons.iter().filter(|b| b.action != Action::Nothing).collect();
        for b in &real {
            assert!(b.x >= -0.5 && b.y >= -0.5 && b.x + b.w <= w + 0.5 && b.y + b.h <= h + 0.5, "{:?} off screen on {:?}", b.action, u.screen);
        }
        for (i, a) in real.iter().enumerate() {
            for b in &real[i + 1..] {
                let apart = a.x + a.w <= b.x + 0.5 || b.x + b.w <= a.x + 0.5 || a.y + a.h <= b.y + 0.5 || b.y + b.h <= a.y + 0.5;
                assert!(apart, "{:?} overlaps {:?} on {:?}", a.action, b.action, u.screen);
            }
        }
    }

    #[test]
    fn picking_playing_editing_and_deleting_worlds() {
        let mut u = title();
        u.set_worlds(worlds(7));
        build(&mut u);
        tap(&mut u, Action::Open(Screen::WorldList));
        check_layout(&u, 2400.0, 1080.0);
        // Nothing picked: PLAY, EDIT, DELETE and RE-CREATE do nothing.
        assert!(u.buttons.iter().all(|b| !matches!(b.action, Action::PlayWorld | Action::EditWorld | Action::AskDelete | Action::RecreateWorld)));
        // Seven worlds don't all fit: the list scrolls.
        assert!(u.buttons.iter().all(|b| b.action != Action::SelectWorld(6)));
        let rows = u.buttons.iter().filter(|b| matches!(b.action, Action::SelectWorld(_))).count();
        assert!(rows >= 3, "{rows} rows show");
        for _ in 0..7 - rows {
            tap(&mut u, Action::ScrollWorlds(1));
        }
        find(&u, Action::SelectWorld(6));
        // At the end, DOWN does nothing more.
        assert!(u.buttons.iter().all(|b| b.action != Action::ScrollWorlds(1)));
        // Pick one, then tap it again to play it.
        tap(&mut u, Action::SelectWorld(5));
        assert_eq!(u.selected, Some(5));
        assert_eq!(u.take_request(), None);
        tap(&mut u, Action::SelectWorld(5));
        assert_eq!(u.take_request(), Some(Request::Play("W5".into())));
        tap(&mut u, Action::PlayWorld);
        assert_eq!(u.take_request(), Some(Request::Play("W5".into())));
        // Rename it with the on-screen keyboard.
        tap(&mut u, Action::EditWorld);
        assert_eq!((u.screen, u.fields[0].as_str()), (Screen::EditWorld, "WORLD 5"));
        tap(&mut u, Action::Focus(0));
        check_layout(&u, 2400.0, 1080.0);
        for _ in 0..2 {
            tap(&mut u, Action::Backspace);
        }
        for c in "ISLE 2".chars() {
            tap(&mut u, Action::Key(c));
        }
        tap(&mut u, Action::KeysDone);
        tap(&mut u, Action::SaveWorld);
        assert_eq!(u.take_request(), Some(Request::Rename { folder: "W5".into(), name: "WORLDISLE 2".into() }));
        assert_eq!(u.screen, Screen::WorldList);
        // Delete it, after saying yes.
        tap(&mut u, Action::AskDelete);
        assert_eq!(u.screen, Screen::DeleteWorld);
        check_layout(&u, 2400.0, 1080.0);
        tap(&mut u, Action::Open(Screen::WorldList));
        assert_eq!(u.take_request(), None, "cancel deletes nothing");
        tap(&mut u, Action::AskDelete);
        tap(&mut u, Action::ConfirmDelete);
        assert_eq!(u.take_request(), Some(Request::Delete("W5".into())));
        assert_eq!((u.screen, u.selected), (Screen::WorldList, None));
        // The list comes back without it; nothing is picked.
        let mut left = worlds(7);
        left.remove(5);
        u.set_worlds(left);
        build(&mut u);
        check_layout(&u, 2400.0, 1080.0);
    }

    #[test]
    fn creating_a_world_with_a_name_and_a_seed() {
        for (w, h) in [(2400, 1080), (1920, 1080), (2800, 1260), (1080, 2400), (2340, 1080)] {
            let mut u = title();
            u.set_surface(w, h, 2.75);
            u.set_worlds(worlds(1));
            build(&mut u);
            tap(&mut u, Action::Open(Screen::WorldList));
            tap(&mut u, Action::NewWorld);
            assert_eq!((u.screen, u.fields[0].as_str(), u.fields[1].as_str()), (Screen::CreateWorld, "NEW WORLD", ""));
            check_layout(&u, w as f32, h as f32);
            // Typing into the name: the keyboard comes up, the field stays visible above it.
            tap(&mut u, Action::Focus(0));
            check_layout(&u, w as f32, h as f32);
            let field = |u: &Ui, i: u8| u.buttons.iter().find(|b| b.action == Action::Focus(i)).map(|b| (b.y, b.h)).unwrap();
            let q = find(&u, Action::Key('Q'));
            let q = q.y;
            assert!(field(&u, 0).0 + field(&u, 0).1 <= q, "{w}x{h}: the name field is under the keyboard");
            assert!(field(&u, 1).0 + field(&u, 1).1 <= q, "{w}x{h}: the seed field is under the keyboard");
            for _ in 0.."NEW WORLD".len() {
                tap(&mut u, Action::Backspace);
            }
            tap(&mut u, Action::Backspace); // on an empty field: nothing
            for c in "GLACIER".chars() {
                tap(&mut u, Action::Key(c));
            }
            tap(&mut u, Action::Focus(1));
            for c in "-42".chars() {
                tap(&mut u, Action::Key(c));
            }
            tap(&mut u, Action::KeysDone);
            assert!(u.buttons.iter().all(|b| !matches!(b.action, Action::Key(_))), "the keyboard went away");
            tap(&mut u, Action::CreateWorld);
            assert_eq!(u.take_request(), Some(Request::Create { name: "GLACIER".into(), seed: "-42".into() }));
        }
        // RE-CREATE starts from the picked world's name and seed; an empty name can't be made.
        let mut u = title();
        u.set_worlds(worlds(3));
        u.apply(Action::Open(Screen::WorldList));
        u.apply(Action::SelectWorld(2));
        u.apply(Action::RecreateWorld);
        assert_eq!((u.fields[0].as_str(), u.fields[1].as_str()), ("WORLD 2", "2"));
        u.fields[0].clear();
        build(&mut u);
        assert!(u.buttons.iter().all(|b| b.action != Action::CreateWorld));
        // Names and seeds have a length limit, and only letters, digits, space and '-'.
        u.apply(Action::Focus(0));
        for _ in 0..40 {
            u.type_text("ab.");
        }
        assert_eq!(u.fields[0].len(), crate::worlds::MAX_NAME);
        assert!(u.fields[0].chars().all(|c| c == 'A' || c == 'B'));
    }

    #[test]
    fn the_loading_screen_shows_progress() {
        let mut u = title();
        u.screen = Screen::Loading;
        u.loading_chunks = 60;
        let out = build(&mut u);
        assert!(!out.is_empty());
        assert!(u.buttons.is_empty(), "nothing to press while a world loads");
    }

    #[test]
    fn the_settings_page_is_the_games_settings_panel() {
        let mut u = title();
        u.apply(Action::Open(Screen::Settings));
        build(&mut u);
        // (no TIME buttons: the time of day belongs to each world)
        assert!(u.buttons.iter().all(|b| !matches!(b.action, Action::Time(_))));
        for a in [Action::ToggleFog, Action::RenderDist(1), Action::ToggleView] {
            let b = find(&u, a);
            assert!(b.x >= 0.0 && b.x + b.w <= 2400.0 && b.y + b.h <= 1080.0, "{a:?}");
        }
        assert!(u.apply(Action::RenderDist(1)) || u.settings.render_chunks == RENDER_MAX);
        // No way back to the title from inside its own page except DONE.
        assert_eq!(u.buttons.iter().filter(|b| b.action == Action::Open(Screen::Title)).count(), 1);
    }

    #[test]
    fn the_game_menu_leads_back_to_the_title_screen() {
        let mut u = ui();
        u.menu_open = true;
        build(&mut u);
        let b = find(&u, Action::Open(Screen::Title));
        let a = u.hit(b.x + 1.0, b.y + 1.0).unwrap();
        u.apply(a);
        assert_eq!(u.screen, Screen::Title);
        assert!(!u.menu_open, "the menu closes with it");
    }

    #[test]
    fn every_title_screen_text_can_be_drawn() {
        for s in [Screen::Multiplayer, Screen::Mods, Screen::Cosmetics] {
            let (heading, lines) = page_text(s);
            for t in std::iter::once(&heading).chain(lines.iter()) {
                assert!(t.chars().all(font::has_art), "{t}");
            }
        }
        for t in [
            "BLOCKFORGE",
            "SINGLEPLAYER",
            "MULTIPLAYER",
            "MODS",
            "SETTINGS",
            "COSMETICS",
            "DONE",
            "MAIN MENU",
            "BLOCKFORGE ALPHA 0.1.0",
            "SELECT WORLD",
            "PLAY SELECTED WORLD",
            "CREATE NEW WORLD",
            "EDIT WORLD",
            "RE-CREATE",
            "NO WORLDS YET. CREATE ONE!",
            "SEED FOR THE WORLD GENERATOR",
            "LEAVE BLANK FOR A RANDOM SEED",
            "ARE YOU SURE YOU WANT TO DELETE THIS WORLD?",
            "'X' WILL BE LOST FOREVER!",
            "(A LONG TIME!)",
            "LOADING TERRAIN",
            "IN EACH WORLD",
            "QWERTYUIOPASDFGHJKLZXCVBNM-1234567890 DEL SPACE",
        ] {
            assert!(t.chars().all(font::has_art), "{t}");
        }
    }

    fn build(u: &mut Ui) -> Vec<HudVertex> {
        let mut out = Vec::new();
        u.build(&Perf::new(), &mut out);
        out
    }

    fn center_of(u: &Ui, a: Action) -> (f32, f32) {
        let b = u.buttons.iter().rev().find(|b| b.action == a).expect("button present");
        (b.x + b.w * 0.5, b.y + b.h * 0.5)
    }

    #[test]
    fn settings_survive_a_round_trip_through_text() {
        let s = Settings {
            render_chunks: 9,
            lod_level: 3,
            lod_quality: 2,
            fog: false,
            cull: CullMode::Frustum,
            vsync: false,
            greedy: true,
            show_stats: false,
            clouds: false,
            god_rays: false,
            walk: false,
            view: CamView::Front,
        };
        let mut back = Settings::default();
        back.apply_text(&s.to_text());
        assert_eq!(back, s);
    }

    #[test]
    fn bad_settings_text_is_ignored_or_clamped() {
        let mut s = Settings::default();
        s.apply_text("render_chunks=200\nlod_level=banana\nlod_quality=99\nfog=maybe\ncull=SIDEWAYS\nno equals sign\nfuture_key=1\nlod_chunks=16\n");
        assert_eq!(s, Settings { render_chunks: RENDER_MAX, lod_quality: LOD_QUALITY.len() as u8 - 1, ..Settings::default() });
        s.apply_text("lod_level=200");
        assert_eq!(s.lod_level, LOD_DISTANCES.len() as u8 - 1);
        s.apply_text("render_chunks=0");
        assert_eq!(s.render_chunks, RENDER_MIN);
    }

    #[test]
    fn vertex_count_is_whole_rectangles_and_within_budget() {
        let mut u = ui();
        u.menu_open = true;
        let out = build(&mut u);
        assert_eq!(out.len() % 6, 0);
        assert!(out.len() > 100 && out.len() <= MAX_HUD_VERTS, "{}", out.len());
    }

    #[test]
    fn menu_button_opens_the_menu_and_reveals_the_controls() {
        let mut u = ui();
        build(&mut u);
        assert!(u.buttons.iter().all(|b| b.action != Action::RenderDist(1)));
        let (x, y) = center_of(&u, Action::ToggleMenu);
        assert_eq!(u.hit(x, y), Some(Action::ToggleMenu));
        u.apply(Action::ToggleMenu);
        build(&mut u);
        for a in [
            Action::RenderDist(-1),
            Action::RenderDist(1),
            Action::LodDist(-1),
            Action::LodDist(1),
            Action::Time(-1),
            Action::Time(1),
            Action::ToggleFog,
            Action::CycleCull,
            Action::ToggleVsync,
            Action::ToggleMesher,
            Action::ToggleStats,
            Action::CycleLodQuality,
            Action::ToggleClouds,
            Action::ToggleRays,
            Action::ToggleWalk,
            Action::ToggleView,
        ] {
            let (x, y) = center_of(&u, a);
            assert_eq!(u.hit(x, y), Some(a), "{a:?}");
        }
    }

    #[test]
    fn every_button_is_on_screen_and_finger_sized() {
        let mut u = ui();
        u.menu_open = true;
        build(&mut u);
        for b in &u.buttons {
            assert!(b.x >= 0.0 && b.y >= 0.0 && b.x + b.w <= 2400.0 && b.y + b.h <= 1080.0);
            if b.action != Action::Nothing {
                assert!(b.w >= 48.0 * 2.75 - 0.5 && b.h >= 48.0 * 2.75 - 0.5);
            }
        }
        // Nothing sits under the status bar or the navigation bar.
        for b in &u.buttons {
            assert!(b.y >= SAFE_TOP_DP * 2.75 - 0.5, "{:?} is under the status bar", b.action);
            assert!(b.x + b.w <= 2400.0 - SAFE_RIGHT_DP * 2.75 + 0.5, "{:?} is under the nav bar", b.action);
        }
        // And it still fits on a portrait screen.
        let landscape = u.buttons.len();
        u.set_surface(1080, 2400, 2.75);
        build(&mut u);
        assert!(u.buttons.iter().all(|b| b.x >= 0.0 && b.x + b.w <= 1080.0));
        assert!(landscape > 10);
    }

    #[test]
    fn the_hud_keeps_clear_of_the_safe_area_the_system_reports() {
        let mut u = ui();
        u.settings.walk = true;
        u.settings.show_stats = true;
        // Immersive mode with nothing in the way: the menu button moves into the corner.
        u.set_safe_area(Some([0.0; 4]));
        build(&mut u);
        let menu = u.buttons.iter().find(|b| b.action == Action::ToggleMenu).unwrap();
        assert!(menu.x + menu.w > 2400.0 - 20.0 * 2.75 && menu.y < 10.0 * 2.75, "{:?}", (menu.x, menu.y));
        // A camera cut-out on the left and the gesture bar at the bottom.
        let safe = [120.0, 0.0, 30.0, 70.0];
        u.set_safe_area(Some(safe));
        for acts in [false, true] {
            u.acts_open = acts;
            let out = build(&mut u);
            for b in &u.buttons {
                assert!(b.x >= safe[0] && b.y >= safe[1], "{:?}", b.action);
                assert!(b.x + b.w <= 2400.0 - safe[2] && b.y + b.h <= 1080.0 - safe[3], "{:?}", b.action);
            }
            // The stats panel too (drawn first, from the top left).
            assert!(out.iter().take(6).all(|v| v.pos[0] >= safe[0]));
        }
        // Unknown again: back to the fixed margins.
        u.set_safe_area(None);
        build(&mut u);
        let menu = u.buttons.iter().find(|b| b.action == Action::ToggleMenu).unwrap();
        assert!(menu.y >= SAFE_TOP_DP * 2.75 && menu.x + menu.w <= 2400.0 - SAFE_RIGHT_DP * 2.75);
    }

    #[test]
    fn taps_on_the_panel_background_are_swallowed() {
        let mut u = ui();
        u.menu_open = true;
        build(&mut u);
        let panel = u.buttons.iter().find(|b| b.action == Action::Nothing).unwrap();
        // Top-left corner of the panel: padding, no button there.
        assert_eq!(u.hit(panel.x + 2.0, panel.y + 2.0), Some(Action::Nothing));
        // Far from any UI: falls through to the game.
        assert_eq!(u.hit(100.0, 700.0), None);
    }

    #[test]
    fn render_distance_clamps_and_reports_changes() {
        let mut u = ui();
        assert!(u.apply(Action::RenderDist(1)));
        assert_eq!(u.settings.render_chunks, 7);
        for _ in 0..50 {
            u.apply(Action::RenderDist(1));
        }
        assert_eq!(u.settings.render_chunks, RENDER_MAX);
        assert!(!u.apply(Action::RenderDist(1)), "no change at the limit");
        for _ in 0..50 {
            u.apply(Action::RenderDist(-1));
        }
        assert_eq!(u.settings.render_chunks, RENDER_MIN);
    }

    #[test]
    fn lod_steps_through_the_distances_and_never_asks_the_server_for_anything() {
        let mut u = ui();
        assert_eq!(lod_label(u.settings.lod_level), "16K");
        assert!(!u.apply(Action::LodDist(1)));
        assert_eq!(lod_label(u.settings.lod_level), "32K");
        assert!(!u.apply(Action::LodDist(1)));
        assert_eq!(lod_label(u.settings.lod_level), "32K", "stops at the top");
        for _ in 0..50 {
            u.apply(Action::LodDist(-1));
        }
        assert_eq!(lod_label(u.settings.lod_level), "OFF");
        assert_eq!(u.settings.lod_level, 0);
        u.apply(Action::LodDist(1));
        assert_eq!(lod_label(u.settings.lod_level), "1K");
        let q = u.settings.lod_quality;
        assert!(!u.apply(Action::CycleLodQuality));
        assert_ne!(u.settings.lod_quality, q);
        for _ in 0..LOD_QUALITY.len() - 1 {
            u.apply(Action::CycleLodQuality);
        }
        assert_eq!(u.settings.lod_quality, q, "cycles back round");
    }

    #[test]
    fn cull_mode_cycles_through_all_five_states_and_maps_to_the_right_flags() {
        let mut u = ui();
        assert_eq!(u.settings.cull, CullMode::All);
        assert!(u.settings.cull.frustum() && u.settings.cull.backface() && u.settings.cull.occlusion());
        u.apply(Action::CycleCull);
        assert_eq!(u.settings.cull, CullMode::NoCaves);
        assert!(u.settings.cull.frustum() && u.settings.cull.backface() && !u.settings.cull.occlusion());
        u.apply(Action::CycleCull);
        assert_eq!(u.settings.cull, CullMode::Frustum);
        assert!(u.settings.cull.frustum() && !u.settings.cull.backface());
        u.apply(Action::CycleCull);
        assert_eq!(u.settings.cull, CullMode::Back);
        assert!(!u.settings.cull.frustum() && u.settings.cull.backface());
        u.apply(Action::CycleCull);
        assert_eq!(u.settings.cull, CullMode::Off);
        assert!(!u.settings.cull.frustum() && !u.settings.cull.backface());
        u.apply(Action::CycleCull);
        assert_eq!(u.settings.cull, CullMode::All);
    }

    #[test]
    fn menu_rows_shrink_to_fit_a_shorter_screen() {
        let mut u = Ui::new();
        u.set_surface(2400, 1080, 3.0); // 360dp tall
        u.menu_open = true;
        let mut out = Vec::new();
        u.build(&Perf::new(), &mut out);
        for b in &u.buttons {
            assert!(b.y + b.h <= 1080.0, "{:?} runs off the bottom ({})", b.action, b.y + b.h);
        }
    }

    #[test]
    fn vsync_toggle_flips_and_shows_na_when_unsupported() {
        let mut u = ui();
        u.menu_open = true;
        assert!(u.settings.vsync);
        u.apply(Action::ToggleVsync);
        assert!(!u.settings.vsync);
        u.vsync_supported = false;
        build(&mut u); // must not panic with the N/A label
        u.apply(Action::ToggleVsync);
        assert!(u.settings.vsync);
    }

    #[test]
    fn mesher_toggle_flips() {
        let mut u = ui();
        assert!(!u.settings.greedy);
        assert!(!u.apply(Action::ToggleMesher), "it never asks the server for anything");
        assert!(u.settings.greedy);
        u.apply(Action::ToggleMesher);
        assert!(!u.settings.greedy);
    }

    #[test]
    fn every_toggle_label_fits_its_button() {
        // Longest labels in the grid, at the default density.
        let mut u = ui();
        u.menu_open = true;
        u.settings.cull = CullMode::Frustum;
        u.settings.greedy = true;
        build(&mut u);
        let panel_w = 376.0 * 2.75;
        let cell_w = (panel_w - 8.0 * 2.75 * 4.0) / 3.0;
        let px = (2.5f32 * 2.75).round();
        let mut tpx = px;
        while tpx > 2.0 && text_width(tpx, "CULL FRUSTUM") > cell_w * 0.94 {
            tpx -= 1.0;
        }
        assert!(tpx >= 4.0, "label would shrink to {tpx}px");
    }

    #[test]
    fn toggles_flip() {
        let mut u = ui();
        u.apply(Action::ToggleFog);
        u.apply(Action::ToggleStats);
        assert!(!u.settings.fog && !u.settings.show_stats);
        u.apply(Action::ToggleFog);
        assert!(u.settings.fog);
    }

    #[test]
    fn hiding_stats_shrinks_the_hud() {
        let mut u = ui();
        let with = build(&mut u).len();
        u.apply(Action::ToggleStats);
        let without = build(&mut u).len();
        assert!(without < with);
    }

    #[test]
    fn every_character_in_the_stats_text_has_art() {
        let mut p = Perf::new();
        p.fps = 59.6;
        p.frame_ms = 16.8;
        p.cpu_cores = Some(1.83);
        p.gpu_ms = Some(5.77);
        p.mem_mb = Some(412.4);
        p.chunks = 885;
        p.meshes = 312;
        p.main_thread = Some(0.42);
        p.quads = 123_456;
        p.present_mode = "IMMEDIATE";
        p.place = crate::game::place_lines(&blockforge::worldgen::WorldGen::new(1, &blockforge::registry::BlockRegistry::builtin()).describe(0, 0));
        p.place.extend(blockforge::biome::BIOMES.iter().map(|b| b.name.to_uppercase()));
        p.place.extend(blockforge::terrain::LANDFORMS.iter().map(|l| l.name.to_uppercase()));
        let mut all = stats_lines(&p);
        p.cpu_cores = None;
        p.gpu_ms = None;
        p.mem_mb = None;
        all.extend(stats_lines(&p));
        for line in all {
            for c in line.chars() {
                assert!(font::has_art(c), "no glyph for {c:?} in {line:?}");
            }
        }
        for s in ["REND", "LOD", "FOG", "CULL", "ALL", "FRUST", "BACK", "VSYNC", "N/A", "MESH", "GREEDY", "SIMPLE", "QUADS", "STATS", "ON", "OFF", "+", "-", "LODQ", "LOW", "MED", "HIGH", "32K", "SECT", "BUILDING"] {
            assert!(s.chars().all(font::has_art), "{s}");
        }
    }

    #[test]
    fn gpu_percentage_is_busy_time_over_frame_time() {
        let mut p = Perf::new();
        p.fps = 60.0;
        p.gpu_ms = Some(8.33);
        let lines = stats_lines(&p);
        assert!(lines.iter().any(|l| l.contains("GPU 8.3MS") && l.contains("(50%)")), "{lines:?}");
    }

    #[test]
    fn text_width_matches_glyph_advance() {
        assert_eq!(text_width(2.0, ""), 0.0);
        assert_eq!(text_width(2.0, "A"), 10.0); // 6 * 2 advance, minus the trailing gap
        assert_eq!(text_width(2.0, "AB"), 22.0);
    }
}
