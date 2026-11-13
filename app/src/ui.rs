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

/// Android draws the status bar over the top edge and the navigation bar over the right
/// edge in landscape. These fixed insets come from a real phone's screenshots (status bar
/// about 24dp, nav bar about 51dp). The proper fix is immersive mode or reading real insets.
pub const SAFE_TOP_DP: f32 = 28.0;
pub const SAFE_RIGHT_DP: f32 = 56.0;

/// Which culling is active. Useful for measuring what each one buys on a real device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CullMode {
    All,
    Frustum,
    Back,
    Off,
}

impl CullMode {
    pub fn frustum(self) -> bool {
        matches!(self, CullMode::All | CullMode::Frustum)
    }

    pub fn backface(self) -> bool {
        matches!(self, CullMode::All | CullMode::Back)
    }

    pub fn next(self) -> CullMode {
        match self {
            CullMode::All => CullMode::Frustum,
            CullMode::Frustum => CullMode::Back,
            CullMode::Back => CullMode::Off,
            CullMode::Off => CullMode::All,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CullMode::All => "ALL",
            CullMode::Frustum => "FRUST",
            CullMode::Back => "BACK",
            CullMode::Off => "OFF",
        }
    }

    fn from_label(s: &str) -> Option<CullMode> {
        [CullMode::All, CullMode::Frustum, CullMode::Back, CullMode::Off].into_iter().find(|m| m.label() == s)
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
        }
    }
}

impl Settings {
    /// `key=value` lines, one per setting, for the settings file kept between launches.
    pub fn to_text(&self) -> String {
        format!(
            "render_chunks={}\nlod_level={}\nlod_quality={}\nfog={}\ncull={}\nvsync={}\ngreedy={}\nshow_stats={}\n",
            self.render_chunks,
            self.lod_level,
            self.lod_quality,
            self.fog,
            self.cull.label(),
            self.vsync,
            self.greedy,
            self.show_stats
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
                "fog" | "vsync" | "greedy" | "show_stats" => {
                    let Ok(v) = value.parse::<bool>() else { continue };
                    match key {
                        "fog" => self.fog = v,
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
    ToggleMenu,
    RenderDist(i8),
    LodDist(i8),
    CycleLodQuality,
    ToggleFog,
    CycleCull,
    ToggleVsync,
    ToggleMesher,
    ToggleStats,
    /// Moves the time of day by this many hours (the game owns the clock and does it).
    Time(i8),
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
    pub menu_open: bool,
    /// False when the surface offers no uncapped present mode (the toggle then shows N/A).
    pub vsync_supported: bool,
    /// The time of day shown in the menu (the game keeps it up to date).
    pub hours: f64,
    size: (f32, f32),
    dp: f32,
    buttons: Vec<Button>,
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
    v.push(format!("DRAWN {}/{}", perf.drawn, perf.meshes));
    v.push(format!("QUADS {}", perf.quads));
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
            menu_open: false,
            vsync_supported: true,
            hours: crate::sky::START_HOUR,
            size: (1.0, 1.0),
            dp: 1.0,
            buttons: Vec::new(),
        }
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
            Action::Time(_) => false,
        }
    }

    /// Rebuilds the vertex list and the button list for this frame.
    pub fn build(&mut self, perf: &Perf, out: &mut Vec<HudVertex>) {
        out.clear();
        self.buttons.clear();
        let (w, h) = self.size;
        let dp = self.dp;

        // Crosshair
        let (cx, cy) = (w * 0.5, h * 0.5);
        let len = 10.0 * dp;
        let th = (1.5 * dp).max(2.0);
        rect(out, cx - len, cy - th * 0.5, len * 2.0, th, XHAIR);
        rect(out, cx - th * 0.5, cy - len, th, len * 2.0, XHAIR);

        let margin = 8.0 * dp;
        let safe_top = SAFE_TOP_DP * dp;
        let safe_right = SAFE_RIGHT_DP * dp;
        if self.settings.show_stats {
            self.draw_stats(perf, out, margin, safe_top + 4.0 * dp);
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
        rect(out, x, y, w, h, BTN);
        text_centered(out, x + w * 0.5, y + h * 0.5, px, label, WHITE);
        self.buttons.push(Button { x, y, w, h, action });
    }

    fn draw_menu(&mut self, out: &mut Vec<HudVertex>, right: f32, top: f32) {
        let dp = self.dp;
        let px = (2.5 * dp).round().max(2.0);
        // Never wider than the space left of the right edge (matters on narrow screens).
        let panel_w = (360.0 * dp).min((right - 8.0 * dp).max(0.0));
        let pad = 8.0 * dp;
        let bw = 48.0 * dp;
        let gap = 6.0 * dp;
        let rows = 6usize; // three stepper rows + three rows of two toggle buttons
        // Rows shrink (down to button height) on short screens so the panel stays visible.
        let avail = self.size.1 - top - 8.0 * dp;
        let row_h = (56.0 * dp).min((avail - pad * 2.0) / rows as f32).max(bw);
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
            let by = ry + (row_h - bw) * 0.5;
            let ty = ry + (row_h - 7.0 * px) * 0.5;
            if i == 0 {
                text(out, label_x, ty, px, "REND", WHITE);
                text(out, value_x, ty, px, &s.render_chunks.to_string(), WHITE);
                self.button(out, minus_x, by, bw, bw, "-", px, Action::RenderDist(-1));
                self.button(out, plus_x, by, bw, bw, "+", px, Action::RenderDist(1));
            } else if i == 1 {
                text(out, label_x, ty, px, "LOD", WHITE);
                text(out, value_x, ty, px, &lod_label(s.lod_level), WHITE);
                self.button(out, minus_x, by, bw, bw, "-", px, Action::LodDist(-1));
                self.button(out, plus_x, by, bw, bw, "+", px, Action::LodDist(1));
            } else {
                text(out, label_x, ty, px, "TIME", WHITE);
                text(out, value_x, ty, px, &crate::sky::time_label(self.hours), WHITE);
                self.button(out, minus_x, by, bw, bw, "-", px, Action::Time(-1));
                self.button(out, plus_x, by, bw, bw, "+", px, Action::Time(1));
            }
        }

        // Rows 3-5: toggles in two columns; each button carries its own label.
        let on_off = |b: bool| if b { "ON" } else { "OFF" };
        let vsync_label = if !self.vsync_supported { "N/A" } else { on_off(s.vsync) };
        let toggles: [(Action, String); 6] = [
            (Action::ToggleFog, format!("FOG {}", on_off(s.fog))),
            (Action::CycleCull, format!("CULL {}", s.cull.label())),
            (Action::ToggleVsync, format!("VSYNC {}", vsync_label)),
            (Action::ToggleMesher, format!("MESH {}", if s.greedy { "GREEDY" } else { "SIMPLE" })),
            (Action::ToggleStats, format!("STATS {}", on_off(s.show_stats))),
            (Action::CycleLodQuality, format!("LODQ {}", LOD_QUALITY[s.lod_quality as usize % LOD_QUALITY.len()].0)),
        ];
        let cell_w = (panel_w - pad * 3.0) / 2.0;
        for (i, (action, label)) in toggles.iter().enumerate() {
            let row = 3 + i / 2;
            let col = i % 2;
            let x = x0 + pad + col as f32 * (cell_w + pad);
            let ry = top + pad + row_h * row as f32;
            let by = ry + (row_h - bw) * 0.5;
            // Shrink the text (in whole font pixels) if the label wouldn't fit its button.
            let mut tpx = px;
            while tpx > 2.0 && text_width(tpx, label) > cell_w * 0.94 {
                tpx -= 1.0;
            }
            self.button(out, x, by, cell_w, bw, label, tpx, *action);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ui() -> Ui {
        let mut u = Ui::new();
        u.set_surface(2400, 1080, 2.75);
        u
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
        u.set_surface(1080, 2400, 2.75);
        build(&mut u);
        assert!(u.buttons.iter().all(|b| b.x >= 0.0 && b.x + b.w <= 1080.0));
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
    fn cull_mode_cycles_through_all_four_states_and_maps_to_the_right_flags() {
        let mut u = ui();
        assert_eq!(u.settings.cull, CullMode::All);
        assert!(u.settings.cull.frustum() && u.settings.cull.backface());
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
        let panel_w = 360.0 * 2.75;
        let cell_w = (panel_w - 8.0 * 2.75 * 3.0) / 2.0;
        let px = (2.5f32 * 2.75).round();
        let mut tpx = px;
        while tpx > 2.0 && text_width(tpx, "MESH GREEDY") > cell_w * 0.94 {
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
