//! The menus, in a dark fantasy look: weathered stone set in iron frames with bronze edges,
//! blue crystal gems, buttons with pointed ends, a blue glow on the picked or pressed item,
//! the BlockForge logo in a medieval face and serif type with icons. Everything comes from
//! the UI atlas (`draw.rs`, `tools/ui_ornate.py`); the world's panorama turns behind the
//! menus, and the title screen shows the character on a pedestal.

use super::draw::{self, nine};
use super::{items, lod_label, rect, Action, Button, HudVertex, Screen, Showcase, Slot, Ui, LOD_QUALITY, XHAIR};
use crate::atlas::FontId::{self, Serif};
use crate::perf::Perf;

const INK: [u8; 4] = [238, 228, 208, 255];
const DIM: [u8; 4] = [172, 160, 142, 255];
const BRONZE: [u8; 4] = [214, 182, 128, 255];
const BLUE: [u8; 4] = [138, 186, 255, 255];
const FAINT: [u8; 4] = [214, 182, 128, 40];
const OPAQUE: [u8; 4] = [255, 255, 255, 255];
const PRESSED: [u8; 4] = [190, 192, 204, 255];
const DISABLED: [u8; 4] = [120, 116, 112, 235];
const DANGER: [u8; 4] = [255, 150, 130, 255];
const WARN: [u8; 4] = [255, 196, 90, 255];
const SEPIA: [u8; 4] = [196, 160, 118, 255];

/// The logo picture's proportions (1200 x 340).
const LOGO_ASPECT: f32 = 1200.0 / 340.0;

/// How a button looks.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Style {
    /// Small, with clipped corners (keys, icon buttons, the game's buttons).
    Stone,
    /// Long, with pointed ends and a gem at each tip; icon and label centred together.
    Bar,
    /// Like `Bar`, with the icon at the left and the label centred (the main menu).
    Menu,
    /// A sunken box, icon and label at the left (the settings' groups).
    Row,
}

/// What the top bar leaves between the logo and its square buttons (`x0..x1`, at `y`, `h`
/// high), and where the page under it starts.
struct Top {
    x0: f32,
    x1: f32,
    y: f32,
    h: f32,
    below: f32,
}

/// The settings page's groups: name, icon and what is in them.
pub const TABS: [(&str, &str, &str); 5] = [
    ("General", "monitor", "How you get about and what you see of the game: walking or flying, the camera, the stats overlay and the time of day."),
    ("Video", "image", "How far and how finely the world is drawn, and its sky: fog, clouds and god rays. Turn these down if the game runs slowly."),
    ("Shaders", "auto_awesome", "The shader look: shadows, sunlight, bounce light, colour and leaves in the wind. Pick a preset or tune each part; they apply with Shaders on."),
    ("Performance", "speed", "What the renderer leaves out and how it builds the terrain. The defaults suit most phones."),
    ("Controls", "touch_app", "The buttons on the screen: where they are and how big, how solid they look, how fast dragging turns the view, the stick's size, building by touch, and one button for crouch, crawl and roll."),
];

/// One setting on the settings page.
enum Control {
    /// On or off.
    Switch(bool, Action),
    /// A value with arrows to step it down and up.
    Stepper(String, Action, Action),
    /// A value that changes to the next when tapped.
    Choice(String, Action),
    /// A value that cannot be changed here.
    Fixed(String),
}

/// A short description of each block, for the inventory.
fn block_note(name: &str) -> &'static str {
    match name {
        "Stone" => "The rock under everything. Solid and plain, good for foundations and walls.",
        "Dirt" => "Soft earth from under the grass. Quick to dig and easy to shape.",
        "Grass" => "Dirt with a living top. It takes the colour of the land it grows in.",
        "Sand" => "Fine sand from beaches and deserts.",
        "Log" => "A trunk of a tree, rings on its ends and bark round its sides.",
        "Leaves" => "A tangle of leaves from the treetops.",
        "Glowstone" => "A warm, bright crystal block that lights up its corner of the world.",
        "Snow" => "Packed snow from the cold peaks.",
        "Gravel" => "Loose stones from riverbeds and mountainsides.",
        "Dry Grass" => "Grass from dry lands, pale and thin.",
        "Lush Grass" => "Grass from wet, warm lands, thick and green.",
        "Cold Grass" => "Grass from cold lands, short and dark.",
        "Granite" => "Pink speckled granite from the mountains' roots.",
        "Basalt" => "Dark rock from old lava flows.",
        "Limestone" => "Pale, soft limestone from layered cliffs.",
        "Sandstone" => "Sand pressed into stone over ages; warm and smooth.",
        _ => "A building block.",
    }
}

impl Ui {
    /// Screen pixels per atlas pixel for frame edges.
    fn k(&self) -> f32 {
        self.dp * 0.42
    }

    /// A button; glows while pressed (or when `glow`). Disabled buttons are dimmed and do
    /// nothing.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn button(
        &mut self,
        out: &mut Vec<HudVertex>,
        style: Style,
        (x, y, w, h): (f32, f32, f32, f32),
        label: &str,
        icon: Option<&str>,
        font: FontId,
        action: Action,
        enabled: bool,
        glow: bool,
    ) {
        let pressed = enabled && self.pressed_action() == Some(action);
        let tint = if !enabled {
            DISABLED
        } else if pressed {
            PRESSED
        } else {
            OPAQUE
        };
        let lit = enabled && (pressed || glow);
        // Atlas pixels to screen pixels: the long buttons are 88 atlas pixels high.
        let s = h / 88.0;
        let (text_x0, text_x1) = match style {
            Style::Bar | Style::Menu => {
                nine(out, "bar", x, y, w, h, s, tint);
                if lit {
                    let g = 24.0 * s;
                    nine(out, "glow_hex", x - g, y - g, w + 2.0 * g, h + 2.0 * g, s, OPAQUE);
                }
                (x + 34.0 * s, x + w - 34.0 * s)
            }
            Style::Stone => {
                let k = (h / 88.0).min(w / 112.0).max(0.25);
                nine(out, "button", x, y, w, h, k, tint);
                if lit {
                    let g = 18.0 * k;
                    nine(out, "glow", x - g, y - g, w + 2.0 * g, h + 2.0 * g, k, OPAQUE);
                }
                (x + 10.0 * k, x + w - 10.0 * k)
            }
            Style::Row => {
                draw::field(out, x, y, w, h, self.k() * 0.8);
                if lit {
                    let g = 18.0 * self.k();
                    nine(out, "glow", x - g, y - g, w + 2.0 * g, h + 2.0 * g, self.k(), OPAQUE);
                }
                (x + 10.0 * self.dp, x + w - 8.0 * self.dp)
            }
        };
        let ink = if !enabled {
            [140, 134, 126, 255]
        } else if lit {
            [250, 244, 230, 255]
        } else {
            INK
        };
        let single = label.chars().count() == 1;
        let size = (h * if single { 0.5 } else { 0.42 }).min(26.0 * self.dp);
        // (the arrow triangles are small in their squares)
        let icon_size = h * if icon.is_some_and(|i| i.starts_with("arrow_")) { 0.8 } else { 0.5 };
        let gap = h * 0.2;
        let cy = y + h * 0.5;
        let room = text_x1 - text_x0;
        let has_label = !label.is_empty();
        match (style, icon) {
            (Style::Menu, Some(name)) | (Style::Row, Some(name)) => {
                // Icon at the left; the label centred in the rest (left-aligned for rows).
                let ix = text_x0 + icon_size * 0.6;
                draw::icon(out, name, ix + 1.0, cy + 1.0, icon_size, [0, 0, 0, 160]);
                draw::icon(out, name, ix, cy, icon_size, if lit { BLUE } else { ink });
                let lx0 = ix + icon_size * 0.6 + gap;
                let size = draw::fit(font, label, size, text_x1 - lx0);
                if style == Style::Row {
                    draw::left(out, font, label, lx0, cy, size, ink, true);
                } else {
                    draw::centered(out, font, label, (lx0 + text_x1) * 0.5 - icon_size * 0.3, cy, size, ink, true);
                }
            }
            _ => {
                let fitted = if has_label { draw::fit(font, label, size, room - icon.map_or(0.0, |_| icon_size + gap)) } else { size };
                // A label squeezed too small to read: the icon alone says it.
                let has_label = has_label && (icon.is_none() || fitted >= size * 0.6);
                let size = fitted;
                let tw = if has_label { draw::width(font, label, size) } else { 0.0 };
                let total = tw + icon.map_or(0.0, |_| icon_size + if has_label { gap } else { 0.0 });
                let mut cx = x + (w - total) * 0.5;
                if let Some(name) = icon {
                    draw::icon(out, name, cx + icon_size * 0.5 + 1.0, cy + 1.0, icon_size, [0, 0, 0, 160]);
                    draw::icon(out, name, cx + icon_size * 0.5, cy, icon_size, ink);
                    cx += icon_size + gap;
                }
                if has_label {
                    draw::left(out, font, label, cx, cy, size, ink, true);
                }
            }
        }
        self.buttons.push(Button { x, y, w, h, action: if enabled { action } else { Action::Nothing } });
    }

    /// The natural width of a bar button with this label and icon at height `h`.
    fn bar_width(&self, label: &str, icon: bool, h: f32) -> f32 {
        let size = (h * 0.42).min(26.0 * self.dp);
        draw::width(Serif, label, size) + if icon { h * 1.0 } else { 0.0 } + h * 1.3
    }

    /// The bar along the top of a page: the small logo at the left and `squares` (icon
    /// buttons: icon, action, enabled) at the right, the last at the edge. Returns the room
    /// between them for tabs or a title, and where the page below starts.
    fn top_bar(&mut self, out: &mut Vec<HudVertex>, squares: &[(&str, Action, bool)]) -> Top {
        let (w, h) = self.size;
        let dp = self.dp;
        let [sl, st, sr, _] = self.margins();
        let bh = 42.0 * dp;
        let y = st + 6.0 * dp;
        let lh = (50.0 * dp).min(h * 0.14);
        let lw = lh * LOGO_ASPECT;
        let lx = sl + 8.0 * dp;
        draw::image(out, "logo", lx, st + 2.0 * dp, lw, lh, OPAQUE);
        let mut x = w - sr - 8.0 * dp;
        for &(icon, action, enabled) in squares.iter().rev() {
            x -= bh;
            self.button(out, Style::Stone, (x, y, bh, bh), "", Some(icon), Serif, action, enabled, false);
            x -= 8.0 * dp;
        }
        Top { x0: lx + lw + 12.0 * dp, x1: x - 4.0 * dp, y, h: bh, below: (st + 2.0 * dp + lh).max(y + bh) + 8.0 * dp }
    }

    /// The page's area under the top bar: left, top, right, bottom. Panels fill it, edge to
    /// edge.
    fn page_box(&self, t: &Top) -> (f32, f32, f32, f32) {
        let (w, h) = self.size;
        let dp = self.dp;
        let [sl, _, sr, sb] = self.margins();
        (sl + 8.0 * dp, t.below, w - sr - 8.0 * dp, h - sb - 8.0 * dp)
    }

    /// A title plaque in the top bar, in the middle of the screen if there is room.
    fn bar_plaque(&self, out: &mut Vec<HudVertex>, t: &Top, title: &str, icon: Option<&str>) {
        let [sl, _, sr, _] = self.margins();
        let room = t.x1 - t.x0;
        let pw = (300.0 * self.dp).min(room);
        let cx = ((sl + self.size.0 - sr) * 0.5).clamp(t.x0 + pw * 0.5, t.x1 - pw * 0.5);
        let py = t.y + 4.0 * self.dp;
        self.plaque(out, cx, py, pw, t.h - 4.0 * self.dp, title, icon);
    }

    /// Bar buttons side by side across `x..x + w` at `y`, `h` high, each taking its `share`
    /// of the width. Items: label, icon, action, enabled, glowing, share.
    #[allow(clippy::type_complexity)]
    fn button_row(&mut self, out: &mut Vec<HudVertex>, (x, y, w, h): (f32, f32, f32, f32), items: &[(&str, &str, Action, bool, bool, f32)]) {
        let gap = 8.0 * self.dp;
        let total: f32 = items.iter().map(|it| it.5).sum();
        let room = w - gap * (items.len() as f32 - 1.0).max(0.0);
        let mut bx = x;
        for it in items {
            let bw = room * it.5 / total.max(0.001);
            self.button(out, Style::Bar, (bx, y, bw, h), it.0, Some(it.1), Serif, it.2, it.3, it.4);
            bx += bw + gap;
        }
    }

    /// A title plaque centred at `cx`, with a gem in a setting over its middle and an optional
    /// icon before the title.
    #[allow(clippy::too_many_arguments)]
    fn plaque(&self, out: &mut Vec<HudVertex>, cx: f32, y: f32, w: f32, h: f32, title: &str, icon: Option<&str>) {
        nine(out, "plaque", cx - w * 0.5, y, w, h, h / 110.0, OPAQUE);
        let g = h * 0.5;
        draw::image(out, "setting", cx - g * 0.5, y - g * 0.42, g, g, OPAQUE);
        let size = draw::fit(Serif, title, h * 0.5, w - h * 2.6);
        let tw = draw::width(Serif, title, size);
        let isz = h * 0.55;
        let total = tw + icon.map_or(0.0, |_| isz + h * 0.2);
        let mut x = cx - total * 0.5;
        if let Some(name) = icon {
            draw::icon(out, name, x + isz * 0.5, y + h * 0.52, isz, BRONZE);
            x += isz + h * 0.2;
        }
        draw::left(out, Serif, title, x, y + h * 0.52, size, INK, true);
    }

    /// A heading between two small gems with rules running out from them.
    fn heading(&self, out: &mut Vec<HudVertex>, cx: f32, cy: f32, text: &str, size: f32, max_w: f32) {
        let dp = self.dp;
        let size = draw::fit(Serif, text, size, max_w - 70.0 * dp);
        let tw = draw::width(Serif, text, size);
        let g = size * 0.6;
        for side in [-1.0f32, 1.0] {
            let gx = cx + side * (tw * 0.5 + 10.0 * dp + g * 0.5);
            draw::image(out, "gem", gx - g * 0.5, cy - g * 0.5, g, g, OPAQUE);
            let rl = ((max_w - tw) * 0.5 - 26.0 * dp - g).max(0.0);
            if rl > 8.0 * dp {
                let rx = if side < 0.0 { gx - g * 0.5 - rl } else { gx + g * 0.5 };
                rect(out, rx, cy - 0.5 * dp, rl, (1.0 * dp).max(1.0), [150, 124, 86, 200]);
            }
        }
        draw::centered(out, Serif, text, cx, cy, size, INK, true);
    }

    /// A bronze rule across `w` with a small diamond at each end.
    fn rule(&self, out: &mut Vec<HudVertex>, x: f32, cy: f32, w: f32) {
        let h = 10.0 * self.dp;
        nine(out, "rule", x, cy - h * 0.5, w, h, h / 24.0, OPAQUE);
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
            Screen::Controls => self.draw_controls_editor(out),
            Screen::Inventory => self.draw_inventory(out),
            Screen::Multiplayer => self.draw_page(
                out,
                "Multiplayer",
                "public",
                &["No servers yet.", "The game already runs as a client and a server, even on its own: playing together over Wi-Fi is next."],
            ),
            Screen::Mods => self.draw_page(
                out,
                "Mods",
                "extension",
                &["No mods installed.", "Blocks come from assets/blocks.toml for now; mods will add their own blocks, items and behaviours."],
            ),
            Screen::Cosmetics => self.draw_page(out, "Cosmetics", "palette", &["Skins and outfits for your character.", "Coming soon."]),
            Screen::Playing => {}
        }
    }

    /// The main menu: the logo and a column of buttons on the left, the player at the top
    /// right, the character on a pedestal on the right, the version at the bottom left.
    fn draw_title(&mut self, out: &mut Vec<HudVertex>) {
        let (w, h) = self.size;
        let dp = self.dp;
        let [sl, st, sr, sb] = self.margins();

        // The character's pedestal first: the character is drawn over it, the rest over him.
        let plate_h = 34.0 * dp;
        let plate_y = h - sb - 10.0 * dp - plate_h;
        let cx_char = sl + (w - sl - sr) * 0.76;
        let ped_w = (190.0 * dp).min((w - sl - sr) * 0.3);
        let ped_h = ped_w * 180.0 / 520.0;
        let ped_y = plate_y - 6.0 * dp - ped_h;
        let feet = ped_y + ped_h * 0.21;
        let top_room = st + 56.0 * dp;
        let char_h = (h * 0.5).min(feet - top_room).max(40.0 * dp);
        draw::image(out, "pedestal", cx_char - ped_w * 0.5, ped_y, ped_w, ped_h, OPAQUE);
        self.under = out.len();
        self.showcase = Some(Showcase { x: cx_char, feet, height: char_h, yaw: self.showcase_yaw });
        // The skin's name between arrows that turn him.
        let pw = 150.0 * dp;
        nine(out, "bar", cx_char - pw * 0.5, plate_y, pw, plate_h, plate_h / 88.0, OPAQUE);
        draw::centered(out, Serif, "Default", cx_char, plate_y + plate_h * 0.5, plate_h * 0.5, INK, true);
        let a = plate_h;
        self.button(out, Style::Stone, (cx_char - pw * 0.5 - a - 6.0 * dp, plate_y, a, a), "", Some("arrow_left"), Serif, Action::TurnShowcase(-1), true, false);
        self.button(out, Style::Stone, (cx_char + pw * 0.5 + 6.0 * dp, plate_y, a, a), "", Some("arrow_right"), Serif, Action::TurnShowcase(1), true, false);

        // The player, at the top right.
        let chip_w = 170.0 * dp;
        let chip_h = 38.0 * dp;
        let chip_x = w - sr - 12.0 * dp - chip_w;
        let chip_y = st + 6.0 * dp;
        self.button(out, Style::Bar, (chip_x, chip_y, chip_w, chip_h), "", None, Serif, Action::Open(Screen::Cosmetics), true, false);
        draw::icon(out, "person", chip_x + chip_h * 1.0, chip_y + chip_h * 0.5, chip_h * 0.55, DIM);
        draw::left(out, Serif, "Player", chip_x + chip_h * 1.5, chip_y + chip_h * 0.52, chip_h * 0.46, INK, true);
        draw::icon(out, "arrow_drop_down", chip_x + chip_w - chip_h * 0.95, chip_y + chip_h * 0.5, chip_h * 0.6, INK);

        // The logo and the buttons under it.
        let gap = 7.0 * dp;
        let footer = 26.0 * dp;
        let left_w = (w - sl - sr) * 0.5;
        let avail = h - st - sb - footer - 6.0 * dp;
        let buttons_min = 6.0 * 38.0 * dp + 5.0 * gap;
        let mut lw = (left_w * 0.95).min(440.0 * dp);
        let mut lh = lw / LOGO_ASPECT;
        let cap = (h * 0.26).min(avail - buttons_min).max(avail * 0.16);
        if lh > cap {
            lh = cap;
            lw = lh * LOGO_ASPECT;
        }
        // The buttons' column, with the logo centred over it.
        let bw = (290.0 * dp).min(left_w * 0.8);
        let bx = sl + 24.0 * dp;
        let lx = (bx + (bw - lw) * 0.5).max(sl + 6.0 * dp);
        let ly = st;
        draw::image(out, "logo", lx, ly, lw, lh, OPAQUE);
        let (y0, y1) = (ly + lh + 4.0 * dp, h - sb - footer);
        let bh = ((y1 - y0 - 5.0 * gap) / 6.0).clamp(26.0 * dp, 46.0 * dp);
        let items: [(&str, &str, Action); 6] = [
            ("Single Player", "play_arrow", Action::Open(Screen::WorldList)),
            ("Multiplayer", "public", Action::Open(Screen::Multiplayer)),
            ("Settings", "settings", Action::OpenSettings),
            ("Mods", "extension", Action::Open(Screen::Mods)),
            ("Cosmetics", "palette", Action::Open(Screen::Cosmetics)),
            ("Quit", "exit_to_app", Action::Quit),
        ];
        // The first button glows until another is pressed, as a focus would.
        let focus = self.pressed_action().unwrap_or(items[0].2);
        for (i, (label, icon, action)) in items.iter().enumerate() {
            let y = y0 + i as f32 * (bh + gap);
            self.button(out, Style::Menu, (bx, y, bw, bh), label, Some(icon), Serif, *action, true, *action == focus);
        }

        let version = format!("v{}", env!("CARGO_PKG_VERSION"));
        let vy = h - sb - 13.0 * dp;
        let vs = 15.0 * dp;
        let vw = draw::width(Serif, &version, vs);
        let vx = sl + 60.0 * dp;
        self.rule(out, vx - 54.0 * dp, vy, 46.0 * dp);
        draw::left(out, Serif, &version, vx, vy, vs, DIM, true);
        self.rule(out, vx + vw + 8.0 * dp, vy, 46.0 * dp);
    }

    /// The world list's row height (also how far a drag scrolls one row).
    pub(super) fn list_row_height(&self) -> f32 {
        let (_, h) = self.size;
        let dp = self.dp;
        (h * 0.15).clamp(48.0 * dp, 64.0 * dp)
    }

    /// SINGLE PLAYER: tabs, Back and the settings in the top bar; the worlds on the left; on the
    /// right the picked one's picture and details, and under them what to do.
    fn draw_world_list(&mut self, out: &mut Vec<HudVertex>) {
        let dp = self.dp;
        let k = self.k();
        let t = self.top_bar(out, &[("arrow_left", Action::Open(Screen::Title), true), ("settings", Action::OpenSettings, true)]);
        self.tabs(out, &t, false);
        let some = self.picked().is_some();
        let (x0, y0, x1, y1) = self.page_box(&t);
        let gap = 14.0 * dp;
        let lw = ((x1 - x0) - gap) * 0.58;
        let ph = (y1 - y0).max(40.0 * dp);
        // The right column: the picked world, then two rows of buttons.
        let (dx, dw) = (x0 + lw + gap, x1 - x0 - lw - gap);
        let bh = 40.0 * dp;
        let bgap = 8.0 * dp;
        let row2 = y1 - bh;
        let row1 = row2 - bgap - bh;
        self.button_row(out, (dx, row1, dw, bh), &[("Re-Create", "refresh", Action::RecreateWorld, some, false, 0.34), ("Create New World", "add", Action::NewWorld, true, false, 0.66)]);
        self.button_row(
            out,
            (dx, row2, dw, bh),
            &[("Edit", "edit", Action::EditWorld, some, false, 1.0), ("Delete", "delete", Action::AskDelete, some, false, 1.0), ("Play", "play_arrow", Action::PlayWorld, some, some, 1.0)],
        );
        let (dy, dh) = (y0, (row1 - 10.0 * dp - y0).max(40.0 * dp));
        draw::panel(out, x0, y0, lw, ph, k);
        self.buttons.push(Button { x: x0, y: y0, w: lw, h: ph, action: Action::Nothing });
        draw::panel(out, dx, dy, dw, dh, k);
        self.buttons.push(Button { x: dx, y: dy, w: dw, h: dh, action: Action::Nothing });

        // The list.
        let pad = 12.0 * dp;
        let n = self.worlds.len();
        let rh = self.list_row_height().min((ph - 2.0 * pad).max(20.0 * dp));
        let fits = (((ph - 2.0 * pad) / rh).floor() as usize).max(1);
        self.scroll = self.scroll.min(n.saturating_sub(fits));
        let (rx, rw) = (x0 + pad, lw - 2.0 * pad - if n > fits { 8.0 * dp } else { 0.0 });
        if n == 0 {
            draw::centered(out, Serif, "No worlds yet", x0 + lw * 0.5, y0 + ph * 0.45, 22.0 * dp, INK, true);
            draw::centered(out, Serif, "Create one to start playing.", x0 + lw * 0.5, y0 + ph * 0.45 + 26.0 * dp, 16.0 * dp, DIM, true);
        }
        let wide = rw > 400.0 * dp;
        for (slot, i) in (self.scroll..n.min(self.scroll + fits)).enumerate() {
            let ry = y0 + pad + slot as f32 * rh;
            let bh = rh - 6.0 * dp;
            let picked = self.selected == Some(i);
            draw::field(out, rx, ry, rw, bh, k * 0.7);
            if picked {
                let g = 18.0 * k;
                nine(out, "glow", rx - g, ry - g, rw + 2.0 * g, bh + 2.0 * g, k, OPAQUE);
            }
            let e = self.worlds[i].clone();
            let th = bh - 10.0 * dp;
            let tw = th * 16.0 / 9.0;
            self.thumb(out, e.thumb, rx + 6.0 * dp, ry + 5.0 * dp, tw, th);
            let tx = rx + 6.0 * dp + tw + 10.0 * dp;
            let size_w = 66.0 * dp;
            let date_w = if wide { (rw * 0.3).min(170.0 * dp) } else { 0.0 };
            let name_room = rx + rw - size_w - date_w - tx - 10.0 * dp;
            let ns = draw::fit(Serif, &e.name, bh * 0.34, name_room);
            draw::left(out, Serif, &e.name, tx, ry + bh * 0.34, ns, INK, true);
            let sub = if wide { format!("Seed {}", e.seed) } else { e.last_played.clone() };
            draw::left(out, Serif, &sub, tx, ry + bh * 0.7, draw::fit(Serif, &sub, bh * 0.26, name_room), if picked { BLUE } else { DIM }, true);
            let sep = |out: &mut Vec<HudVertex>, x: f32| rect(out, x, ry + bh * 0.2, (1.0 * dp).max(1.0), bh * 0.6, [150, 124, 86, 120]);
            if wide {
                let dx0 = rx + rw - size_w - date_w;
                sep(out, dx0);
                draw::left(out, Serif, "Last Played:", dx0 + 10.0 * dp, ry + bh * 0.34, bh * 0.24, DIM, true);
                draw::left(out, Serif, &e.last_played, dx0 + 10.0 * dp, ry + bh * 0.68, draw::fit(Serif, &e.last_played, bh * 0.27, date_w - 16.0 * dp), INK, true);
            }
            sep(out, rx + rw - size_w);
            draw::centered(out, Serif, &e.size, rx + rw - size_w * 0.5, ry + bh * 0.5, draw::fit(Serif, &e.size, bh * 0.28, size_w - 10.0 * dp), INK, true);
            self.buttons.push(Button { x: rx, y: ry, w: rw, h: bh, action: Action::SelectWorld(i) });
        }
        if n > fits {
            // Where the list is scrolled to (drag the list to scroll it).
            let (sx, sy, sh) = (x0 + lw - pad - 4.0 * dp, y0 + pad, ph - 2.0 * pad);
            rect(out, sx, sy, 3.0 * dp, sh, [0, 0, 0, 120]);
            let frac = fits as f32 / n as f32;
            let pos = self.scroll as f32 / (n - fits).max(1) as f32;
            let bh = (sh * frac).max(20.0 * dp);
            rect(out, sx, sy + (sh - bh) * pos, 3.0 * dp, bh, [214, 182, 128, 220]);
        }

        // The picked world.
        let Some(e) = self.picked().cloned() else {
            draw::centered(out, Serif, "Pick a world", dx + dw * 0.5, dy + dh * 0.5, 20.0 * dp, DIM, true);
            return;
        };
        let pad = 16.0 * dp;
        let iw = dw - 2.0 * pad;
        let line = (19.0 * dp).min(dh * 0.075);
        let text_h = 30.0 * dp + line * 4.0 + 30.0 * dp;
        let th = (iw * 9.0 / 16.0).min(dh - 2.0 * pad - text_h).max(30.0 * dp);
        let tw = th * 16.0 / 9.0;
        self.thumb(out, e.thumb, dx + (dw - tw) * 0.5, dy + pad, tw, th);
        let mut ty = dy + pad + th + 18.0 * dp;
        self.heading(out, dx + dw * 0.5, ty, &e.name, 22.0 * dp, iw);
        ty += 24.0 * dp;
        let info: [(&str, &str, &String); 4] =
            [("terrain", "Seed:", &e.seed), ("event", "Last Played:", &e.last_played), ("timer", "Play Time:", &e.play_time), ("sd_storage", "World Size:", &e.size)];
        let s = (line * 0.8).min(16.0 * dp);
        let label_w = draw::width(Serif, "Last Played: ", s);
        for (icon, label, value) in info {
            draw::icon(out, icon, dx + pad + s * 0.6, ty, s * 1.1, DIM);
            draw::left(out, Serif, label, dx + pad + s * 1.6, ty, s, DIM, true);
            let vx = dx + pad + s * 1.6 + label_w;
            draw::left(out, Serif, value, vx, ty, draw::fit(Serif, value, s, dx + dw - pad - vx), INK, true);
            ty += line;
        }
        self.rule(out, dx + pad, ty, iw);
        let created = format!("Created {}", e.created);
        draw::centered(out, Serif, &created, dx + dw * 0.5, ty + line, draw::fit(Serif, &created, s, iw), DIM, true);
    }

    /// SINGLE PLAYER and MULTIPLAYER, as tabs in the top bar.
    fn tabs(&mut self, out: &mut Vec<HudVertex>, t: &Top, multi: bool) {
        let dp = self.dp;
        let tw = (180.0 * dp).min((t.x1 - t.x0 - 8.0 * dp) * 0.5);
        let single = if multi { Action::Open(Screen::WorldList) } else { Action::Nothing };
        let other = if multi { Action::Nothing } else { Action::Open(Screen::Multiplayer) };
        self.button(out, Style::Bar, (t.x0, t.y, tw, t.h), "Single Player", Some("person"), Serif, single, true, !multi);
        self.button(out, Style::Bar, (t.x0 + tw + 8.0 * dp, t.y, tw, t.h), "Multiplayer", Some("public"), Serif, other, true, multi);
    }

    /// A world's picture in a thin frame (the default picture if it has none yet).
    fn thumb(&self, out: &mut Vec<HudVertex>, slot: Option<usize>, x: f32, y: f32, w: f32, h: f32) {
        let b = 3.0 * self.dp;
        draw::field(out, x - b, y - b, w + 2.0 * b, h + 2.0 * b, self.k() * 0.5);
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
            draw::left(out, Serif, hint, tx, y + h * 0.5, draw::fit(Serif, hint, size, w - 24.0 * self.dp), [120, 112, 102, 255], false);
        } else {
            let s = draw::fit(Serif, &format!("{value}|"), size, w - 24.0 * self.dp);
            let tw = draw::left(out, Serif, &value, tx, y + h * 0.5, s, INK, false);
            if focused && (self.seconds * 2.0).fract() < 0.55 {
                rect(out, tx + tw + 2.0 * self.dp, y + h * 0.2, 2.0 * self.dp, h * 0.6, INK);
            }
        }
        self.buttons.push(Button { x, y, w, h, action: Action::Focus(field as u8) });
    }

    /// CREATE NEW WORLD (`create`) or EDIT WORLD: the fields in a panel filling the page, the
    /// button to finish at its bottom right (the keyboard takes the bottom while typing).
    fn draw_world_form(&mut self, out: &mut Vec<HudVertex>, create: bool) {
        let (_, h) = self.size;
        let dp = self.dp;
        let [_, _, _, sb] = self.margins();
        let typing = self.focus.is_some();
        let t = self.top_bar(out, &[("arrow_left", Action::Open(Screen::WorldList), true)]);
        self.bar_plaque(out, &t, if create { "Create New World" } else { "Edit World" }, Some(if create { "add" } else { "edit" }));
        let (px, top, x1, y1) = self.page_box(&t);
        let bottom = if typing { h - sb - self.keyboard_height() - 10.0 * dp } else { y1 };
        let pw = x1 - px;
        let ph = (bottom - top).max(60.0 * dp);
        draw::panel(out, px, top, pw, ph, self.k());
        self.buttons.push(Button { x: px, y: top, w: pw, h: ph, action: Action::Nothing });
        let finish_h = if typing {
            0.0
        } else {
            let named = !self.fields[0].trim().is_empty();
            let (label, icon, action) = if create { ("Create World", "add", Action::CreateWorld) } else { ("Save", "check", Action::SaveWorld) };
            let (bw, bh) = ((240.0 * dp).min(pw * 0.5), 42.0 * dp);
            self.button(out, Style::Bar, (px + pw - 18.0 * dp - bw, top + ph - 16.0 * dp - bh, bw, bh), label, Some(icon), Serif, action, named, named);
            bh + 16.0 * dp
        };
        // The fields: centred, not wider than reads well.
        let cw = (pw - 44.0 * dp).min(640.0 * dp);
        let (px, pw) = (px + (pw - cw) * 0.5 - 22.0 * dp, cw + 44.0 * dp);
        let ph = ph - finish_h;
        let pad = 22.0 * dp;
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
            draw::left(out, Serif, name, x, y + label * 0.5, label, BRONZE, true);
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
        5.0 * self.key_h() + 4.0 * 5.0 * self.dp + 20.0 * self.dp
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
        let kw = ((w - sl - sr - 40.0 * dp - 9.0 * gap) / 10.0).min(66.0 * dp);
        let width = 10.0 * kw + 9.0 * gap;
        let (px, pw) = (cx - width * 0.5 - 10.0 * dp, width + 20.0 * dp);
        draw::panel(out, px, y0, pw, total_h, self.k() * 0.8);
        self.buttons.push(Button { x: px, y: y0, w: pw, h: total_h, action: Action::Nothing });
        let rows = ["1234567890", "qwertyuiop", "asdfghjkl", "zxcvbnm"];
        for (r, keys) in rows.iter().enumerate() {
            let n = keys.chars().count() as f32;
            let rw = n * kw + (n - 1.0) * gap;
            let ry = y0 + 10.0 * dp + r as f32 * (kh + gap);
            for (i, c) in keys.chars().enumerate() {
                let c = if self.caps { c.to_ascii_uppercase() } else { c };
                let x = cx - rw * 0.5 + i as f32 * (kw + gap);
                self.button(out, Style::Stone, (x, ry, kw, kh), &c.to_string(), None, Serif, Action::Key(c), true, false);
            }
        }
        let ry = y0 + 10.0 * dp + 4.0 * (kh + gap);
        let x0 = cx - width * 0.5;
        let unit = kw + gap;
        let caps = self.caps;
        self.button(out, Style::Stone, (x0, ry, 1.5 * unit - gap, kh), "", Some("keyboard_capslock"), Serif, Action::Shift, true, caps);
        self.button(out, Style::Stone, (x0 + 1.5 * unit, ry, unit - gap, kh), "-", None, Serif, Action::Key('-'), true, false);
        self.button(out, Style::Stone, (x0 + 2.5 * unit, ry, 4.0 * unit - gap, kh), "", Some("space_bar"), Serif, Action::Key(' '), true, false);
        self.button(out, Style::Stone, (x0 + 6.5 * unit, ry, 1.5 * unit - gap, kh), "", Some("backspace"), Serif, Action::Backspace, true, false);
        self.button(out, Style::Stone, (x0 + 8.0 * unit, ry, 2.0 * unit - gap, kh), "Done", Some("check"), Serif, Action::KeysDone, true, false);
    }

    /// DELETE WORLD: the question in a panel filling the page, Cancel and Delete under it.
    fn draw_delete(&mut self, out: &mut Vec<HudVertex>) {
        let dp = self.dp;
        let t = self.top_bar(out, &[("arrow_left", Action::Open(Screen::WorldList), true)]);
        self.bar_plaque(out, &t, "Delete World", Some("delete"));
        let (x0, top, x1, y1) = self.page_box(&t);
        let (pw, ph) = (x1 - x0, y1 - top);
        let cx = x0 + pw * 0.5;
        draw::panel(out, x0, top, pw, ph, self.k());
        self.buttons.push(Button { x: x0, y: top, w: pw, h: ph, action: Action::Nothing });
        let bh = 42.0 * dp;
        let rw = (440.0 * dp).min(pw - 40.0 * dp);
        let by = top + ph - 18.0 * dp - bh;
        self.button_row(
            out,
            (cx - rw * 0.5, by, rw, bh),
            &[("Cancel", "arrow_left", Action::Open(Screen::WorldList), true, false, 1.0), ("Delete", "delete", Action::ConfirmDelete, true, false, 1.0)],
        );
        let name = self.picked().map(|e| e.name.clone()).unwrap_or_default();
        let mid = top + (by - top) * 0.5;
        let tw = pw.min(700.0 * dp);
        draw::icon(out, "warning", cx, mid - 40.0 * dp, 44.0 * dp, WARN);
        let q = "Are you sure you want to delete this world?";
        draw::centered(out, Serif, q, cx, mid + 2.0 * dp, draw::fit(Serif, q, 21.0 * dp, tw * 0.86), INK, true);
        let l = format!("'{name}' will be lost forever! (A long time!)");
        draw::centered(out, Serif, &l, cx, mid + 30.0 * dp, draw::fit(Serif, &l, 17.0 * dp, tw * 0.86), DANGER, true);
    }

    /// The settings: groups on the left, the group's settings in the middle, what the group
    /// is about on the right. Changes take effect at once.
    fn draw_settings(&mut self, out: &mut Vec<HudVertex>) {
        let (w, h) = self.size;
        let dp = self.dp;
        let k = self.k();
        let back = self.settings_back;
        if back == Screen::Pause {
            rect(out, 0.0, 0.0, w, h, [0, 0, 0, 120]);
        }
        // Reset, Back (to where the settings were opened from) and X (from the game menu,
        // straight back into the game).
        let close = if back == Screen::Pause { Screen::Playing } else { back };
        let t = self.top_bar(out, &[("restart_alt", Action::ResetSettings, true), ("arrow_left", Action::Open(back), true), ("close", Action::Open(close), true)]);
        self.bar_plaque(out, &t, "Settings", Some("settings"));
        let (x0, y0, x1, y1) = self.page_box(&t);
        let (fw, fh) = (x1 - x0, y1 - y0);
        draw::panel(out, x0, y0, fw, fh, k);
        self.buttons.push(Button { x: x0, y: y0, w: fw, h: fh, action: Action::Nothing });
        let pad = 16.0 * dp;
        let (ix, iy, iw, ih) = (x0 + pad, y0 + pad, fw - 2.0 * pad, fh - 2.0 * pad);

        // Groups.
        let tw = (190.0 * dp).min(iw * 0.25);
        let tgap = 6.0 * dp;
        let trh = (40.0 * dp).min((ih - tgap * (TABS.len() as f32 - 1.0)) / TABS.len() as f32);
        for (i, (name, icon, _)) in TABS.iter().enumerate() {
            let y = iy + i as f32 * (trh + tgap);
            let on = self.tab as usize == i;
            self.button(out, Style::Row, (ix, y, tw, trh), name, Some(icon), Serif, Action::Tab(i as u8), true, on);
        }
        let sep = |out: &mut Vec<HudVertex>, x: f32| rect(out, x, iy, (1.5 * dp).max(1.0), ih, [150, 124, 86, 150]);
        let cx0 = ix + tw + 14.0 * dp;
        sep(out, cx0 - 7.0 * dp);
        let s = self.settings;
        let on_off = |b: bool, a: Action| Control::Switch(b, a);
        let rows: Vec<(&str, Control)> = match self.tab {
            0 => vec![
                ("Mode", Control::Stepper(if s.walk { "Walk".into() } else { "Fly".into() }, Action::ToggleWalk, Action::ToggleWalk)),
                (
                    "Camera",
                    Control::Stepper(
                        match s.view {
                            super::CamView::First => "First Person".into(),
                            super::CamView::Back => "Behind".into(),
                            super::CamView::Front => "In Front".into(),
                        },
                        Action::ToggleView,
                        Action::ToggleView,
                    ),
                ),
                ("Show Stats", on_off(s.show_stats, Action::ToggleStats)),
                if back == Screen::Pause {
                    ("Time of Day", Control::Stepper(crate::sky::time_label(self.hours), Action::Time(-1), Action::Time(1)))
                } else {
                    ("Time of Day", Control::Fixed("Set in each world".into()))
                },
            ],
            1 => vec![
                ("Render Distance", Control::Stepper(format!("{} Chunks", s.render_chunks), Action::RenderDist(-1), Action::RenderDist(1))),
                ("LOD Distance", Control::Stepper(lod_label(s.lod_level), Action::LodDist(-1), Action::LodDist(1))),
                ("LOD Quality", Control::Stepper(quality_name(s.lod_quality), Action::CycleLodQuality, Action::CycleLodQuality)),
                ("Fog", on_off(s.fog, Action::ToggleFog)),
                ("Clouds", on_off(s.clouds, Action::ToggleClouds)),
                ("God Rays", on_off(s.god_rays, Action::ToggleRays)),
                ("Resolution", Control::Stepper(super::resolution_label(s.resolution), Action::CycleResolution, Action::CycleResolution)),
                if self.vsync_supported {
                    ("VSync", on_off(s.vsync, Action::ToggleVsync))
                } else {
                    ("VSync", Control::Fixed("Always on".into()))
                },
            ],
            2 => {
                use crate::look::Field;
                let l = s.look;
                let mut v = vec![
                    ("Shaders", Control::Stepper(crate::shadows::Quality::from_index(s.shaders).label().into(), Action::CycleShaders, Action::CycleShaders)),
                    ("Preset", Control::Stepper(l.preset_name().into(), Action::CyclePreset(-1), Action::CyclePreset(1))),
                ];
                for f in Field::ALL {
                    v.push((f.label(), Control::Stepper(l.value_label(f), Action::LookStep(f, -1), Action::LookStep(f, 1))));
                }
                v.push(("Warm Torchlight", on_off(l.warm_torches, Action::ToggleTorches)));
                v
            }
            3 => vec![
                ("Culling", Control::Stepper(cull_name(s.cull), Action::CycleCull, Action::CycleCull)),
                ("Grass", Control::Stepper(["Low", "Medium", "High"][s.grass.min(2) as usize].into(), Action::CycleGrass, Action::CycleGrass)),
                ("Mesher", Control::Stepper(if s.greedy { "Greedy".into() } else { "Simple".into() }, Action::ToggleMesher, Action::ToggleMesher)),
            ],
            _ => vec![
                ("Button Layout", Control::Choice("Customize".into(), Action::OpenControls)),
                ("Button Opacity", Control::Stepper(format!("{}%", s.hud_opacity), Action::HudOpacity(-1), Action::HudOpacity(1))),
                ("Look Sensitivity", Control::Stepper(s.look_speed.to_string(), Action::LookSpeed(-1), Action::LookSpeed(1))),
                ("Joystick Size", Control::Stepper(super::controls::JOY_SIZES[s.joy_size as usize % 3].into(), Action::JoySize(-1), Action::JoySize(1))),
                (
                    "Building",
                    Control::Stepper(if s.touch_build { "Touch + Buttons".into() } else { "Buttons".into() }, Action::ToggleTouchBuild, Action::ToggleTouchBuild),
                ),
                (
                    "Crouch, Crawl, Roll",
                    Control::Stepper(if s.one_stance { "One Button".into() } else { "Three Buttons".into() }, Action::ToggleOneStance, Action::ToggleOneStance),
                ),
            ],
        };
        // With many settings the note on the right gives way to a second column.
        let many = rows.len() > 5;
        // What the group is about, on the right.
        let dw = if many { 0.0 } else { (220.0 * dp).min(iw * 0.27) };
        let dx = ix + iw - dw;
        let (tab_name, tab_icon, tab_note) = TABS[self.tab as usize % TABS.len()];
        if !many {
        sep(out, dx - 9.0 * dp);
        draw::field(out, dx, iy, dw, ih, k * 0.7);
        let dpad = 12.0 * dp;
        draw::icon(out, tab_icon, dx + dpad + 12.0 * dp, iy + dpad + 12.0 * dp, 24.0 * dp, BRONZE);
        draw::left(out, Serif, tab_name, dx + dpad + 30.0 * dp, iy + dpad + 13.0 * dp, draw::fit(Serif, tab_name, 20.0 * dp, dw - 50.0 * dp), INK, true);
        self.rule(out, dx + dpad, iy + dpad + 34.0 * dp, dw - 2.0 * dpad);
        // The note, then a sepia picture in what room is left.
        let ns = 14.0 * dp;
        let mut ny = iy + dpad + 52.0 * dp;
        for l in draw::wrap(Serif, tab_note, ns, dw - 2.0 * dpad) {
            if ny > iy + ih - dpad - ns {
                break;
            }
            draw::left(out, Serif, &l, dx + dpad, ny, ns, DIM, true);
            ny += ns * 1.3;
        }
        let pic_h = ((dw - 2.0 * dpad) * 9.0 / 16.0).min(iy + ih - dpad - ny);
        if let (Some(s), true) = (self.default_thumb, pic_h > 40.0 * dp) {
            let pw = pic_h * 16.0 / 9.0;
            draw::thumb_tinted(out, crate::gfx::thumb_uv(s), dx + (dw - pw) * 0.5, iy + ih - dpad - pic_h, pw, pic_h, SEPIA);
        }
        }

        // The group's settings in the middle.
        let (mx, mw) = (cx0, if many { ix + iw - cx0 } else { dx - 18.0 * dp - cx0 });
        draw::icon(out, tab_icon, mx + 14.0 * dp, iy + 14.0 * dp, 26.0 * dp, INK);
        draw::left(out, Serif, tab_name, mx + 36.0 * dp, iy + 15.0 * dp, 24.0 * dp, INK, true);
        self.rule(out, mx, iy + 36.0 * dp, mw);
        let ry0 = iy + 44.0 * dp;
        let n = rows.len().max(1);
        let cols = if many { 2 } else { 1 };
        let per = n.div_ceil(cols);
        let colgap = 24.0 * dp;
        let (mx0, mw) = (mx, (mw - colgap * (cols - 1) as f32) / cols as f32);
        let rh = (44.0 * dp).min((iy + ih - ry0) / per as f32);
        let cw = (170.0 * dp).min(mw * 0.5);
        let ch = (rh * 0.76).min(34.0 * dp);
        for (i, (label, control)) in rows.into_iter().enumerate() {
            let (col, row) = (i / per, i % per);
            let mx = mx0 + col as f32 * (mw + colgap);
            if col > 0 && row == 0 {
                rect(out, mx - colgap * 0.5, ry0, (1.0 * dp).max(1.0), per as f32 * rh, [150, 124, 86, 150]);
            }
            let (i, n) = (row, per);
            let ry = ry0 + row as f32 * rh;
            let cy = ry + rh * 0.5;
            let ls = draw::fit(Serif, label, (rh * 0.46).min(19.0 * dp), mw - cw - 20.0 * dp);
            draw::left(out, Serif, label, mx + 6.0 * dp, cy, ls, INK, true);
            if i + 1 < n {
                rect(out, mx, ry + rh - 0.5 * dp, mw, (1.0 * dp).max(1.0), FAINT);
            }
            let cx = mx + mw - cw;
            let vs = (ch * 0.52).min(17.0 * dp);
            match control {
                Control::Switch(on, a) => {
                    let sw = ch * 120.0 / 56.0;
                    let sx = mx + mw - sw;
                    draw::image(out, if on { "toggle_on" } else { "toggle_off" }, sx, cy - ch * 0.5, sw, ch, OPAQUE);
                    self.buttons.push(Button { x: sx - 10.0 * dp, y: ry, w: sw + 10.0 * dp, h: rh, action: a });
                }
                Control::Stepper(value, down, up) => {
                    draw::field(out, cx, cy - ch * 0.5, cw, ch, k * 0.6);
                    draw::centered(out, Serif, &value, cx + cw * 0.5, cy, draw::fit(Serif, &value, vs, cw - 1.8 * ch), INK, false);
                    draw::icon(out, "arrow_left", cx + ch * 0.5, cy, ch * 0.9, BRONZE);
                    draw::icon(out, "arrow_right", cx + cw - ch * 0.5, cy, ch * 0.9, BRONZE);
                    self.buttons.push(Button { x: cx, y: ry, w: cw * 0.5, h: rh, action: down });
                    self.buttons.push(Button { x: cx + cw * 0.5, y: ry, w: cw * 0.5, h: rh, action: up });
                }
                Control::Choice(value, a) => {
                    self.button(out, Style::Bar, (cx, cy - ch * 0.5, cw, ch), &value, Some("open_with"), Serif, a, true, false);
                }
                Control::Fixed(value) => {
                    draw::right(out, Serif, &value, mx + mw - 4.0 * dp, cy, draw::fit(Serif, &value, vs, cw), DIM, true);
                }
            }
        }
    }

    /// The game menu, over the world.
    fn draw_pause(&mut self, out: &mut Vec<HudVertex>) {
        let (w, h) = self.size;
        let dp = self.dp;
        let [sl, st, sr, sb] = self.margins();
        rect(out, 0.0, 0.0, w, h, [0, 0, 0, 130]);
        let cx = (sl + w - sr) * 0.5;
        let ph = 46.0 * dp;
        let pw = (300.0 * dp).min(w - sl - sr - 20.0 * dp);
        let gap = 10.0 * dp;
        let bh = (46.0 * dp).min((h - st - sb - ph - 50.0 * dp - 2.0 * gap) / 3.0);
        let total = ph + 26.0 * dp + 3.0 * bh + 2.0 * gap;
        let y0 = st + ((h - st - sb) - total) * 0.5 + 10.0 * dp;
        self.plaque(out, cx, y0, pw, ph, "Game Menu", None);
        let bw = (300.0 * dp).min(w - sl - sr - 20.0 * dp);
        let items: [(&str, &str, Action); 3] = [
            ("Back to Game", "play_arrow", Action::Open(Screen::Playing)),
            ("Settings", "settings", Action::OpenSettings),
            ("Save & Quit to Title", "exit_to_app", Action::Open(Screen::Title)),
        ];
        for (i, (label, icon, action)) in items.iter().enumerate() {
            let y = y0 + ph + 26.0 * dp + i as f32 * (bh + gap);
            self.button(out, Style::Menu, (cx - bw * 0.5, y, bw, bh), label, Some(icon), Serif, *action, true, i == 0 && self.pressed_action().is_none());
        }
    }

    fn draw_loading(&mut self, out: &mut Vec<HudVertex>) {
        let (w, h) = self.size;
        let dp = self.dp;
        let [sl, _, sr, _] = self.margins();
        let cx = (sl + w - sr) * 0.5;
        let lw = ((w - sl - sr) * 0.5).min(460.0 * dp);
        let lh = lw / LOGO_ASPECT;
        let ly = h * 0.44 - lh;
        draw::image(out, "logo", cx - lw * 0.5, ly, lw, lh, OPAQUE);
        draw::centered(out, Serif, "Loading terrain...", cx, ly + lh + 22.0 * dp, 22.0 * dp, INK, true);
        let bw = (340.0 * dp).min((w - sl - sr) * 0.7);
        let bh = 20.0 * dp;
        let by = ly + lh + 44.0 * dp;
        draw::field(out, cx - bw * 0.5, by, bw, bh, self.k() * 0.6);
        let f = (self.loading_chunks as f32 / 120.0).min(1.0);
        let inner = bw - 10.0 * dp;
        let (fx, fy, fh) = (cx - bw * 0.5 + 5.0 * dp, by + 5.0 * dp, bh - 10.0 * dp);
        rect(out, fx, fy, inner * f, fh, [40, 120, 240, 255]);
        rect(out, fx, fy, inner * f, fh * 0.45, [120, 186, 255, 255]);
        let g = bh * 1.3;
        draw::image(out, "gem", fx + inner * f - g * 0.5, by + bh * 0.5 - g * 0.5, g, g, OPAQUE);
        let n = format!("{} chunks", self.loading_chunks);
        draw::centered(out, Serif, &n, cx, by + bh + 18.0 * dp, 15.0 * dp, DIM, true);
    }

    /// MULTIPLAYER, MODS, COSMETICS: what is (not) there yet, in a panel filling the page.
    fn draw_page(&mut self, out: &mut Vec<HudVertex>, title: &str, icon: &str, lines: &[&str]) {
        let dp = self.dp;
        let t = self.top_bar(out, &[("arrow_left", Action::Open(Screen::Title), true)]);
        if title == "Multiplayer" {
            self.tabs(out, &t, true);
        } else {
            self.bar_plaque(out, &t, title, Some(icon));
        }
        let (x0, top, x1, y1) = self.page_box(&t);
        let (pw, ph) = (x1 - x0, y1 - top);
        let cx = x0 + pw * 0.5;
        draw::panel(out, x0, top, pw, ph, self.k());
        let pw = pw.min(640.0 * dp);
        let size = 18.0 * dp;
        let mut wrapped: Vec<(String, bool)> = Vec::new();
        for (i, l) in lines.iter().enumerate() {
            for part in draw::wrap(Serif, l, size, pw * 0.82) {
                wrapped.push((part, i == 0));
            }
        }
        let line = size * 1.45;
        let block = 60.0 * dp + line * wrapped.len() as f32;
        let mut y = top + (ph - block) * 0.5;
        draw::icon(out, icon, cx, y + 22.0 * dp, 44.0 * dp, BLUE);
        y += 60.0 * dp;
        for (l, first) in wrapped {
            draw::centered(out, Serif, &l, cx, y + line * 0.5, size, if first { INK } else { DIM }, true);
            y += line;
        }
    }

    /// The inventory, over the game: every block in a grid on the left; the picked one's
    /// picture, name and story on the right, with the hotbar to put it in.
    fn draw_inventory(&mut self, out: &mut Vec<HudVertex>) {
        let (w, h) = self.size;
        let dp = self.dp;
        let k = self.k();
        let [sl, st, sr, sb] = self.margins();
        rect(out, 0.0, 0.0, w, h, [0, 0, 0, 120]);
        // The top bar: the left window's tabs at the left, the right window's tabs and the
        // X at the right, the screen's name between them.
        let (sq, gap) = (44.0 * dp, 8.0 * dp);
        let ty = st + 6.0 * dp;
        let mut lx = sl + 8.0 * dp;
        for (n, (name, icon)) in items::LEFT_TABS.iter().enumerate() {
            let on = self.left_tab as usize == n;
            self.button(out, Style::Stone, (lx, ty, sq, sq), "", Some(icon), Serif, Action::LeftTab(n as u8), true, on);
            let _ = name;
            lx += sq + gap;
        }
        let mut rx = w - sr - 8.0 * dp - sq;
        self.button(out, Style::Stone, (rx, ty, sq, sq), "", Some("close"), Serif, Action::Open(Screen::Playing), true, false);
        for (n, (_, icon)) in items::RIGHT_TABS.iter().enumerate().rev() {
            rx -= sq + gap;
            let on = self.right_tab as usize == n;
            self.button(out, Style::Stone, (rx, ty, sq, sq), "", Some(icon), Serif, Action::RightTab(n as u8), true, on);
        }
        let t = Top { x0: lx + 6.0 * dp, x1: rx - 10.0 * dp, y: ty, h: sq - 2.0 * dp, below: ty + sq + 8.0 * dp };
        self.bar_plaque(out, &t, "Inventory", Some("backpack"));

        // The bottom row: MOVE SIMILAR, MOVE ALL and SORT under the left window, the hotbar
        // in the middle, a line about what is picked at the right.
        let (x0, y0, x1, _) = self.page_box(&t);
        let y1 = h - sb - 6.0 * dp;
        let row = 46.0 * dp;
        let ry = y1 - row;
        let wy1 = ry - 8.0 * dp;
        let mut bx = x0 + 14.0 * dp;
        let hot_any = self.hotbar.iter().any(|s| !s.is_empty());
        let inv_any = self.inventory.iter().any(|s| !s.is_empty());
        for (icon, action, on) in [("merge_type", Action::MoveSimilar, hot_any && inv_any), ("keyboard_double_arrow_up", Action::MoveAll, hot_any), ("sort", Action::SortItems, inv_any)] {
            self.button(out, Style::Stone, (bx, ry, row, row), "", Some(icon), Serif, action, on, false);
            bx += row + gap;
        }
        let hs = row;
        let hx = ((x0 + x1) * 0.5 - 4.5 * hs).max(bx + 6.0 * dp);
        for n in 0..9u8 {
            self.item_slot(out, Slot::Hotbar(n), hx + n as f32 * hs, ry, hs, n as usize == self.slot.min(8));
        }
        let note_x0 = hx + 9.0 * hs + 12.0 * dp;
        let note = match (&self.item_note, self.picked) {
            (Some(n), _) => n.clone(),
            (None, Some(p)) => {
                let v = self.stack(p);
                match p {
                    Slot::Palette(_) => format!("{}: tap a slot for it", self.item_name(v.id)),
                    _ => format!("{} x{}: tap where it goes", self.item_name(v.id), v.count),
                }
            }
            (None, None) => "Tap a block, then a slot".to_string(),
        };
        if x1 - note_x0 > 60.0 * dp {
            // One line if it fits at a readable size, else two.
            let room = x1 - note_x0;
            let size = draw::fit(Serif, &note, 15.0 * dp, room);
            if size >= 12.0 * dp {
                draw::left(out, Serif, &note, note_x0, ry + row * 0.5, size, DIM, true);
            } else {
                let lines = draw::wrap(Serif, &note, 13.0 * dp, room);
                for (n, l) in lines.iter().take(2).enumerate() {
                    draw::left(out, Serif, l, note_x0, ry + row * 0.5 + (n as f32 - 0.5) * 16.0 * dp, 13.0 * dp, DIM, true);
                }
            }
        }

        // The two windows.
        let wgap = 12.0 * dp;
        let lw = (x1 - x0 - wgap) * 0.53;
        let (rwx, rw) = (x0 + lw + wgap, x1 - x0 - lw - wgap);
        let wh = (wy1 - y0).max(80.0 * dp);
        for (x, ww) in [(x0, lw), (rwx, rw)] {
            draw::panel(out, x, y0, ww, wh, k);
            self.buttons.push(Button { x, y: y0, w: ww, h: wh, action: Action::Nothing });
        }
        let pad = 14.0 * dp;
        // Each window's heading: its tab's icon and name.
        let heading = |out: &mut Vec<HudVertex>, x: f32, (name, icon): (&str, &str)| {
            draw::icon(out, icon, x + pad + 11.0 * dp, y0 + pad + 10.0 * dp, 22.0 * dp, INK);
            draw::left(out, Serif, name, x + pad + 28.0 * dp, y0 + pad + 10.0 * dp, 20.0 * dp, INK, true);
        };
        let left = items::LEFT_TABS[self.left_tab as usize % items::LEFT_TABS.len()];
        let right = items::RIGHT_TABS[self.right_tab as usize % items::RIGHT_TABS.len()];
        heading(out, x0, left);
        heading(out, rwx, right);
        let top = y0 + pad + 26.0 * dp;
        let (lbox, rbox) = ((x0 + pad, top, lw - 2.0 * pad, y0 + wh - pad - top), (rwx + pad, top, rw - 2.0 * pad, y0 + wh - pad - top));
        match self.left_tab {
            1 => self.draw_crafting(out, lbox),
            2 => self.draw_equipment(out, lbox),
            _ => {
                let (gx, gy, gw, gh) = lbox;
                let s = ((gw / items::INV_COLS as f32).min(gh / items::INV_ROWS as f32)).min(52.0 * dp);
                let ox = gx + (gw - s * items::INV_COLS as f32) * 0.5;
                let oy = gy + ((gh - s * items::INV_ROWS as f32) * 0.5).max(0.0);
                for n in 0..items::INV_SLOTS {
                    let (c, r) = (n % items::INV_COLS, n / items::INV_COLS);
                    self.item_slot(out, Slot::Inv(n as u8), ox + c as f32 * s, oy + r as f32 * s, s, false);
                }
            }
        }
        match self.right_tab {
            0 => {
                let (gx, gy, gw, gh) = rbox;
                let msg = "Nothing to wear yet: armour and tools will show here once the game has them.";
                let size = 16.0 * dp;
                for (n, l) in draw::wrap(Serif, msg, size, gw * 0.85).iter().enumerate() {
                    draw::centered(out, Serif, l, gx + gw * 0.5, gy + gh * 0.4 + n as f32 * size * 1.4, size, DIM, true);
                }
            }
            _ => self.draw_blocks(out, rbox),
        }
        self.under = out.len();
    }

    /// BLOCKS: a tab for each group (building, natural, wood, minerals) and the group's
    /// blocks under them, by material, scrolled by dragging.
    fn draw_blocks(&mut self, out: &mut Vec<HudVertex>, (gx, gy, gw, gh): (f32, f32, f32, f32)) {
        let dp = self.dp;
        let groups = crate::icons::GROUPS;
        let th = (36.0 * dp).min(gh * 0.2);
        let tw = gw / groups.len() as f32;
        for (n, (_, name, icon)) in groups.iter().enumerate() {
            let on = self.block_group as usize == n;
            let r = (gx + n as f32 * tw + 2.0 * dp, gy, tw - 4.0 * dp, th);
            let label = if tw > 110.0 * dp { *name } else { "" };
            self.button(out, Style::Stone, r, label, Some(icon), Serif, Action::BlockGroup(n as u8), true, on);
        }
        // The biome filter: arrows either side of its name.
        let fy = gy + th + 4.0 * dp;
        let fh = (30.0 * dp).min(gh * 0.16);
        self.button(out, Style::Stone, (gx + 2.0 * dp, fy, fh, fh), "", Some("chevron_left"), Serif, Action::BlockBiome(-1), true, false);
        self.button(out, Style::Stone, (gx + gw - fh - 2.0 * dp, fy, fh, fh), "", Some("chevron_right"), Serif, Action::BlockBiome(1), true, false);
        let biome = match self.block_biome {
            0 => "All biomes",
            b => crate::icons::BIOMES[(b as usize - 1) % crate::icons::BIOMES.len()].1,
        };
        let size = draw::fit(Serif, biome, fh * 0.6, gw - 2.0 * fh - 16.0 * dp);
        draw::centered(out, Serif, biome, gx + gw * 0.5, fy + fh * 0.5, size, INK, true);
        let top = fy + fh + 6.0 * dp;
        let (area_h, bar) = (gy + gh - top, 6.0 * dp);
        let (g, b) = (self.block_group, self.block_biome);
        let shown: Vec<usize> = (0..self.blocks.len()).filter(|&i| self.blocks[i].group == g && (b == 0 || self.blocks[i].biome == b)).collect();
        if shown.is_empty() {
            let msg = "No blocks of this kind for this biome.";
            draw::centered(out, Serif, msg, gx + gw * 0.5, top + area_h * 0.4, draw::fit(Serif, msg, 16.0 * dp, gw * 0.9), DIM, true);
        }
        let cols = ((gw - bar) / (50.0 * dp)).floor().clamp(4.0, 12.0) as usize;
        // As many whole rows as fill the window (the slots a little smaller to fit them).
        let sw = ((gw - bar - 4.0 * dp) / cols as f32).min(60.0 * dp);
        let vis = ((area_h / sw).round() as usize).max(1);
        let s = sw.min(area_h / vis as f32);
        let rows = shown.len().div_ceil(cols);
        self.palette_max = rows.saturating_sub(vis);
        self.palette_row = s;
        self.palette_scroll = self.palette_scroll.min(self.palette_max);
        let ox = gx + (gw - bar - s * cols as f32) * 0.5;
        let first = self.palette_scroll * cols;
        for (k, &i) in shown.iter().enumerate().skip(first).take(vis * cols) {
            let k = k - first;
            self.item_slot(out, Slot::Palette(i as u16), ox + (k % cols) as f32 * s, top + (k / cols) as f32 * s, s, false);
        }
        // The scroll bar: where the shown rows are among all of them.
        if self.palette_max > 0 {
            let x = gx + gw - bar;
            let h = vis as f32 * s;
            rect(out, x, top, bar * 0.6, h, [60, 50, 40, 120]);
            let (t0, t1) = (self.palette_scroll as f32 / rows as f32, (self.palette_scroll + vis) as f32 / rows as f32);
            rect(out, x, top + h * t0, bar * 0.6, h * (t1 - t0), [235, 215, 175, 220]);
        }
    }

    /// A slot of the inventory screen: its frame, what is in it and how many, lit while
    /// picked up (or, for the hotbar, while in hand); tapping it is `Action::Item`.
    fn item_slot(&mut self, out: &mut Vec<HudVertex>, slot: Slot, x: f32, y: f32, s: f32, in_hand: bool) {
        let k = self.k();
        let inset = s * 0.04;
        let (x, y, s) = (x + inset, y + inset, s - 2.0 * inset);
        nine(out, "slot", x, y, s, s, k * 0.45, OPAQUE);
        let v = self.stack(slot);
        if let Some(icon) = self.icon_of(v.id).filter(|_| !v.is_empty()) {
            draw::thumb(out, crate::gfx::icon_uv(icon), x + s * 0.14, y + s * 0.11, s * 0.72, s * 0.72);
            if v.count > 1 && !matches!(slot, Slot::Palette(_)) {
                let c = v.count.to_string();
                draw::right(out, Serif, &c, x + s - s * 0.08, y + s * 0.8, s * 0.32, INK, true);
            }
        } else if let Slot::Equip(n) = slot {
            let l = items::EQUIP_SLOTS[n as usize % 4];
            draw::centered(out, Serif, l, x + s * 0.5, y + s * 0.5, draw::fit(Serif, l, s * 0.26, s * 0.85), [150, 140, 125, 200], false);
        }
        if self.picked == Some(slot) || in_hand {
            let g = 18.0 * k * 0.5;
            nine(out, "glow", x - g, y - g, s + 2.0 * g, s + 2.0 * g, k * 0.5, OPAQUE);
        }
        self.buttons.push(Button { x, y, w: s, h: s, action: Action::Item(slot) });
    }

    /// A block's name, as the palette shows it.
    fn item_name(&self, id: u16) -> String {
        self.blocks.iter().find(|b| b.id == id).map_or_else(String::new, |b| b.name.clone())
    }

    /// CRAFTING: the 2 x 2 grid, an arrow and what it makes; the recipes under it.
    fn draw_crafting(&mut self, out: &mut Vec<HudVertex>, (gx, gy, gw, gh): (f32, f32, f32, f32)) {
        let dp = self.dp;
        let s = (gh * 0.24).min(gw * 0.18).min(50.0 * dp);
        let total = 2.0 * s + 50.0 * dp + s * 1.15;
        let ox = gx + (gw - total) * 0.5;
        let oy = gy + 6.0 * dp;
        for n in 0..items::CRAFT_SLOTS {
            self.item_slot(out, Slot::Craft(n as u8), ox + (n % 2) as f32 * s, oy + (n / 2) as f32 * s, s, false);
        }
        let ax = ox + 2.0 * s + 25.0 * dp;
        draw::icon(out, "arrow_forward", ax, oy + s, 30.0 * dp, INK);
        let r = s * 1.15;
        let ready = self.craft_result().is_some();
        self.item_slot(out, Slot::Result, ax + 25.0 * dp, oy + s - r * 0.5, r, ready);
        // What can be made.
        let size = 13.0 * dp;
        let mut y = oy + 2.0 * s + 16.0 * dp;
        draw::left(out, Serif, "Recipes", gx, y, 15.0 * dp, INK, true);
        y += 18.0 * dp;
        // With blocks in the grid, the recipes that use them; else every recipe (as many as fit).
        let held: Vec<u16> = self.craft.iter().filter(|s| !s.is_empty()).map(|s| s.id).collect();
        let lines: Vec<String> = self
            .recipes
            .iter()
            .filter(|r| held.is_empty() || r.inputs.iter().any(|(id, _)| held.contains(id)))
            .map(|r| {
                let ins: Vec<String> = r.inputs.iter().map(|(id, n)| format!("{n} {}", self.item_name(*id))).collect();
                format!("{}  =  {} {}", ins.join(" + "), r.output.count, self.item_name(r.output.id))
            })
            .collect();
        let cols = 2;
        let col_w = gw / cols as f32;
        let rows = ((((gy + gh) - y) / (size * 1.45)).floor() as usize).max(1);
        for (n, l) in lines.iter().enumerate().take(rows * cols) {
            let (c, row) = (n / rows.max(1), n % rows.max(1));
            draw::left(out, Serif, l, gx + c as f32 * col_w, y + row as f32 * size * 1.45, draw::fit(Serif, l, size, col_w - 8.0 * dp), DIM, true);
        }
    }

    /// EQUIPMENT: what he wears, in a column, and the character himself (turned by the
    /// arrows under him).
    fn draw_equipment(&mut self, out: &mut Vec<HudVertex>, (gx, gy, gw, gh): (f32, f32, f32, f32)) {
        let dp = self.dp;
        let s = (gh / 4.0).min(54.0 * dp);
        for n in 0..4u8 {
            self.item_slot(out, Slot::Equip(n), gx, gy + n as f32 * s, s, false);
        }
        // The character: in the rest of the window, on his feet above the arrows.
        let (cx0, cw) = (gx + s + 12.0 * dp, gw - s - 12.0 * dp);
        let arrows = 40.0 * dp;
        let feet = gy + gh - arrows - 10.0 * dp;
        let height = (feet - gy - 6.0 * dp).max(40.0 * dp);
        let cx = cx0 + cw * 0.5;
        self.showcase = Some(Showcase { x: cx, feet, height, yaw: self.showcase_yaw });
        let ay = gy + gh - arrows;
        self.button(out, Style::Stone, (cx - arrows * 1.6, ay, arrows, arrows), "", Some("chevron_left"), Serif, Action::TurnShowcase(-1), true, false);
        self.button(out, Style::Stone, (cx + arrows * 0.6, ay, arrows, arrows), "", Some("chevron_right"), Serif, Action::TurnShowcase(1), true, false);
    }

    /// The HUD in the game: crosshair, stats, the game's buttons.
    pub(super) fn draw_hud(&mut self, perf: &Perf, out: &mut Vec<HudVertex>) {
        let (w, h) = self.size;
        let dp = self.dp;
        let (cx, cy) = (w * 0.5, h * 0.5);
        let len = 10.0 * dp;
        let th = (1.5 * dp).max(2.0);
        rect(out, cx - len, cy - th * 0.5, len * 2.0, th, XHAIR);
        rect(out, cx - th * 0.5, cy - len, th, len * 2.0, XHAIR);
        let [safe_left, safe_top, _, _] = self.margins();
        if self.settings.show_stats {
            self.draw_stats(perf, out, safe_left + 8.0 * dp, safe_top + 4.0 * dp);
        }
        self.draw_controls(out);
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
