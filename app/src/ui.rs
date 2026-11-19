//! Immediate-mode HUD: stats overlay, crosshair, and a small settings menu.
//!
//! Everything is built each frame into a flat list of coloured triangles (`HudVertex`),
//! including text (see `font`). Touch hit-testing uses the button list from the same
//! pass, so what you see is exactly what you can tap. Sizes are in dp (density-
//! independent pixels) times the window's scale factor, so buttons stay finger-sized.

use crate::font;

mod controls;
mod draw;
mod screens;

pub use controls::{BlockItem, Joy, Layout, JOY_RADII};
use crate::perf::Perf;

/// One corner of a HUD triangle (`ui.wgsl`): pixel position, texture place, sRGB colour and
/// how to read the texture (`MODE_*`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HudVertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: [u8; 4],
    pub mode: u32,
}

/// The UI atlas's colours times the vertex colour.
pub const MODE_IMAGE: u32 = 0;
/// The UI atlas's alpha as a distance field (text, icons).
pub const MODE_SDF: u32 = 1;
/// The world thumbnails' texture.
pub const MODE_THUMB: u32 = 2;

/// Multiple of 6 (two triangles per rectangle).
pub const MAX_HUD_VERTS: usize = 60_000;

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
    /// How opaque the game's buttons are, in percent (`OPACITY_MIN`..100).
    pub hud_opacity: u8,
    /// How fast dragging turns the view, 1..10 (5 = the default speed).
    pub look_speed: u8,
    /// The stick's size: an index into `controls::JOY_RADII`.
    pub joy_size: u8,
}

pub const OPACITY_MIN: u8 = 30;

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
            hud_opacity: 85,
            look_speed: 5,
            joy_size: 1,
        }
    }
}

impl Settings {
    /// `key=value` lines, one per setting, for the settings file kept between launches.
    pub fn to_text(&self) -> String {
        format!(
            "render_chunks={}\nlod_level={}\nlod_quality={}\nfog={}\ncull={}\nvsync={}\ngreedy={}\nshow_stats={}\nclouds={}\ngod_rays={}\nwalk={}\nview={}\nhud_opacity={}\nlook_speed={}\njoy_size={}\n",
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
            self.view.key(),
            self.hud_opacity,
            self.look_speed,
            self.joy_size
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
                "hud_opacity" | "look_speed" | "joy_size" => {
                    let Ok(v) = value.parse::<u8>() else { continue };
                    match key {
                        "hud_opacity" => self.hud_opacity = v.clamp(OPACITY_MIN, 100),
                        "look_speed" => self.look_speed = v.clamp(1, 10),
                        _ => self.joy_size = v.min(controls::JOY_RADII.len() as u8 - 1),
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
    /// The keyboard's shift key (capitals on and off).
    Shift,
    /// World list: the pencil on a row (pick it and edit it).
    EditWorldAt(usize),
    /// Opens the settings page (from the title screen or the game menu); changes there take
    /// effect at once.
    OpenSettings,
    /// Settings page: which group of settings shows.
    Tab(u8),
    ResetSettings,
    /// The title screen's QUIT.
    Quit,
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
    /// The hotbar: picks a slot; opens the block picker; puts block `n` of `Ui::blocks` in
    /// the picked slot.
    Slot(u8),
    ToggleInventory,
    PickBlock(u16),
    /// Settings > Controls.
    HudOpacity(i8),
    LookSpeed(i8),
    JoySize(i8),
    /// Opens the controls editor; its toolbar: bigger or smaller, hide, the list of hidden
    /// controls, put one back, everything back where it was, and done.
    OpenControls,
    CtlGrab(u8),
    CtlSize(i8),
    CtlHide,
    CtlAddList,
    CtlShow(u8),
    CtlReset,
    CtlDone,
    /// The inventory: picks block `n` of `Ui::blocks` (tapping the picked one again puts it in
    /// the hotbar), puts the picked one in the hotbar's slot, empties the slot.
    InvPick(u16),
    AddToHotbar,
    ClearSlot,
    /// The title screen's arrows under the character: turn him.
    TurnShowcase(i8),
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
    /// The game menu, over the world: back to the game, settings, save and quit.
    Pause,
    /// Settings > Controls > Customize: moving, resizing, hiding and adding the game's
    /// buttons.
    Controls,
    /// Every block, to put in the hotbar (over the game).
    Inventory,
    Playing,
}

/// One saved world as the world list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldEntry {
    pub folder: String,
    pub name: String,
    /// The seed as typed back into the create page by RE-CREATE.
    pub seed: String,
    /// "November 14, 2026 - 10:45" and the like, ready to show.
    pub last_played: String,
    pub play_time: String,
    pub size: String,
    /// When it was made ("11/03/2026").
    pub created: String,
    /// Its picture's slot in the renderer's thumbnails (`gfx::thumb_uv`), if it has one.
    pub thumb: Option<usize>,
}

/// What the app has to do for the world screens (they only draw and collect input).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Play(String),
    Create { name: String, seed: String },
    Rename { folder: String, name: String },
    Delete(String),
    /// QUIT on the title screen: close the app.
    Quit,
    /// The controls editor is done: save the layout (`controls.txt`).
    SaveControls,
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
    /// The touch holding a menu button down, and that button: menus act when the finger
    /// lifts off the same button (and the button glows meanwhile).
    pressed: Option<(u64, Action)>,
    /// The settings page's group showing, and where BACK goes.
    pub tab: u8,
    settings_back: Screen,
    /// The keyboard's capitals (`caps_auto`: only for the next letter).
    caps: bool,
    caps_auto: bool,
    /// The thumbnail slot shown for worlds without a picture of their own.
    pub default_thumb: Option<usize>,
    /// Where the game's buttons are (the player's layout), the one picked in the editor,
    /// the finger dragging one (touch, control, offset to its centre) and the ADD list.
    pub layout: Layout,
    sel_ctl: Option<usize>,
    drag: Option<(u64, usize, f32, f32)>,
    add_open: bool,
    /// The blocks the player can place (icon `i` is block `blocks[i]`), the hotbar's
    /// blocks (ids, 0 = empty), the picked slot and when it last changed, and whether the
    /// block picker is open.
    pub blocks: Vec<BlockItem>,
    pub hotbar: [u16; 9],
    pub slot: usize,
    slot_changed: f32,
    /// The floating stick, while it shows (the game moves it).
    pub joystick: Option<Joy>,
    /// What the fingers on the game's buttons are holding (they glow).
    pub touching: Vec<Action>,
    /// The title screen's character: where it stands (set by `build`), and which way it turns.
    pub showcase: Option<Showcase>,
    pub showcase_yaw: f32,
    /// The first `under` vertices of the title screen go under its character (the pedestal).
    pub under: usize,
    /// The inventory's picked block (an index into `blocks`).
    pub inv_sel: Option<usize>,
    /// A finger dragging the world list: touch, where it started, the scroll then, and
    /// whether it has moved far enough to be a drag (then it picks nothing).
    list_drag: Option<(u64, f32, usize, bool)>,
}

/// Where the title screen's character is drawn: the middle of his feet on screen, and his
/// height (1.8 blocks) in pixels; `yaw`: which way he faces (0 = at the viewer).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Showcase {
    pub x: f32,
    pub feet: f32,
    pub height: f32,
    pub yaw: f32,
}

const WHITE: [u8; 4] = [255, 255, 255, 255];
const PANEL: [u8; 4] = [8, 10, 14, 205];
const BTN: [u8; 4] = [255, 255, 255, 48];
const XHAIR: [u8; 4] = [255, 255, 255, 215];

fn rect(out: &mut Vec<HudVertex>, x: f32, y: f32, w: f32, h: f32, c: [u8; 4]) {
    draw::rect(out, x, y, w, h, c);
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
            pressed: None,

            tab: 0,
            settings_back: Screen::Title,
            caps: true,
            caps_auto: true,
            default_thumb: None,
            layout: Layout::default(),
            sel_ctl: None,
            drag: None,
            add_open: false,
            blocks: Vec::new(),
            hotbar: [0; 9],
            slot: 0,
            slot_changed: -10.0,
            joystick: None,
            touching: Vec::new(),
            showcase: None,
            showcase_yaw: 0.0,
            under: 0,
            inv_sel: None,
            list_drag: None,
        }
    }

    /// Whether the open world shows behind the UI (in the game, its menu, its settings).
    pub fn over_world(&self) -> bool {
        match self.screen {
            Screen::Playing | Screen::Pause => true,
            Screen::Inventory => true,
            Screen::Settings | Screen::Controls => self.settings_back == Screen::Pause,
            _ => false,
        }
    }

    /// A finger touched the screen. Returns what to do now: in the game, buttons act at
    /// once (JUMP is held, PUNCH fires); in the menus only keys do, other buttons wait for
    /// the finger to lift (`release`). `None` also when the touch missed every button.
    pub fn press(&mut self, id: u64, x: f32, y: f32) -> Option<Action> {
        let a = self.hit(x, y)?;
        if let Action::CtlGrab(i) = a {
            self.grab_control(id, i as usize, x, y);
            return None;
        }
        if let Action::SelectWorld(_) = a {
            self.list_drag = Some((id, y, self.scroll, false));
        }
        if self.screen == Screen::Playing || matches!(a, Action::Key(_) | Action::Backspace | Action::Shift | Action::Nothing) {
            return Some(a);
        }
        self.pressed = Some((id, a));
        None
    }

    /// A finger lifted: the button it pressed, if it is still on it.
    pub fn release(&mut self, id: u64, x: f32, y: f32) -> Option<Action> {
        if self.drag.is_some_and(|d| d.0 == id) {
            self.drag = None;
            return None;
        }
        if let Some((did, _, _, dragged)) = self.list_drag {
            if did == id {
                self.list_drag = None;
                if dragged {
                    self.pressed = None;
                    return None;
                }
            }
        }
        match self.pressed {
            Some((pid, a)) if pid == id => {
                self.pressed = None;
                (self.hit(x, y) == Some(a)).then_some(a)
            }
            _ => None,
        }
    }

    /// Whether a touch is holding a menu button down (the game leaves it alone).
    pub fn holds(&self, id: u64) -> bool {
        self.pressed.is_some_and(|(pid, _)| pid == id) || self.drag.is_some_and(|d| d.0 == id)
    }

    /// The button being held down, if any (it glows).
    pub fn pressed_action(&self) -> Option<Action> {
        self.pressed.map(|(_, a)| a)
    }

    /// The settings that actions change.
    fn edited(&mut self) -> &mut Settings {
        &mut self.settings
    }

    /// A finger moved on the world list: past a few dp it scrolls the list a row at a time
    /// (and no longer picks the world it started on).
    fn drag_list(&mut self, id: u64, y: f32) {
        let Some((did, y0, s0, dragged)) = self.list_drag else { return };
        if did != id {
            return;
        }
        let dy = y - y0;
        if !dragged && dy.abs() < 10.0 * self.dp {
            return;
        }
        let rh = self.list_row_height().max(1.0);
        let rows = (-dy / rh).round() as i64;
        let max = self.worlds.len().saturating_sub(1) as i64;
        self.scroll = (s0 as i64 + rows).clamp(0, max) as usize;
        self.list_drag = Some((did, y0, s0, true));
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

    /// Screen pixels per dp.
    pub fn dp_scale(&self) -> f32 {
        self.dp
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
            Action::Slot(_)
            | Action::ToggleInventory
            | Action::PickBlock(_)
            | Action::OpenControls
            | Action::CtlGrab(_)
            | Action::CtlSize(_)
            | Action::CtlHide
            | Action::CtlAddList
            | Action::CtlShow(_)
            | Action::CtlReset
            | Action::CtlDone => {
                self.apply_controls(a);
                false
            }
            Action::HudOpacity(d) => {
                let s = self.edited();
                s.hud_opacity = (s.hud_opacity as i32 + d as i32 * 10).clamp(OPACITY_MIN as i32, 100) as u8;
                false
            }
            Action::LookSpeed(d) => {
                let s = self.edited();
                s.look_speed = (s.look_speed as i32 + d as i32).clamp(1, 10) as u8;
                false
            }
            Action::JoySize(d) => {
                let s = self.edited();
                s.joy_size = (s.joy_size as i32 + d as i32).clamp(0, controls::JOY_RADII.len() as i32 - 1) as u8;
                false
            }
            Action::Open(s) => {
                self.screen = s;
                self.acts_open = false;
                self.focus = None;
                self.pressed = None;
                false
            }
            Action::OpenSettings => {
                self.tab = 0;
                self.settings_back = if self.screen == Screen::Pause { Screen::Pause } else { Screen::Title };
                self.screen = Screen::Settings;
                false
            }
            Action::Tab(t) => {
                self.tab = t;
                false
            }
            Action::ResetSettings => {
                let before = self.settings.render_chunks;
                self.settings = Settings { vsync: self.settings.vsync, ..Settings::default() };
                self.settings.render_chunks != before
            }
            Action::TurnShowcase(d) => {
                self.showcase_yaw += d as f32 * std::f32::consts::FRAC_PI_4;
                false
            }
            Action::InvPick(k) => {
                let k = k as usize;
                if k < self.blocks.len() {
                    if self.inv_sel == Some(k) {
                        self.apply(Action::AddToHotbar);
                    }
                    self.inv_sel = Some(k);
                }
                false
            }
            Action::AddToHotbar => {
                if let Some(k) = self.inv_sel {
                    self.apply(Action::PickBlock(k as u16));
                }
                false
            }
            Action::ClearSlot => {
                self.hotbar[self.slot.min(8)] = 0;
                false
            }
            Action::Quit => {
                self.requests.push(Request::Quit);
                false
            }
            Action::EditWorldAt(i) => {
                if i < self.worlds.len() {
                    self.selected = Some(i);
                    self.apply(Action::EditWorld);
                }
                false
            }
            Action::Shift => {
                self.caps = !self.caps;
                self.caps_auto = false;
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
                    _ => ["New World".to_string(), String::new()],
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
                let f = i as usize % 2;
                self.focus = Some(f);
                // A name starts with a capital, as a phone's keyboard does it.
                self.caps = f == 0 && self.fields[f].is_empty();
                self.caps_auto = self.caps;
                false
            }
            Action::Key(c) => {
                if let Some(f) = self.focus {
                    let max = if f == 0 { crate::worlds::MAX_NAME } else { crate::worlds::MAX_SEED };
                    if (c.is_ascii_alphanumeric() || c == ' ' || c == '-') && self.fields[f].chars().count() < max {
                        self.fields[f].push(c);
                        if c.is_ascii_alphabetic() && self.caps_auto {
                            self.caps = false;
                            self.caps_auto = false;
                        }
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
            Action::RenderDist(d) => {
                let s = self.edited();
                let old = s.render_chunks;
                s.render_chunks = step(old, d, RENDER_MIN, RENDER_MAX);
                s.render_chunks != old
            }
            Action::LodDist(d) => {
                let s = self.edited();
                s.lod_level = step(s.lod_level, d, 0, LOD_DISTANCES.len() as u8 - 1);
                false
            }
            Action::CycleLodQuality => {
                let s = self.edited();
                s.lod_quality = (s.lod_quality + 1) % LOD_QUALITY.len() as u8;
                false
            }
            Action::ToggleFog => {
                let s = self.edited();
                s.fog = !s.fog;
                false
            }
            Action::CycleCull => {
                let s = self.edited();
                s.cull = s.cull.next();
                false
            }
            Action::ToggleVsync => {
                let s = self.edited();
                s.vsync = !s.vsync;
                false
            }
            Action::ToggleMesher => {
                let s = self.edited();
                s.greedy = !s.greedy;
                false
            }
            Action::ToggleStats => {
                let s = self.edited();
                s.show_stats = !s.show_stats;
                false
            }
            Action::ToggleClouds => {
                let s = self.edited();
                s.clouds = !s.clouds;
                false
            }
            Action::ToggleRays => {
                let s = self.edited();
                s.god_rays = !s.god_rays;
                false
            }
            Action::ToggleWalk => {
                let s = self.edited();
                s.walk = !s.walk;
                false
            }
            Action::ToggleView => {
                let s = self.edited();
                s.view = s.view.next();
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
        self.showcase = None;
        self.under = 0;
        if self.screen == Screen::Playing {
            self.draw_hud(perf, out);
        } else {
            self.draw_screen(out);
        }
        out.truncate(MAX_HUD_VERTS);
    }

    fn draw_stats(&self, perf: &Perf, out: &mut Vec<HudVertex>, x: f32, y: f32) {
        use crate::atlas::FontId::Pixel;
        let size = 12.0 * self.dp;
        let lines = stats_lines(perf);
        let pad = 5.0 * self.dp;
        let line_h = size * 1.15;
        let width = lines.iter().map(|l| draw::width(Pixel, l, size)).fold(0.0f32, f32::max);
        let height = line_h * lines.len() as f32;
        rect(out, x, y, width + 2.0 * pad, height + 2.0 * pad, PANEL);
        for (i, l) in lines.iter().enumerate() {
            draw::left(out, Pixel, l, x + pad, y + pad + line_h * (i as f32 + 0.5), size, WHITE, false);
        }
    }
}

#[cfg(test)]
mod tests;
