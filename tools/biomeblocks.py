"""About a hundred blocks for each of the 15 biomes (imported by tools/blockset.py, which
paints them and writes them after its own blocks).

Each biome has its own materials: two stones, two woods, a plaster colour, an accent colour,
a soil and a sand. Every block is a material in a pattern:

  * building, for each stone: cobbles, bricks, polished, small and large bricks,
    herringbone, tiles, diagonal tiles, chiselled, pillar, cracked and mossy bricks, mossy
    cobbles, flagstones, pebble mosaic, ashlar, framed, chequered, frieze, rough-hewn,
    shingles, inlaid, cut stone and basket weave;
  * natural, for each stone: the stone, mossy, cracked, weathered, rubble and layered;
  * wood, for each species: log, stripped log, planks, vertical planks, parquet, diagonal
    planks, panel, carved panel, shingles, timber frame, crate, log wall, lattice, leaves;
  * decor: plaster, timbered and cross-timbered plaster, painted plaster, wattle, daub, roof
    tiles, mosaic;
  * ground: coarse soil, stony soil, rooty soil, packed path, pebbles, sand.

The list is append-only like blockset.BLOCKS: add biomes, materials or patterns at the end.
"""
import numpy as np

import paint_textures as P
from paint_textures import N, rng, noise, fbm, voronoi, shade, ramp, quantize, clean, mottle, specks, tone, strata, lerp_stops

ys, xs = np.mgrid[0:N, 0:N]


def C(*rgb):
    return np.array(rgb, dtype=float)


def hexc(h):
    return C(int(h[0:2], 16) / 255, int(h[2:4], 16) / 255, int(h[4:6], 16) / 255)


def stops_from(h, spread=0.22):
    """Four colours around a middle colour: darker to lighter, a little cooler in shadow."""
    c = hexc(h)
    return [c * (1 - spread) + C(-0.01, 0, 0.02), c * (1 - spread * 0.4), c * (1 + spread * 0.25), np.minimum(c * (1 + spread * 0.6) + 0.02, 1)]


def blur(img, k=1):
    out = img.copy()
    for _ in range(k):
        out = (out + np.roll(out, 1, 0) + np.roll(out, -1, 0) + np.roll(out, 1, 1) + np.roll(out, -1, 1)) / 5
    return out


# ------------------------------------------------------------------ the biomes
# key: (label, stones [(name, label, style, colour)], woods [species], plaster, accent, soil, sand)
BIOMES = [
    ('wildwood', 'Ancient Wildwood', [('mossrock', 'Mossrock', 'mottled', '6b7266'), ('elderstone', 'Elderstone', 'granular', '7a6e62')],
     ['redwood', 'elder'], 'd8cfb6', '4f7a3a', '3a2a1c', 'a89a78'),
    ('amberwood', 'Amberwood', [('ochrestone', 'Ochrestone', 'layered', 'b08a52'), ('honeystone', 'Honeystone', 'smooth', 'd2b070')],
     ['maple', 'goldbark'], 'efe0bc', 'c0581c', '4a3220', 'c9a86e'),
    ('frostpeak', 'Frostpeak Highlands', [('frostgranite', 'Frost Granite', 'granular', '8f98a2'), ('rimeslate', 'Rimeslate', 'cleaved', '5d6874')],
     ['fir', 'frostpine'], 'e6ebef', '4f7ea8', '41392f', 'b7b5ad'),
    ('stonefang', 'Stonefang Mountains', [('fangbasalt', 'Fang Basalt', 'pitted', '3e3d40'), ('ironstone', 'Ironstone', 'mottled', '7a5444')],
     ['ironwood', 'cragpine'], 'bdb6aa', 'a33a22', '3b3029', '8c8478'),
    ('sunscorched', 'Sunscorched Expanse', [('sunstone', 'Sunstone', 'layered', 'd49a5a'), ('dunerock', 'Dunerock', 'smooth', 'dcc08c')],
     ['palm', 'acacia'], 'f0dcb0', '2f6f8f', '7a5636', 'e2c48a'),
    ('oasis', 'Emerald Oasis', [('jadestone', 'Jadestone', 'veined', '6f9a82'), ('travertine', 'Travertine', 'pitted', 'd8c9a6')],
     ['bamboo', 'fig'], 'f4ecd8', '1f8a7a', '4a3a26', 'e8d8a8'),
    ('murkwater', 'Murkwater Swamp', [('bogstone', 'Bogstone', 'mottled', '545a48'), ('mirestone', 'Mirestone', 'pitted', '5e5348')],
     ['cypress', 'willow'], 'a8a48a', '6e7a2a', '2a2218', '6e6a54'),
    ('tundra', 'Whitewind Tundra', [('windstone', 'Windstone', 'cleaved', '8a8c8c'), ('palestone', 'Palestone', 'smooth', 'c8c8c0')],
     ['birch', 'larch'], 'eeeeea', '7a4a6e', '4a4038', 'c0bcb2'),
    ('glacier', 'Crystal Glacier', [('glacierstone', 'Glacierstone', 'veined', 'a6bccb'), ('bluemarble', 'Blue Marble', 'veined', '7f9fbf')],
     ['frostwood', 'silverwood'], 'e8f2f8', '3aa6d8', '4a4a52', 'd0dce4'),
    ('taiga', 'Winterpine Taiga', [('taigarock', 'Taiga Rock', 'granular', '7d746c'), ('greystone', 'Greystone', 'mottled', '6e7072')],
     ['pine', 'spruce'], 'dcd2c0', '8a2a22', '33281e', 'a49a88'),
    ('blight', 'Blightlands', [('rotstone', 'Rotstone', 'pitted', '4a3a4a'), ('ashstone', 'Ashstone', 'mottled', '4e4a46')],
     ['blightwood', 'ashwood'], '8a8288', '7aa02a', '231c22', '5a5258'),
    ('petrified', 'Petrified Kingdom', [('petrastone', 'Petrastone', 'layered', 'a49684'), ('dustrock', 'Dustrock', 'smooth', 'bcae96')],
     ['deadwood', 'stonebark'], 'd6ccba', 'a07040', '5a4c3c', 'c8b898'),
    ('hollow', 'Hollow Earth', [('gloomstone', 'Gloomstone', 'mottled', '2e3038'), ('cavernstone', 'Cavernstone', 'layered', '4a4440')],
     ['glowwood', 'fungalwood'], '6a6870', '3ad0c0', '1e1a18', '5a5450'),
    ('stormbreak', 'Stormbreak Coast', [('tidestone', 'Tidestone', 'pitted', '6c7478'), ('seaslate', 'Sea Slate', 'cleaved', '4c5862')],
     ['driftwood', 'seapine'], 'e4e0d6', '2a5a8a', '3c342c', 'cfc2a2'),
    ('grasslands', "Giants' Grasslands", [('fieldstone', 'Fieldstone', 'granular', '8e8676'), ('meadowstone', 'Meadowstone', 'smooth', 'b4ac94')],
     ['oak', 'elm'], 'efe6cc', 'b03a2a', '3e2c1c', 'b8a47c'),
]

# New tree species: (bark ramp, wood ramp, plank ramp, leaf colours, bark style). Ramps are
# (hue, saturation, dark, light). The species blockset already has keep its textures.
NEW_WOODS = {
    'elder': ((25, 0.20, 0.10, 0.30), (28, 0.35, 0.25, 0.45), (26, 0.36, 0.22, 0.42), ['1c2c14', '2a3e1c', '3a5026', '4a6030'], 'deep'),
    'goldbark': ((35, 0.25, 0.30, 0.55), (42, 0.50, 0.45, 0.72), (40, 0.50, 0.42, 0.70), ['6a4008', '9a6410', 'c88c1c', 'e8b440'], 'plate'),
    'frostpine': ((210, 0.08, 0.25, 0.50), (40, 0.20, 0.55, 0.78), (35, 0.18, 0.50, 0.74), ['14282c', '1e3a40', '2c4e54', '3e6268'], 'scaly'),
    'ironwood': ((10, 0.15, 0.12, 0.30), (10, 0.35, 0.22, 0.40), (8, 0.35, 0.20, 0.38), ['26301a', '364224', '48542e', '5a6638'], 'deep'),
    'cragpine': ((30, 0.25, 0.18, 0.42), (34, 0.40, 0.42, 0.66), (32, 0.40, 0.40, 0.64), ['10241e', '18342a', '224436', '2e5442'], 'scaly'),
    'acacia': ((20, 0.15, 0.30, 0.52), (18, 0.60, 0.40, 0.62), (16, 0.60, 0.38, 0.60), ['46501a', '626c22', '7e862e', '9aa03c'], 'plate'),
    'bamboo': ((80, 0.45, 0.35, 0.62), (55, 0.45, 0.55, 0.78), (50, 0.45, 0.52, 0.76), ['2c5a14', '3e7a1c', '529626', '6aae34'], 'bamboo'),
    'fig': ((40, 0.10, 0.40, 0.62), (45, 0.30, 0.55, 0.75), (44, 0.30, 0.52, 0.72), ['143a14', '1e501c', '2a6626', '387c32'], 'plate'),
    'larch': ((15, 0.35, 0.20, 0.45), (30, 0.45, 0.42, 0.66), (28, 0.45, 0.40, 0.62), ['4a5a14', '62721c', '7c8a26', '96a234'], 'scaly'),
    'frostwood': ((200, 0.15, 0.55, 0.80), (195, 0.18, 0.62, 0.86), (200, 0.18, 0.58, 0.82), ['4a7a8a', '64949e', '82b0b6', 'a4ccd0'], 'plate'),
    'silverwood': ((220, 0.05, 0.55, 0.82), (50, 0.10, 0.65, 0.85), (40, 0.10, 0.62, 0.82), ['4a5a4e', '647464', '808e7e', 'a0aa9a'], 'birch'),
    'ashwood': ((0, 0.00, 0.06, 0.22), (20, 0.15, 0.20, 0.38), (20, 0.12, 0.18, 0.34), ['2a2826', '3c3a36', '504c48', '66625c'], 'deep'),
    'deadwood': ((35, 0.08, 0.30, 0.50), (38, 0.12, 0.45, 0.62), (36, 0.12, 0.42, 0.60), ['4a3c26', '62502e', '7a6438', '927a46'], 'plate'),
    'stonebark': ((30, 0.04, 0.32, 0.56), (35, 0.08, 0.45, 0.66), (34, 0.06, 0.42, 0.62), ['4a5444', '5e6856', '727c68', '88907c'], 'deep'),
    'glowwood': ((260, 0.25, 0.12, 0.32), (180, 0.45, 0.35, 0.60), (185, 0.40, 0.30, 0.55), ['0e3a3a', '146060', '1e8a86', '40c8bc'], 'plate'),
    'fungalwood': ((30, 0.15, 0.55, 0.75), (25, 0.25, 0.60, 0.80), (22, 0.25, 0.55, 0.75), ['4a2a4e', '663a6a', '845086', 'a46aa4'], 'plate'),
    'seapine': ((25, 0.20, 0.22, 0.44), (36, 0.30, 0.48, 0.70), (34, 0.28, 0.45, 0.68), ['0e2a2a', '163c3c', '204e4e', '2c6262'], 'scaly'),
    'elm': ((28, 0.18, 0.20, 0.42), (32, 0.35, 0.42, 0.64), (30, 0.35, 0.40, 0.62), ['2a4a16', '3a621e', '4c7a28', '609232'], 'plate'),
}

# Ramps for the species blockset already paints (their derived blocks need them too).
OLD_WOOD_RAMPS = {
    'oak': ((30, 0.30, 0.18, 0.45), (35, 0.40, 0.40, 0.66), (30, 0.40, 0.30, 0.58)),
    'driftwood': ((40, 0.08, 0.40, 0.74), (40, 0.10, 0.56, 0.78), (40, 0.10, 0.44, 0.74)),
}

# Blocks the earlier set already has for a species: (log, planks, leaves) names or None.
EXISTING_SPECIES = {'oak': ('log', 'oak_planks', 'leaves'), 'driftwood': ('driftwood_log', 'driftwood_planks', None)}


# ------------------------------------------------------------------ stone bases

def stone_base(name, style, colour):
    """A natural stone of the given style in colours around `colour`."""
    r = rng(name)
    st = stops_from(colour)
    if style == 'granular':
        v = mottle(r, coarse=4, grain=0.45)
        img = lerp_stops(v, st)
        img = np.where(specks(r, 0.08)[..., None], img * 1.22, img)
        img = np.where(specks(r, 0.07)[..., None], img * 0.70, img)
    elif style == 'layered':
        v = 0.55 * strata(r, 7, 0.9) + 0.45 * mottle(r, grain=0.4)
        img = lerp_stops(v, st)
        img = np.where(specks(r, 0.03)[..., None], img * 0.84, img)
    elif style == 'pitted':
        img = lerp_stops(mottle(r, coarse=3, grain=0.3), st)
        img = np.where(specks(r, 0.05, 2)[..., None], img * 0.68, img)
        f1, f2, _ = voronoi(r, 3, jitter=0.7)
        img = np.where(((f2 - f1) < 0.5)[..., None], img * 0.82, img)
    elif style == 'cleaved':
        y = ys + 4 * (fbm(r, 4, 2) - 0.5)
        sheets = 0.5 + 0.5 * np.sin(y / N * 2 * np.pi * 7 + 2.0 * noise(r, 8))
        img = lerp_stops(0.45 * sheets + 0.55 * mottle(r, coarse=3, grain=0.25), st)
    elif style == 'veined':
        img = lerp_stops(mottle(r, coarse=3, grain=0.2) * 0.7 + 0.3, st)
        warp = 6 * (fbm(r, 3, 3) - 0.5)
        vein = np.abs(np.sin((xs + ys * 0.6 + warp * 4) / N * 2 * np.pi * 1.5)) < 0.07
        img = np.where(vein[..., None], img * 0.74, img)
        vein2 = np.abs(np.sin((xs * 0.4 - ys + warp * 3) / N * 2 * np.pi)) < 0.04
        img = np.where(vein2[..., None], np.minimum(img * 1.12, 1), img)
    elif style == 'smooth':
        img = lerp_stops(mottle(r, coarse=3, grain=0.18), st)
        img = np.where(specks(r, 0.02)[..., None], img * 0.88, img)
    else:  # mottled
        img = lerp_stops(mottle(r, coarse=3, grain=0.3), st)
        img = np.where(specks(r, 0.03, 2)[..., None], img * 0.75, img)
        img = np.where(specks(r, 0.03)[..., None], img * 1.15, img)
    return np.clip(tone(img, 0.016, r), 0, 1)


# ------------------------------------------------------------------ stone patterns

def bevel_box(mask_x, mask_y, w, h, amount=0.12):
    """Lit top-left edges, shaded bottom-right ones, of boxes w x h at offsets."""
    return (np.where(mask_y == 1, amount, 0) + np.where(mask_x == 1, amount * 0.5, 0)
            - np.where(mask_y == h - 1, amount, 0) - np.where(mask_x == w - 1, amount * 0.5, 0))


def bricks(name, base, rows=4, cols=2, mortar=0.45, shift=0.5, vary=0.14, chip=0.02):
    r = rng(name)
    bh, bw = N // rows, N // cols
    row = ys // bh
    off = ((row % 2) * shift * bw).astype(int)
    ix, iy = (xs + off) % bw, ys % bh
    joint = (ix == 0) | (iy == 0)
    bid = row * 7 + (xs + off) // bw
    v = ((bid * 0.618) % 1.0 - 0.5) * vary
    img = base * (1.0 + v + bevel_box(ix, iy, bw, bh))[..., None]
    img = np.where(joint[..., None], base * mortar, img)
    img = np.where((specks(r, chip) & ~joint)[..., None], img * 0.82, img)
    return img


def cobbles(name, base, n=5, mortar=0.35):
    r = rng(name)
    f1, f2, ids = voronoi(r, n, jitter=0.85)
    edge = np.clip((f2 - f1) / 2.2, 0, 1)
    h = np.sqrt(edge)
    img = base * (1.0 + 0.6 * shade(h) + 0.12 * (((ids * 0.618) % 1.0) - 0.5) - 0.1 * (1 - h))[..., None]
    return np.where((edge < 0.2)[..., None], base * mortar * (0.8 + 0.4 * noise(r, 16))[..., None], img)


def polished(name, base, border=0.75):
    img = blur(base, 2) * 0.6 + base * 0.4
    m = np.minimum(np.minimum(xs, ys), np.minimum(N - 1 - xs, N - 1 - ys))
    img = np.where((m == 0)[..., None], img * border, img)
    img = np.where(((m == 1) & ((xs == 1) | (ys == 1)))[..., None], img * 1.1, img)
    return np.where(((m == 1) & ((xs == N - 2) | (ys == N - 2)))[..., None], img * 0.88, img)


def herringbone(name, base, mortar=0.5):
    """Bricks 4 x 8 laid in a zig-zag."""
    r = rng(name)
    u, v = (xs + ys) % 16, (ys - xs) % 16
    a = ((xs // 4 + ys // 4) % 2 == 0)
    # Two orientations alternate in 4-pixel diagonal bands.
    k = ((xs + ys) // 8) % 2
    lx = np.where(k == 0, xs % 8, ys % 8)
    ly = np.where(k == 0, (ys + (xs // 8) * 4) % 4, (xs + (ys // 8) * 4) % 4)
    joint = (lx == 0) | (ly == 0)
    bid = (xs // 8) * 5 + (ys // 4) * 3 + k
    img = base * (1.0 + ((bid * 0.618) % 1 - 0.5) * 0.14 + np.where(ly == 1, 0.08, 0) - np.where(ly == 3, 0.08, 0))[..., None]
    del u, v, a
    img = np.where(joint[..., None], base * mortar, img)
    return np.where(specks(r, 0.015)[..., None], img * 0.85, img)


def tiles(name, base, n=4, mortar=0.55, diagonal=False):
    r = rng(name)
    s = N // n
    if diagonal:
        u, v = (xs + ys) % (s * 2), (xs - ys) % (s * 2)
        ix, iy = u // 2, v // 2
        joint = (u < 2) | (v < 2)
        tid = ((xs + ys) // (s * 2)) * 3 + ((xs - ys) // (s * 2))
        light = np.where(u < 4, 0.08, 0) - np.where(v > s * 2 - 3, 0.08, 0)
    else:
        ix, iy = xs % s, ys % s
        joint = (ix == 0) | (iy == 0)
        tid = (xs // s) * 5 + (ys // s)
        light = bevel_box(ix, iy, s, s, 0.1)
    flat = blur(base, 1)
    img = flat * (1.0 + ((tid * 0.618) % 1 - 0.5) * 0.12 + light)[..., None]
    img = np.where(joint[..., None], base * mortar, img)
    del ix, iy, r
    return img


def chiselled(name, base, accent):
    """Polished, with a carved square-in-square and a central knot."""
    img = polished(name, base)
    m = np.maximum(np.abs(xs - 15.5), np.abs(ys - 15.5))
    groove = (np.abs(m - 11) < 0.6) | (np.abs(m - 6) < 0.6)
    img = np.where(groove[..., None], img * 0.62, img)
    img = np.where((np.abs(m - 10) < 0.6)[..., None] | (np.abs(m - 5) < 0.6)[..., None], img * 1.12, img)
    d = np.abs(xs - 15.5) + np.abs(ys - 15.5)
    img = np.where((d < 3)[..., None], img * 0.75 + accent * 0.25, img)
    return img


def pillar_side(name, base):
    """Vertical fluting with a capital and base band."""
    img = blur(base, 1) * 0.7 + base * 0.3
    flute = (xs % 6)
    img = img * (1.0 + 0.10 * np.cos(flute / 6 * 2 * np.pi))[..., None]
    img = np.where((flute == 0)[..., None], img * 0.72, img)
    band = (ys < 3) | (ys > N - 4)
    img = np.where(band[..., None], blur(base, 1) * 1.05, img)
    img = np.where(((ys == 3) | (ys == N - 4))[..., None], base * 0.6, img)
    return img


def pillar_top(name, base):
    img = polished(name, base)
    d = np.sqrt((xs - 15.5) ** 2 + (ys - 15.5) ** 2)
    img = np.where((np.abs(d - 12) < 0.7)[..., None], img * 0.68, img)
    return np.where((d < 11.5)[..., None], img * (1.04 - 0.004 * d)[..., None], img)


def cracked(name, img, count=4):
    r = rng(name + ' cracks')
    out = img.copy()
    for k in range(count):
        path = (r.integers(0, N) + np.cumsum(r.integers(-1, 2, N))) % N
        crack = (xs == path[:, None]) if k % 2 == 0 else (ys == path[None, :])
        crack &= noise(r, 3) > 0.42
        out = np.where(crack[..., None], out * 0.55, out)
    return out


def mossy(name, img, amount=0.75):
    r = rng(name + ' moss')
    moss = fbm(r, 4, 3) + 0.3 * r.random((N, N))
    green = lerp_stops(r.random((N, N)), [C(0.20, 0.30, 0.12), C(0.30, 0.42, 0.16), C(0.40, 0.52, 0.22)])
    return np.where((moss > amount)[..., None], green, img)


def flagstone(name, base):
    r = rng(name)
    f1, f2, ids = voronoi(r, 3, jitter=0.9)
    edge = (f2 - f1)
    img = blur(base, 1) * (1.0 + 0.1 * (((ids * 0.618) % 1.0) - 0.5))[..., None]
    img = np.where((edge < 0.8)[..., None], base * 0.4, img)
    img = np.where(((edge >= 0.8) & (edge < 1.6))[..., None], img * 0.92, img)
    return img


def pebble_mosaic(name, base, accent):
    r = rng(name)
    f1, f2, ids = voronoi(r, 9, jitter=0.9)
    edge = np.clip((f2 - f1) / 1.4, 0, 1)
    pick = (ids * 0.618) % 1.0
    col = np.where((pick > 0.82)[..., None], base * 0.6 + accent * 0.4, base * (0.85 + 0.3 * pick)[..., None])
    img = col * (1.0 + 0.8 * shade(np.sqrt(edge)))[..., None]
    return np.where((edge < 0.25)[..., None], base * 0.35, img)


def ashlar(name, base, mortar=0.45):
    """Random rectangles in courses of differing heights."""
    r = rng(name)
    img = base.copy()
    y = 0
    k = 0
    while y < N:
        h = int(r.choice([5, 6, 8, 10]))
        h = min(h, N - y)
        x = int(r.integers(0, 8))
        while True:
            w = int(r.integers(6, 15))
            v = (r.random() - 0.5) * 0.16
            for xx in range(x, x + w):
                img[y:y + h, xx % N] = base[y:y + h, xx % N] * (1 + v)
                img[y, xx % N] *= 1.1
                img[y + h - 1, xx % N] *= 0.88
            img[y:y + h, x % N] = base[y:y + h, x % N] * mortar
            x += w
            k += 1
            if x >= N + 0:
                break
        img[y, :] = base[y, :] * mortar
        y += h
    return img


def framed(name, base, accent):
    img = blur(base, 1)
    m = np.minimum(np.minimum(xs, ys), np.minimum(N - 1 - xs, N - 1 - ys))
    trim = m < 4
    img = np.where(trim[..., None], base * 1.08, img)
    img = np.where((m == 4)[..., None], base * 0.55, img)
    img = np.where((m == 0)[..., None], base * 0.7, img)
    img = np.where((m == 1)[..., None] & ((xs == 1) | (ys == 1))[..., None], base * 1.2, img)
    return img


def checker(name, base):
    s = 8
    alt = ((xs // s + ys // s) % 2 == 0)
    img = np.where(alt[..., None], blur(base, 1) * 1.12, blur(base, 1) * 0.72)
    return np.where(((xs % s == 0) | (ys % s == 0))[..., None], img * 0.82, img)


def frieze(name, base, accent):
    """Smooth stone with a carved band of meander (Greek key) through the middle."""
    img = blur(base, 1) * 0.8 + base * 0.2
    band = (ys >= 10) & (ys < 22)
    img = np.where(band[..., None], img * 0.92, img)
    u, v = xs % 8, ys - 10
    key = band & (((v == 1) & (u < 7)) | ((u == 6) & (v >= 1) & (v <= 9)) | ((v == 9) & (u >= 2) & (u <= 6)) | ((u == 2) & (v >= 4) & (v <= 9)) | ((v == 4) & (u >= 2) & (u <= 4)))
    img = np.where(key[..., None], img * 0.55, img)
    img = np.where(((ys == 9) | (ys == 22))[..., None], base * 0.6, img)
    img = np.where(((ys == 8) | (ys == 23))[..., None], base * 1.15, img)
    del accent
    return img


def rough_hewn(name, base):
    r = rng(name)
    img = base.copy()
    for _ in range(70):
        y, x = r.integers(0, N, 2)
        l = int(r.integers(2, 5))
        for k in range(l):
            img[(y + k) % N, (x + k) % N] *= 0.72
            img[(y + k) % N, (x + k + 1) % N] *= 1.12
    return img


def shingles(name, base, scale_w=8, scale_h=6, mortar=0.45):
    """Overlapping rounded tiles in rows (a roof)."""
    r = rng(name)
    row = ys // scale_h
    off = (row % 2) * (scale_w // 2)
    lx = (xs + off) % scale_w
    ly = ys % scale_h
    # A rounded bottom edge: the tile ends lower in its middle.
    curve = scale_h - 1 - (np.abs(lx - (scale_w - 1) / 2) / (scale_w / 2)) ** 2 * 2
    under = ly > curve
    tid = row * 7 + (xs + off) // scale_w
    img = base * (1.0 + ((tid * 0.618) % 1 - 0.5) * 0.18 - 0.25 * (ly / scale_h))[..., None]
    img = np.where(under[..., None], base * mortar, img)
    img = np.where((lx == 0)[..., None] & ~under[..., None], img * 0.8, img)
    del r
    return img


def inlaid(name, base, accent):
    img = polished(name, base)
    diag = (np.abs(xs - ys) < 1) | (np.abs(xs + ys - (N - 1)) < 1)
    img = np.where(diag[..., None], accent * 0.85, img)
    d = np.abs(xs - 15.5) + np.abs(ys - 15.5)
    return np.where((np.abs(d - 9) < 0.7)[..., None], accent, img)


def basket(name, base, mortar=0.5):
    """Pairs of bricks, alternately across and upright, in 8 x 8 squares."""
    q = ((xs // 8 + ys // 8) % 2 == 0)
    lx, ly = xs % 8, ys % 8
    a = np.where(q, ly % 4, lx % 4)
    b = np.where(q, lx, ly)
    joint = (a == 0) | (b == 0)
    img = base * (1.0 + np.where(a == 1, 0.08, 0) - np.where(a == 3, 0.08, 0) + 0.06 * q)[..., None]
    return np.where(joint[..., None], base * mortar, img)


def layered_rock(name, base):
    r = rng(name)
    s = strata(r, 6, 1.0)
    return base * (0.85 + 0.3 * s)[..., None]


def rubble(name, base):
    r = rng(name)
    f1, f2, ids = voronoi(r, 6, jitter=1.0, stretch=(1.0, 0.7))
    edge = np.clip((f2 - f1) / 1.6, 0, 1)
    img = base * (0.8 + 0.4 * ((ids * 0.37) % 1.0) + 0.9 * shade(np.sqrt(edge)))[..., None]
    return np.where((edge < 0.3)[..., None], base * 0.3, img)


def weathered(name, base):
    r = rng(name + ' lichen')
    lichen = fbm(r, 5, 2) > 0.68
    img = np.where(specks(r, 0.05, 2)[..., None], base * 0.75, base)
    col = lerp_stops(r.random((N, N)), [C(0.58, 0.60, 0.42), C(0.70, 0.70, 0.50)])
    return np.where(lichen[..., None], col, img)


STONE_PATTERNS = [
    # (suffix, label, group, slab, painter(name, base, accent) or ('pillar',))
    ('cobbles', 'Cobbles', 'building', True, lambda n, b, a: cobbles(n, b)),
    ('bricks', 'Bricks', 'building', True, lambda n, b, a: bricks(n, b)),
    ('polished', 'Polished', 'building', True, lambda n, b, a: polished(n, b)),
    ('small_bricks', 'Small Bricks', 'building', True, lambda n, b, a: bricks(n, b, rows=8, cols=4)),
    ('large_bricks', 'Large Bricks', 'building', True, lambda n, b, a: bricks(n, b, rows=2, cols=1)),
    ('herringbone', 'Herringbone', 'building', True, lambda n, b, a: herringbone(n, b)),
    ('tiles', 'Tiles', 'building', True, lambda n, b, a: tiles(n, b)),
    ('diagonal_tiles', 'Diagonal Tiles', 'building', True, lambda n, b, a: tiles(n, b, diagonal=True)),
    ('chiselled', 'Chiselled', 'building', False, chiselled),
    ('pillar', 'Pillar', 'building', False, 'pillar'),
    ('cracked_bricks', 'Cracked Bricks', 'building', True, lambda n, b, a: cracked(n, bricks(n, b))),
    ('mossy_bricks', 'Mossy Bricks', 'building', True, lambda n, b, a: mossy(n, bricks(n, b))),
    ('mossy_cobbles', 'Mossy Cobbles', 'building', True, lambda n, b, a: mossy(n, cobbles(n, b))),
    ('flagstones', 'Flagstones', 'building', True, lambda n, b, a: flagstone(n, b)),
    ('pebble_mosaic', 'Pebble Mosaic', 'building', True, pebble_mosaic),
    ('ashlar', 'Ashlar', 'building', True, lambda n, b, a: ashlar(n, b)),
    ('framed', 'Framed', 'building', False, framed),
    ('chequered', 'Chequered', 'building', True, lambda n, b, a: checker(n, b)),
    ('frieze', 'Frieze', 'building', False, frieze),
    ('rough_hewn', 'Rough-hewn', 'building', True, lambda n, b, a: rough_hewn(n, b)),
    ('shingles', 'Shingles', 'building', True, lambda n, b, a: shingles(n, b)),
    ('inlaid', 'Inlaid', 'building', False, inlaid),
    ('cut', 'Cut Stone', 'building', True, lambda n, b, a: bricks(n, blur(b, 1), rows=2, cols=2, shift=0.0, mortar=0.55)),
    ('basketweave', 'Basketweave', 'building', True, lambda n, b, a: basket(n, b)),
]

NATURAL_PATTERNS = [
    ('', '', lambda n, b: b),
    ('mossy', 'Mossy', lambda n, b: mossy(n, b, 0.7)),
    ('cracked', 'Cracked', lambda n, b: cracked(n, b, 5)),
    ('weathered', 'Weathered', weathered),
    ('rubble', 'Rubble', rubble),
    ('layered', 'Layered', layered_rock),
]


# ------------------------------------------------------------------ wood patterns

def wood_colours(ramp_t, n=5):
    h, s, d, l = ramp_t
    return ramp(h, s, d, l, n, dark_hue=h - 10, light_hue=h + 8)


def stripped_side(name, wood):
    r = rng(name)
    c = wood_colours(wood, 5)
    grain = 0.5 + 0.5 * np.sin(xs * 1.6 + 1.2 * fbm(r, 3, 2))
    v = 0.35 * grain + 0.35 * fbm(r, 6, 2) + 0.3 * r.random((N, N))
    idx = 2 + quantize(v, 2, dither=0.1)
    knot = (np.sqrt(((xs - 20) * 1.0) ** 2 + ((ys - 9) * 0.5) ** 2) < 1.8)
    idx = np.where(knot, 0, idx)
    return c[clean(np.minimum(idx, 4))]


def stripped_top(name, wood):
    r = rng(name)
    c = wood_colours(wood, 5)
    d = np.sqrt((xs + 0.5 - 16) ** 2 + (ys + 0.5 - 16) ** 2) + 1.2 * (fbm(r, 4, 2) - 0.5)
    idx = np.where(np.floor(d / 2.0) % 2 == 0, 3, 2)
    idx = np.where(d < 1.6, 1, idx)
    rim = np.maximum(np.abs(xs + 0.5 - 16), np.abs(ys + 0.5 - 16)) > 14.5
    return c[np.where(rim, 1, idx)]


def plank_rows(name, plank, boards=4, vertical=False, seams=True):
    import blockset as B
    img = B.planks_tex(name, *plank, boards=boards)
    return np.transpose(img, (1, 0, 2)) if vertical else img


def parquet(name, plank):
    c = wood_colours(plank, 5)
    r = rng(name)
    q = ((xs // 8 + ys // 8) % 2 == 0)
    lx, ly = xs % 8, ys % 8
    a = np.where(q, ly % 4, lx % 4)
    b = np.where(q, lx, ly)
    grain = 0.5 + 0.5 * np.sin(np.where(q, xs, ys) * 1.1 + 2 * fbm(r, 4, 2))
    sid = (xs // 8) * 3 + (ys // 4) * 5 + (xs // 4) * 7 * (~q)
    v = 0.5 * grain + 0.35 * ((sid * 0.618) % 1) + 0.15 * r.random((N, N))
    idx = 1 + quantize(v, 4, dither=0.1)
    idx = np.where((a == 0) | (b == 0), 0, idx)
    return c[np.minimum(idx, 4)]


def diagonal_planks(name, plank):
    c = wood_colours(plank, 5)
    r = rng(name)
    u = (xs + ys) % 8
    bid = (xs + ys) // 8
    grain = 0.5 + 0.5 * np.sin((xs - ys) * 0.8 + 2 * fbm(r, 4, 2))
    v = 0.5 * grain + 0.35 * ((bid * 0.618) % 1) + 0.15 * r.random((N, N))
    idx = 1 + quantize(v, 4, dither=0.1)
    idx = np.where(u == 0, 0, idx)
    return c[np.minimum(idx, 4)]


def panel(name, plank, carved=False):
    c = wood_colours(plank, 6)
    r = rng(name)
    m = np.minimum(np.minimum(xs, ys), np.minimum(N - 1 - xs, N - 1 - ys))
    grain = 0.5 + 0.5 * np.sin(ys * 1.4 + 2 * fbm(r, 4, 2))
    v = 0.55 * grain + 0.3 * fbm(r, 6, 2) + 0.15 * r.random((N, N))
    idx = 1 + quantize(v, 3, dither=0.1)
    frame = m < 5
    fgrain = 1 + quantize(0.5 + 0.5 * np.sin(np.where(np.minimum(xs, N - 1 - xs) < np.minimum(ys, N - 1 - ys), ys, xs) * 1.2 + 2 * fbm(r, 4, 2)), 3)
    idx = np.where(frame, np.minimum(fgrain + 1, 5), idx)
    idx = np.where(m == 5, 0, idx)
    idx = np.where((m == 6), np.minimum(idx + 1, 5), idx)
    idx = np.where(m == 0, 0, idx)
    if carved:
        d = np.abs(xs - 15.5) + np.abs(ys - 15.5)
        idx = np.where((np.abs(d - 7) < 0.7) | (np.abs(d - 3) < 0.7), 0, idx)
        idx = np.where((np.abs(d - 6) < 0.6), 5, idx)
    return c[idx]


def wood_shingles(name, plank):
    import blockset as B
    base = B.planks_tex(name + ' base', *plank)
    return shingles(name, base, scale_w=8, scale_h=8, mortar=0.4)


def timber_frame(name, plank, fill):
    """Dark beams round the edge and across, light fill between (a framed wall)."""
    c = wood_colours((plank[0], plank[1], plank[2] * 0.6, plank[3] * 0.7), 4)
    r = rng(name)
    beam = (xs < 3) | (xs > N - 4) | (ys < 3) | (ys > N - 4) | (np.abs(xs - ys) < 2)
    img = fill.copy()
    g = 1 + quantize(0.5 + 0.5 * np.sin((xs + ys) * 0.9 + 2 * fbm(r, 4, 2)), 3)
    img = np.where(beam[..., None], c[np.minimum(g, 3)], img)
    return img


def crate(name, plank):
    c = wood_colours(plank, 5)
    r = rng(name)
    m = np.minimum(np.minimum(xs, ys), np.minimum(N - 1 - xs, N - 1 - ys))
    boards = 1 + quantize(0.5 + 0.5 * np.sin(ys * 0.9 + 2 * fbm(r, 4, 2)) + 0.4 * ((ys // 8) * 0.618 % 1), 3)
    idx = np.where(ys % 8 == 0, 0, boards)
    frame = m < 4
    cross = (np.abs(xs - ys) < 2) | (np.abs(xs + ys - (N - 1)) < 2)
    idx = np.where(frame | cross, np.minimum(boards + 1, 4), idx)
    idx = np.where((m == 4) & ~cross, 0, idx)
    idx = np.where(m == 0, 0, idx)
    nails = ((xs == 1) | (xs == N - 2)) & ((ys == 1) | (ys == N - 2))
    img = c[idx]
    return np.where(nails[..., None], C(0.3, 0.3, 0.32), img)


def log_wall(name, bark, wood):
    """Horizontal logs stacked, their round ends at the left (a cabin wall)."""
    import blockset as B
    side = B.bark_tex(name + ' bark', *bark, 'plate')
    side = np.transpose(side, (1, 0, 2))
    c = wood_colours(wood, 4)
    img = side.copy()
    lh = 8
    ly = ys % lh
    img = img * (0.75 + 0.5 * np.sin(ly / lh * np.pi))[..., None]
    img = np.where((ly == 0)[..., None], img * 0.45, img)
    del c
    return img


def lattice(name, plank):
    c = wood_colours(plank, 5)
    u, v = (xs + ys) % 11, (xs - ys) % 11
    slat = (u < 3) | (v < 3)
    over = (u < 3) & ((xs + ys) // 11 + (xs - ys) // 11) % 2 == 0
    idx = np.where(slat, 3, 0)
    idx = np.where((u == 0) | (v == 0), 2, idx)
    idx = np.where(over & (u == 1), 4, idx)
    img = c[idx]
    return np.where((~slat)[..., None], c[0] * 0.5, img)


# ------------------------------------------------------------------ decor and ground

def plaster(name, colour, rough=0.3):
    r = rng(name)
    st = stops_from(colour, 0.08)
    img = lerp_stops(mottle(r, coarse=4, grain=rough), st)
    img = np.where(specks(r, 0.02, 2)[..., None], img * 0.9, img)
    return np.clip(tone(img, 0.01, r), 0, 1)


def painted(name, colour, accent):
    img = plaster(name, colour) * 0.3 + accent * 0.7
    r = rng(name)
    worn = fbm(r, 5, 2) > 0.75
    return np.where(worn[..., None], plaster(name + ' under', colour), img * (0.92 + 0.12 * r.random((N, N)))[..., None])


def wattle(name, plank):
    c = wood_colours(plank, 5)
    r = rng(name)
    stake = (xs % 8 < 2)
    weave_row = ys // 4
    over = ((xs // 8 + weave_row) % 2 == 0)
    v = 0.5 + 0.5 * np.sin(xs * 0.7 + r.random() * 6)
    idx = 1 + quantize(v + 0.4 * over, 3)
    idx = np.where(ys % 4 == 0, 0, idx)
    idx = np.where(stake & ~over, 0, idx)
    idx = np.where(stake & over, 4, idx)
    return c[np.minimum(idx, 4)]


def daub(name, soil):
    import blockset as B
    base = B.soil_like(name, stops_from(soil, 0.25), holes=0.02, grit=0.04, grit_c=C(0.7, 0.6, 0.36), stones=0.0)
    r = rng(name + ' straw')
    straw = specks(r, 0.04)
    return np.where(straw[..., None], C(0.74, 0.64, 0.36), base * 1.35)


def roof_tiles(name, colour):
    base = plaster(name + ' clay', colour, 0.4)
    return shingles(name, base, scale_w=8, scale_h=8, mortar=0.5)


def mosaic(name, base, accent, plaster_c):
    r = rng(name)
    s = 4
    tid = (xs // s) * 11 + (ys // s) * 7
    d = np.abs(xs // s - 3.5) + np.abs(ys // s - 3.5)
    pal = np.where((np.abs(d - 3) < 0.6)[..., None], accent, np.where((d < 1.5)[..., None], hexc(plaster_c), base))
    img = pal * (0.88 + 0.24 * ((tid * 0.618) % 1))[..., None]
    img = np.where(((xs % s == 0) | (ys % s == 0))[..., None], base * 0.4, img)
    del r
    return img


def coarse_soil(name, soil):
    import blockset as B
    return B.soil_like(name, stops_from(soil, 0.3), holes=0.05, grit=0.08, stones=0.03)


def stony_soil(name, soil, stone):
    img = coarse_soil(name, soil)
    r = rng(name + ' stones')
    f1, f2, ids = voronoi(r, 6, jitter=0.9)
    st = (f1 < 1.3) & (((ids * 0.618) % 1) > 0.55)
    h = np.clip(1 - f1 / 1.3, 0, 1)
    return np.where(st[..., None], stone * (0.8 + 0.5 * h + 0.6 * shade(h))[..., None], img)


def rooty_soil(name, soil, wood):
    img = coarse_soil(name, soil)
    r = rng(name + ' roots')
    c = wood_colours(wood, 4)
    for _ in range(5):
        x, y = r.integers(0, N, 2)
        dx = r.choice([-1, 1])
        for k in range(18):
            img[y % N, x % N] = c[1] * 0.8
            img[(y + 1) % N, x % N] = c[0] * 0.8
            x += dx if r.random() < 0.6 else 0
            y += 1 if r.random() < 0.5 else 0
    return img


def packed_path(name, soil):
    r = rng(name)
    img = lerp_stops(mottle(r, coarse=4, grain=0.2), stops_from(soil, 0.15)) * 1.25
    ruts = (np.abs(np.sin(xs / N * 2 * np.pi * 2 + 2 * fbm(r, 3, 2))) > 0.96) & (noise(r, 6) > 0.4)
    img = np.where(ruts[..., None], img * 0.82, img)
    return np.where(specks(r, 0.03)[..., None], img * 1.15, img)


def pebbles(name, soil, stone):
    r = rng(name)
    f1, f2, ids = voronoi(r, 10, jitter=0.95)
    edge = np.clip((f2 - f1) / 1.2, 0, 1)
    tint = np.where((((ids * 0.618) % 1) > 0.5)[..., None], stone, stone * 0.75 + hexc(soil) * 0.25)
    img = tint * (0.85 + 0.3 * ((ids * 0.37) % 1) + 0.9 * shade(np.sqrt(edge)))[..., None]
    return np.where((edge < 0.3)[..., None], hexc(soil) * 0.8, img)


def biome_sand(name, colour):
    r = rng(name)
    ripple = 0.5 + 0.5 * np.sin((ys / N * 3 + fbm(r, 4, 2) * 1.5) * 2 * np.pi)
    v = 0.55 * mottle(r, grain=0.45) + 0.25 * ripple + 0.2 * r.random((N, N))
    img = lerp_stops(v, stops_from(colour, 0.15))
    return np.where(specks(r, 0.03)[..., None], img * 0.82, img)


# ------------------------------------------------------------------ the list

def build():
    """(blocks, textures): blocks as blockset.B tuples plus a biome, textures name -> RGB."""
    import blockset as B
    blocks, tex = [], {}

    def add(name, group, family, biome, t=None, **kw):
        kw['biome'] = biome
        blocks.append(B.B(name, group, family, t, **kw))

    for key, blabel, stones, woods, plaster_c, accent_h, soil, sand in BIOMES:
        accent = hexc(accent_h)
        bases = {}
        for sname, slabel, style, colour in stones:
            base = stone_base(sname, style, colour)
            bases[sname] = base
            for suf, plabel, painter in NATURAL_PATTERNS:
                name = f'{sname}_{suf}' if suf else sname
                lab = f'{plabel} {slabel}'.strip()
                tex[name] = painter(name, base)
                add(name, 'natural', sname, key, label=lab, hardness=1.5)
            for suf, plabel, group, slab, painter in STONE_PATTERNS:
                name = f'{sname}_{suf}'
                lab = f'{slabel} {plabel}'
                if painter == 'pillar':
                    tex[name + '_side'] = pillar_side(name, base)
                    tex[name + '_top'] = pillar_top(name, base)
                    add(name, group, sname, key, {'top': name + '_top', 'bottom': name + '_top', 'side': name + '_side'}, label=lab, hardness=2.0)
                else:
                    tex[name] = painter(name, base, accent)
                    kw = {'slab': True} if slab else {}
                    add(name, group, sname, key, label=lab, hardness=2.0, **kw)
        for sp in woods:
            lab = sp.replace('_', ' ').title()
            if sp in NEW_WOODS:
                bark, wood, plank, leaves, style = NEW_WOODS[sp]
                lc = [hexc(h) for h in leaves]
                tex[f'{sp}_log_side'] = B.bark_tex(f'{sp}_log_side', *bark, style) if style != 'bamboo' else bamboo_side(sp, bark)
                tex[f'{sp}_log_top'] = B.log_top_tex(f'{sp}_log_top', bark, wood)
                tex[f'{sp}_planks'] = B.planks_tex(f'{sp}_planks', *plank)
                tex[f'{sp}_leaves'] = B.leaves_tex(f'{sp}_leaves', lc, 'pine' if style == 'scaly' else 'maple')
                add(f'{sp}_log', 'wood', sp, key, {'top': f'{sp}_log_top', 'bottom': f'{sp}_log_top', 'side': f'{sp}_log_side'}, label=f'{lab} Log', hardness=2.0)
                add(f'{sp}_planks', 'wood', sp, key, label=f'{lab} Planks', hardness=1.5, slab=True)
                add(f'{sp}_leaves', 'wood', sp, key, label=f'{lab} Leaves', opaque=False, render='cutout', hardness=0.2)
            elif sp in OLD_WOOD_RAMPS:
                bark, wood, plank = OLD_WOOD_RAMPS[sp]
            else:
                b0, w0, p0, _, _ = B.WOODS[sp]
                bark, wood, plank = b0, w0, p0
            fill = plaster(f'{sp}_frame_fill', plaster_c)
            sets = [
                ('stripped_log', 'Stripped Log', None, False),
                ('vertical_planks', 'Vertical Planks', lambda n: plank_rows(n, plank, vertical=True), True),
                ('parquet', 'Parquet', lambda n: parquet(n, plank), True),
                ('diagonal_planks', 'Diagonal Planks', lambda n: diagonal_planks(n, plank), True),
                ('panel', 'Panel', lambda n: panel(n, plank), False),
                ('carved_panel', 'Carved Panel', lambda n: panel(n, plank, carved=True), False),
                ('shingles', 'Shingles', lambda n: wood_shingles(n, plank), True),
                ('timber_frame', 'Timber Frame', lambda n: timber_frame(n, plank, fill), False),
                ('crate', 'Crate', lambda n: crate(n, plank), False),
                ('log_wall', 'Log Wall', lambda n: log_wall(n, bark, wood), False),
                ('lattice', 'Lattice', lambda n: lattice(n, plank), False),
            ]
            for suf, plabel, painter, slab in sets:
                name = f'{sp}_{suf}'
                if suf == 'stripped_log':
                    tex[name + '_side'] = stripped_side(name + '_side', wood)
                    tex[name + '_top'] = stripped_top(name + '_top', wood)
                    add(name, 'wood', sp, key, {'top': name + '_top', 'bottom': name + '_top', 'side': name + '_side'}, label=f'Stripped {lab} Log', hardness=2.0)
                    continue
                tex[name] = painter(name)
                kw = {'slab': True} if slab else {}
                add(name, 'wood', sp, key, label=f'{lab} {plabel}', hardness=1.5, **kw)
        # Decor.
        s0 = bases[stones[0][0]]
        wood0 = woods[0]
        p0 = NEW_WOODS[wood0][2] if wood0 in NEW_WOODS else (OLD_WOOD_RAMPS[wood0][2] if wood0 in OLD_WOOD_RAMPS else B.WOODS[wood0][2])
        fam = f'{key}_decor'
        pl = plaster(f'{key}_plaster', plaster_c)
        decor = [
            ('plaster', 'Plaster', pl, True),
            ('timbered_plaster', 'Timbered Plaster', timber_frame(f'{key}_timbered', p0, pl) * np.where(((np.abs(xs - ys) < 2))[..., None], 1, 1), False),
            ('cross_timbered_plaster', 'Cross-timbered Plaster', cross_timber(f'{key}_cross', p0, pl), False),
            ('painted_plaster', 'Painted Plaster', painted(f'{key}_painted', plaster_c, accent), True),
            ('wattle', 'Wattle', wattle(f'{key}_wattle', p0), False),
            ('daub', 'Daub', daub(f'{key}_daub', soil), False),
            ('roof_tiles', 'Roof Tiles', roof_tiles(f'{key}_roof', accent_h), True),
            ('mosaic', 'Mosaic', mosaic(f'{key}_mosaic', s0, accent, plaster_c), True),
        ]
        for suf, plabel, img, slab in decor:
            name = f'{key}_{suf}'
            tex[name] = img
            kw = {'slab': True} if slab else {}
            add(name, 'building', fam, key, label=f'{blabel} {plabel}', hardness=1.2, **kw)
        # Ground.
        gfam = f'{key}_ground'
        s1 = bases[stones[1][0]]
        wood_r = NEW_WOODS[wood0][1] if wood0 in NEW_WOODS else (OLD_WOOD_RAMPS[wood0][1] if wood0 in OLD_WOOD_RAMPS else B.WOODS[wood0][1])
        ground = [
            ('coarse_soil', 'Coarse Soil', coarse_soil(f'{key}_coarse_soil', soil), {}),
            ('stony_soil', 'Stony Soil', stony_soil(f'{key}_stony_soil', soil, s1.mean((0, 1))), {}),
            ('rooty_soil', 'Rooty Soil', rooty_soil(f'{key}_rooty_soil', soil, wood_r), {}),
            ('path', 'Path', packed_path(f'{key}_path', soil), {}),
            ('pebbles', 'Pebbles', pebbles(f'{key}_pebbles', soil, s0.mean((0, 1))), {'behavior': 'falling'}),
            ('sand', 'Sand', biome_sand(f'{key}_sand', sand), {'behavior': 'falling'}),
        ]
        for suf, plabel, img, kw in ground:
            name = f'{key}_{suf}'
            tex[name] = img
            add(name, 'natural', gfam, key, label=f'{blabel} {plabel}', hardness=0.6, **kw)
    return blocks, tex


def cross_timber(name, plank, fill):
    c = wood_colours((plank[0], plank[1], plank[2] * 0.6, plank[3] * 0.7), 4)
    beam = (xs < 3) | (xs > N - 4) | (ys < 3) | (ys > N - 4) | (np.abs(xs - ys) < 2) | (np.abs(xs + ys - (N - 1)) < 2) | (np.abs(ys - 15.5) < 1.5)
    r = rng(name)
    g = 1 + quantize(0.5 + 0.5 * np.sin((xs - ys) * 0.9 + 2 * fbm(r, 4, 2)), 3)
    return np.where(beam[..., None], c[np.minimum(g, 3)], fill)


def bamboo_side(name, bark):
    r = rng(name + ' bamboo')
    c = wood_colours(bark, 5)
    stalk = xs // 8
    lx = xs % 8
    node_y = (r.integers(0, 12, 4))[stalk]
    ly = (ys + node_y) % 12
    h = np.sin((lx + 0.5) / 8 * np.pi)
    v = h * 0.8 + 0.2 * r.random((N, N))
    idx = 1 + quantize(v, 4)
    idx = np.where(lx == 0, 0, idx)
    idx = np.where(ly == 0, 0, idx)
    idx = np.where(ly == 1, np.minimum(idx + 1, 4), idx)
    return c[np.minimum(idx, 4)]
