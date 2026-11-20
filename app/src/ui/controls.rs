//! The game's on-screen controls: what they are, where they sit (the player can move,
//! resize, hide and add them in the controls editor), the hotbar and its block picker, the
//! floating stick, and the editor itself.

use super::draw::{self, nine};
use super::screens::Style;
use super::{items, rect, Action, Button, HudVertex, Request, Screen, Stack, Ui};
use crate::atlas::FontId::Serif;

/// How a control looks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Look {
    /// A round stone button (attack, place, jump).
    Round,
    /// A square stone button with a word or an icon.
    Square,
    /// The hotbar: nine block slots and the button that opens the block picker.
    Hotbar,
}

/// One on-screen control of the game.
pub struct Control {
    /// Its name in `controls.txt`.
    pub key: &'static str,
    /// Its name in the editor.
    pub name: &'static str,
    pub look: Look,
    /// Width and height in dp at scale 1.
    pub size: (f32, f32),
    pub label: &'static str,
    /// An atlas icon, or "@pickaxe" (the atlas picture) or "@block" (the hotbar's block).
    pub icon: Option<&'static str>,
    pub action: Action,
    /// Only while walking as the character (the fly camera has no body).
    pub walk: bool,
    /// On screen until hidden (the extras start hidden: the editor's ADD puts them on).
    pub shown: bool,
    /// Can't be hidden: the pause button is the only way to the game menu.
    pub keep: bool,
}

const SLOT_DP: f32 = 40.0;
const HOTBAR_DP: (f32, f32) = (9.0 * SLOT_DP + 6.0 + SLOT_DP, SLOT_DP);

#[allow(clippy::too_many_arguments)]
const fn ctl(key: &'static str, name: &'static str, look: Look, size: (f32, f32), label: &'static str, icon: Option<&'static str>, action: Action, walk: bool, shown: bool) -> Control {
    Control { key, name, look, size, label, icon, action, walk, shown, keep: false }
}

/// Every control, in drawing order.
pub const CONTROLS: [Control; 18] = [
    ctl("hotbar", "Hotbar", Look::Hotbar, HOTBAR_DP, "", None, Action::Nothing, false, true),
    ctl("attack", "Attack", Look::Round, (82.0, 82.0), "", Some("@pickaxe"), Action::Hold("attack"), false, true),
    ctl("place", "Place", Look::Round, (64.0, 64.0), "", Some("@block"), Action::Hold("place"), false, true),
    ctl("jump", "Jump", Look::Round, (72.0, 72.0), "", Some("keyboard_double_arrow_up"), Action::Hold("jump"), true, true),
    // Flying: held, sinks (JUMP rises).
    ctl("down", "Down", Look::Round, (60.0, 60.0), "", Some("keyboard_double_arrow_down"), Action::Hold("down"), false, true),
    ctl("roll", "Roll", Look::Square, (64.0, 40.0), "Roll", None, Action::Hold("roll"), true, true),
    ctl("crouch", "Crouch", Look::Square, (64.0, 40.0), "Crouch", None, Action::Press("crouch"), true, true),
    ctl("crawl", "Crawl", Look::Square, (64.0, 40.0), "Crawl", None, Action::Press("crawl"), true, true),
    // Instead of the three above when the setting asks for one button: tap to crouch, hold to
    // crawl, double tap to roll.
    ctl("stance", "Crouch / Crawl / Roll", Look::Square, (72.0, 44.0), "Crouch", None, Action::Hold("stance"), true, true),
    ctl("act", "Act", Look::Square, (76.0, 48.0), "Act", None, Action::ToggleActs, true, true),
    Control { key: "pause", name: "Pause", look: Look::Square, size: (48.0, 48.0), label: "", icon: Some("pause"), action: Action::Open(Screen::Pause), walk: false, shown: true, keep: true },
    // Extras: the editor's ADD puts them on screen.
    ctl("view", "Camera", Look::Square, (64.0, 40.0), "", Some("videocam"), Action::ToggleView, true, false),
    ctl("use", "Use", Look::Square, (64.0, 40.0), "Use", None, Action::Press("interact"), true, false),
    ctl("swing", "Swing", Look::Square, (64.0, 40.0), "Swing", None, Action::Press("swing"), true, false),
    ctl("eat", "Eat", Look::Square, (64.0, 40.0), "Eat", None, Action::Press("eat"), true, false),
    ctl("wave", "Wave", Look::Square, (64.0, 40.0), "Wave", None, Action::Press("wave"), true, false),
    ctl("sit", "Sit", Look::Square, (64.0, 40.0), "Sit", None, Action::Press("sit"), true, false),
    ctl("stats", "Stats", Look::Square, (64.0, 40.0), "", Some("speed"), Action::ToggleStats, false, false),
];

/// The joystick's radius in dp for each `Settings::joy_size`.
pub const JOY_RADII: [f32; 3] = [44.0, 56.0, 68.0];
pub const JOY_SIZES: [&str; 3] = ["Small", "Medium", "Large"];

/// Where the player put a control: its centre as fractions of the screen (`None`: where it
/// goes by default), its size relative to the default, and whether it is on screen.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Place {
    pub at: Option<[f32; 2]>,
    pub scale: f32,
    pub shown: bool,
}

pub const SCALE_MIN: f32 = 0.5;
pub const SCALE_MAX: f32 = 2.0;

/// Every control's place, in `CONTROLS` order; saved as `controls.txt`.
#[derive(Clone, PartialEq, Debug)]
pub struct Layout {
    pub places: Vec<Place>,
}

impl Default for Layout {
    fn default() -> Self {
        Layout { places: CONTROLS.iter().map(|c| Place { at: None, scale: 1.0, shown: c.shown }).collect() }
    }
}

impl Layout {
    /// `key=x,y,scale,shown` lines (`-,-` for a control in its default spot).
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        for (c, p) in CONTROLS.iter().zip(&self.places) {
            let at = p.at.map_or("-,-".to_string(), |a| format!("{:.4},{:.4}", a[0], a[1]));
            s += &format!("{}={},{:.2},{}\n", c.key, at, p.scale, p.shown as u8);
        }
        s
    }

    /// Reads what `to_text` wrote. Unknown controls and bad values are skipped, numbers
    /// clamped, so an old or damaged file still gives a usable layout.
    pub fn from_text(text: &str) -> Layout {
        let mut l = Layout::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let Some(i) = CONTROLS.iter().position(|c| c.key == key.trim()) else { continue };
            let v: Vec<&str> = value.split(',').map(str::trim).collect();
            if v.len() != 4 {
                continue;
            }
            let num = |s: &str| s.parse::<f32>().ok().filter(|x| x.is_finite());
            let at = match (num(v[0]), num(v[1])) {
                (Some(x), Some(y)) => Some([x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)]),
                _ => None,
            };
            let Some(scale) = num(v[2]) else { continue };
            l.places[i] = Place { at, scale: scale.clamp(SCALE_MIN, SCALE_MAX), shown: v[3] != "0" || CONTROLS[i].keep };
        }
        l
    }
}

/// A block the player can place: its id and name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockItem {
    pub id: u16,
    pub name: String,
}

/// The stick as drawn: where it sits, where the thumb is, its radius and how visible it is
/// (it fades out after the thumb lifts).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Joy {
    pub base: [f32; 2],
    pub knob: [f32; 2],
    pub radius: f32,
    pub alpha: f32,
}

const INK: [u8; 4] = [240, 232, 216, 255];
const DIM: [u8; 4] = [166, 172, 186, 255];

impl Ui {
    /// Control `i`'s default centre (pixels) and the scale it gets there.
    fn default_place(&self, i: usize) -> (f32, f32, f32) {
        let dp = self.dp;
        let (w, h) = self.size;
        let [sl, st, sr, sb] = self.margins();
        let (r, b, t) = (w - sr - 10.0 * dp, h - sb - 10.0 * dp, st + 4.0 * dp);
        let at = |x: f32, y: f32| (r - x * dp, b - y * dp, 1.0);
        match CONTROLS[i].key {
            "pause" => (r - 24.0 * dp, t + 24.0 * dp, 1.0),
            "act" => (r - (48.0 + 8.0 + 38.0) * dp, t + 24.0 * dp, 1.0),
            "jump" => at(36.0, 36.0),
            "down" => at(198.0, 102.0),
            "attack" => at(123.0, 47.0),
            "place" => at(36.0, 118.0),
            "roll" | "stance" => at(123.0, 118.0),
            "crouch" => at(123.0, 166.0),
            "crawl" => at(36.0, 178.0),
            "hotbar" => {
                // Between the left edge and the attack button, centred on the screen if it can.
                let (x0, x1) = (sl + 10.0 * dp, r - 172.0 * dp);
                let full = HOTBAR_DP.0 * dp;
                let s = ((x1 - x0) / full).clamp(0.5, 1.0);
                let half = full * s * 0.5;
                let cx = ((sl + w - sr) * 0.5).clamp(x0 + half, (x1 - half).max(x0 + half));
                (cx, b - HOTBAR_DP.1 * dp * s * 0.5, s)
            }
            _ => {
                // Extras: a row across the middle, where the editor shows them first.
                let first = CONTROLS.iter().position(|c| c.key == "view").unwrap_or(0);
                let k = i.saturating_sub(first) as f32;
                (w * 0.5 + (k % 4.0 - 1.5) * 76.0 * dp, h * 0.42 + (k / 4.0).floor() * 52.0 * dp, 1.0)
            }
        }
    }

    /// Control `i`'s rectangle on screen: x, y, w, h (kept inside the safe area).
    pub fn control_rect(&self, i: usize) -> [f32; 4] {
        let c = &CONTROLS[i];
        let p = self.layout.places[i];
        let (dx, dy, ds) = self.default_place(i);
        let s = ds * p.scale;
        let (cw, ch) = (c.size.0 * self.dp * s, c.size.1 * self.dp * s);
        let (cx, cy) = p.at.map_or((dx, dy), |a| (a[0] * self.size.0, a[1] * self.size.1));
        let [sl, st, sr, sb] = self.margins();
        let (w, h) = self.size;
        let x = (cx - cw * 0.5).min(w - sr - cw).max(sl);
        let y = (cy - ch * 0.5).min(h - sb - ch).max(st);
        [x, y, cw, ch]
    }

    /// Whether control `i` is on screen now (walk-only ones need the character).
    fn control_visible(&self, i: usize) -> bool {
        self.layout.places[i].shown && self.mode_fits(i) && self.stance_fits(i)
    }

    /// Whether control `i` belongs to the mode he is in: walking-only buttons, JUMP (which
    /// also rises in flight) and DOWN (flight only).
    fn mode_fits(&self, i: usize) -> bool {
        match CONTROLS[i].key {
            "jump" => true,
            "down" => !self.settings.walk,
            _ => !CONTROLS[i].walk || self.settings.walk,
        }
    }

    /// Whether control `i` belongs to the stance setting in force: the three buttons, or the
    /// one that does all three.
    fn stance_fits(&self, i: usize) -> bool {
        match CONTROLS[i].key {
            "roll" | "crouch" | "crawl" => !self.settings.one_stance,
            "stance" => self.settings.one_stance,
            _ => true,
        }
    }

    /// The icon slot of the block with this id (`gfx::icon_uv`).
    pub fn icon_of(&self, id: u16) -> Option<usize> {
        self.blocks.iter().position(|b| b.id == id)
    }

    /// The block in the hotbar's picked slot (0: none).
    pub fn held_block(&self) -> u16 {
        self.hotbar[self.slot.min(8)].id
    }

    fn alpha(&self, settings_alpha: u8) -> [u8; 4] {
        [255, 255, 255, (settings_alpha as u32 * 255 / 100) as u8]
    }

    /// One control, as it looks in the game (`grab`: in the editor, where it is picked up
    /// rather than pressed).
    fn draw_control(&mut self, out: &mut Vec<HudVertex>, i: usize, grab: bool) {
        let c = &CONTROLS[i];
        let [x, y, w, h] = self.control_rect(i);
        let opacity = self.settings.hud_opacity;
        let tint = self.alpha(opacity);
        let ink = [INK[0], INK[1], INK[2], tint[3]];
        let k = self.dp * 0.42 * (w / (c.size.0 * self.dp)).clamp(0.6, 1.4);
        let down = !grab && self.touching.contains(&c.action);
        let lit = down || (c.action == Action::ToggleActs && self.acts_open) || (grab && self.sel_ctl == Some(i));
        match c.look {
            Look::Round => {
                draw::image(out, "round", x, y, w, h, if down { [200, 204, 216, tint[3]] } else { tint });
                if lit {
                    // (the ring sits 14% in from the picture's edge)
                    let g = w * 0.14 / 0.72;
                    draw::image(out, "glow_round", x - g, y - g, w + 2.0 * g, h + 2.0 * g, [255, 255, 255, 230]);
                }
                let (cx, cy) = (x + w * 0.5, y + h * 0.5);
                match c.icon {
                    Some("@pickaxe") => draw::image(out, "pickaxe", cx - w * 0.3, cy - h * 0.3, w * 0.6, h * 0.6, tint),
                    Some("@block") => {
                        if let Some(icon) = self.icon_of(self.held_block()) {
                            draw::thumb(out, crate::gfx::icon_uv(icon), cx - w * 0.3, cy - h * 0.32, w * 0.6, h * 0.6);
                        }
                        draw::icon(out, "add", x + w * 0.76, y + h * 0.76, w * 0.32, [150, 200, 255, tint[3]]);
                    }
                    Some(name) => {
                        draw::icon(out, name, cx + 1.5, cy + 1.5, w * 0.5, [0, 0, 0, tint[3] / 2]);
                        draw::icon(out, name, cx, cy, w * 0.5, ink);
                    }
                    None => {}
                }
                self.buttons.push(Button { x, y, w, h, action: if grab { Action::CtlGrab(i as u8) } else { c.action } });
            }
            Look::Square => {
                nine(out, "button", x, y, w, h, k, if down { [200, 204, 216, tint[3]] } else { tint });
                if lit {
                    let g = 18.0 * k;
                    nine(out, "glow", x - g, y - g, w + 2.0 * g, h + 2.0 * g, k, [255, 255, 255, 230]);
                }
                let (cx, cy) = (x + w * 0.5, y + h * 0.5);
                if let Some(name) = c.icon {
                    draw::icon(out, name, cx, cy, h * 0.56, ink);
                } else {
                    let size = draw::fit(Serif, c.label, h * 0.46, w * 0.8);
                    draw::centered(out, Serif, c.label, cx, cy, size, ink, true);
                }
                self.buttons.push(Button { x, y, w, h, action: if grab { Action::CtlGrab(i as u8) } else { c.action } });
            }
            Look::Hotbar => {
                let s = h; // one slot
                for n in 0..9 {
                    let sx = x + n as f32 * s;
                    nine(out, "slot", sx, y, s, s, k * 0.8, tint);
                    if let Some(icon) = self.icon_of(self.hotbar[n].id) {
                        draw::thumb(out, crate::gfx::icon_uv(icon), sx + s * 0.14, y + s * 0.12, s * 0.72, s * 0.72);
                    }
                    if !grab {
                        self.buttons.push(Button { x: sx, y, w: s, h: s, action: Action::Slot(n as u8) });
                    }
                }
                let sel = x + self.slot.min(8) as f32 * s;
                let g = 18.0 * k;
                nine(out, "glow", sel - g, y - g, s + 2.0 * g, s + 2.0 * g, k, [255, 255, 255, 255]);
                let ix = x + w - s;
                nine(out, "button", ix, y, s, s, k * 0.8, tint);
                draw::icon(out, "backpack", ix + s * 0.5, y + s * 0.5, s * 0.58, ink);
                if grab {
                    self.buttons.push(Button { x, y, w, h, action: Action::CtlGrab(i as u8) });
                } else {
                    self.buttons.push(Button { x: ix, y, w: s, h: s, action: Action::ToggleInventory });
                    // The picked block's name, for a moment after picking it.
                    if self.seconds - self.slot_changed < 1.6 {
                        if let Some(b) = self.blocks.iter().find(|b| b.id == self.held_block()) {
                            let name = b.name.clone();
                            draw::centered(out, Serif, &name, x + 4.5 * s, y - 14.0 * self.dp, 17.0 * self.dp, INK, true);
                        }
                    }
                }
            }
        }
    }

    /// The game's controls, the stick, the action grid and the block picker.
    pub(super) fn draw_controls(&mut self, out: &mut Vec<HudVertex>) {
        if let Some(j) = self.joystick {
            let a = (j.alpha.clamp(0.0, 1.0) * 255.0) as u8;
            let r = j.radius;
            draw::image(out, "joy_base", j.base[0] - r, j.base[1] - r, 2.0 * r, 2.0 * r, [255, 255, 255, a]);
            let kr = r * 0.48;
            draw::image(out, "joy_knob", j.knob[0] - kr, j.knob[1] - kr, 2.0 * kr, 2.0 * kr, [255, 255, 255, a]);
        }
        for i in 0..CONTROLS.len() {
            if self.control_visible(i) {
                self.draw_control(out, i, false);
            }
        }
        let act = CONTROLS.iter().position(|c| c.key == "act").unwrap_or(0);
        if self.acts_open && self.control_visible(act) {
            let r = self.control_rect(act);
            self.draw_act_grid(out, r);
        }

    }

    /// Everything else the character can do, in a grid by the ACT button.
    fn draw_act_grid(&mut self, out: &mut Vec<HudVertex>, act: [f32; 4]) {
        let dp = self.dp;
        let (w, h) = self.size;
        let [sl, st, sr, sb] = self.margins();
        let grid: [[(&str, Action); 4]; 4] = [
            [("Punch", Action::Press("punch")), ("Use", Action::Press("interact")), ("Sit", Action::Press("sit")), ("1 Hand", Action::Press("one_handed"))],
            [("Swing", Action::Press("swing")), ("Use L", Action::Press("interact_left")), ("Wave", Action::Press("wave")), ("1 Hand L", Action::Press("one_handed_left"))],
            [("Chop", Action::Press("chop")), ("Eat", Action::Press("eat")), ("Die", Action::Press("die")), ("2 Hands", Action::Press("two_handed"))],
            [("Hit", Action::Press("hit")), ("View", Action::ToggleView), ("Revive", Action::Press("revive")), ("Carry", Action::Press("two_handed_item"))],
        ];
        let gap = 6.0 * dp;
        let gw = (84.0 * dp).min((w - sl - sr - 40.0 * dp) / 4.0);
        let gh = (40.0 * dp).min((h - st - sb - 40.0 * dp) / 4.0 - gap);
        let (pw, ph) = (4.0 * gw + 5.0 * gap, 4.0 * gh + 5.0 * gap);
        // Under the button if there is room, else above it; right edges lined up.
        let px = (act[0] + act[2] - pw).clamp(sl, (w - sr - pw).max(sl));
        let below = act[1] + act[3] + gap;
        let py = if below + ph <= h - sb { below } else { (act[1] - gap - ph).max(st) };
        draw::panel(out, px, py, pw, ph, dp * 0.3);
        self.buttons.push(Button { x: px, y: py, w: pw, h: ph, action: Action::Nothing });
        for (r, row) in grid.iter().enumerate() {
            for (c, (label, action)) in row.iter().enumerate() {
                let (bx, by) = (px + gap + c as f32 * (gw + gap), py + gap + r as f32 * (gh + gap));
                self.button(out, Style::Stone, (bx, by, gw, gh), label, None, Serif, *action, true, false);
            }
        }
    }

    /// The controls editor: every control on screen, to drag about; a toolbar to resize,
    /// hide, add and reset them.
    pub(super) fn draw_controls_editor(&mut self, out: &mut Vec<HudVertex>) {
        let (w, h) = self.size;
        let dp = self.dp;
        let [sl, st, sr, _] = self.margins();
        rect(out, 0.0, 0.0, w, h, [0, 0, 0, if self.over_world() { 70 } else { 110 }]);
        draw::centered(out, Serif, "Drag a control to move it. Pick one to resize or hide it.", (sl + w - sr) * 0.5, h * 0.62, 17.0 * dp, DIM, true);
        for i in 0..CONTROLS.len() {
            if !self.layout.places[i].shown || !self.stance_fits(i) {
                continue;
            }
            let [x, y, cw, ch] = self.control_rect(i);
            // A thin blue outline: what can be picked up.
            let t = (1.5 * dp).max(2.0);
            let c = [140, 190, 255, 150];
            rect(out, x - t, y - t, cw + 2.0 * t, t, c);
            rect(out, x - t, y + ch, cw + 2.0 * t, t, c);
            rect(out, x - t, y, t, ch, c);
            rect(out, x + cw, y, t, ch, c);
            self.draw_control(out, i, true);
        }
        // Toolbar.
        let bh = 40.0 * dp;
        let items: [(&str, &str, Action, f32); 6] = [
            ("", "remove", Action::CtlSize(-1), 44.0),
            ("", "add", Action::CtlSize(1), 44.0),
            ("Hide", "visibility_off", Action::CtlHide, 96.0),
            ("Add", "add_box", Action::CtlAddList, 90.0),
            ("Reset", "restart_alt", Action::CtlReset, 100.0),
            ("Done", "check", Action::CtlDone, 100.0),
        ];
        let gap = 8.0 * dp;
        let total: f32 = items.iter().map(|i| i.3 * dp).sum::<f32>() + gap * (items.len() - 1) as f32;
        // Along the top, in the widest stretch no control reaches into (so every control
        // can still be grabbed); lower down if the top is too crowded.
        let mut y = st + 6.0 * dp;
        let (mut x0, mut room) = (sl + 10.0 * dp, 0.0);
        for band in [y, h * 0.30, h * 0.45] {
            let (lo, hi) = (band - 8.0 * dp, band + bh + 8.0 * dp);
            let mut taken: Vec<(f32, f32)> = (0..CONTROLS.len())
                .filter(|&i| self.layout.places[i].shown)
                .map(|i| self.control_rect(i))
                .filter(|r| r[1] < hi && r[1] + r[3] > lo)
                .map(|r| (r[0] - 10.0 * dp, r[0] + r[2] + 10.0 * dp))
                .collect();
            taken.sort_by(|a, b| a.0.total_cmp(&b.0));
            let (mut best, mut at, mut from) = (0.0f32, sl + 10.0 * dp, sl + 10.0 * dp);
            for (a, b) in taken.into_iter().chain([(w - sr - 10.0 * dp, f32::MAX)]) {
                if a - from > best {
                    (best, at) = (a - from, from);
                }
                from = from.max(b);
            }
            if best > room {
                (y, x0, room) = (band, at, best);
            }
            if room >= total * 0.75 {
                break;
            }
        }
        let k = (room / total).min(1.0);
        let mut x = x0 + (room - total * k) * 0.5;
        let cx = x + total * k * 0.5;
        let sel = self.sel_ctl;
        for (label, icon, action, bw) in items {
            let enabled = match action {
                Action::CtlSize(_) => sel.is_some(),
                Action::CtlHide => sel.is_some_and(|i| !CONTROLS[i].keep),
                _ => true,
            };
            let glow = action == Action::CtlAddList && self.add_open;
            self.button(out, Style::Bar, (x, y, bw * dp * k, bh), label, Some(icon), Serif, action, enabled, glow);
            x += (bw * dp + gap) * k;
        }
        let info = match sel {
            Some(i) => format!("{}  {}%", CONTROLS[i].name, (self.layout.places[i].scale * 100.0).round()),
            None => "Pick a control".to_string(),
        };
        draw::centered(out, Serif, &info, cx, y + bh + 16.0 * dp, 17.0 * dp, INK, true);
        if self.add_open {
            self.draw_add_list(out, cx, y + bh + 32.0 * dp);
        }
    }

    /// The hidden controls, for ADD to put back on screen.
    fn draw_add_list(&mut self, out: &mut Vec<HudVertex>, cx: f32, top: f32) {
        let dp = self.dp;
        let hidden: Vec<usize> = (0..CONTROLS.len()).filter(|&i| !self.layout.places[i].shown && self.stance_fits(i)).collect();
        let (bw, bh, gap, pad) = (110.0 * dp, 38.0 * dp, 6.0 * dp, 10.0 * dp);
        let (w, _) = self.size;
        let [sl, _, sr, _] = self.margins();
        let fit = (((w - sl - sr - 20.0 * dp - 2.0 * pad + gap) / (bw + gap)) as usize).max(1);
        let cols = hidden.len().clamp(1, 5.min(fit));
        let rows = hidden.len().div_ceil(cols).max(1);
        let (pw, ph) = (cols as f32 * (bw + gap) - gap + 2.0 * pad, rows as f32 * (bh + gap) - gap + 2.0 * pad);
        let px = (cx - pw * 0.5).clamp(sl + 10.0 * dp, (w - sr - 10.0 * dp - pw).max(sl));
        draw::panel(out, px, top, pw, ph, dp * 0.3);
        self.buttons.push(Button { x: px, y: top, w: pw, h: ph, action: Action::Nothing });
        if hidden.is_empty() {
            draw::centered(out, Serif, "All on screen", cx, top + ph * 0.5, 16.0 * dp, DIM, true);
            return;
        }
        for (n, i) in hidden.into_iter().enumerate() {
            let (bx, by) = (px + pad + (n % cols) as f32 * (bw + gap), top + pad + (n / cols) as f32 * (bh + gap));
            self.button(out, Style::Stone, (bx, by, bw, bh), CONTROLS[i].name, Some("add"), Serif, Action::CtlShow(i as u8), true, false);
        }
    }

    /// A finger went down in the editor on control `i`: pick it and start dragging it.
    pub(super) fn grab_control(&mut self, id: u64, i: usize, x: f32, y: f32) {
        let [rx, ry, rw, rh] = self.control_rect(i);
        self.sel_ctl = Some(i);
        self.drag = Some((id, i, rx + rw * 0.5 - x, ry + rh * 0.5 - y));
    }

    /// A finger moved: drags the control it holds in the editor.
    pub fn moved(&mut self, id: u64, x: f32, y: f32) {
        self.drag_list(id, y);
        let Some((did, i, dx, dy)) = self.drag else { return };
        if did != id {
            return;
        }
        let [_, _, cw, ch] = self.control_rect(i);
        let [sl, st, sr, sb] = self.margins();
        let (w, h) = self.size;
        let cx = (x + dx).min(w - sr - cw * 0.5).max(sl + cw * 0.5);
        let cy = (y + dy).min(h - sb - ch * 0.5).max(st + ch * 0.5);
        self.layout.places[i].at = Some([cx / w, cy / h]);
    }

    /// The controls editor's buttons, and the HUD's hotbar and block picker.
    pub(super) fn apply_controls(&mut self, a: Action) {
        match a {
            Action::Slot(n) => {
                self.slot = (n as usize).min(8);
                self.slot_changed = self.seconds;
            }
            Action::ToggleInventory => {
                if self.screen == Screen::Inventory {
                    self.close_inventory();
                    self.screen = Screen::Playing;
                } else {
                    self.screen = Screen::Inventory;
                    self.picked = None;
                }
                self.acts_open = false;
            }
            Action::PickBlock(k) => {
                if let Some(b) = self.blocks.get(k as usize) {
                    self.hotbar[self.slot.min(8)] = Stack::of(b.id, items::STACK_MAX);
                    self.slot_changed = self.seconds;
                }
            }
            Action::OpenControls => {
                self.screen = Screen::Controls;
                self.sel_ctl = None;
                self.add_open = false;
                self.drag = None;
            }
            Action::CtlSize(d) => {
                if let Some(i) = self.sel_ctl {
                    let p = &mut self.layout.places[i];
                    p.scale = ((p.scale * 10.0).round() / 10.0 + d as f32 * 0.1).clamp(SCALE_MIN, SCALE_MAX);
                }
            }
            Action::CtlHide => {
                if let Some(i) = self.sel_ctl.filter(|&i| !CONTROLS[i].keep) {
                    self.layout.places[i].shown = false;
                    self.sel_ctl = None;
                }
            }
            Action::CtlShow(i) => {
                if let Some(p) = self.layout.places.get_mut(i as usize) {
                    p.shown = true;
                    self.sel_ctl = Some(i as usize);
                    self.add_open = false;
                }
            }
            Action::CtlAddList => self.add_open = !self.add_open,
            Action::CtlReset => {
                self.layout = Layout::default();
                self.sel_ctl = None;
            }
            Action::CtlDone => {
                self.screen = Screen::Settings;
                self.drag = None;
                self.requests.push(Request::SaveControls);
            }
            _ => {}
        }
    }
}
