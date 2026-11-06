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
pub const LOD_MIN: u8 = 4;
pub const LOD_MAX: u8 = 32;

/// Android draws the status bar over the top edge and the navigation bar over the right
/// edge in landscape. These fixed insets come from a real phone's screenshots (status bar
/// about 24dp, nav bar about 51dp). The proper fix is immersive mode or reading real insets.
pub const SAFE_TOP_DP: f32 = 28.0;
pub const SAFE_RIGHT_DP: f32 = 56.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Chunks streamed and drawn around the player.
    pub render_chunks: u8,
    /// Where LOD terrain should start, in chunks. Stored and shown, but nothing consumes
    /// it yet: LOD meshing isn't built (see README).
    pub lod_chunks: u8,
    pub fog: bool,
    pub show_stats: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { render_chunks: 6, lod_chunks: 16, fog: true, show_stats: true }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// A region that swallows touches (the menu panel) without doing anything.
    Nothing,
    ToggleMenu,
    RenderDist(i8),
    LodDist(i8),
    ToggleFog,
    ToggleStats,
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
    size: (f32, f32),
    dp: f32,
    buttons: Vec<Button>,
}

const WHITE: [u8; 4] = [255, 255, 255, 255];
const DIM: [u8; 4] = [190, 195, 205, 170];
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
    match perf.gpu_ms {
        Some(g) => v.push(format!("GPU {:.1}MS  ({:.0}%)", g, (g * perf.fps / 10.0).min(999.0))),
        None => v.push("GPU N/A".to_string()),
    }
    if let Some(m) = perf.mem_mb {
        v.push(format!("MEM {:.0}MB", m));
    }
    v.push(format!("CHUNKS {}  MESHED {}", perf.chunks, perf.meshes));
    v
}

impl Default for Ui {
    fn default() -> Self {
        Self::new()
    }
}

impl Ui {
    pub fn new() -> Ui {
        Ui { settings: Settings::default(), menu_open: false, size: (1.0, 1.0), dp: 1.0, buttons: Vec::new() }
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
                self.settings.lod_chunks = step(self.settings.lod_chunks, d * 2, LOD_MIN, LOD_MAX);
                false
            }
            Action::ToggleFog => {
                self.settings.fog = !self.settings.fog;
                false
            }
            Action::ToggleStats => {
                self.settings.show_stats = !self.settings.show_stats;
                false
            }
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
            self.draw_menu(out, bx + bs, by + bs + 4.0 * dp);
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
        let row_h = 56.0 * dp;
        let bw = 48.0 * dp;
        let gap = 6.0 * dp;
        let rows = 4usize;
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

        for i in 0..rows {
            let ry = top + pad + row_h * i as f32;
            let by = ry + (row_h - bw) * 0.5;
            let ty = ry + (row_h - 7.0 * px) * 0.5;
            match i {
                0 => {
                    text(out, label_x, ty, px, "REND", WHITE);
                    text(out, value_x, ty, px, &s.render_chunks.to_string(), WHITE);
                    self.button(out, minus_x, by, bw, bw, "-", px, Action::RenderDist(-1));
                    self.button(out, plus_x, by, bw, bw, "+", px, Action::RenderDist(1));
                }
                1 => {
                    text(out, label_x, ty, px, "LOD", WHITE);
                    let v = s.lod_chunks.to_string();
                    text(out, value_x, ty, px, &v, WHITE);
                    // Honest label: the setting exists but nothing uses it yet.
                    text(out, value_x + text_width(px, "00") + 10.0 * dp, ty, px, "SOON", DIM);
                    self.button(out, minus_x, by, bw, bw, "-", px, Action::LodDist(-1));
                    self.button(out, plus_x, by, bw, bw, "+", px, Action::LodDist(1));
                }
                2 => {
                    text(out, label_x, ty, px, "FOG", WHITE);
                    let label = if s.fog { "ON" } else { "OFF" };
                    self.button(out, minus_x, by, 2.0 * bw + gap, bw, label, px, Action::ToggleFog);
                }
                _ => {
                    text(out, label_x, ty, px, "STATS", WHITE);
                    let label = if s.show_stats { "ON" } else { "OFF" };
                    self.button(out, minus_x, by, 2.0 * bw + gap, bw, label, px, Action::ToggleStats);
                }
            }
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
            Action::ToggleFog,
            Action::ToggleStats,
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
    fn lod_steps_by_two_and_never_asks_the_server_for_anything() {
        let mut u = ui();
        assert!(!u.apply(Action::LodDist(1)));
        assert_eq!(u.settings.lod_chunks, 18);
        for _ in 0..50 {
            u.apply(Action::LodDist(-1));
        }
        assert_eq!(u.settings.lod_chunks, LOD_MIN);
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
        for s in ["REND", "LOD", "FOG", "STATS", "SOON", "ON", "OFF", "+", "-"] {
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
