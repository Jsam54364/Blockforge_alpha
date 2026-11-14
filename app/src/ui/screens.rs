//! The menus and the HUD, in the look of the concept art: dark stone buttons with metal
//! rims, framed panels, a title plaque with blue gems, the BlockForge logo, serif type and
//! icons, and a blue glow on the button being pressed. Everything comes from the UI atlas
//! (`draw.rs`); the world's own panorama turns behind the menus.

use super::draw::{self, nine};
use super::{lod_label, rect, Action, Button, HudVertex, Screen, Ui, LOD_QUALITY, XHAIR};
use crate::atlas::FontId::{Pixel, Serif};
use crate::perf::Perf;

const INK: [u8; 4] = [240, 232, 216, 255];
const DIM: [u8; 4] = [166, 172, 186, 255];
const BLUE: [u8; 4] = [138, 186, 255, 255];
const FAINT: [u8; 4] = [255, 255, 255, 26];
const OPAQUE: [u8; 4] = [255, 255, 255, 255];
const PRESSED: [u8; 4] = [196, 200, 214, 255];
const DISABLED: [u8; 4] = [128, 128, 132, 235];
const DANGER: [u8; 4] = [255, 150, 140, 255];
const WARN: [u8; 4] = [255, 196, 90, 255];

/// How a button looks.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    /// Square-cornered stone with rivets.
    Stone,
    /// Corners cut at 45 degrees (the bottom row, QUIT).
    Bar,
}

/// The settings page's groups: name and icon.
pub const TABS: [(&str, &str); 3] = [("General", "settings"), ("Video", "desktop_windows"), ("Performance", "speed")];

/// One setting on the settings page.
enum Control {
    /// On or off.
    Switch(bool, Action),
    /// A value with buttons to step it down and up.
    Stepper(String, Action, Action),
    /// A value that cycles to the next.
    Choice(String, Action),
    /// A value that cannot be changed here.
    Fixed(String),
}

impl Ui {
    /// Screen pixels per atlas pixel for frame edges.
    fn k(&self) -> f32 {
        self.dp * 0.42
    }

    /// A stone or bar button with an optional icon; glows while pressed (or when `glow`).
    #[allow(clippy::too_many_arguments)]
    fn button(
        &mut self,
        out: &mut Vec<HudVertex>,
        style: Style,
        (x, y, w, h): (f32, f32, f32, f32),
        label: &str,
        icon: Option<&str>,
        font: crate::atlas::FontId,
        action: Action,
        enabled: bool,
        glow: bool,
    ) {
        let k = self.k();
        let pressed = enabled && self.pressed_action() == Some(action);
        let tint = if !enabled {
            DISABLED
        } else if pressed {
            PRESSED
        } else {
            OPAQUE
        };
        let piece = if style == Style::Stone { "button" } else { "bar" };
        nine(out, piece, x, y, w, h, k, tint);
        if enabled && (pressed || glow) {
            let g = 18.0 * k;
            nine(out, "glow", x - g, y - g, w + 2.0 * g, h + 2.0 * g, k, OPAQUE);
        }
        let ink = if enabled { INK } else { [150, 150, 154, 255] };
        let size = (h * if label.chars().count() == 1 { 0.5 } else if font == Pixel { 0.40 } else { 0.46 }).min(30.0 * self.dp);
        let icon_size = h * 0.52;
        let gap = h * 0.22;
        let room = w - (h * 0.6).min(w * 0.3);
        let has_label = !label.is_empty();
        let fitted = if has_label { draw::fit(font, label, size, room - icon.map_or(0.0, |_| icon_size + gap)) } else { size };
        // A label squeezed too small to read: the icon alone says it.
        let has_label = has_label && (icon.is_none() || fitted >= size * 0.6);
        let size = fitted;
        let tw = if has_label { draw::width(font, label, size) } else { 0.0 };
        let total = tw + icon.map_or(0.0, |_| icon_size + if has_label { gap } else { 0.0 });
        let mut cx = x + (w - total) * 0.5;
        let cy = y + h * 0.5;
        if let Some(name) = icon {
            draw::icon(out, name, cx + icon_size * 0.5 + 1.0, cy + 1.0, icon_size, [0, 0, 0, 150]);
            draw::icon(out, name, cx + icon_size * 0.5, cy, icon_size, ink);
            cx += icon_size + gap;
        }
        if has_label {
            draw::left(out, font, label, cx, cy, size, ink, true);
        }
        self.buttons.push(Button { x, y, w, h, action: if enabled { action } else { Action::Nothing } });
    }

    /// The small logo at the top left and the title plaque beside it; returns where the
    /// content can start.
    fn top_bar(&mut self, out: &mut Vec<HudVertex>, title: &str) -> f32 {
        let (w, h) = self.size;
        let dp = self.dp;
        let [sl, st, sr, _] = self.margins();
        let lh = (64.0 * dp).min(h * 0.17);
        let lw = lh * 3.5;
        let (lx, ly) = (sl + 8.0 * dp, st + 6.0 * dp);
        let area_x = lx + lw + 12.0 * dp;
        let area_w = w - sr - 10.0 * dp - area_x;
        let ph = (50.0 * dp).min(lh * 0.82);
        let (px, py, pw, bottom) = if area_w >= 180.0 * dp {
            let pw = (360.0 * dp).min(area_w);
            (area_x + (area_w - pw) * 0.5, ly + (lh - ph) * 0.5, pw, ly + lh)
        } else {
            // Narrow screens: the plaque under the logo.
            let pw = (360.0 * dp).min(w - sl - sr - 20.0 * dp);
            (sl + (w - sl - sr - pw) * 0.5, ly + lh + 4.0 * dp, pw, ly + lh + 4.0 * dp + ph)
        };
        draw::image(out, "logo", lx, ly, lw, lh, OPAQUE);
        nine(out, "plaque", px, py, pw, ph, self.k(), OPAQUE);
        let size = draw::fit(Serif, title, ph * 0.52, pw - ph * 2.2);
        draw::centered(out, Serif, title, px + pw * 0.5, py + ph * 0.5, size, INK, true);
        bottom + 10.0 * dp
    }

    /// A row of bar buttons along the bottom; returns its top.
    fn bottom_bar(&mut self, out: &mut Vec<HudVertex>, items: &[(&str, &str, Action, bool)]) -> f32 {
        let (w, h) = self.size;
        let dp = self.dp;
        let [sl, _, sr, sb] = self.margins();
        let bh = 46.0 * dp;
        let y = h - sb - 8.0 * dp - bh;
        let gap = 12.0 * dp;
        let avail = w - sl - sr - 20.0 * dp;
        let n = items.len() as f32;
        let bw = ((avail - gap * (n - 1.0)) / n).min(250.0 * dp);
        let x0 = sl + 10.0 * dp + (avail - (bw * n + gap * (n - 1.0))) * 0.5;
        for (i, (label, icon, action, on)) in items.iter().enumerate() {
            self.button(out, Style::Bar, (x0 + i as f32 * (bw + gap), y, bw, bh), label, Some(icon), Serif, *action, *on, false);
        }
        y
    }

    pub(super) fn draw_screen(&mut self, out: &mut Vec<HudVertex>) {
        match self.screen {
            Screen::Title => self.draw_title(out),
            Screen::WorldList => self.draw_world_list(out),
            Screen::CreateWorld => self.draw_world_form(out, true),
            Screen::EditWorld => self.draw_world_form(out, false),
            Screen::DeleteWorld => self.draw_delete(out),
            Screen::Settings => self.draw_settings(out),
            Screen::Pause => self.draw_pause(out),
            Screen::Loading => self.draw_loading(out),
            Screen::Multiplayer => self.draw_page(
                out,
                "Multi Player",
                "group",
                &["No servers yet.", "The game already runs as a client and a server,", "even on its own: playing together over Wi-Fi is next."],
            ),
            Screen::Mods => self.draw_page(
                out,
                "Mods",
                "extension",
                &["No mods installed.", "Blocks come from assets/blocks.toml for now;", "mods will add their own blocks, items and behaviours."],
            ),
            Screen::Cosmetics => self.draw_page(out, "Customize", "brush", &["Skins and outfits for your character.", "Coming soon."]),
            Screen::Playing => {}
        }
    }

    /// The main menu: the logo over a column of buttons, QUIT below them.
    fn draw_title(&mut self, out: &mut Vec<HudVertex>) {
        let (w, h) = self.size;
        let dp = self.dp;
        let [sl, st, sr, sb] = self.margins();
        let cx = (sl + w - sr) * 0.5;
        let gap = 6.0 * dp;
        let qgap = 14.0 * dp;
        let footer = 18.0 * dp;
        let avail = h - st - sb - footer - 8.0 * dp;
        // The buttons want at least 34dp each; the logo gets the rest, up to 30% of the screen.
        let buttons_min = 6.0 * 34.0 * dp + 4.0 * gap + qgap;
        let mut lw = ((w - sl - sr) * if w < h { 0.9 } else { 0.6 }).min(560.0 * dp);
        let mut lh = lw / 3.5;
        let cap = (h * 0.3).min(avail - buttons_min).max(avail * 0.18);
        if lh > cap {
            lh = cap;
            lw = lh * 3.5;
        }
        let ly = st + 4.0 * dp;
        draw::image(out, "logo", cx - lw * 0.5, ly, lw, lh, OPAQUE);

        let (y0, y1) = (ly + lh + 4.0 * dp, h - sb - footer);
        let bh = ((y1 - y0 - 4.0 * gap - qgap) / 6.0).clamp(24.0 * dp, 48.0 * dp);
        let bw = (300.0 * dp).min((w - sl - sr) * if w < h { 0.8 } else { 0.6 });
        let used = 6.0 * bh + 4.0 * gap + qgap;
        let by = y0 + ((y1 - y0 - used) * 0.5).max(0.0);
        let items: [(&str, &str, Action); 5] = [
            ("Single Player", "person", Action::Open(Screen::WorldList)),
            ("Multi Player", "group", Action::Open(Screen::Multiplayer)),
            ("Settings", "settings", Action::OpenSettings),
            ("Customize", "brush", Action::Open(Screen::Cosmetics)),
            ("Mods", "extension", Action::Open(Screen::Mods)),
        ];
        // The first button glows until another is pressed, as a focus would.
        let focus = self.pressed_action().unwrap_or(items[0].2);
        for (i, (label, icon, action)) in items.iter().enumerate() {
            let y = by + i as f32 * (bh + gap);
            let x = cx - bw * 0.5;
            self.button(out, Style::Stone, (x, y, bw, bh), label, Some(icon), Pixel, *action, true, *action == focus);
            if *action == focus {
                let g = bh * 0.5;
                draw::image(out, "gem", x - g * 0.62, y + (bh - g) * 0.5, g, g, OPAQUE);
                draw::image(out, "gem", x + bw - g * 0.38, y + (bh - g) * 0.5, g, g, OPAQUE);
            }
        }
        let qw = bw * 0.6;
        let qy = by + 5.0 * (bh + gap) - gap + qgap;
        self.button(out, Style::Bar, (cx - qw * 0.5, qy, qw, bh), "Quit", Some("logout"), Pixel, Action::Quit, true, false);

        let version = format!("BlockForge alpha {}", env!("CARGO_PKG_VERSION"));
        draw::left(out, Serif, &version, sl + 8.0 * dp, h - sb - 10.0 * dp, 14.0 * dp, DIM, true);
    }

    /// SINGLE PLAYER: the worlds on the left, the picked one's picture and details on the
    /// right, and what to do along the bottom.
    fn draw_world_list(&mut self, out: &mut Vec<HudVertex>) {
        let (w, _) = self.size;
        let dp = self.dp;
        let k = self.k();
        let [sl, _, sr, _] = self.margins();
        let top = self.top_bar(out, "Single Player");
        let some = self.picked().is_some();
        let bar = self.bottom_bar(
            out,
            &[
                ("Back", "undo", Action::Open(Screen::Title), true),
                ("Create New World", "add", Action::NewWorld, true),
                ("Play", "play_arrow", Action::PlayWorld, some),
                ("Re-Create", "refresh", Action::RecreateWorld, some),
            ],
        );
        let (x0, x1) = (sl + 10.0 * dp, w - sr - 10.0 * dp);
        let (y0, y1) = (top, bar - 10.0 * dp);
        let gap = 12.0 * dp;
        // Side by side; on a narrow (portrait) screen the picked world goes under the list.
        let stacked = x1 - x0 < 560.0 * dp;
        let (lw, ph, (dx, dy, dw, dh)) = if stacked {
            let dh = (200.0 * dp).min((y1 - y0) * 0.45);
            (x1 - x0, (y1 - y0 - dh - gap).max(40.0 * dp), (x0, y1 - dh, x1 - x0, dh))
        } else {
            let lw = ((x1 - x0) - gap) * 0.6;
            (lw, (y1 - y0).max(40.0 * dp), (x0 + lw + gap, y0, x1 - x0 - lw - gap, (y1 - y0).max(40.0 * dp)))
        };
        draw::panel(out, x0, y0, lw, ph, k);
        self.buttons.push(Button { x: x0, y: y0, w: lw, h: ph, action: Action::Nothing });
        draw::panel(out, dx, dy, dw, dh, k);
        self.buttons.push(Button { x: dx, y: dy, w: dw, h: dh, action: Action::Nothing });

        // The list.
        let pad = 10.0 * dp;
        let n = self.worlds.len();
        let rh = (64.0 * dp).min((ph - 2.0 * pad) / 2.0).max(44.0 * dp);
        let fits = (((ph - 2.0 * pad) / rh).floor() as usize).max(1);
        self.scroll = self.scroll.min(n.saturating_sub(fits));
        let scrollbar = n > fits;
        let sbw = if scrollbar { 26.0 * dp } else { 0.0 };
        let (rx, rw) = (x0 + pad, lw - 2.0 * pad - sbw);
        if n == 0 {
            draw::centered(out, Serif, "No worlds yet", x0 + lw * 0.5, y0 + ph * 0.45, 22.0 * dp, INK, true);
            draw::centered(out, Serif, "Create one to start playing.", x0 + lw * 0.5, y0 + ph * 0.45 + 26.0 * dp, 16.0 * dp, DIM, true);
        }
        for (slot, i) in (self.scroll..n.min(self.scroll + fits)).enumerate() {
            let ry = y0 + pad + slot as f32 * rh;
            let picked = self.selected == Some(i);
            if picked {
                rect(out, rx, ry, rw, rh - 3.0 * dp, [70, 120, 220, 40]);
                let g = 18.0 * k;
                nine(out, "glow", rx - g, ry - g, rw + 2.0 * g, rh - 3.0 * dp + 2.0 * g, k, OPAQUE);
            } else if slot + 1 < fits && i + 1 < n {
                rect(out, rx + 4.0 * dp, ry + rh - 2.0 * dp, rw - 8.0 * dp, 1.0f32.max(dp * 0.5), FAINT);
            }
            let th = rh - 12.0 * dp;
            let tw = th * 16.0 / 9.0;
            let e = self.worlds[i].clone();
            self.thumb(out, e.thumb, rx + 6.0 * dp, ry + 4.5 * dp, tw, th);
            let tx = rx + 6.0 * dp + tw + 10.0 * dp;
            let pencil = 34.0 * dp;
            let room = rw - (tx - rx) - pencil - 10.0 * dp;
            let ns = draw::fit(Serif, &e.name, rh * 0.30, room);
            draw::left(out, Serif, &e.name, tx, ry + rh * 0.24, ns, INK, true);
            // The size at the right of the seed's line.
            let ss = (rh * 0.23).min(15.0 * dp);
            let size_w = draw::width(Serif, &e.size, ss);
            draw::right(out, Serif, &e.size, tx + room, ry + rh * 0.52, ss, INK, true);
            let seed = format!("Seed {}", e.seed);
            draw::left(out, Serif, &seed, tx, ry + rh * 0.52, draw::fit(Serif, &seed, rh * 0.23, room - size_w - 10.0 * dp), if picked { BLUE } else { DIM }, true);
            draw::left(out, Serif, &e.last_played, tx, ry + rh * 0.76, draw::fit(Serif, &e.last_played, rh * 0.22, room), DIM, true);
            self.buttons.push(Button { x: rx, y: ry, w: rw - pencil - 4.0 * dp, h: rh - 3.0 * dp, action: Action::SelectWorld(i) });
            let (ex, ey) = (rx + rw - pencil, ry + (rh - pencil) * 0.5 - 4.0 * dp);
            draw::icon(out, "edit", ex + pencil * 0.5, ey + pencil * 0.5, pencil * 0.62, INK);
            self.buttons.push(Button { x: ex, y: ey, w: pencil, h: pencil, action: Action::EditWorldAt(i) });
        }
        if scrollbar {
            let (sx, sy, sh) = (x0 + lw - pad - sbw + 8.0 * dp, y0 + pad, ph - 2.0 * pad);
            nine(out, "field", sx, sy, sbw - 8.0 * dp, sh, k * 0.6, OPAQUE);
            let frac = fits as f32 / n as f32;
            let pos = self.scroll as f32 / (n - fits).max(1) as f32;
            let th = (sh * frac).max(24.0 * dp);
            rect(out, sx + 3.0 * dp, sy + 3.0 * dp + (sh - th - 6.0 * dp) * pos, sbw - 14.0 * dp, th, [150, 150, 160, 230]);
            self.buttons.push(Button { x: sx - 4.0 * dp, y: sy, w: sbw, h: sh * 0.5, action: Action::ScrollWorlds(-1) });
            self.buttons.push(Button { x: sx - 4.0 * dp, y: sy + sh * 0.5, w: sbw, h: sh * 0.5, action: Action::ScrollWorlds(1) });
        }

        // The picked world.
        let Some(e) = self.picked().cloned() else {
            draw::centered(out, Serif, "Pick a world", dx + dw * 0.5, dy + dh * 0.5, 20.0 * dp, DIM, true);
            return;
        };
        let bh = 40.0 * dp;
        let info: [(&str, String); 3] = [("Seed", e.seed.clone()), ("Play Time", e.play_time.clone()), ("World Size", e.size.clone())];
        let line = (20.0 * dp).min(dh * 0.08).max(16.0 * dp);
        let s = (line * 0.78).min(16.0 * dp);
        let by = dy + dh - pad - bh;
        let bw = (dw - 2.0 * pad - 10.0 * dp) * 0.5;
        if stacked {
            // Picture on the left, name and details beside it, EDIT and DELETE under them.
            let th = (by - 10.0 * dp - dy - pad).max(20.0 * dp);
            let tw = (th * 16.0 / 9.0).min(dw * 0.45);
            let th = tw * 9.0 / 16.0;
            self.thumb(out, e.thumb, dx + pad, dy + pad, tw, th);
            let tx = dx + pad + tw + 12.0 * dp;
            let room = dx + dw - pad - tx;
            let mut ty = dy + pad + 12.0 * dp;
            draw::left(out, Serif, &e.name, tx, ty, draw::fit(Serif, &e.name, 22.0 * dp, room), INK, true);
            ty += 24.0 * dp;
            for (label, value) in info {
                let lx = draw::left(out, Serif, &format!("{label}: "), tx, ty, s, DIM, true);
                draw::left(out, Serif, &value, tx + lx, ty, draw::fit(Serif, &value, s, room - lx), INK, true);
                ty += line;
            }
        } else {
            let text_h = 28.0 * dp + line * info.len() as f32;
            let tw_max = dw - 2.0 * pad;
            let th = (tw_max * 9.0 / 16.0).min(dh - 2.0 * pad - text_h - bh - 18.0 * dp).max(30.0 * dp);
            let tw = th * 16.0 / 9.0;
            self.thumb(out, e.thumb, dx + (dw - tw) * 0.5, dy + pad, tw, th);
            let mut ty = dy + pad + th + 16.0 * dp;
            let ns = draw::fit(Serif, &e.name, 24.0 * dp, tw_max);
            draw::left(out, Serif, &e.name, dx + pad, ty, ns, INK, true);
            ty += 22.0 * dp;
            for (label, value) in info {
                let lx = draw::left(out, Serif, &format!("{label}: "), dx + pad, ty, s, DIM, true);
                draw::left(out, Serif, &value, dx + pad + lx, ty, draw::fit(Serif, &value, s, tw_max - lx), INK, true);
                ty += line;
            }
        }
        self.button(out, Style::Stone, (dx + pad, by, bw, bh), "Edit", Some("open_in_full"), Serif, Action::EditWorld, true, false);
        self.button(out, Style::Stone, (dx + pad + bw + 10.0 * dp, by, bw, bh), "Delete", Some("delete"), Serif, Action::AskDelete, true, false);
    }

    /// A world's picture in a thin frame (the default picture if it has none yet).
    fn thumb(&self, out: &mut Vec<HudVertex>, slot: Option<usize>, x: f32, y: f32, w: f32, h: f32) {
        let b = 2.0 * self.dp;
        nine(out, "field", x - b, y - b, w + 2.0 * b, h + 2.0 * b, self.k() * 0.5, OPAQUE);
        match slot.or(self.default_thumb) {
            Some(s) => draw::thumb(out, crate::gfx::thumb_uv(s), x, y, w, h),
            None => rect(out, x, y, w, h, [40, 60, 90, 255]),
        }
    }

    /// A text field: the field's frame, its text with a blinking caret while typed into, or
    /// a hint while empty.
    fn text_field(&mut self, out: &mut Vec<HudVertex>, (x, y, w, h): (f32, f32, f32, f32), field: usize, hint: &str) {
        let focused = self.focus == Some(field);
        draw::field(out, x, y, w, h, self.k() * 0.8);
        if focused {
            let g = 18.0 * self.k();
            nine(out, "glow", x - g, y - g, w + 2.0 * g, h + 2.0 * g, self.k(), OPAQUE);
        }
        let size = h * 0.5;
        let value = self.fields[field].clone();
        let tx = x + 12.0 * self.dp;
        if value.is_empty() && !focused {
            draw::left(out, Serif, hint, tx, y + h * 0.5, draw::fit(Serif, hint, size, w - 24.0 * self.dp), [110, 112, 120, 255], false);
        } else {
            let s = draw::fit(Serif, &format!("{value}|"), size, w - 24.0 * self.dp);
            let tw = draw::left(out, Serif, &value, tx, y + h * 0.5, s, INK, false);
            if focused && (self.seconds * 2.0).fract() < 0.55 {
                rect(out, tx + tw + 2.0 * self.dp, y + h * 0.2, 2.0 * self.dp, h * 0.6, INK);
            }
        }
        self.buttons.push(Button { x, y, w, h, action: Action::Focus(field as u8) });
    }

    /// CREATE NEW WORLD (`create`) or EDIT WORLD.
    fn draw_world_form(&mut self, out: &mut Vec<HudVertex>, create: bool) {
        let (w, h) = self.size;
        let dp = self.dp;
        let [sl, _, sr, sb] = self.margins();
        let cx = (sl + w - sr) * 0.5;
        let typing = self.focus.is_some();
        let top = self.top_bar(out, if create { "Create New World" } else { "Edit World" });
        let bottom = if typing {
            h - sb - self.keyboard_height() - 8.0 * dp
        } else {
            let named = !self.fields[0].trim().is_empty();
            let items = if create {
                [("Back", "undo", Action::Open(Screen::WorldList), true), ("Create World", "add", Action::CreateWorld, named)]
            } else {
                [("Back", "undo", Action::Open(Screen::WorldList), true), ("Save", "check", Action::SaveWorld, named)]
            };
            self.bottom_bar(out, &items) - 10.0 * dp
        };
        let pw = (600.0 * dp).min(w - sl - sr - 20.0 * dp);
        let px = cx - pw * 0.5;
        let ph = (bottom - top).max(60.0 * dp);
        draw::panel(out, px, top, pw, ph, self.k());
        self.buttons.push(Button { x: px, y: top, w: pw, h: ph, action: Action::Nothing });
        let pad = 16.0 * dp;
        let fields: &[(&str, &str)] =
            if create { &[("World Name", "New World"), ("Seed for the World Generator", "Leave blank for a random seed")] } else { &[("World Name", "New World")] };
        let label = 17.0 * dp;
        let fh = (42.0 * dp).min((ph - 2.0 * pad) / fields.len() as f32 - label - 10.0 * dp).max(28.0 * dp);
        let block = label + 6.0 * dp + fh;
        // Side by side when stacked fields don't fit above the keyboard.
        let side = create && block * 2.0 + 10.0 * dp > ph - 2.0 * pad;
        let n = fields.len() as f32;
        let fw = if side { (pw - 2.0 * pad - 12.0 * dp * (n - 1.0)) / n } else { pw - 2.0 * pad };
        let used = if side { block } else { block * n + 10.0 * dp * (n - 1.0) };
        let mut y = top + pad + ((ph - 2.0 * pad - used) * if typing { 0.0 } else { 0.4 }).max(0.0);
        for (i, (name, hint)) in fields.iter().enumerate() {
            let x = if side { px + pad + i as f32 * (fw + 12.0 * dp) } else { px + pad };
            draw::left(out, Serif, name, x, y + label * 0.5, label, INK, true);
            self.text_field(out, (x, y + label + 6.0 * dp, fw, fh), i, hint);
            if !side {
                y += block + 10.0 * dp;
            }
        }
        if !create {
            if let Some(e) = self.picked() {
                let s = format!("Seed {}", e.seed);
                draw::left(out, Serif, &s, px + pad, y + 8.0 * dp, 16.0 * dp, DIM, true);
            }
        }
        if typing {
            self.draw_keyboard(out);
        }
    }

    fn keyboard_height(&self) -> f32 {
        5.0 * self.key_h() + 4.0 * 5.0 * self.dp + 16.0 * self.dp
    }

    fn key_h(&self) -> f32 {
        (36.0 * self.dp).min(self.size.1 * 0.085).max(26.0 * self.dp)
    }

    /// The on-screen keyboard (Android's own does not reach a NativeActivity reliably).
    fn draw_keyboard(&mut self, out: &mut Vec<HudVertex>) {
        let (w, h) = self.size;
        let dp = self.dp;
        let [sl, _, sr, sb] = self.margins();
        let cx = (sl + w - sr) * 0.5;
        let kh = self.key_h();
        let gap = 5.0 * dp;
        let total_h = self.keyboard_height();
        let y0 = h - sb - total_h;
        let kw = ((w - sl - sr - 28.0 * dp - 9.0 * gap) / 10.0).min(66.0 * dp);
        let width = 10.0 * kw + 9.0 * gap;
        draw::panel(out, cx - width * 0.5 - 8.0 * dp, y0, width + 16.0 * dp, total_h, self.k());
        self.buttons.push(Button { x: cx - width * 0.5 - 8.0 * dp, y: y0, w: width + 16.0 * dp, h: total_h, action: Action::Nothing });
        let rows = ["1234567890", "qwertyuiop", "asdfghjkl", "zxcvbnm"];
        for (r, keys) in rows.iter().enumerate() {
            let n = keys.chars().count() as f32;
            let rw = n * kw + (n - 1.0) * gap;
            let ry = y0 + 8.0 * dp + r as f32 * (kh + gap);
            for (i, c) in keys.chars().enumerate() {
                let c = if self.caps { c.to_ascii_uppercase() } else { c };
                let x = cx - rw * 0.5 + i as f32 * (kw + gap);
                self.button(out, Style::Stone, (x, ry, kw, kh), &c.to_string(), None, Pixel, Action::Key(c), true, false);
            }
        }
        let ry = y0 + 8.0 * dp + 4.0 * (kh + gap);
        let x0 = cx - width * 0.5;
        let unit = kw + gap;
        let caps = self.caps;
        self.button(out, Style::Stone, (x0, ry, 1.5 * unit - gap, kh), "", Some("keyboard_capslock"), Pixel, Action::Shift, true, caps);
        self.button(out, Style::Stone, (x0 + 1.5 * unit, ry, unit - gap, kh), "-", None, Pixel, Action::Key('-'), true, false);
        self.button(out, Style::Stone, (x0 + 2.5 * unit, ry, 4.0 * unit - gap, kh), "", Some("space_bar"), Pixel, Action::Key(' '), true, false);
        self.button(out, Style::Stone, (x0 + 6.5 * unit, ry, 1.5 * unit - gap, kh), "", Some("backspace"), Pixel, Action::Backspace, true, false);
        self.button(out, Style::Stone, (x0 + 8.0 * unit, ry, 2.0 * unit - gap, kh), "Done", Some("check"), Pixel, Action::KeysDone, true, false);
    }

    fn draw_delete(&mut self, out: &mut Vec<HudVertex>) {
        let (w, _) = self.size;
        let dp = self.dp;
        let [sl, _, sr, _] = self.margins();
        let cx = (sl + w - sr) * 0.5;
        let top = self.top_bar(out, "Delete World");
        let bar = self.bottom_bar(out, &[("Cancel", "undo", Action::Open(Screen::WorldList), true), ("Delete", "delete", Action::ConfirmDelete, true)]);
        let pw = (560.0 * dp).min(w - sl - sr - 20.0 * dp);
        let ph = bar - 10.0 * dp - top;
        draw::panel(out, cx - pw * 0.5, top, pw, ph, self.k());
        let name = self.picked().map(|e| e.name.clone()).unwrap_or_default();
        let mid = top + ph * 0.5;
        draw::icon(out, "warning", cx, mid - 40.0 * dp, 44.0 * dp, WARN);
        let q = "Are you sure you want to delete this world?";
        draw::centered(out, Serif, q, cx, mid + 2.0 * dp, draw::fit(Serif, q, 21.0 * dp, pw * 0.9), INK, true);
        let l = format!("'{name}' will be lost forever! (A long time!)");
        draw::centered(out, Serif, &l, cx, mid + 30.0 * dp, draw::fit(Serif, &l, 17.0 * dp, pw * 0.9), DANGER, true);
    }

    /// The settings: groups on the left, the group's settings on the right.
    fn draw_settings(&mut self, out: &mut Vec<HudVertex>) {
        let (w, _) = self.size;
        let dp = self.dp;
        let k = self.k();
        let [sl, _, sr, _] = self.margins();
        if self.settings_back == Screen::Pause {
            let (w, h) = self.size;
            rect(out, 0.0, 0.0, w, h, [0, 0, 0, 120]);
        }
        let top = self.top_bar(out, "Settings");
        let back = self.settings_back;
        let bar = self.bottom_bar(
            out,
            &[("Back", "undo", Action::Open(back), true), ("Reset to Default", "restart_alt", Action::ResetSettings, true), ("Apply", "check", Action::ApplySettings, true)],
        );
        let (x0, x1) = (sl + 10.0 * dp, w - sr - 10.0 * dp);
        let (y0, y1) = (top, bar - 10.0 * dp);
        let tw = (220.0 * dp).min((x1 - x0) * 0.28);
        let th = (46.0 * dp).min((y1 - y0 - 2.0 * 8.0 * dp) / TABS.len() as f32);
        for (i, (name, icon)) in TABS.iter().enumerate() {
            let y = y0 + i as f32 * (th + 8.0 * dp);
            let on = self.tab as usize == i;
            self.button(out, Style::Stone, (x0, y, tw, th), name, Some(icon), Serif, Action::Tab(i as u8), true, on);
        }
        let px = x0 + tw + 16.0 * dp;
        let (pw, ph) = (x1 - px, y1 - y0);
        draw::panel(out, px, y0, pw, ph, k);
        self.buttons.push(Button { x: px, y: y0, w: pw, h: ph, action: Action::Nothing });

        let s = self.draft;
        let on_off = |b: bool, a: Action| Control::Switch(b, a);
        let rows: Vec<(&str, Control)> = match self.tab {
            0 => {
                let mut v = vec![
                    ("Mode", Control::Choice(if s.walk { "Walk".into() } else { "Fly".into() }, Action::ToggleWalk)),
                    (
                        "Camera",
                        Control::Choice(
                            match s.view {
                                super::CamView::First => "First Person".into(),
                                super::CamView::Back => "Behind".into(),
                                super::CamView::Front => "In Front".into(),
                            },
                            Action::ToggleView,
                        ),
                    ),
                    ("Show Stats", on_off(s.show_stats, Action::ToggleStats)),
                ];
                v.push(if back == Screen::Pause {
                    ("Time of Day", Control::Stepper(crate::sky::time_label(self.hours), Action::Time(-1), Action::Time(1)))
                } else {
                    ("Time of Day", Control::Fixed("Set in each world".into()))
                });
                v
            }
            1 => vec![
                ("Render Distance", Control::Stepper(format!("{} chunks", s.render_chunks), Action::RenderDist(-1), Action::RenderDist(1))),
                ("LOD Distance", Control::Stepper(lod_label(s.lod_level), Action::LodDist(-1), Action::LodDist(1))),
                ("LOD Quality", Control::Choice(quality_name(s.lod_quality), Action::CycleLodQuality)),
                ("Fog", on_off(s.fog, Action::ToggleFog)),
                ("Clouds", on_off(s.clouds, Action::ToggleClouds)),
                ("God Rays", on_off(s.god_rays, Action::ToggleRays)),
                if self.vsync_supported {
                    ("VSync", on_off(s.vsync, Action::ToggleVsync))
                } else {
                    ("VSync", Control::Fixed("Always on".into()))
                },
            ],
            _ => vec![
                ("Culling", Control::Choice(cull_name(s.cull), Action::CycleCull)),
                ("Mesher", Control::Choice(if s.greedy { "Greedy".into() } else { "Simple".into() }, Action::ToggleMesher)),
            ],
        };
        let pad = 14.0 * dp;
        let n = rows.len().max(1);
        let cols = if (ph - 2.0 * pad) / (n as f32) < 40.0 * dp && pw > 520.0 * dp { 2 } else { 1 };
        let per = n.div_ceil(cols);
        let full_pw = pw;
        let pw = (full_pw - (cols - 1) as f32 * pad) / cols as f32;
        let rh = (46.0 * dp).min((ph - 2.0 * pad) / per as f32);
        let cw = (200.0 * dp).min(pw * 0.46);
        let ch = (rh * 0.78).min(38.0 * dp);
        let x_panel = px;
        for (i, (label, control)) in rows.into_iter().enumerate() {
            let (col, row) = (i / per, i % per);
            let px = x_panel + col as f32 * (pw + pad);
            let ry = y0 + pad + row as f32 * rh;
            let cy = ry + rh * 0.5;
            if col > 0 && row == 0 {
                rect(out, px - pad * 0.5, y0 + pad, (0.5 * dp).max(1.0), ph - 2.0 * pad, FAINT);
            }
            let ls = draw::fit(Serif, label, (rh * 0.44).min(20.0 * dp), pw - cw - 3.0 * pad);
            draw::left(out, Serif, label, px + pad + 4.0 * dp, cy, ls, INK, true);
            if row > 0 {
                rect(out, px + pad, ry, pw - 2.0 * pad, (0.5 * dp).max(1.0), FAINT);
                let g = 7.0 * dp;
                draw::image(out, "gem", px + pad - g * 0.5, ry - g * 0.5, g, g, OPAQUE);
                draw::image(out, "gem", px + pw - pad - g * 0.5, ry - g * 0.5, g, g, OPAQUE);
            }
            let cx = px + pw - pad - cw;
            let vs = (ch * 0.5).min(18.0 * dp);
            match control {
                Control::Switch(on, a) => {
                    let sw = ch * 132.0 / 52.0;
                    let sx = px + pw - pad - sw;
                    draw::image(out, if on { "toggle_on" } else { "toggle_off" }, sx, cy - ch * 0.5, sw, ch, OPAQUE);
                    let label = if on { "ON" } else { "OFF" };
                    let lx = if on { sx + sw * 0.27 } else { sx + sw * 0.66 };
                    draw::centered(out, Serif, label, lx, cy, vs, INK, true);
                    self.buttons.push(Button { x: sx, y: ry, w: sw, h: rh, action: a });
                }
                Control::Stepper(value, down, up) => {
                    draw::field(out, cx, cy - ch * 0.5, cw, ch, k * 0.8);
                    draw::centered(out, Serif, &value, cx + cw * 0.5, cy, draw::fit(Serif, &value, vs, cw - 1.7 * ch), INK, false);
                    draw::icon(out, "chevron_left", cx + ch * 0.5, cy, ch * 0.7, BLUE);
                    draw::icon(out, "chevron_right", cx + cw - ch * 0.5, cy, ch * 0.7, BLUE);
                    self.buttons.push(Button { x: cx, y: ry, w: cw * 0.5, h: rh, action: down });
                    self.buttons.push(Button { x: cx + cw * 0.5, y: ry, w: cw * 0.5, h: rh, action: up });
                }
                Control::Choice(value, a) => {
                    draw::field(out, cx, cy - ch * 0.5, cw, ch, k * 0.8);
                    draw::left(out, Serif, &value, cx + 12.0 * dp, cy, draw::fit(Serif, &value, vs, cw - ch - 16.0 * dp), INK, false);
                    draw::icon(out, "expand_more", cx + cw - ch * 0.5, cy, ch * 0.7, INK);
                    self.buttons.push(Button { x: cx, y: ry, w: cw, h: rh, action: a });
                }
                Control::Fixed(value) => {
                    draw::right(out, Serif, &value, px + pw - pad, cy, draw::fit(Serif, &value, vs, cw), DIM, true);
                }
            }
        }
    }

    /// The game menu, over the world.
    fn draw_pause(&mut self, out: &mut Vec<HudVertex>) {
        let (w, h) = self.size;
        let dp = self.dp;
        let [sl, st, sr, sb] = self.margins();
        rect(out, 0.0, 0.0, w, h, [0, 0, 0, 120]);
        let cx = (sl + w - sr) * 0.5;
        let ph = 50.0 * dp;
        let pw = (340.0 * dp).min(w - sl - sr - 20.0 * dp);
        let gap = 10.0 * dp;
        let bh = (48.0 * dp).min((h - st - sb - ph - 40.0 * dp - 2.0 * gap) / 3.0);
        let total = ph + 20.0 * dp + 3.0 * bh + 2.0 * gap;
        let y0 = st + ((h - st - sb) - total) * 0.5;
        nine(out, "plaque", cx - pw * 0.5, y0, pw, ph, self.k(), OPAQUE);
        draw::centered(out, Serif, "Game Menu", cx, y0 + ph * 0.5, ph * 0.52, INK, true);
        let bw = (320.0 * dp).min(w - sl - sr - 20.0 * dp);
        let items: [(&str, &str, Action); 3] = [
            ("Back to Game", "play_arrow", Action::Open(Screen::Playing)),
            ("Settings", "settings", Action::OpenSettings),
            ("Save & Quit to Title", "logout", Action::Open(Screen::Title)),
        ];
        for (i, (label, icon, action)) in items.iter().enumerate() {
            let y = y0 + ph + 20.0 * dp + i as f32 * (bh + gap);
            self.button(out, Style::Stone, (cx - bw * 0.5, y, bw, bh), label, Some(icon), Serif, *action, true, false);
        }
    }

    fn draw_loading(&mut self, out: &mut Vec<HudVertex>) {
        let (w, h) = self.size;
        let dp = self.dp;
        let [sl, _, sr, _] = self.margins();
        let cx = (sl + w - sr) * 0.5;
        let lw = ((w - sl - sr) * 0.5).min(460.0 * dp);
        let lh = lw / 3.5;
        let ly = h * 0.42 - lh;
        draw::image(out, "logo", cx - lw * 0.5, ly, lw, lh, OPAQUE);
        draw::centered(out, Serif, "Loading terrain", cx, ly + lh + 22.0 * dp, 22.0 * dp, INK, true);
        let bw = (320.0 * dp).min((w - sl - sr) * 0.7);
        let bh = 16.0 * dp;
        let by = ly + lh + 44.0 * dp;
        nine(out, "field", cx - bw * 0.5, by, bw, bh, self.k() * 0.5, OPAQUE);
        let f = (self.loading_chunks as f32 / 120.0).min(1.0);
        let inner = bw - 6.0 * dp;
        rect(out, cx - bw * 0.5 + 3.0 * dp, by + 3.0 * dp, inner * f, bh - 6.0 * dp, [60, 140, 255, 255]);
        rect(out, cx - bw * 0.5 + 3.0 * dp, by + 3.0 * dp, inner * f, (bh - 6.0 * dp) * 0.45, [130, 190, 255, 255]);
        let n = format!("{} chunks", self.loading_chunks);
        draw::centered(out, Serif, &n, cx, by + bh + 16.0 * dp, 15.0 * dp, DIM, true);
    }

    /// MULTI PLAYER, MODS, CUSTOMIZE: what is (not) there yet.
    fn draw_page(&mut self, out: &mut Vec<HudVertex>, title: &str, icon: &str, lines: &[&str]) {
        let (w, _) = self.size;
        let dp = self.dp;
        let [sl, _, sr, _] = self.margins();
        let cx = (sl + w - sr) * 0.5;
        let top = self.top_bar(out, title);
        let bar = self.bottom_bar(out, &[("Back", "undo", Action::Open(Screen::Title), true)]);
        let pw = (600.0 * dp).min(w - sl - sr - 20.0 * dp);
        let ph = bar - 10.0 * dp - top;
        draw::panel(out, cx - pw * 0.5, top, pw, ph, self.k());
        let size = lines.iter().map(|l| draw::fit(Serif, l, 18.0 * dp, pw * 0.88)).fold(18.0 * dp, f32::min);
        let line = size * 1.5;
        let block = 54.0 * dp + line * lines.len() as f32;
        let mut y = top + (ph - block) * 0.5;
        draw::icon(out, icon, cx, y + 20.0 * dp, 44.0 * dp, BLUE);
        y += 54.0 * dp;
        for (i, l) in lines.iter().enumerate() {
            draw::centered(out, Serif, l, cx, y + line * 0.5, size, if i == 0 { INK } else { DIM }, true);
            y += line;
        }
    }

    /// The HUD in the game: crosshair, stats, the menu button and the character's buttons.
    pub(super) fn draw_hud(&mut self, perf: &Perf, out: &mut Vec<HudVertex>) {
        let (w, h) = self.size;
        let _ = h;
        let dp = self.dp;
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
        let bs = 48.0 * dp;
        let (bx, by) = (w - safe_right - margin - bs, safe_top + 4.0 * dp);
        self.button(out, Style::Stone, (bx, by, bs, bs), "", Some("pause"), Pixel, Action::Open(Screen::Pause), true, false);
        if self.settings.walk {
            self.draw_character_buttons(out, bx, by, bs);
        }
    }

    /// The character's controls while walking: jump, roll, crouch and crawl at the bottom
    /// right (under the right thumb), and ACT beside the menu button, which opens a grid of
    /// everything else he can do.
    fn draw_character_buttons(&mut self, out: &mut Vec<HudVertex>, menu_x: f32, menu_y: f32, bs: f32) {
        let dp = self.dp;
        let h = self.size.1;
        let gap = 6.0 * dp;
        let right = menu_x + bs;
        let (cw, ch) = (78.0 * dp, 52.0 * dp);
        let bottom = h - self.margins()[3] - 10.0 * dp;
        let pad: [[(&str, Action); 2]; 2] = [
            [("Crouch", Action::Press("crouch")), ("Crawl", Action::Press("crawl"))],
            [("Roll", Action::Hold("roll")), ("Jump", Action::Hold("jump"))],
        ];
        for (r, row) in pad.iter().enumerate() {
            for (c, (label, action)) in row.iter().enumerate() {
                let x = right - (2 - c) as f32 * cw - (1 - c) as f32 * gap;
                let y = bottom - (2 - r) as f32 * ch - (1 - r) as f32 * gap;
                self.button(out, Style::Stone, (x, y, cw, ch), label, None, Pixel, *action, true, false);
            }
        }
        let aw = bs * 1.6;
        let ax = menu_x - gap - aw;
        let acts = self.acts_open;
        self.button(out, Style::Stone, (ax, menu_y, aw, bs), "Act", None, Pixel, Action::ToggleActs, true, acts);
        if !self.acts_open {
            return;
        }
        let grid: [[(&str, Action); 4]; 4] = [
            [("Punch", Action::Press("punch")), ("Use", Action::Press("interact")), ("Sit", Action::Press("sit")), ("1 Hand", Action::Press("one_handed"))],
            [("Swing", Action::Press("swing")), ("Use L", Action::Press("interact_left")), ("Wave", Action::Press("wave")), ("1 Hand L", Action::Press("one_handed_left"))],
            [("Chop", Action::Press("chop")), ("Eat", Action::Press("eat")), ("Die", Action::Press("die")), ("2 Hands", Action::Press("two_handed"))],
            [("Hit", Action::Press("hit")), ("View", Action::ToggleView), ("Revive", Action::Press("revive")), ("Carry", Action::Press("two_handed_item"))],
        ];
        let top = menu_y + bs + gap;
        let space = (bottom - 2.0 * ch - 2.0 * gap) - top;
        let gh = (44.0 * dp).min((space - 3.0 * gap - 12.0 * dp) / 4.0).max(30.0 * dp);
        let room = right - self.margins()[0] - 8.0 * dp - 2.0 * gap - 4.0 * dp;
        let gw = (88.0 * dp).min((room - 3.0 * gap) / 4.0);
        let left = right - 4.0 * gw - 3.0 * gap;
        let (px, py, pw, ph) = (left - gap - 4.0 * dp, top - gap - 2.0 * dp, 4.0 * gw + 5.0 * gap + 8.0 * dp, 4.0 * gh + 5.0 * gap + 4.0 * dp);
        draw::panel(out, px, py, pw, ph, self.k() * 0.7);
        self.buttons.push(Button { x: px, y: py, w: pw, h: ph, action: Action::Nothing });
        for (r, row) in grid.iter().enumerate() {
            for (c, (label, action)) in row.iter().enumerate() {
                self.button(out, Style::Stone, (left + c as f32 * (gw + gap), top + r as f32 * (gh + gap), gw, gh), label, None, Pixel, *action, true, false);
            }
        }
    }
}

fn quality_name(q: u8) -> String {
    match LOD_QUALITY[q as usize % LOD_QUALITY.len()].0 {
        "LOW" => "Low".into(),
        "MED" => "Medium".into(),
        "HIGH" => "High".into(),
        other => other.into(),
    }
}

fn cull_name(c: super::CullMode) -> String {
    match c {
        super::CullMode::All => "All".into(),
        super::CullMode::NoCaves => "No Caves".into(),
        super::CullMode::Frustum => "Frustum".into(),
        super::CullMode::Back => "Back Faces".into(),
        super::CullMode::Off => "Off".into(),
    }
}
