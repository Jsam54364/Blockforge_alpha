#!/usr/bin/env python3
"""Paints the block textures in assets/textures: original 32 x 32 textures in the manner of
Vintage Story's. Run from anywhere; needs numpy and Pillow.

    python3 tools/paint_textures.py

The terrain textures (soil, grass, sand, gravel, rocks, snow) are fine-grained and low in
contrast, so ground reads as a material from a distance instead of as a pattern:
  * a smooth colour ramp per material, with soft mottling over a few pixels;
  * grain at the scale of single pixels, and each material's own detail on top: blades for
    grass, specks and crumbs for soil, pebbles for gravel, crystals for granite, strata for
    limestone and sandstone, pits and cracks for basalt and andesite;
  * seamless: everything wraps around the edges.
Wood, leaves, bedrock and glowstone are indexed pixel art (short hue-shifted ramps, shapes lit
from the top left, lone pixels merged).
Grass tops and leaves are greyscale: the game tints them by biome. A grass block's side is
soil plus `<name>_overlay.png`, the greyscale fringe that gets tinted.

Adding a rock: write a function like `granite`, add it to `TERRAIN`, add a block to
assets/blocks.toml and the name to app/src/textures.rs, and give it a place in
`WorldGen::rock_at`.
"""
import colorsys
import os
import zlib

import numpy as np
from PIL import Image

N = 32
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'assets', 'textures')
BAYER4 = (np.array([[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]]) + 0.5) / 16 - 0.5
BAYER = np.tile(BAYER4, (N // 4, N // 4))


def rng(name):
    return np.random.default_rng(zlib.crc32(name.encode()))


# ---- fields (all wrap around, so textures tile) ----

def noise(r, cells):
    """Smooth value noise, 0..1, on a `cells` x `cells` lattice stretched over the tile."""
    lat = r.random((cells, cells))
    t = (np.arange(N) + 0.5) / N * cells - 0.5
    i0 = np.floor(t).astype(int)
    f = t - i0
    f = f * f * (3 - 2 * f)
    a, b = i0 % cells, (i0 + 1) % cells
    rows = lat[a][:, a] * (1 - f)[None, :] + lat[a][:, b] * f[None, :]
    rows2 = lat[b][:, a] * (1 - f)[None, :] + lat[b][:, b] * f[None, :]
    return rows * (1 - f)[:, None] + rows2 * f[:, None]


def fbm(r, cells, octaves=3, gain=0.5):
    total, amp, norm = np.zeros((N, N)), 1.0, 0.0
    for _ in range(octaves):
        total += amp * noise(r, min(cells, N))
        norm += amp
        amp *= gain
        cells *= 2
    return total / norm


def voronoi(r, n, jitter=0.8, warp=None, stretch=(1.0, 1.0)):
    """Wrapping Voronoi over an n x n grid of jittered points: (f1, f2, cell id) per pixel."""
    cell = N / n
    pts = (np.stack(np.meshgrid(np.arange(n), np.arange(n), indexing='ij'), -1) + 0.5
           + (r.random((n, n, 2)) - 0.5) * jitter) * cell
    ids = r.permutation(n * n).reshape(n, n)
    ys, xs = np.mgrid[0:N, 0:N] + 0.5
    if warp is not None:
        xs, ys = xs + warp[0], ys + warp[1]
    f1 = np.full((N, N), 1e9)
    f2 = np.full((N, N), 1e9)
    idm = np.zeros((N, N), dtype=int)
    for i in range(n):
        for j in range(n):
            for oy in (-N, 0, N):
                for ox in (-N, 0, N):
                    py, px = pts[i, j, 0] + oy, pts[i, j, 1] + ox
                    d = np.sqrt(((xs - px) / stretch[0]) ** 2 + ((ys - py) / stretch[1]) ** 2)
                    closer = d < f1
                    f2 = np.where(closer, f1, np.minimum(f2, d))
                    idm = np.where(closer, ids[i, j], idm)
                    f1 = np.where(closer, d, f1)
    return f1, f2, idm


def shade(h):
    """Light from the top left: slopes facing it brighten (central differences, wrapping)."""
    hx = (np.roll(h, -1, 1) - np.roll(h, 1, 1)) * 0.5
    hy = (np.roll(h, -1, 0) - np.roll(h, 1, 0)) * 0.5
    return hx + hy


# ---- colour ----

def ramp(hue, sat, l_dark, l_light, n, dark_hue=None, light_hue=None):
    """n colours from dark to light. The hue moves from `dark_hue` in the shadows (cooler:
    toward blue for greys and greens, toward red-brown for warm colours) through `hue` to
    `light_hue` in the highlights (warmer, toward yellow). Saturation peaks in the middle."""
    dark_hue = hue if dark_hue is None else dark_hue
    light_hue = hue if light_hue is None else light_hue
    out = []
    for k in range(n):
        t = k / (n - 1)
        s = sat * (0.7 + 0.6 * (1 - abs(t - 0.5) * 2))
        l = l_dark + (l_light - l_dark) * t
        base = np.array(colorsys.hls_to_rgb(hue / 360.0, l, min(s, 1.0)))
        # Blend toward the shifted hue in RGB (a hue walk could pass through green or red).
        other, amount = (dark_hue, 1 - t * 2) if t < 0.5 else (light_hue, t * 2 - 1)
        shifted = np.array(colorsys.hls_to_rgb((other % 360) / 360.0, l, min(s * 1.3 + 0.03, 1.0)))
        out.append(base + (shifted - base) * 0.6 * amount)
    return np.clip(np.array(out), 0, 1)


def grey_ramp(l_dark, l_light, n):
    return np.array([[v, v, v] for v in np.linspace(l_dark, l_light, n)])


def quantize(v, n, dither=0.0, lo_hi=(3, 97)):
    """Values to ramp indices 0..n-1, spread over the value range, with ordered dithering."""
    lo, hi = np.percentile(v, lo_hi)
    t = (v - lo) / max(hi - lo, 1e-6)
    return np.clip(np.floor(t * n + dither * BAYER * 2 + 0.0), 0, n - 1).astype(int)


def neighbours(idx):
    return [np.roll(np.roll(idx, dy, 0), dx, 1) for dy in (-1, 0, 1) for dx in (-1, 0, 1) if dy or dx]


def clean(idx, passes=2, keep=None):
    """Merge lone pixels (no neighbour of the same colour) into their most common neighbour."""
    for _ in range(passes):
        nb = np.stack(neighbours(idx))
        same = (nb == idx[None]).sum(0)
        lone = same == 0
        if keep is not None:
            lone &= ~keep
        if not lone.any():
            break
        counts = np.stack([(nb == k).sum(0) for k in range(idx.max() + 1)])
        idx = np.where(lone, counts.argmax(0), idx)
    return idx


def separate(idx, n):
    """The darkest and lightest colours never touch: lift such darkest pixels one step."""
    near_light = np.zeros_like(idx, dtype=bool)
    for nb in neighbours(idx):
        near_light |= nb == n - 1
    return np.where((idx == 0) & near_light, 1, idx)


def paint(idx, colours):
    return colours[idx] * 255.0


def save(arr, name, mode='RGB'):
    os.makedirs(OUT, exist_ok=True)
    Image.fromarray(np.clip(np.round(arr), 0, 255).astype(np.uint8), mode).save(os.path.join(OUT, name + '.png'))


# ---- materials: each returns (ramp indices, colours) ----

def bark():
    r = rng('bark')
    # Plates between vertical furrows: columns of varying width.
    widths, x = [], 0
    while x < N:
        w = int(r.integers(3, 7))
        widths.append(min(w, N - x))
        x += w
    col_id = np.repeat(np.arange(len(widths)), widths)[:N]
    pos = np.concatenate([np.linspace(0, 1, w, endpoint=False) for w in widths])[:N]
    xs = np.tile(pos, (N, 1))
    ids = np.tile(col_id, (N, 1))
    # Plates break up vertically too.
    breaks = noise(r, 4)
    yv = (np.arange(N)[:, None] / N * 3 + breaks * 2 + ids * 0.37) % 1.0
    h = np.sin(xs * np.pi) ** 0.6 * 1.2 - (yv < 0.1) * 0.8 + 0.3 * fbm(r, 8, 1)
    v = 1.5 * shade(h) + 0.4 * h + 0.2 * ((ids * 0.618) % 1.0)
    n = 6
    idx = quantize(v, n, dither=0.2)
    furrow = xs < 0.18
    idx = np.where(furrow, np.minimum(idx, 1), idx)
    idx = clean(idx)
    return separate(idx, n), ramp(26, 0.32, 0.12, 0.45, n, dark_hue=12, light_hue=34)


def log_top():
    r = rng('log_top')
    ys, xs = np.mgrid[0:N, 0:N] + 0.5
    d = np.sqrt((xs - 16) ** 2 + (ys - 16) ** 2) + 1.2 * (fbm(r, 4, 2) - 0.5)
    ring = (np.floor(d / 2.0) % 2 == 0)
    wood = ramp(34, 0.42, 0.32, 0.62, 5, dark_hue=24, light_hue=40)
    barkc = ramp(26, 0.32, 0.12, 0.40, 4, dark_hue=12, light_hue=34)
    idx = np.where(ring, 3, 2)
    idx = np.where(d < 1.6, 1, idx)
    crack = (np.abs(np.arctan2(ys - 16, xs - 16) - 0.7) < 0.08) & (d > 3) & (d < 11)
    idx = np.where(crack, 1, idx)
    img = wood[idx]
    rim = np.maximum(np.abs(xs - 16), np.abs(ys - 16)) > 13.5
    bark_idx = np.clip((fbm(r, 8, 1) * 4).astype(int), 0, 3)
    img = np.where(rim[:, :, None], barkc[bark_idx], img)
    return img * 255.0


def leaves_indices():
    r = rng('leaves')
    f1, f2, ids = voronoi(r, 9, jitter=0.9, stretch=(1.0, 0.75))
    rad = 1.7 + 0.8 * ((ids * 0.618) % 1.0)
    inside = f1 < rad
    h = np.where(inside, 1 - f1 / rad, 0)
    v = 1.6 * shade(h) + 0.6 * h + 0.3 * ((ids * 0.37) % 1.0)
    n = 5
    idx = np.where(inside, 1 + quantize(v, n - 1), 0)
    idx = clean(idx)
    return separate(idx, n)


def bedrock():
    r = rng('bedrock')
    f1, f2, ids = voronoi(r, 5, warp=(3 * (fbm(r, 4, 2) - 0.5), 3 * (fbm(r, 4, 2) - 0.5)))
    h = np.clip((f2 - f1) / 3, 0, 1) + 0.6 * fbm(r, 8, 2)
    v = 1.8 * shade(h) + 0.5 * ((ids * 0.618) % 1.0) + 0.3 * h
    n = 5
    idx = quantize(v, n, dither=0.3)
    idx = clean(idx)
    return separate(idx, n), ramp(220, 0.06, 0.06, 0.42, n, dark_hue=235, light_hue=40)


def glowstone():
    r = rng('glowstone')
    f1, f2, ids = voronoi(r, 5, jitter=0.9)
    edge = f2 - f1
    v = 0.6 * ((ids * 0.618) % 1.0) + 0.8 * np.clip(1 - f1 / 4, 0, 1) + 1.0 * shade(np.clip(edge / 3, 0, 1))
    n = 5
    idx = np.where(edge < 0.9, 0, 1 + quantize(v, n - 1))
    idx = clean(idx)
    colours = np.array([[0.42, 0.24, 0.08], [0.80, 0.50, 0.18], [0.93, 0.70, 0.30], [0.99, 0.86, 0.48], [1.0, 0.96, 0.74]])
    return idx, colours


GRASS_GREYS = grey_ramp(0.30, 0.82, 5)

# ---- terrain, in the manner of Vintage Story: fine grain, low contrast ----

def lerp_stops(v, stops):
    """v (0..1) through a list of sRGB colours, evenly spaced, linearly blended."""
    stops = np.array(stops, dtype=float)
    t = np.clip(v, 0, 1) * (len(stops) - 1)
    i = np.minimum(t.astype(int), len(stops) - 2)
    f = (t - i)[..., None]
    return stops[i] * (1 - f) + stops[i + 1] * f


def mottle(r, coarse=4, fine=16, grain=0.25):
    """Soft patches + pixel grain, about 0..1 with most values near the middle."""
    v = 0.45 * fbm(r, coarse, 2) + 0.30 * noise(r, fine) + grain * r.random((N, N))
    return (v - v.min()) / (v.max() - v.min())


def specks(r, share, size=1):
    """A mask with `share` of the pixels set, as single pixels or small crumbs."""
    m = r.random((N, N)) < share
    if size > 1:
        m = m | np.roll(m & (r.random((N, N)) < 0.6), 1, 1) | np.roll(m & (r.random((N, N)) < 0.4), 1, 0)
    return m


def tone(img, amount, r):
    """A slight colour drift between pixels (warmer/cooler), so nothing looks flat-shaded."""
    d = (r.random((N, N)) - 0.5) * amount
    return img + np.stack([d, d * 0.3, -d], -1)


def soil():
    r = rng('soil')
    img = lerp_stops(mottle(r, grain=0.35), [(0.17, 0.13, 0.10), (0.25, 0.19, 0.15), (0.31, 0.25, 0.19), (0.38, 0.31, 0.24)])
    img = np.where(specks(r, 0.035)[..., None], img * 0.62, img)                       # holes
    img = np.where(specks(r, 0.03)[..., None], img * 0.4 + np.array([0.42, 0.38, 0.32]) * 0.6, img)  # grit
    img = np.where(specks(r, 0.008)[..., None], np.array([0.46, 0.43, 0.38]), img)     # small stones
    return tone(img, 0.03, r)


def sand():
    r = rng('sand')
    ripple = 0.5 + 0.5 * np.sin((np.arange(N)[:, None] / N * 3 + fbm(r, 4, 2) * 1.5) * 2 * np.pi)
    v = 0.55 * mottle(r, grain=0.45) + 0.25 * ripple + 0.2 * r.random((N, N))
    img = lerp_stops(v, [(0.66, 0.57, 0.39), (0.76, 0.68, 0.48), (0.84, 0.77, 0.57), (0.90, 0.84, 0.66)])
    img = np.where(specks(r, 0.03)[..., None], img * 0.78, img)
    img = np.where(specks(r, 0.02)[..., None], img * 0.3 + 0.7 * np.array([0.95, 0.92, 0.80]), img)
    return tone(img, 0.025, r)


def gravel():
    r = rng('gravel')
    f1, f2, ids = voronoi(r, 9, jitter=0.95)
    edge = np.clip((f2 - f1) / 1.6, 0, 1)
    kinds = np.array([(0.52, 0.51, 0.50), (0.40, 0.40, 0.42), (0.60, 0.56, 0.50), (0.47, 0.40, 0.34),
                      (0.66, 0.65, 0.63), (0.36, 0.38, 0.40), (0.55, 0.47, 0.42)])
    base = kinds[ids % len(kinds)]
    lit = 1.0 + 0.9 * shade(edge) + 0.12 * (r.random((N, N)) - 0.5)
    img = base * lit[..., None]
    img = np.where((edge < 0.3)[..., None], img * (0.45 + 0.5 * edge[..., None] / 0.3), img)
    return tone(img, 0.02, r)


def andesite():
    r = rng('andesite')
    v = mottle(r, coarse=3, grain=0.3)
    img = lerp_stops(v, [(0.33, 0.33, 0.34), (0.42, 0.42, 0.42), (0.49, 0.49, 0.48), (0.56, 0.56, 0.54)])
    # A couple of hairline cracks, and scattered pale crystals.
    ys, xs = np.mgrid[0:N, 0:N]
    for k in range(2):
        path = (r.integers(0, N) + np.cumsum(r.integers(-1, 2, N))) % N
        crack = (xs == path[:, None]) if k == 0 else (ys == path[None, :])
        crack &= (noise(r, 3) > 0.55) & (r.random((N, N)) < 0.8)
        img = np.where(crack[..., None], img * 0.68, img)
    img = np.where(specks(r, 0.03)[..., None], img * 1.22, img)
    img = np.where(specks(r, 0.03)[..., None], img * 0.8, img)
    return tone(img, 0.015, r)


def granite():
    r = rng('granite')
    # Three minerals in small interlocking crystals: pink feldspar, pale quartz, black mica.
    a, b = 0.5 * noise(r, 16) + 0.5 * r.random((N, N)), 0.45 * noise(r, 16) + 0.55 * r.random((N, N))
    img = lerp_stops(mottle(r, grain=0.3), [(0.60, 0.48, 0.45), (0.68, 0.56, 0.52), (0.74, 0.63, 0.58)])
    quartz = lerp_stops(r.random((N, N)), [(0.72, 0.71, 0.69), (0.84, 0.83, 0.80)])
    img = np.where((a > 0.70)[..., None], quartz, img)
    mica = lerp_stops(r.random((N, N)), [(0.20, 0.18, 0.19), (0.34, 0.31, 0.31)])
    img = np.where((b > 0.76)[..., None], mica, img)
    return tone(img, 0.02, r)


def basalt():
    r = rng('basalt')
    v = mottle(r, coarse=3, grain=0.35)
    img = lerp_stops(v, [(0.11, 0.12, 0.14), (0.16, 0.17, 0.19), (0.21, 0.22, 0.24), (0.26, 0.27, 0.28)])
    img = np.where(specks(r, 0.04, 2)[..., None], img * 0.55, img)     # gas pits
    img = np.where(specks(r, 0.02)[..., None], img * 1.45, img)
    f1, f2, _ = voronoi(r, 3, jitter=0.7)                              # faint column joints
    img = np.where(((f2 - f1) < 0.7)[..., None], img * 0.78, img)
    return tone(img, 0.012, r)


def strata(r, bands, wobble):
    """Layers: a value per row band, with the layers gently warped. 0..1."""
    levels = r.random(bands)
    y = np.arange(N)[:, None] / N * bands + wobble * (fbm(r, 4, 2) - 0.5) * bands / 4
    i0 = np.floor(y).astype(int)
    f = y - i0
    return levels[i0 % bands] * (1 - f) + levels[(i0 + 1) % bands] * f


def limestone():
    r = rng('limestone')
    v = 0.45 * strata(r, 5, 1.0) + 0.55 * mottle(r, grain=0.3)
    img = lerp_stops(v, [(0.58, 0.56, 0.49), (0.67, 0.65, 0.57), (0.74, 0.72, 0.64), (0.80, 0.78, 0.71)])
    img = np.where(specks(r, 0.035, 2)[..., None], img * 0.8, img)     # pits and shell marks
    img = np.where(specks(r, 0.02)[..., None], img * 1.1, img)
    return tone(img, 0.02, r)


def sandstone():
    r = rng('sandstone')
    v = 0.6 * strata(r, 9, 0.7) + 0.4 * mottle(r, grain=0.5)
    img = lerp_stops(v, [(0.55, 0.37, 0.23), (0.66, 0.47, 0.29), (0.74, 0.56, 0.36), (0.80, 0.64, 0.43)])
    img = np.where(specks(r, 0.03)[..., None], img * 0.82, img)
    return tone(img, 0.025, r)


def snow():
    r = rng('snow')
    v = 0.7 * fbm(r, 4, 3) + 0.3 * r.random((N, N))
    img = lerp_stops(v, [(0.80, 0.85, 0.92), (0.88, 0.91, 0.96), (0.94, 0.96, 0.98), (0.98, 0.99, 1.0)])
    return np.where(specks(r, 0.02)[..., None], np.array([1.0, 1.0, 1.0]), img)


def grass_top(name, length, density, contrast, bare):
    """Greyscale grass seen from above: a dense mat of short blades (strokes a pixel wide),
    lighter where they catch the light. `bare` lets some darker ground show (dry grass)."""
    r = rng(name)
    g = 0.76 + 0.10 * (fbm(r, 4, 2) - 0.5) + 0.06 * (r.random((N, N)) - 0.5)
    for _ in range(int(density * N * N)):
        y, x = r.integers(0, N, 2)
        n = int(r.integers(1, length + 1))
        lean = int(r.integers(-1, 2))
        level = 0.76 + contrast * (r.random() - 0.5)
        for k in range(n):
            yy, xx = (y - k) % N, (x + (lean * k) // 2) % N
            g[yy, xx] = level + 0.05 * k / max(n, 1)    # tips a touch lighter
    if bare > 0:
        patch = fbm(r, 5, 2) + 0.25 * r.random((N, N))
        g = np.where(patch > 1.0 - bare * 0.55, g * 0.84, g)
    return np.clip(g, 0.40, 1.0)


# name: (blade length, density, contrast, bare ground)
GRASSES = {
    'grass_top': (3, 0.55, 0.40, 0.0),
    'dry_grass_top': (2, 0.45, 0.34, 0.5),
    'lush_grass_top': (4, 0.65, 0.50, 0.0),
    'cold_grass_top': (2, 0.40, 0.28, 0.25),
}

# name: painter. `dirt` is the soil under every kind of grass.
TERRAIN = {
    'stone': andesite, 'granite': granite, 'basalt': basalt, 'limestone': limestone, 'sandstone': sandstone,
    'dirt': soil, 'sand': sand, 'gravel': gravel, 'snow': snow,
}


def fringe_lengths(name):
    """How far the grass hangs down each column of a grass block's side: most of the way, with
    a ragged lower edge, so grassy slopes read as grass and soil only shows at their feet."""
    r = rng(name + ' fringe')
    lens = np.round(15 + 8 * noise(r, 5)[0] + r.random(N) * 4).astype(int)
    for x in range(N):
        if r.random() < 0.2:
            lens[x] += int(r.integers(2, 6))
    return np.minimum(lens, N - 3)


def main():
    images = {}
    for name, fn in TERRAIN.items():
        images[name] = np.clip(fn(), 0, 1)
        save(images[name] * 255, name)
        print(f'{name:16} terrain')
    for name, fn in [('log_side', bark), ('bedrock', bedrock), ('glowstone', glowstone)]:
        idx, colours = fn()
        save(paint(idx, colours), name)
        print(f'{name:16} {len(colours)} colours')
    save(log_top(), 'log_top')
    save(paint(leaves_indices(), GRASS_GREYS)[:, :, 0], 'leaves', 'L')
    print(f'{"log_top":16} rings and bark\n{"leaves":16} greyscale')
    for name, (length, density, contrast, bare) in GRASSES.items():
        grey = grass_top(name, length, density, contrast, bare)
        save(grey * 255, name, 'L')
        # The side: soil (a little darker right under the grass) plus the fringe as an overlay.
        side = images['dirt'].copy()
        over = np.zeros((N, N, 4))
        for x, n in enumerate(fringe_lengths(name)):
            for y in range(min(n, N)):
                # Hanging grass is in its own shade: darker further down.
                g = grey[y, x] * (0.92 - 0.30 * y / N) * (0.75 if y >= n - 1 else 1.0)
                over[y, x] = (g * 255, g * 255, g * 255, 255)
            for y in range(n, min(n + 2, N)):
                side[y, x] *= 0.8
        base = name.replace('_top', '_side')
        save(side * 255, base)
        save(over, base + '_overlay', 'RGBA')
        print(f'{name:16} greyscale blades; {base} + overlay')


if __name__ == '__main__':
    main()
