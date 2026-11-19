"""Builds the menus' look: assets/ui/atlas.png and assets/ui/atlas.json.

One RGBA texture holds everything the menus draw with:
  * fonts as signed distance fields (SDF: crisp at any size, alpha = distance to the edge):
    Crimson Text 600 ("serif", the menus' text) and Pixelify Sans 500 ("pixel", the stats
    overlay), printable ASCII;
  * icons from Material Icons, also as SDF, by name;
  * the ornate pieces painted by ui_ornate.py: buttons with pointed ends, small clipped
    buttons, framed stone panels with bronze corner brackets, sunken boxes and slots, the
    title plaque, rules, switches, gems, the blue glow (straight, pointed and round), round
    buttons and the stick for the game's controls, the pickaxe, the pedestal;
  * the BlockForge logo (Almendra Bold, steel letters with flourished rules and a gem);
  * a white square for plain rectangles.

The fonts come from Fontsource's npm packages (all SIL Open Font License; the licences are
copied to assets/ui/licenses, credits in assets/ui/CREDITS.md):

    cd <dir> && for p in almendra crimson-text pixelify-sans material-icons; do
        npm pack @fontsource/$p && mkdir -p $p && tar xzf fontsource-$p-*.tgz -C $p; done
    python3 tools/make_ui_atlas.py <dir> assets/ui
"""

import json
import math
import os
import sys

import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont
from scipy import ndimage

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ui_ornate as O  # noqa: E402

SRC = sys.argv[1]
OUT = sys.argv[2]
ATLAS = 2048
rng = np.random.default_rng(7)


def font_file(pkg, name):
    return os.path.join(SRC, pkg, "package", "files", name)


SERIF = font_file("crimson-text", "crimson-text-latin-600-normal.woff")
PIXEL = font_file("pixelify-sans", "pixelify-sans-latin-500-normal.woff")
LOGO = font_file("almendra", "almendra-latin-700-normal.woff")
ICONS = font_file("material-icons", "material-icons-latin-400-normal.woff")

ICON_CODES = {
    "person": 0xE7FD, "group": 0xE7EF, "settings": 0xE8B8, "brush": 0xE3AE, "extension": 0xE87B,
    "logout": 0xE9BA, "undo": 0xE166, "add": 0xE145, "play_arrow": 0xE037, "upload": 0xF09B,
    "edit": 0xE3C9, "delete": 0xE872, "open_in_full": 0xF1CE, "desktop_windows": 0xE30C,
    "volume_up": 0xE050, "sports_esports": 0xEA28, "keyboard": 0xE312, "view_in_ar": 0xE9FE,
    "palette": 0xE40A, "language": 0xE894, "check": 0xE5CA, "arrow_back": 0xE5C4, "menu": 0xE5D2,
    "close": 0xE5CD, "save": 0xE161, "pause": 0xE034, "home": 0xE88A, "landscape": 0xE3F7,
    "tune": 0xE429, "speed": 0xE9E4, "backspace": 0xE14A, "keyboard_capslock": 0xE318,
    "space_bar": 0xE256, "warning": 0xE002, "restart_alt": 0xF053, "directions_run": 0xE566,
    "sd_storage": 0xE1C2, "calendar_today": 0xE935, "timer": 0xE425, "terrain": 0xE564,
    "videocam": 0xE04B, "visibility": 0xE8F4, "grid_view": 0xE9B0, "expand_less": 0xE5CE,
    "expand_more": 0xE5CF, "chevron_left": 0xE5CB, "chevron_right": 0xE5CC, "cloud": 0xE2BD,
    "wb_sunny": 0xE430, "layers": 0xE53B, "memory": 0xE322, "touch_app": 0xE913, "info": 0xE88E,
    "public": 0xE80B, "refresh": 0xE5D5, "casino": 0xEB40, "accessibility_new": 0xE92C,
    "filter_hdr": 0xE3DF, "auto_awesome": 0xE65F, "fullscreen": 0xE5D0,
    # The game's controls and their editor.
    "apps": 0xE5C3, "open_with": 0xE89F, "remove": 0xE15B, "visibility_off": 0xE8F5,
    "keyboard_double_arrow_up": 0xEACF, "add_box": 0xE146, "opacity": 0xE91C, "gamepad": 0xE30F,
    "zoom_out_map": 0xE56B, "front_hand": 0xE769, "gavel": 0xE90E, "inventory_2": 0xE1A1,
    # The ornate menus.
    "arrow_left": 0xE5DE, "arrow_right": 0xE5DF, "arrow_drop_down": 0xE5C5, "backpack": 0xF19C,
    "image": 0xE3F4, "exit_to_app": 0xE879, "star": 0xE838, "download": 0xF090,
    "description": 0xE873, "event": 0xE878, "monitor": 0xEF5B, "insert_drive_file": 0xE24D,
}


# ---------------------------------------------------------------------------------------
# Packing
# ---------------------------------------------------------------------------------------

class Packer:
    """Shelves, tallest first."""

    def __init__(self, size):
        self.size = size
        self.img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
        self.items = []

    def add(self, key, img):
        self.items.append((key, img))

    def pack(self):
        out = {}
        x = y = shelf = 0
        pad = 2
        for key, img in sorted(self.items, key=lambda kv: -kv[1].size[1]):
            w, h = img.size
            if x + w + pad > self.size:
                x, y, shelf = 0, y + shelf + pad, 0
            if y + h > self.size:
                raise SystemExit(f"atlas full at {key}")
            self.img.paste(img, (x, y))
            out[key] = [x, y, w, h]
            x += w + pad
            shelf = max(shelf, h)
        print(f"atlas used to y={y + shelf} of {self.size}")
        return out


# ---------------------------------------------------------------------------------------
# Signed distance fields
# ---------------------------------------------------------------------------------------

def sdf_from_mask(mask_hi, factor, spread):
    """mask_hi: bool array at `factor` times the output size. Returns uint8 alpha:
    128 on the edge, more inside, `spread` output pixels to 0 / 255."""
    inside = ndimage.distance_transform_edt(mask_hi)
    outside = ndimage.distance_transform_edt(~mask_hi)
    d = (inside - outside) / factor  # in output pixels, + inside
    h, w = d.shape
    d = d[: h - h % factor, : w - w % factor].reshape(h // factor, factor, w // factor, factor).mean(axis=(1, 3))
    a = np.clip(0.5 + d / (2 * spread), 0, 1)
    return (a * 255 + 0.5).astype(np.uint8)


def sdf_glyph(font_path, ch, em, spread, factor=4):
    """(image, offset x, offset y from the pen at the baseline, advance) at `em` pixels."""
    font = ImageFont.truetype(font_path, em * factor)
    asc, _ = font.getmetrics()
    adv = font.getlength(ch) / factor
    bbox = font.getbbox(ch)
    pad = spread * factor + factor
    if bbox[2] <= bbox[0] or bbox[3] <= bbox[1]:
        return None, 0, 0, adv
    w = bbox[2] - bbox[0] + 2 * pad
    h = bbox[3] - bbox[1] + 2 * pad
    w += (-w) % factor
    h += (-h) % factor
    img = Image.new("L", (w, h), 0)
    ImageDraw.Draw(img).text((pad - bbox[0], pad - bbox[1]), ch, font=font, fill=255)
    a = sdf_from_mask(np.asarray(img) > 127, factor, spread)
    rgba = np.zeros(a.shape + (4,), np.uint8)
    rgba[..., :3] = 255
    rgba[..., 3] = a
    ox = (bbox[0] - pad) / factor
    oy = (bbox[1] - pad - asc) / factor  # from the baseline, y down
    return Image.fromarray(rgba, "RGBA"), ox, oy, adv


def font_entry(packer, key, path, em, spread):
    font = ImageFont.truetype(path, em)
    asc, desc = font.getmetrics()
    glyphs = {}
    for c in range(32, 127):
        img, ox, oy, adv = sdf_glyph(path, chr(c), em, spread)
        g = {"adv": round(adv, 3)}
        if img is not None:
            packer.add((key, c), img)
            g["off"] = [round(ox, 3), round(oy, 3)]
        glyphs[str(c)] = g
    return {"em": em, "spread": spread, "ascent": asc, "descent": desc, "glyphs": glyphs}


def icon_entry(packer, em, spread):
    out = {}
    for name, code in ICON_CODES.items():
        img, ox, oy, adv = sdf_glyph(ICONS, chr(code), em, spread)
        packer.add(("icon", name), img)
        out[name] = {"adv": round(adv, 3), "off": [round(ox, 3), round(oy, 3)]}
    return {"em": em, "spread": spread, "glyphs": out}


# ---------------------------------------------------------------------------------------
# Painting helpers
# ---------------------------------------------------------------------------------------

def to_img(arr, alpha=None):
    a = np.clip(arr, 0, 255).astype(np.uint8)
    if a.shape[2] == 3:
        al = np.full(a.shape[:2], 255, np.uint8) if alpha is None else np.clip(alpha, 0, 255).astype(np.uint8)
        a = np.dstack([a, al])
    return Image.fromarray(a, "RGBA")


def glow_frame(w, h, inset, radius, color=(70, 150, 255)):
    """A blue outline with a soft outer glow, open in the middle."""
    ss = 2
    m = Image.new("L", (w * ss, h * ss), 0)
    d = ImageDraw.Draw(m)
    d.rounded_rectangle([inset * ss, inset * ss, (w - inset) * ss - 1, (h - inset) * ss - 1], radius=radius * ss, outline=255, width=3 * ss)
    m = m.resize((w, h), Image.LANCZOS)
    blur = m.filter(ImageFilter.GaussianBlur(inset * 0.45))
    a = np.maximum(np.asarray(m).astype(float), np.asarray(blur).astype(float) * 1.6)
    core = np.asarray(m).astype(float) / 255
    rgb = np.zeros((h, w, 3))
    rgb[...] = color
    rgb = rgb * (1 - core[..., None]) + np.array([210, 235, 255]) * core[..., None]
    return to_img(rgb, np.clip(a, 0, 255))


def joystick_base(size):
    """The stick's ring: a faint dark disc inside one thin light ring."""
    ss = 4
    S = size * ss
    m = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(m)
    d.ellipse([3 * ss, 3 * ss, S - 3 * ss, S - 3 * ss], fill=(0, 0, 0, 70), outline=(235, 238, 245, 170), width=3 * ss)
    return m.resize((size, size), Image.LANCZOS)


def joystick_knob(size):
    """The stick's thumb: a plain light disc with a soft edge."""
    ss = 4
    S = size * ss
    m = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(m)
    d.ellipse([2 * ss, 2 * ss, S - 2 * ss, S - 2 * ss], fill=(235, 238, 245, 150), outline=(255, 255, 255, 210), width=2 * ss)
    return m.resize((size, size), Image.LANCZOS)


def pickaxe(size=96):
    """A pickaxe: a wooden handle from bottom left to top right, an iron head curving across
    its top, both outlined dark."""
    ss = 4
    S = size * ss
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    # The head: an arc of a circle centred at the handle's foot, so it crosses the
    # handle's top at a right angle; the handle under it, ending in its middle.
    cx, cy = 0.12 * S, 0.90 * S
    r = 0.78 * S
    a = (0.20 * S, 0.82 * S)
    b = (cx + r * 0.7071, cy - r * 0.7071)
    for width, colour in ((0.13, (22, 20, 26, 255)), (0.07, (126, 84, 44, 255))):
        d.line([a, b], fill=colour, width=int(width * S))
    d.line([(a[0] - 0.012 * S, a[1] - 0.012 * S), (b[0] - 0.012 * S, b[1] - 0.012 * S)], fill=(176, 122, 68, 255), width=int(0.022 * S))
    box = [cx - r, cy - r, cx + r, cy + r]
    for width, colour in ((0.18, (22, 20, 26, 255)), (0.12, (120, 128, 142, 255))):
        d.arc(box, start=-84, end=-6, fill=colour, width=int(width * S))
    d.arc([cx - r + 0.04 * S, cy - r + 0.04 * S, cx + r - 0.04 * S, cy + r - 0.04 * S], start=-80, end=-10, fill=(214, 222, 236, 255), width=int(0.03 * S))
    return img.resize((size, size), Image.LANCZOS)


def glow_ring(size, color=(70, 150, 255)):
    """A round version of the selection glow, for round buttons."""
    ss = 2
    inset = size * 0.14
    m = Image.new("L", (size * ss, size * ss), 0)
    ImageDraw.Draw(m).ellipse([inset * ss, inset * ss, (size - inset) * ss - 1, (size - inset) * ss - 1], outline=255, width=3 * ss)
    m = m.resize((size, size), Image.LANCZOS)
    blur = m.filter(ImageFilter.GaussianBlur(inset * 0.45))
    a = np.maximum(np.asarray(m).astype(float), np.asarray(blur).astype(float) * 1.6)
    core = np.asarray(m).astype(float) / 255
    rgb = np.zeros((size, size, 3))
    rgb[...] = color
    rgb = rgb * (1 - core[..., None]) + np.array([210, 235, 255]) * core[..., None]
    return to_img(rgb, np.clip(a, 0, 255))


def main():
    packer = Packer(ATLAS)
    pieces = {}

    white = Image.new("RGBA", (8, 8), (255, 255, 255, 255))
    packer.add(("piece", "white"), white)
    pieces["white"] = [0, 0, 0, 0]

    # The ornate look (ui_ornate.py): margins are what stays unscaled at each edge.
    add = lambda name, img, margins: (packer.add(("piece", name), img), pieces.__setitem__(name, margins))
    add("button", O.square_button(112, 88, 14), [24, 22, 24, 22])
    add("bar", O.hex_button(360, 88, 30), [46, 0, 46, 0])
    add("glow_hex", O.glow_shape(360, 88, O.hex_poly(360, 88, 30), 24), [70, 0, 70, 0])
    add("panel", O.frame_panel(240, 240, 18, 60), [74, 74, 74, 74])
    add("plaque", O.plaque(560, 110, 46), [96, 0, 96, 0])
    add("glow", glow_frame(112, 112, 18, 4), [40, 40, 40, 40])
    add("field", O.inset_frame(96, 96, 4), [14, 14, 14, 14])
    add("slot", O.slot(96), [12, 12, 12, 12])
    add("rule", O.rule(256, 24), [26, 0, 26, 0])
    add("toggle_on", O.toggle(True), [0, 0, 0, 0])
    add("toggle_off", O.toggle(False), [0, 0, 0, 0])
    add("gem", O.gem(48), [0, 0, 0, 0])
    add("setting", O.setting(64), [0, 0, 0, 0])
    # The game's controls: round buttons, the stick, the attack button's pickaxe.
    add("round", O.round_button(128), [0, 0, 0, 0])
    add("glow_round", glow_ring(160), [0, 0, 0, 0])
    add("joy_base", joystick_base(200), [0, 0, 0, 0])
    add("joy_knob", joystick_knob(96), [0, 0, 0, 0])
    add("pickaxe", pickaxe(), [0, 0, 0, 0])
    # The title screen's logo and the pedestal its character stands on.
    add("logo", O.logo(LOGO), [0, 0, 0, 0])
    add("pedestal", O.pedestal(520, 180), [0, 0, 0, 0])

    fonts = {
        "serif": font_entry(packer, "serif", SERIF, 56, 6),
        "pixel": font_entry(packer, "pixel", PIXEL, 48, 6),
    }
    icons = icon_entry(packer, 72, 6)

    rects = packer.pack()
    for name, margins in pieces.items():
        pieces[name] = {"rect": rects[("piece", name)], "margins": margins}
    for key, f in fonts.items():
        for c, g in f["glyphs"].items():
            if (key, int(c)) in rects:
                g["rect"] = rects[(key, int(c))]
    for name, g in icons["glyphs"].items():
        g["rect"] = rects[("icon", name)]

    os.makedirs(OUT, exist_ok=True)
    packer.img.save(os.path.join(OUT, "atlas.png"), optimize=True)
    with open(os.path.join(OUT, "atlas.json"), "w") as fh:
        json.dump({"size": ATLAS, "pieces": pieces, "fonts": fonts, "icons": icons}, fh, separators=(",", ":"))
    print("wrote", OUT)


main()
