"""The menus' ornate look, painted procedurally: dark weathered stone set in iron frames with
bronze edges, blue crystal gems at the corners and tips, and a blue glow for the picked item.

Used by make_ui_atlas.py. Every function returns a PIL RGBA image.
"""

import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont
from scipy import ndimage

rng = np.random.default_rng(11)

# Palette (sRGB).
OUTLINE = (12, 10, 9)
IRON_HI = (150, 142, 130)
IRON = (68, 63, 57)
IRON_LO = (22, 20, 18)
BRONZE_HI = (204, 170, 116)
BRONZE = (134, 104, 68)
BRONZE_LO = (70, 52, 34)
STONE_TOP = (46, 41, 36)
STONE_BOTTOM = (25, 22, 19)
BLUE = (64, 156, 255)
BLUE_CORE = (200, 232, 255)


def to_img(rgb, alpha):
    a = np.dstack([np.clip(rgb, 0, 255), np.clip(alpha, 0, 255)]).astype(np.uint8)
    return Image.fromarray(a, "RGBA")


def poly_mask(w, h, poly, ss=4):
    m = Image.new("L", (w * ss, h * ss), 0)
    ImageDraw.Draw(m).polygon([(x * ss, y * ss) for x, y in poly], fill=255)
    return np.asarray(m.resize((w, h), Image.LANCZOS)).astype(float) / 255


def depth(mask):
    """Distance from the shape's edge, inside it (pixels); the picture's border counts as
    an edge."""
    return ndimage.distance_transform_edt(np.pad(mask > 0.5, 1))[1:-1, 1:-1]


def band(d, a, b):
    """1 where a <= depth < b, softly."""
    return np.clip(d - a + 0.5, 0, 1) * np.clip(b - d + 0.5, 0, 1)


def stone_face(h, w, top=STONE_TOP, bottom=STONE_BOTTOM, rough=1.0):
    """Weathered dark stone: a gradient, large blotches, grain, faint cracks and scratches."""
    t = np.linspace(0, 1, h)[:, None, None]
    c = np.array(top, float) * (1 - t) + np.array(bottom, float) * t
    c = np.broadcast_to(c, (h, w, 3)).copy()
    big = ndimage.gaussian_filter(rng.normal(0, 1, (h, w)), 7.0)
    big /= max(big.std(), 1e-6)
    mid = ndimage.gaussian_filter(rng.normal(0, 1, (h, w)), 2.0)
    mid /= max(mid.std(), 1e-6)
    grain = rng.normal(0, 1, (h, w))
    c += ((big * 3.5 + mid * 2.8 + grain * 3.0) * rough)[..., None]
    # Cracks: thin dark lines along ridges of smooth noise.
    n = ndimage.gaussian_filter(rng.normal(0, 1, (h, w)), 4.0)
    n /= max(n.std(), 1e-6)
    cracks = np.exp(-(n / 0.05) ** 2) * (ndimage.gaussian_filter(rng.normal(0, 1, (h, w)), 10) > 0.3)
    c *= (1 - 0.35 * cracks * rough)[..., None]
    return c


def lit(mask, width, light=(-0.55, -0.8)):
    """How lit a raised rim `width` pixels wide round the shape is (+ facing the light, which
    comes from the top left): its outer slope faces out, its inner slope faces in."""
    d0 = depth(mask)
    d = np.minimum(d0, width)
    gy, gx = np.gradient(ndimage.gaussian_filter(d, 1.0))
    toward = gx * light[0] + gy * light[1]  # the inward direction against the light
    outer = d0 < width * 0.5
    return np.where(outer, -toward, toward) * (d0 < width)


def metal(h, w, mask, lo, mid, hi, shine):
    """A metal band: `mid` lit by `shine` (-1..1) toward `hi` or `lo`."""
    s = np.clip(shine, -1, 1)[..., None]
    c = np.array(mid, float) + np.maximum(s, 0) * (np.array(hi, float) - mid) + np.maximum(-s, 0) * (np.array(lo, float) - mid)
    grain = ndimage.gaussian_filter(rng.normal(0, 1, (h, w)), 0.8)[..., None] * 6
    return c + grain


def gem(size, glow=True):
    """A blue crystal: a diamond with lit and shaded facets and a soft glow."""
    s = size
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    c = s / 2
    r = s * (0.34 if glow else 0.46)
    if glow:
        g = Image.new("L", (s, s), 0)
        ImageDraw.Draw(g).polygon([(c, c - r * 1.2), (c + r * 1.2, c), (c, c + r * 1.2), (c - r * 1.2, c)], fill=210)
        g = g.filter(ImageFilter.GaussianBlur(s * 0.09))
        layer = np.zeros((s, s, 4), np.uint8)
        layer[..., 0], layer[..., 1], layer[..., 2] = 60, 150, 255
        layer[..., 3] = np.asarray(g) // 2
        img = Image.alpha_composite(img, Image.fromarray(layer, "RGBA"))
    ss = 4
    big = Image.new("RGBA", (s * ss, s * ss), (0, 0, 0, 0))
    d = ImageDraw.Draw(big)
    C, R = c * ss, r * ss
    top, right, bottom, left = (C, C - R), (C + R, C), (C, C + R), (C - R, C)
    d.polygon([top, right, bottom, left], fill=(8, 14, 30, 255))
    k = R * 0.14
    t2, r2, b2, l2 = (C, C - R + k), (C + R - k, C), (C, C + R - k), (C - R + k, C)
    m = (C, C)
    d.polygon([t2, m, l2], fill=(150, 215, 255, 255))
    d.polygon([t2, r2, m], fill=(70, 150, 245, 255))
    d.polygon([l2, m, b2], fill=(35, 95, 210, 255))
    d.polygon([m, r2, b2], fill=(18, 56, 150, 255))
    d.polygon([(C - R * 0.2, C - R * 0.42), (C, C - R * 0.74), (C + R * 0.06, C - R * 0.48)], fill=(238, 250, 255, 255))
    img.alpha_composite(big.resize((s, s), Image.LANCZOS))
    return img


def setting(size):
    """A gem in a small bronze diamond setting (for corners and tips)."""
    s = size
    ss = 4
    big = Image.new("RGBA", (s * ss, s * ss), (0, 0, 0, 0))
    d = ImageDraw.Draw(big)
    C = s * ss / 2
    R = C * 0.96
    d.polygon([(C, C - R), (C + R, C), (C, C + R), (C - R, C)], fill=OUTLINE + (255,))
    R2 = R * 0.86
    d.polygon([(C, C - R2), (C + R2, C), (C, C + R2), (C - R2, C)], fill=BRONZE + (255,))
    d.polygon([(C, C - R2), (C + R2, C), (C, C), (C - R2, C)], fill=BRONZE_HI + (255,))
    img = big.resize((s, s), Image.LANCZOS)
    g = gem(int(s * 0.78), glow=False)
    img.alpha_composite(g, ((s - g.width) // 2, (s - g.height) // 2))
    return img


def ornate(w, h, poly, frame=9, body_alpha=255, rough=1.0, stone_top=STONE_TOP, stone_bottom=STONE_BOTTOM):
    """A shape with a black outline, an iron frame lit from the top left with a bronze edge on
    each side of it, and a weathered stone face."""
    outer = poly_mask(w, h, poly)
    d = depth(outer)
    shine = lit(outer, frame + 2) * 2.2
    rgb = metal(h, w, outer, IRON_LO, IRON, IRON_HI, shine)
    # Bronze edges: just inside the outline and on the inner rim of the frame.
    br1 = band(d, 1.5, 3.5)
    br2 = band(d, frame - 2.0, frame)
    bronze = metal(h, w, outer, BRONZE_LO, BRONZE, BRONZE_HI, shine * 1.2 + 0.2)
    rgb = rgb * (1 - br1[..., None]) + bronze * br1[..., None]
    rgb = rgb * (1 - br2[..., None]) + bronze * br2[..., None]
    # A dark groove between the frame and the face, which is sunk a little behind it.
    groove = band(d, frame, frame + 1.5)
    rgb = rgb * (1 - groove[..., None]) + np.array(OUTLINE, float) * groove[..., None]
    face_m = np.clip(d - frame - 1.0, 0, 1)
    face = stone_face(h, w, stone_top, stone_bottom, rough)
    sunk = np.clip(1 - (d - frame) / 5.0, 0, 1) * 0.45
    face *= (1 - sunk)[..., None]
    rgb = rgb * (1 - face_m[..., None]) + face * face_m[..., None]
    # Outline.
    ol = np.clip(1.6 - d, 0, 1)
    rgb = rgb * (1 - ol[..., None]) + np.array(OUTLINE, float) * ol[..., None]
    alpha = outer * 255
    if body_alpha < 255:
        alpha = alpha * (1 - face_m) + face_m * body_alpha * outer
    return to_img(rgb, alpha)


def hex_poly(w, h, tip):
    cy = h / 2
    return [(0, cy), (tip, 0), (w - tip, 0), (w, cy), (w - tip, h), (tip, h)]


def hex_button(w=360, h=88, tip=30):
    """The menus' button: a long stone plate with pointed ends in an iron and bronze frame,
    a gem at each tip."""
    img = ornate(w, h, hex_poly(w, h, tip), frame=10)
    s = 26
    for x in (tip * 0.62, w - tip * 0.62):
        img.alpha_composite(setting(s), (int(x - s / 2), int(h / 2 - s / 2)))
    return img


def glow_shape(w, h, poly, pad, color=BLUE):
    """A bright blue line along the shape's outline with a soft glow outside and in, on a
    canvas `pad` larger each side."""
    W, H = w + 2 * pad, h + 2 * pad
    ss = 2
    m = Image.new("L", (W * ss, H * ss), 0)
    ImageDraw.Draw(m).polygon([((x + pad) * ss, (y + pad) * ss) for x, y in poly], outline=255, width=4 * ss)
    m = m.resize((W, H), Image.LANCZOS)
    blur = np.asarray(m.filter(ImageFilter.GaussianBlur(pad * 0.42))).astype(float)
    core = np.asarray(m).astype(float)
    a = np.maximum(core, blur * 2.2)
    rgb = np.zeros((H, W, 3))
    rgb[...] = color
    k = (core / 255)[..., None]
    rgb = rgb * (1 - k) + np.array(BLUE_CORE, float) * k
    return to_img(rgb, np.clip(a, 0, 255))


def octagon(w, h, c):
    return [(c, 0), (w - c, 0), (w, c), (w, h - c), (w - c, h), (c, h), (0, h - c), (0, c)]


def square_button(w=112, h=88, c=14):
    """A small ornate button with clipped corners (keys, the game's buttons, icon buttons)."""
    img = ornate(w, h, octagon(w, h, c), frame=7)
    d = ImageDraw.Draw(img)
    for (x, y) in [(c + 4, 9), (w - c - 5, 9), (c + 4, h - 10), (w - c - 5, h - 10)]:
        d.ellipse([x - 2, y - 2, x + 2, y + 2], fill=BRONZE_HI + (255,), outline=OUTLINE + (255,))
    return img


def frame_panel(w=240, h=240, frame=18, corner=60):
    """A big panel: weathered stone in a heavy iron frame with bronze edges, and a bronze
    bracket with a gem at each corner."""
    pad = 10
    W, H = w + 2 * pad, h + 2 * pad
    img = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    # (nearly opaque: blending is in linear light, so a little see-through shows a lot)
    inner = ornate(w, h, [(0, 0), (w, 0), (w, h), (0, h)], frame=frame, body_alpha=252, rough=1.2)
    img.alpha_composite(inner, (pad, pad))
    # Corner brackets: an L of bronze over the frame's corner, spiked outward, with a gem.
    for sx, sy in [(0, 0), (1, 0), (0, 1), (1, 1)]:
        b = corner_bracket(corner + pad)
        if sx:
            b = b.transpose(Image.FLIP_LEFT_RIGHT)
        if sy:
            b = b.transpose(Image.FLIP_TOP_BOTTOM)
        img.alpha_composite(b, (W - b.width if sx else 0, H - b.height if sy else 0))
    return img


def corner_bracket(s):
    """The top-left corner ornament: two bronze arms along the edges ending in points, a
    spike out of the corner, and a gem in a setting."""
    ss = 4
    S = s * ss
    big = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(big)
    t = S * 0.16
    o = S * 0.08
    arm = S * 0.92
    for poly in (
        [(o, o), (arm, o), (arm + t * 0.8, o + t * 0.5), (arm, o + t), (o + t, o + t), (o + t, arm), (o + t * 0.5, arm + t * 0.8), (o, arm)],
        [(0, 0), (o + t * 1.6, o + t * 0.6), (o + t * 0.6, o + t * 1.6)],
    ):
        d.polygon(poly, fill=OUTLINE + (255,))
        inner = [(x + (S * 0.012 if x > S * 0.05 else S * 0.02), y + (S * 0.012 if y > S * 0.05 else S * 0.02)) for x, y in poly]
        d.polygon(inner, fill=BRONZE + (255,))
    d.line([(o + t * 0.4, o + t * 0.35), (arm * 0.95, o + t * 0.35)], fill=BRONZE_HI + (255,), width=int(S * 0.025))
    d.line([(o + t * 0.35, o + t * 0.4), (o + t * 0.35, arm * 0.95)], fill=BRONZE_HI + (255,), width=int(S * 0.025))
    img = big.resize((s, s), Image.LANCZOS)
    g = setting(int(s * 0.42))
    img.alpha_composite(g, (int(s * 0.1), int(s * 0.1)))
    return img


def inset_frame(w=96, h=96, frame=4, body=(22, 20, 18), alpha=246):
    """A sunken box: a thin iron and bronze rim round a dark face (list rows, pictures,
    value boxes, text fields)."""
    return ornate(w, h, [(0, 0), (w, 0), (w, h), (0, h)], frame=frame, body_alpha=alpha, rough=0.6,
                  stone_top=tuple(int(c * 1.25) for c in body), stone_bottom=body)


def slot(size=96):
    """An inventory slot: a dark recess with a thin bronze lip, shadowed at the top left."""
    s = size
    m = np.ones((s, s))
    m[:1, :] = m[-1:, :] = 0
    m[:, :1] = m[:, -1:] = 0
    d = depth(m)
    face = stone_face(s, s, (34, 31, 28), (24, 22, 20), 0.6)
    yy, xx = np.mgrid[0:s, 0:s]
    shade = np.clip(1 - np.minimum(yy, xx) / 9.0, 0, 1) * 0.5
    light = np.clip(1 - np.minimum(s - 1 - yy, s - 1 - xx) / 6.0, 0, 1) * 0.25
    face = face * (1 - shade)[..., None] + np.array([90, 76, 56]) * light[..., None]
    rim = band(d, 0, 3.2)
    bronze = np.array(BRONZE, float) * 0.8
    rgb = face * (1 - rim[..., None]) + bronze * rim[..., None]
    ol = band(d, 0, 1.4)
    rgb = rgb * (1 - ol[..., None]) + np.array(OUTLINE, float) * ol[..., None]
    return to_img(rgb, np.full((s, s), 236))


def rule(w=256, h=24):
    """A thin bronze rule with a small diamond at each end (dividers, headings)."""
    ss = 4
    big = Image.new("RGBA", (w * ss, h * ss), (0, 0, 0, 0))
    d = ImageDraw.Draw(big)
    cy = h * ss / 2
    d.line([(10 * ss, cy), (w * ss - 10 * ss, cy)], fill=OUTLINE + (220,), width=4 * ss // 2 + 2)
    d.line([(10 * ss, cy - ss * 0.5), (w * ss - 10 * ss, cy - ss * 0.5)], fill=BRONZE + (255,), width=ss)
    for x in (7 * ss, w * ss - 7 * ss):
        r = 6 * ss
        d.polygon([(x, cy - r), (x + r, cy), (x, cy + r), (x - r, cy)], fill=OUTLINE + (255,))
        r2 = 4.5 * ss
        d.polygon([(x, cy - r2), (x + r2, cy), (x, cy + r2), (x - r2, cy)], fill=BRONZE_HI + (255,))
    return big.resize((w, h), Image.LANCZOS)


def plaque(w=560, h=110, tip=46):
    """The title plaque: a wide pointed plate with gems at the tips (the gem over its middle
    is drawn on its own, so the plate can stretch)."""
    img = ornate(w, h, hex_poly(w, h, tip), frame=11)
    for x in (tip * 0.6, w - tip * 0.6):
        s = 32
        img.alpha_composite(setting(s), (int(x - s / 2), int(h / 2 - s / 2)))
    return img


def toggle(on, w=120, h=56):
    """A switch: a blue pill with its knob at the right (on), a dark one with it at the left."""
    ss = 4
    big = Image.new("RGBA", (w * ss, h * ss), (0, 0, 0, 0))
    d = ImageDraw.Draw(big)
    W, H = w * ss, h * ss
    r = H / 2
    d.rounded_rectangle([0, 0, W - 1, H - 1], radius=r, fill=OUTLINE + (255,))
    d.rounded_rectangle([3 * ss, 3 * ss, W - 3 * ss, H - 3 * ss], radius=r - 3 * ss, fill=BRONZE_LO + (255,))
    inner = [6 * ss, 6 * ss, W - 6 * ss, H - 6 * ss]
    if on:
        d.rounded_rectangle(inner, radius=r - 6 * ss, fill=(28, 96, 214, 255))
        d.rounded_rectangle([inner[0], inner[1], inner[2], H * 0.5], radius=r - 6 * ss, fill=(52, 128, 240, 255))
        kx = W - H
    else:
        d.rounded_rectangle(inner, radius=r - 6 * ss, fill=(26, 24, 22, 255))
        kx = 0
    k = H * 0.16
    d.ellipse([kx + k, k, kx + H - k, H - k], fill=OUTLINE + (255,))
    k2 = k + 2 * ss
    col = (236, 240, 248) if on else (150, 146, 140)
    d.ellipse([kx + k2, k2, kx + H - k2, H - k2], fill=col + (255,))
    d.ellipse([kx + k2 + H * 0.1, k2 + H * 0.06, kx + H - k2 - H * 0.18, H * 0.48], fill=tuple(min(255, c + 18) for c in col) + (255,))
    return big.resize((w, h), Image.LANCZOS)


def round_button(size=128):
    """A round stone button in a bronze-edged iron ring."""
    ss = 4
    S = size * ss
    def disc(inset):
        m = Image.new("L", (S, S), 0)
        ImageDraw.Draw(m).ellipse([inset * ss, inset * ss, S - 1 - inset * ss, S - 1 - inset * ss], fill=255)
        return np.asarray(m.resize((size, size), Image.LANCZOS)).astype(float) / 255
    outer = disc(1)
    d = depth(outer)
    shine = lit(outer, 14) * 2.2
    rgb = metal(size, size, outer, IRON_LO, IRON, IRON_HI, shine)
    bronze = metal(size, size, outer, BRONZE_LO, BRONZE, BRONZE_HI, shine * 1.2 + 0.2)
    for a, b in ((1.5, 3.5), (10.5, 12.5)):
        k = band(d, a, b)[..., None]
        rgb = rgb * (1 - k) + bronze * k
    face = stone_face(size, size, (58, 52, 47), (30, 27, 24), 0.8)
    yy, xx = np.mgrid[0:size, 0:size]
    hl = np.clip(1.0 - np.hypot(xx - size * 0.5, (yy - size * 0.34) * 1.5) / (size * 0.4), 0, 1) ** 2
    face += (hl * 22)[..., None]
    fm = np.clip(d - 12.5, 0, 1)[..., None]
    rgb = rgb * (1 - fm) + face * fm
    ol = np.clip(1.6 - d, 0, 1)[..., None]
    rgb = rgb * (1 - ol) + np.array(OUTLINE, float) * ol
    return to_img(rgb, outer * 255)


def pedestal(w=520, h=180):
    """A stone pedestal seen from a little above: a top face, a front face with a bronze band
    and a gem in a setting."""
    ss = 2
    W, H = w * ss, h * ss
    img = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    top_h = H * 0.42
    inset = W * 0.07
    # Top face (a flattened octagon) and the front faces below it.
    top = [(inset, top_h * 0.5), (inset * 2.2, 0), (W - inset * 2.2, 0), (W - inset, top_h * 0.5), (W - inset * 2.2, top_h), (inset * 2.2, top_h)]
    front = [(inset, top_h * 0.5), (inset * 2.2, top_h), (W - inset * 2.2, top_h), (W - inset, top_h * 0.5), (W - inset, H * 0.8), (W - inset * 2.2, H), (inset * 2.2, H), (inset, H * 0.8)]
    d.polygon(front, fill=OUTLINE + (255,))
    d.polygon(top, fill=OUTLINE + (255,))
    img_np = np.asarray(img).astype(float)
    m_front = poly_mask(W, H, front, ss=1)
    m_top = poly_mask(W, H, top, ss=1)
    face_f = stone_face(H, W, (52, 48, 44), (22, 20, 18), 1.4)
    face_t = stone_face(H, W, (84, 78, 72), (62, 57, 52), 1.2)
    # Block seams on the front.
    for x in np.linspace(inset * 2.2, W - inset * 2.2, 5)[1:-1]:
        face_f[:, int(x) - 2:int(x) + 2] *= 0.55
    face_f[int(H * 0.62):int(H * 0.62) + 3, :] *= 0.55
    rgb = img_np[..., :3]
    rgb = rgb * (1 - m_front[..., None]) + face_f * m_front[..., None]
    shrink_t = np.clip(depth(m_top) - 3, 0, 1)[..., None]
    rgb = rgb * (1 - shrink_t) + face_t * shrink_t
    alpha = np.maximum(m_front, m_top) * 255
    out = to_img(rgb, alpha)
    # A bronze band along the top of the front, and the gem.
    d = ImageDraw.Draw(out)
    d.line([(inset * 2.2, top_h + 8), (W - inset * 2.2, top_h + 8)], fill=BRONZE + (255,), width=6)
    out = out.resize((w, h), Image.LANCZOS)
    s = int(h * 0.36)
    out.alpha_composite(setting(s), (int(w / 2 - s / 2), int(h * 0.5)))
    return out


def logo(font_path, w=1200, h=340):
    """BlockForge in a medieval display face: steel letters with a warm top light, a dark
    outline and bevel, flourished rules out to each side, and a gem under the middle."""
    img = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    font = ImageFont.truetype(font_path, 196)
    text = "BlockForge"
    bb = font.getbbox(text)
    tw, th = bb[2] - bb[0], bb[3] - bb[1]
    tx, ty = (w - tw) // 2 - bb[0], (h - th) // 2 - bb[1] - 18
    m = Image.new("L", (w, h), 0)
    ImageDraw.Draw(m).text((tx, ty), text, font=font, fill=255)
    mask = np.asarray(m).astype(float) / 255
    inside = depth(mask)
    outside = ndimage.distance_transform_edt(mask < 0.5)
    cy = ty + bb[1] + th * 0.62
    # Flourished rules left and right of the word, and under it.
    rules = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    d = ImageDraw.Draw(rules)
    left_end, right_start = (w - tw) // 2 - 16, (w + tw) // 2 + 16
    for yoff, wid in ((-10, 3), (0, 5), (10, 3)):
        d.line([(30, cy + yoff), (left_end, cy + yoff)], fill=OUTLINE + (255,), width=wid + 4)
        d.line([(right_start, cy + yoff), (w - 30, cy + yoff)], fill=OUTLINE + (255,), width=wid + 4)
    for yoff, wid in ((-10, 2), (0, 3), (10, 2)):
        d.line([(34, cy + yoff), (left_end, cy + yoff)], fill=(170, 160, 146, 255), width=wid)
        d.line([(right_start, cy + yoff), (w - 34, cy + yoff)], fill=(170, 160, 146, 255), width=wid)
    for x in (30, w - 30):
        r = 12
        d.polygon([(x, cy - r), (x + r, cy), (x, cy + r), (x - r, cy)], fill=OUTLINE + (255,))
        d.polygon([(x, cy - 8), (x + 8, cy), (x, cy + 8), (x - 8, cy)], fill=BRONZE_HI + (255,))
    base_y = ty + bb[3] + 22
    d.line([(w * 0.28, base_y), (w * 0.72, base_y)], fill=OUTLINE + (255,), width=7)
    d.line([(w * 0.28, base_y), (w * 0.72, base_y)], fill=(170, 160, 146, 255), width=3)
    img.alpha_composite(rules)
    # Shadow and outline.
    shadow = ndimage.gaussian_filter(np.roll(mask, (8, 5), (0, 1)), 5) * 210
    img.alpha_composite(to_img(np.zeros((h, w, 3)), shadow))
    ol = np.clip(9.5 - outside, 0, 1) * 255
    img.alpha_composite(to_img(np.broadcast_to(np.array(OUTLINE, float), (h, w, 3)), ol))
    # Steel face: light above, a warm glint, darker below; bevelled edges.
    t = np.linspace(0, 1, h)[:, None, None]
    steel = np.array((236, 230, 220), float) * (1 - t) + np.array((120, 112, 104), float) * t
    steel = np.broadcast_to(steel, (h, w, 3)).copy()
    gy, gx = np.gradient(ndimage.gaussian_filter(np.minimum(inside, 6.0), 1.2))
    bevel = np.clip((-gy * 0.8 - gx * 0.4) * 34, -70, 70) * (inside < 6.5)
    steel += bevel[..., None]
    grain = ndimage.gaussian_filter(rng.normal(0, 1, (h, w)), 1.0)[..., None] * 8
    steel += grain
    img.alpha_composite(to_img(steel, mask * 255))
    # The gem under the middle.
    s = 64
    img.alpha_composite(setting(s), (int(w / 2 - s / 2), int(base_y - s / 2)))
    return img
