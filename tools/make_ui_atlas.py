"""Builds the menus' look: assets/ui/atlas.png and assets/ui/atlas.json.

One RGBA texture holds everything the menus draw with:
  * fonts as signed distance fields (SDF: crisp at any size, alpha = distance to the edge):
    Crimson Text 600 ("serif", most text) and Pixelify Sans 500 ("pixel", the main menu's
    buttons), printable ASCII;
  * icons from Material Icons, also as SDF, by name;
  * the frames, painted here: stone buttons with metal rims, angled bar buttons, panels, the
    title plaque with blue gems, the blue selection glow, text fields, switches, a gem;
  * the BlockForge logo (Cinzel 900 on a dark iron plate, with blue crystals);
  * a white square for plain rectangles.

The fonts come from Fontsource's npm packages (all SIL Open Font License; the licences are
copied to assets/ui/CREDITS.md):

    cd <dir> && for p in cinzel crimson-text pixelify-sans material-icons; do
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

SRC = sys.argv[1]
OUT = sys.argv[2]
ATLAS = 2048
rng = np.random.default_rng(7)


def font_file(pkg, name):
    return os.path.join(SRC, pkg, "package", "files", name)


SERIF = font_file("crimson-text", "crimson-text-latin-600-normal.woff")
PIXEL = font_file("pixelify-sans", "pixelify-sans-latin-500-normal.woff")
LOGO = font_file("cinzel", "cinzel-latin-900-normal.woff")
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

def vgrad(h, w, top, bottom):
    t = np.linspace(0, 1, h)[:, None, None]
    return np.broadcast_to(np.array(top, float) * (1 - t) + np.array(bottom, float) * t, (h, w, len(top))).copy()


def stone(h, w, top, bottom, amount=7.0):
    """A dark stone face: a gradient with soft blotches and fine grain."""
    c = vgrad(h, w, top, bottom)
    blot = ndimage.gaussian_filter(rng.normal(0, 1, (h, w)), 3.0)
    blot /= max(blot.std(), 1e-6)
    grain = rng.normal(0, 1, (h, w))
    c[..., :3] += (blot * amount * 0.6 + grain * amount * 0.35)[..., None]
    return c


def to_img(arr, alpha=None):
    a = np.clip(arr, 0, 255).astype(np.uint8)
    if a.shape[2] == 3:
        al = np.full(a.shape[:2], 255, np.uint8) if alpha is None else np.clip(alpha, 0, 255).astype(np.uint8)
        a = np.dstack([a, al])
    return Image.fromarray(a, "RGBA")


def shape_mask(w, h, poly, ss=4):
    m = Image.new("L", (w * ss, h * ss), 0)
    ImageDraw.Draw(m).polygon([(x * ss, y * ss) for x, y in poly], fill=255)
    return np.asarray(m.resize((w, h), Image.LANCZOS)).astype(float) / 255


def inset_mask(mask, px):
    """The mask shrunk by `px` pixels (soft)."""
    if px <= 0:
        return mask
    d = ndimage.distance_transform_edt(mask > 0.5)
    return np.clip(d - px + 0.5, 0, 1) * (mask > 0)


def framed(w, h, poly, rim=5, outline=3, body_top=(62, 60, 66), body_bottom=(40, 39, 44), rim_top=(170, 170, 178),
           rim_bottom=(70, 70, 78), alpha_body=255, rivets=None):
    """A shape with a black outline, a metal rim (lit from above) and a stone face."""
    outer = shape_mask(w, h, poly)
    m_rim = inset_mask(outer, outline)
    m_body = inset_mask(outer, outline + rim)
    img = np.zeros((h, w, 4))
    img[..., 3] = outer * 255
    rimc = vgrad(h, w, rim_top, rim_bottom)
    # Rim: brighter on its upper inner edge, darker at the bottom.
    img[..., :3] = rimc * m_rim[..., None]
    body = stone(h, w, body_top, body_bottom)
    # A faint light line along the top of the face, and shade along its bottom.
    dist_top = ndimage.distance_transform_edt(m_body > 0.5)
    lit = np.clip(1.6 - dist_top / 2.0, 0, 1) * (np.arange(h)[:, None] < h * 0.5)
    body[..., :3] += (lit * 22)[..., None]
    img[..., :3] = img[..., :3] * (1 - m_body[..., None]) + body * m_body[..., None]
    alpha = outer * 255
    if alpha_body < 255:
        alpha = alpha * (1 - m_body) + m_body * alpha_body
    img[..., 3] = alpha
    out = to_img(img[..., :3], img[..., 3])
    if rivets:
        d = ImageDraw.Draw(out)
        for (x, y) in rivets:
            d.rectangle([x - 2, y - 2, x + 2, y + 2], fill=(40, 40, 46, 255))
            d.rectangle([x - 1, y - 1, x + 1, y + 1], fill=(205, 205, 212, 255))
    return out


def gem(size):
    """A blue crystal: a diamond with lit and shaded facets and a soft glow."""
    s = size
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    c = s / 2
    r = s * 0.34
    glow = Image.new("L", (s, s), 0)
    ImageDraw.Draw(glow).polygon([(c, c - r * 1.15), (c + r * 1.15, c), (c, c + r * 1.15), (c - r * 1.15, c)], fill=200)
    glow = glow.filter(ImageFilter.GaussianBlur(s * 0.09))
    g = np.zeros((s, s, 4), np.uint8)
    g[..., 0], g[..., 1], g[..., 2] = 60, 150, 255
    g[..., 3] = np.asarray(glow) // 2
    img = Image.alpha_composite(img, Image.fromarray(g, "RGBA"))
    d = ImageDraw.Draw(img)
    top, right, bottom, left = (c, c - r), (c + r, c), (c, c + r), (c - r, c)
    d.polygon([top, right, bottom, left], fill=(10, 20, 40, 255))
    k = r * 0.12
    top2, right2, bottom2, left2 = (c, c - r + k), (c + r - k, c), (c, c + r - k), (c - r + k, c)
    mid = (c, c)
    d.polygon([top2, mid, left2], fill=(150, 215, 255, 255))
    d.polygon([top2, right2, mid], fill=(70, 150, 245, 255))
    d.polygon([left2, mid, bottom2], fill=(35, 95, 210, 255))
    d.polygon([mid, right2, bottom2], fill=(20, 60, 160, 255))
    d.polygon([(c - r * 0.18, c - r * 0.45), (c, c - r * 0.75), (c + r * 0.05, c - r * 0.5)], fill=(235, 250, 255, 255))
    return img


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


def logo():
    """BLOCKFORGE in Cinzel Black (large initials), carved stone with a dark outline, on an
    iron plate with points at both ends, and blue crystals above and below the middle."""
    W, H = 1400, 400
    font = ImageFont.truetype(LOGO, 168)
    text = "BlockForge"
    bbox = font.getbbox(text)
    tw, th = bbox[2] - bbox[0], bbox[3] - bbox[1]
    tx, ty = (W - tw) // 2 - bbox[0], (H - th) // 2 - bbox[1] + 6
    # Letters.
    m = Image.new("L", (W, H), 0)
    ImageDraw.Draw(m).text((tx, ty), text, font=font, fill=255)
    mask = np.asarray(m).astype(float) / 255
    inside = ndimage.distance_transform_edt(mask > 0.5)
    # Iron plate: an elongated hexagon behind the letters, with spiky ends.
    cy = H // 2 + 4
    plate = [(50, cy), (140, cy - 84), (W - 140, cy - 84), (W - 50, cy), (W - 140, cy + 84), (140, cy + 84)]
    spikes_l = [(4, cy), (110, cy - 26), (110, cy + 26)]
    spikes_r = [(W - 4, cy), (W - 110, cy - 26), (W - 110, cy + 26)]
    base = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    for poly in (spikes_l, spikes_r):
        base = Image.alpha_composite(base, framed(W, H, poly, rim=4, outline=3, body_top=(70, 72, 82), body_bottom=(30, 30, 36)))
    plate_img = framed(W, H, plate, rim=7, outline=4, body_top=(54, 53, 60), body_bottom=(26, 26, 31), rim_top=(150, 152, 165), rim_bottom=(52, 52, 60))
    base = Image.alpha_composite(base, plate_img)
    # Shadow of the letters on the plate.
    shadow = Image.fromarray((ndimage.gaussian_filter(np.roll(mask, (10, 6), (0, 1)), 6) * 200).astype(np.uint8), "L")
    sh = Image.new("RGBA", (W, H), (0, 0, 0, 255))
    sh.putalpha(shadow)
    base = Image.alpha_composite(base, sh)
    # Outline: the letters grown by 9 px, near black.
    outside = ndimage.distance_transform_edt(mask < 0.5)
    outline_a = np.clip(10.5 - outside, 0, 1)
    ol = np.zeros((H, W, 4))
    ol[..., :3] = (22, 18, 16)
    ol[..., 3] = outline_a * 255
    base = Image.alpha_composite(base, to_img(ol[..., :3], ol[..., 3]))
    # Carved stone face: warm light at the top, darker below, a lit bevel along the upper
    # edges and shade along the lower ones (from the gradient of the distance to the edge).
    face = stone(H, W, (236, 222, 198), (150, 132, 108), amount=10)
    gy, gx = np.gradient(ndimage.gaussian_filter(np.minimum(inside, 7.0), 1.2))
    light = (-gy * 0.8 - gx * 0.35)
    bevel = np.clip(light * 30, -60, 60) * (inside < 7.5)
    face[..., :3] += bevel[..., None]
    cracks = ndimage.gaussian_filter(rng.normal(0, 1, (H, W)), 1.0) < -1.6
    face[cracks] *= 0.72
    fa = np.clip(mask, 0, 1) * 255
    base = Image.alpha_composite(base, to_img(face[..., :3], fa))
    # Crystals above and below the middle, and smaller ones at the plate's ends.
    for (x, y, s) in [(W // 2, cy - 108, 128), (W // 2, cy + 104, 96), (92, cy, 60), (W - 92, cy, 60)]:
        g = gem(s)
        base.alpha_composite(g, (int(x - s / 2), int(y - s / 2)))
    return base


def round_button(size):
    """A round stone button with a metal rim, lit from above."""
    ss = 4
    S = size * ss
    def disc(inset):
        m = Image.new("L", (S, S), 0)
        ImageDraw.Draw(m).ellipse([inset * ss, inset * ss, S - 1 - inset * ss, S - 1 - inset * ss], fill=255)
        return np.asarray(m.resize((size, size), Image.LANCZOS)).astype(float) / 255
    outer, m_rim, m_body = disc(1), disc(4), disc(10)
    rim = vgrad(size, size, (182, 182, 192), (66, 66, 74))
    body = stone(size, size, (66, 64, 70), (36, 35, 40))
    img = np.zeros((size, size, 3))
    img[...] = (12, 12, 14)
    img = img * (1 - m_rim[..., None]) + rim * m_rim[..., None]
    # A soft highlight across the upper half of the face.
    yy, xx = np.mgrid[0:size, 0:size]
    hl = np.clip(1.0 - np.hypot(xx - size * 0.5, (yy - size * 0.32) * 1.6) / (size * 0.42), 0, 1) ** 2
    body = body + (hl * 26)[..., None]
    img = img * (1 - m_body[..., None]) + body * m_body[..., None]
    return to_img(img, outer * 255)


def joystick_base(size):
    """The stick's ring: a dark see-through disc in a thin metal ring, with four chevrons."""
    ss = 4
    S = size * ss
    m = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(m)
    d.ellipse([2 * ss, 2 * ss, S - 2 * ss, S - 2 * ss], fill=(10, 12, 18, 110), outline=(20, 20, 24, 200), width=4 * ss)
    d.ellipse([5 * ss, 5 * ss, S - 5 * ss, S - 5 * ss], outline=(176, 178, 190, 210), width=3 * ss)
    d.ellipse([S * 0.22, S * 0.22, S * 0.78, S * 0.78], outline=(176, 178, 190, 60), width=2 * ss)
    c = S / 2
    for ang in (0, 90, 180, 270):
        a = np.radians(ang)
        r1, r2, w = S * 0.40, S * 0.33, S * 0.07
        tip = (c + np.cos(a) * r1, c + np.sin(a) * r1)
        b1 = (c + np.cos(a) * r2 - np.sin(a) * w, c + np.sin(a) * r2 + np.cos(a) * w)
        b2 = (c + np.cos(a) * r2 + np.sin(a) * w, c + np.sin(a) * r2 - np.cos(a) * w)
        d.polygon([tip, b1, b2], fill=(150, 200, 255, 170))
    return m.resize((size, size), Image.LANCZOS)


def joystick_knob(size):
    """The stick's thumb: a small round stone with a blue gem in the middle."""
    k = round_button(size)
    g = gem(int(size * 0.62))
    k.alpha_composite(g, ((size - g.width) // 2, (size - g.height) // 2))
    return k


def slot(size):
    """A hotbar slot: a dark see-through square in a thin metal rim."""
    return framed(size, size, [(0, 0), (size, 0), (size, size), (0, size)], rim=5, outline=2, body_top=(30, 30, 36),
                  body_bottom=(18, 18, 22), rim_top=(196, 196, 206), rim_bottom=(96, 96, 106), alpha_body=150)


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


def toggle(on):
    w, h = 132, 52
    img = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rounded_rectangle([1, 1, w - 2, h - 2], radius=10, fill=(12, 12, 14, 255), outline=(110, 110, 118, 255), width=2)
    if on:
        d.rounded_rectangle([4, 4, w - 5, h - 5], radius=8, fill=(26, 90, 210, 255))
        d.rounded_rectangle([4, 4, w - 5, h // 2], radius=8, fill=(44, 116, 236, 255))
        kx = w - 48
    else:
        kx = 6
    # Knob: a small stone block with a metal rim.
    d.rounded_rectangle([kx, 6, kx + 40, h - 7], radius=6, fill=(150, 150, 158, 255), outline=(40, 40, 46, 255), width=2)
    d.rounded_rectangle([kx + 4, 9, kx + 36, h // 2], radius=4, fill=(200, 200, 208, 255))
    return img


def main():
    packer = Packer(ATLAS)
    pieces = {}

    white = Image.new("RGBA", (8, 8), (255, 255, 255, 255))
    packer.add(("piece", "white"), white)
    pieces["white"] = [0, 0, 0, 0]

    # Stone button: rim, rivets in the corners.
    w, h = 128, 96
    packer.add(("piece", "button"), framed(w, h, [(0, 0), (w, 0), (w, h), (0, h)], rivets=[(10, 10), (w - 10, 10), (10, h - 10), (w - 10, h - 10)]))
    pieces["button"] = [30, 26, 30, 26]
    # Pressed / disabled faces come from tinting it.

    # Bar button: corners cut at 45 degrees (the main menu's Quit, the bottom row), 3-slice.
    w, h, c = 168, 96, 26
    poly = [(c, 0), (w - c, 0), (w, c), (w, h - c), (w - c, h), (c, h), (0, h - c), (0, c)]
    packer.add(("piece", "bar"), framed(w, h, poly))
    pieces["bar"] = [44, 0, 44, 0]

    # Panel: translucent stone in a metal frame with corner brackets.
    w = h = 160
    p = framed(w, h, [(0, 0), (w, 0), (w, h), (0, h)], rim=6, outline=3, body_top=(30, 30, 36), body_bottom=(20, 20, 24), alpha_body=205,
               rivets=[(14, 14), (w - 14, 14), (14, h - 14), (w - 14, h - 14)])
    d = ImageDraw.Draw(p)
    for (x, y, sx, sy) in [(4, 4, 1, 1), (w - 5, 4, -1, 1), (4, h - 5, 1, -1), (w - 5, h - 5, -1, -1)]:
        d.line([(x, y), (x + 26 * sx, y)], fill=(185, 185, 195, 255), width=4)
        d.line([(x, y), (x, y + 26 * sy)], fill=(185, 185, 195, 255), width=4)
    packer.add(("piece", "panel"), p)
    pieces["panel"] = [44, 44, 44, 44]

    # Title plaque: pointed ends with blue gems near the tips, 3-slice.
    w, h = 300, 100
    cy = h / 2
    poly = [(0, cy), (44, 8), (w - 44, 8), (w, cy), (w - 44, h - 8), (44, h - 8)]
    pl = framed(w, h, poly, rim=5, outline=3, body_top=(58, 57, 63), body_bottom=(30, 29, 34))
    for x in (30, w - 30):
        pl.alpha_composite(gem(30), (int(x - 15), int(cy - 15)))
    packer.add(("piece", "plaque"), pl)
    pieces["plaque"] = [96, 0, 96, 0]

    # Selection glow.
    packer.add(("piece", "glow"), glow_frame(112, 112, 18, 6))
    pieces["glow"] = [40, 40, 40, 40]

    # Text field: near-black with a thin metal rim.
    w = h = 64
    f = framed(w, h, [(0, 0), (w, 0), (w, h), (0, h)], rim=2, outline=2, body_top=(10, 10, 12), body_bottom=(6, 6, 8), rim_top=(130, 130, 140), rim_bottom=(80, 80, 88))
    packer.add(("piece", "field"), f)
    pieces["field"] = [12, 12, 12, 12]

    # Switches and a gem.
    packer.add(("piece", "toggle_on"), toggle(True))
    pieces["toggle_on"] = [0, 0, 0, 0]
    packer.add(("piece", "toggle_off"), toggle(False))
    pieces["toggle_off"] = [0, 0, 0, 0]
    packer.add(("piece", "gem"), gem(48))
    pieces["gem"] = [0, 0, 0, 0]

    # The game's controls: round buttons, the stick, hotbar slots, the attack button's pickaxe.
    packer.add(("piece", "round"), round_button(128))
    pieces["round"] = [0, 0, 0, 0]
    packer.add(("piece", "glow_round"), glow_ring(160))
    pieces["glow_round"] = [0, 0, 0, 0]
    packer.add(("piece", "joy_base"), joystick_base(200))
    pieces["joy_base"] = [0, 0, 0, 0]
    packer.add(("piece", "joy_knob"), joystick_knob(96))
    pieces["joy_knob"] = [0, 0, 0, 0]
    packer.add(("piece", "slot"), slot(72))
    pieces["slot"] = [10, 10, 10, 10]
    packer.add(("piece", "pickaxe"), pickaxe())
    pieces["pickaxe"] = [0, 0, 0, 0]

    packer.add(("piece", "logo"), logo())
    pieces["logo"] = [0, 0, 0, 0]

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
