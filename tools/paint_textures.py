#!/usr/bin/env python3
"""Paints the block textures in assets/textures: original 32 x 32 pixel art, slightly realistic
but stylised (between Vintage Story and Minecraft). Run from anywhere; needs numpy and Pillow.

    python3 tools/paint_textures.py

Every texture follows the same pixel-art rules:
  * a short colour ramp per material (5-7 colours), hue-shifted: shadows lean toward blue,
    highlights toward yellow;
  * shapes, not noise: rock plates, clods, pebbles, tufts of blades, bark plates, leaves;
    each shape lit from the top left (no pillow shading);
  * clusters: lone pixels are merged into their neighbours, and the darkest and lightest
    colours never touch;
  * texture by material: rough stone, sand and gravel get some dithering, leaves none;
  * seamless: everything wraps around the edges.
Grass tops and leaves are greyscale: the game tints them by biome. A grass block's side is
dirt plus `<name>_overlay.png`, the greyscale fringe that gets tinted.
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

def stone():
    r = rng('stone')
    warp = (3.0 * (fbm(r, 4, 2) - 0.5), 3.0 * (fbm(r, 4, 2) - 0.5))
    f1, f2, ids = voronoi(r, 3, warp=warp)
    edge = f2 - f1
    plate = np.clip(edge / 5.0, 0, 1) ** 0.8
    h = plate * 0.3 + 0.6 * fbm(r, 8, 2) + 0.45 * fbm(r, 16, 1)
    v = 1.2 * shade(h) + 0.4 * fbm(r, 4, 2) + 0.1 * ((ids * 0.618) % 1.0)
    n = 6
    idx = quantize(v, n, dither=0.45)
    idx = clean(idx)
    crack = (edge < 0.8) & ((ids % 2) == 0)
    idx = np.where(crack, np.minimum(idx, 1), idx)
    idx = separate(idx, n)
    return idx, ramp(40, 0.05, 0.28, 0.66, n, dark_hue=215, light_hue=48)


def dirt():
    r = rng('dirt')
    # Clods: rounded lumps of different sizes, lit from the top left, with dark gaps.
    f1, f2, ids = voronoi(r, 9, jitter=0.95)
    lump = np.clip((f2 - f1) / 2.0, 0, 1) ** 0.6
    g1, g2, gids = voronoi(r, 5, jitter=0.9)
    big = np.clip((g2 - g1) / 3.0, 0, 1) ** 0.6
    h = 0.8 * lump + 0.6 * big + 0.2 * fbm(r, 16, 1)
    v = 1.6 * shade(h) + 0.45 * h + 0.18 * ((ids * 0.618) % 1.0)
    n = 6
    idx = quantize(v, n, dither=0.2)
    idx = np.where((f2 - f1) < 0.5, np.minimum(idx, 1), idx)
    # Pebbles: a few small stones, lit from the top left.
    p1, p2, pid = voronoi(r, 6, jitter=0.9)
    pebble = (pid % 5 == 0) & (p1 < 1.5)
    plit = shade(np.where(pebble, np.clip(1.5 - p1, 0, 1.5), 0))
    idx = np.where(pebble, np.clip(np.round(3.5 + 4 * plit), 2, n - 1).astype(int), idx)
    idx = clean(idx)
    idx = separate(idx, n)
    return idx, ramp(28, 0.34, 0.16, 0.50, n, dark_hue=8, light_hue=36)


def grass_top_indices(name, tufts, blade_len, n=5, gaps=0.0):
    """Tufts of short blades seen from above, on a darker ground; greyscale ramp indices."""
    r = rng(name)
    idx = np.where(fbm(r, 4, 2) > 0.5, 1, 1)
    if gaps > 0:
        idx = np.where(fbm(r, 8, 2) < gaps, 0, idx)
    for _ in range(tufts):
        cx, cy = r.random() * N, r.random() * N
        lean = r.random() * 2 * np.pi
        for _ in range(r.integers(3, 6)):
            a = lean + (r.random() - 0.5) * 1.6
            length = blade_len * (0.7 + 0.6 * r.random())
            steps = int(length * 2) + 1
            for s in range(steps):
                t = s / max(steps - 1, 1)
                x = int(np.floor(cx + np.cos(a) * length * t)) % N
                y = int(np.floor(cy + np.sin(a) * length * t)) % N
                level = 2 + int(t * (n - 2) * 0.999)
                idx[y, x] = max(idx[y, x], level)
                # The blade's shadow, bottom right.
                sx, sy = (x + 1) % N, (y + 1) % N
                if idx[sy, sx] <= 1:
                    idx[sy, sx] = 0
    idx = clean(idx, passes=2)
    return separate(idx, n)


def sand():
    r = rng('sand')
    yy, xx = np.mgrid[0:N, 0:N]
    w = yy + 6.0 * (fbm(r, 2, 2) - 0.5) + xx * 0.25
    ripple = np.sin(w / N * 3 * 2 * np.pi) * 0.5 + 0.5
    v = 0.18 * ripple + 0.45 * fbm(r, 8, 2) + 0.4 * fbm(r, 32, 1)
    n = 5
    idx = quantize(v, n, dither=0.55)
    # A few darker grains, in pairs (no lone pixels).
    g = r.random((N, N)) < 0.025
    g |= np.roll(g, 1, 1)
    idx = np.where(g, np.maximum(idx - 2, 0), idx)
    idx = clean(idx, passes=1)
    return separate(idx, n), ramp(44, 0.45, 0.58, 0.86, n, dark_hue=32, light_hue=50)


def gravel():
    r = rng('gravel')
    f1, f2, ids = voronoi(r, 6, jitter=0.95)
    edge = (f2 - f1) * 0.5
    h = np.clip(edge / 1.8, 0, 1) ** 0.5
    base = ((ids * 0.618) % 1.0)
    v = 0.7 * base + 1.6 * shade(h) + 0.15 * fbm(r, 16, 1)
    n = 6
    idx = quantize(v, n, dither=0.3)
    idx = np.where(edge < 0.4, 0, np.maximum(idx, 1))
    idx = clean(idx)
    return separate(idx, n), ramp(30, 0.09, 0.24, 0.72, n, dark_hue=220, light_hue=42)


def snow():
    r = rng('snow')
    h = 0.7 * fbm(r, 4, 2) + 0.3 * fbm(r, 8, 2)
    v = 2.2 * shade(h) + 0.4 * h
    n = 4
    idx = quantize(v, n, dither=0.15, lo_hi=(2, 99))
    idx = np.maximum(idx, np.where(fbm(r, 8, 1) > 0.45, 1, 0))
    idx = clean(idx, passes=3)
    colours = np.array([[0.74, 0.80, 0.92], [0.84, 0.89, 0.97], [0.92, 0.95, 0.99], [0.98, 0.99, 1.0]])
    return idx, colours


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
GRASSES = {
    # name: (tufts, blade length, bare ground fraction)
    'grass_top': (240, 1.9, 0.0),
    'dry_grass_top': (170, 2.6, 0.3),
    'lush_grass_top': (280, 2.1, 0.0),
    'cold_grass_top': (150, 1.6, 0.35),
}


def fringe_lengths(name):
    r = rng(name + ' fringe')
    base = 5 + 3 * noise(r, 4)[0]
    lens = np.round(base + r.random(N) * 2).astype(int)
    for x in range(N):
        if r.random() < 0.18:
            lens[x] += int(r.integers(2, 5))
    return lens


def main():
    for name, fn in [('stone', stone), ('dirt', dirt), ('sand', sand), ('gravel', gravel), ('snow', snow),
                     ('log_side', bark), ('bedrock', bedrock), ('glowstone', glowstone)]:
        idx, colours = fn()
        save(paint(idx, colours), name)
        if name == 'dirt':
            dirt_idx, dirt_colours = idx, colours
        print(f'{name:16} {len(colours)} colours')
    save(log_top(), 'log_top')
    save(paint(leaves_indices(), GRASS_GREYS)[:, :, 0], 'leaves', 'L')
    print(f'{"log_top":16} rings and bark\n{"leaves":16} greyscale')
    for name, (tufts, blade, gaps) in GRASSES.items():
        idx = grass_top_indices(name, tufts, blade, gaps=gaps)
        grey = paint(idx, GRASS_GREYS)[:, :, 0]
        save(grey, name, 'L')
        # The side: dirt (one step darker under the fringe) plus the fringe as a tinted overlay.
        lens = fringe_lengths(name)
        side_idx = dirt_idx.copy()
        over = np.zeros((N, N, 4))
        for x, n in enumerate(lens):
            for y in range(min(n, N)):
                # Blades from the top texture, darker toward the fringe's ragged lower edge.
                k = idx[y, x] if y < n - 1 else max(idx[y, x] - 2, 0)
                k = max(k, 1)
                g = GRASS_GREYS[k][0] * 255
                over[y, x] = (g, g, g, 255)
            if n < N:
                side_idx[n, x] = max(side_idx[n, x] - 1, 0)
        base = name.replace('_top', '_side')
        save(paint(side_idx, dirt_colours), base)
        save(over, base + '_overlay', 'RGBA')
        print(f'{name:16} greyscale tufts; {base} + overlay')


if __name__ == '__main__':
    main()
