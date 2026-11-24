#!/usr/bin/env python3
"""Paints the plant sprites in assets/textures: grass tufts, ferns, flowers, bushes and
reeds, 32 x 32 RGBA with transparent background, drawn bottom-anchored as Vintage Story's
cross-shaped plants are. Original pixel art, made here; none copied from any game.

Grasses, the fern and the leafy bush are greyscale: the game tints them with the biome's
grass colour, as it does grass tops and leaves. The rest have their own colours.

    python3 tools/paint_plants.py     (needs numpy and Pillow)
"""
import math
import os
import random

import numpy as np
from PIL import Image

N = 32
OUT = os.path.join(os.path.dirname(__file__), '..', 'assets', 'textures')


class Canvas:
    def __init__(self):
        self.px = np.zeros((N, N, 4), np.uint8)

    def put(self, x, y, rgb, a=255):
        x, y = int(round(x)), int(round(y))
        if 0 <= x < N and 0 <= y < N:
            self.px[y, x, :3] = [max(0, min(255, int(c))) for c in rgb]
            self.px[y, x, 3] = a

    def save(self, name):
        Image.fromarray(self.px, 'RGBA').save(os.path.join(OUT, name + '.png'))


def grey(v):
    return (v, v, v)


def blade(c, rnd, x0, length, lean, colour, width=1, tip=None):
    """A grass blade from the bottom row up, curving over by `lean` pixels at its tip.
    `colour(t)` gives its colour from base (t = 0) to tip (t = 1)."""
    steps = max(2, int(length * 1.5))
    for i in range(steps + 1):
        t = i / steps
        x = x0 + lean * t * t
        y = N - 1 - t * length
        col = colour(t)
        c.put(x, y, col)
        if width > 1 and t < 0.6:
            c.put(x + 1, y, [v * 0.9 for v in col])
    if tip:
        c.put(x0 + lean, N - 1 - length, tip)


def grass(name, seed, count, lo, hi, tinted=True, base=(110, 150), top=(185, 235), heads=0):
    rnd = random.Random(seed)
    c = Canvas()
    for _ in range(count):
        x0 = rnd.uniform(3, N - 4)
        length = rnd.uniform(lo, hi)
        lean = rnd.uniform(-6, 6) * (length / hi)
        b, t = rnd.uniform(*base), rnd.uniform(*top)
        if tinted:
            col = lambda u, b=b, t=t: grey(b + (t - b) * u)
        else:
            col = lambda u, b=b, t=t: (b + (t - b) * u, (b + (t - b) * u) * 0.82, (b + (t - b) * u) * 0.45)
        blade(c, rnd, x0, length, lean, col, width=2 if rnd.random() < 0.3 else 1)
    # Seed heads on the tallest stems.
    for _ in range(heads):
        x0 = rnd.uniform(6, N - 7)
        length = rnd.uniform(hi * 0.85, hi)
        lean = rnd.uniform(-3, 3)
        blade(c, rnd, x0, length, lean, lambda u: grey(150 + 70 * u))
        tx, ty = x0 + lean, N - 1 - length
        for k in range(4):
            c.put(tx + rnd.choice([-1, 0, 1]), ty + k, grey(225 - 12 * k))
    c.save(name)


def fern(name, seed):
    rnd = random.Random(seed)
    c = Canvas()
    fronds = 7
    for k in range(fronds):
        ang = math.radians(-42 + 84 * k / (fronds - 1) + rnd.uniform(-6, 6))
        length = rnd.uniform(27, 32) * (1.0 - 0.15 * abs(math.sin(ang)))
        x, y = 16 + rnd.uniform(-1.5, 1.5), N - 1
        dx, dy = math.sin(ang), -math.cos(ang)
        steps = int(length * 2)
        for i in range(steps):
            t = i / steps
            # The frond arches over as it grows out.
            dy2 = dy + 0.28 * t * t
            px, py = x + dx * length * t, y + dy2 * length * t * 0.95
            v = 120 + 95 * t
            c.put(px, py, grey(v))
            # Leaflets on both sides, shorter toward the tip.
            if i % 2 == 0 and 0.12 < t < 0.97:
                ll = (1 - t) * 5 + 1
                nx, ny = -dy2, dx
                norm = math.hypot(nx, ny) or 1
                nx, ny = nx / norm, ny / norm
                for s in (-1, 1):
                    for j in range(1, int(ll) + 1):
                        c.put(px + s * nx * j, py + s * ny * j + j * 0.35, grey(v + 15 - j * 4))
    c.save(name)


def bush(name, seed, leaf, berries=None, holes=0.22):
    """A round leafy bush: clusters of leaves over a circle, see-through between them."""
    rnd = random.Random(seed)
    c = Canvas()
    cx, cy, r = 16, 18, 14.5
    for y in range(N):
        for x in range(N):
            d = math.hypot((x - cx) / r, (y - cy) / (r * 0.95))
            if d > 1.0:
                continue
            if rnd.random() < holes * (0.6 + 0.8 * d):
                continue
            # Lighter on top, darker underneath; leaf-sized blotches.
            k = 0.55 + 0.45 * (1 - (y - (cy - r)) / (2 * r)) + rnd.uniform(-0.12, 0.12)
            if ((x // 2 + y // 3) % 3 == 0) and rnd.random() < 0.5:
                k *= 0.8
            c.put(x, y, [v * k for v in leaf])
    # Twigs at the base.
    for x in (13, 16, 19):
        for y in range(N - 4, N):
            c.put(x + (y - N + 4) * (0.3 if x < 16 else -0.3 if x > 16 else 0), y, (78, 58, 38))
    if berries:
        for _ in range(9):
            x, y = rnd.randint(5, 26), rnd.randint(7, 26)
            if c.px[y, x, 3] == 0:
                continue
            for ox, oy in ((0, 0), (1, 0), (0, 1), (1, 1)):
                c.put(x + ox, y + oy, [v * (1.15 if (ox, oy) == (0, 0) else 0.85) for v in berries])
    c.save(name)


def flower(name, seed, petal, centre, heads=3, spike=False):
    rnd = random.Random(seed)
    c = Canvas()
    stem = lambda u: (62 + 30 * u, 108 + 35 * u, 40 + 12 * u)
    # Leaves at the bottom.
    for _ in range(6):
        blade(c, rnd, rnd.uniform(7, 24), rnd.uniform(5, 10), rnd.uniform(-5, 5), stem)
    for h in range(heads):
        x0 = 8 + 16 * (h + 0.5) / heads + rnd.uniform(-3, 3)
        length = rnd.uniform(14, 25)
        lean = rnd.uniform(-3, 3)
        blade(c, rnd, x0, length, lean, stem)
        tx, ty = x0 + lean, N - 1 - length
        if spike:
            # A spike of small flowers (lavender).
            for k in range(7):
                for s in (-1, 0, 1) if k % 2 else (0,):
                    c.put(tx + s, ty + k, [v * (1.1 - 0.05 * k) for v in petal])
        else:
            for ox in range(-2, 3):
                for oy in range(-2, 3):
                    d = abs(ox) + abs(oy)
                    if d <= 3 and not (abs(ox) == 2 and abs(oy) == 2):
                        k = 1.0 if d <= 1 else 0.88 if d == 2 else 0.75
                        c.put(tx + ox, ty + oy, [v * k for v in petal])
            c.put(tx, ty, centre)
    c.save(name)


def dead_bush(name, seed):
    rnd = random.Random(seed)
    c = Canvas()

    def branch(x, y, ang, length, depth):
        steps = int(length * 1.5)
        for i in range(steps):
            t = i / steps
            px, py = x + math.sin(ang) * length * t, y - math.cos(ang) * length * t
            c.put(px, py, (120 - 25 * depth + rnd.uniform(-8, 8), 88 - 18 * depth, 52 - 8 * depth))
        if depth < 3:
            ex, ey = x + math.sin(ang) * length, y - math.cos(ang) * length
            for s in (-1, 1):
                branch(ex, ey, ang + s * rnd.uniform(0.35, 0.7), length * rnd.uniform(0.5, 0.7), depth + 1)

    for ang in (-0.5, 0.05, 0.55):
        branch(16, N - 1, ang + rnd.uniform(-0.1, 0.1), rnd.uniform(8, 11), 0)
    c.save(name)


def reeds(name, seed):
    rnd = random.Random(seed)
    c = Canvas()
    for _ in range(9):
        x0 = rnd.uniform(4, N - 5)
        length = rnd.uniform(18, 30)
        lean = rnd.uniform(-2, 2)
        blade(c, rnd, x0, length, lean, lambda u: (88 + 50 * u, 120 + 40 * u, 60 + 20 * u))
    for _ in range(4):
        x0 = rnd.uniform(7, N - 8)
        length = rnd.uniform(22, 29)
        lean = rnd.uniform(-1, 1)
        blade(c, rnd, x0, length, lean, lambda u: (110 + 40 * u, 122 + 30 * u, 70))
        tx, ty = x0 + lean, N - 1 - length
        for k in range(1, 7):
            for s in (0, 1):
                c.put(tx + s, ty + k, (96 - 6 * k + 10 * s, 62 - 3 * k, 36))
    c.save(name)


def heather(name, seed):
    rnd = random.Random(seed)
    c = Canvas()
    for _ in range(16):
        x0 = rnd.uniform(3, N - 4)
        length = rnd.uniform(6, 14)
        lean = rnd.uniform(-4, 4)
        blade(c, rnd, x0, length, lean, lambda u: (70 + 30 * u, 92 + 20 * u, 58))
        tx, ty = x0 + lean, N - 1 - length
        for k in range(4):
            c.put(tx + rnd.choice([-1, 0, 1]), ty + k, (196 - 10 * k, 112 - 6 * k, 168 - 8 * k))
    c.save(name)



# ---- Grass: four varieties, four heights, every blade one texel wide on whole pixels ----
# (32 texels to the block, like every texture in the game.)

HEIGHTS = {'short': (7, 10), 'normal': (12, 16), 'medium': (18, 23), 'tall': (25, 31)}
COUNTS = {'short': 20, 'normal': 17, 'medium': 15, 'tall': 13}


def pixel_blade(c, x0, length, lean, colour, wide=0):
    """A blade on whole pixels from the bottom row up: `lean` pixels over at its tip (growing
    with the square of the height, so it arches), each row coloured by `colour(t)`; `wide` > 0
    doubles its lower part (that share of its height) for broad-leaved grass."""
    prev = None
    for r in range(length):
        t = r / max(1, length - 1)
        x = x0 + int(round(lean * t * t))
        y = N - 1 - r
        col = colour(t)
        # No gaps where it leans more than a pixel per row.
        if prev is not None and abs(x - prev) > 1:
            step = 1 if x > prev else -1
            for xx in range(prev + step, x, step):
                c.put(xx, y + 1, col)
        c.put(x, y, col)
        if t < wide:
            c.put(x + (1 if lean >= 0 else -1), y, [v * 0.88 for v in col])
        prev = x


def grass_set(prefix_names, seed, kind, suffix=''):
    """One variety at its four heights; `prefix_names` maps height to file name."""
    for hi, (height, name) in enumerate(prefix_names.items()):
        rnd = random.Random(seed * 10 + hi)
        c = Canvas()
        lo_h, hi_h = HEIGHTS[height]
        n = COUNTS[height]
        if kind == 'frost':
            n = n * 2 // 3
        heads = []
        for k in range(n):
            # Spread evenly across the sprite with some jitter, so it reads as a tuft.
            x0 = int(2 + (27 * (k + rnd.random())) / n)
            length = rnd.randint(lo_h, hi_h) if rnd.random() < 0.7 else rnd.randint(max(3, lo_h - 4), lo_h)
            lean = rnd.choice([-1, 1]) * rnd.randint(0, max(1, length // 4))
            if kind == 'meadow':
                b, t = rnd.randint(105, 135), rnd.randint(190, 235)
                colour = lambda u, b=b, t=t: grey(b + (t - b) * u)
                pixel_blade(c, x0, length, lean, colour)
            elif kind == 'lush':
                b, t = rnd.randint(85, 115), rnd.randint(185, 225)
                lean = rnd.choice([-1, 1]) * rnd.randint(length // 5, max(2, length // 3))
                colour = lambda u, b=b, t=t: grey(b + (t - b) * u)
                pixel_blade(c, x0, length, lean, colour, wide=0.6)
            elif kind == 'dry':
                b, t = rnd.uniform(0.6, 0.72), rnd.uniform(0.9, 1.0)
                colour = lambda u, b=b, t=t: [v * (b + (t - b) * u) for v in (226, 198, 132)]
                pixel_blade(c, x0, length, lean, colour)
            else:  # frost
                colour = lambda u: (98 + 70 * u, 116 + 70 * u, 96 + 80 * u) if u < 0.8 else (206, 222, 222)
                pixel_blade(c, x0, length, lean, colour)
            if length >= lo_h and height in ('medium', 'tall') and kind in ('meadow', 'dry') and rnd.random() < 0.3:
                heads.append((x0 + lean, N - length))
        # Seed heads on some of the tallest stems: a few pixels hanging off the tip.
        for (hx, hy) in heads[:3]:
            col = grey(232) if kind == 'meadow' else (236, 214, 150)
            for k in range(4):
                c.put(hx + (k % 2), hy + k, [v * (1.0 - 0.06 * k) for v in col])
        c.save(name + suffix)


# ---- Tall grass over two blocks: 1.5 and 2 blocks high, drawn on a 32 x 64 canvas and cut
# into a bottom and a top sprite (still 32 x 32, 32 texels to the block), which the game
# stacks as two blocks, as Minecraft does its tall grass.

def tall_grass_set(prefix, seed, kind, suffix=''):
    for (size, lo_h, hi_h, n) in (('high', 38, 47, 13), ('giant', 52, 62, 12)):
        rnd = random.Random(seed * 100 + len(size))
        px = np.zeros((2 * N, N, 4), np.uint8)

        def put(x, y, rgb):
            x, y = int(round(x)), int(round(y))
            if 0 <= x < N and 0 <= y < 2 * N:
                px[y, x, :3] = [max(0, min(255, int(c))) for c in rgb]
                px[y, x, 3] = 255

        heads = []
        for k in range(n):
            x0 = int(3 + (25 * (k + rnd.random())) / n)
            length = rnd.randint(lo_h, hi_h) if rnd.random() < 0.75 else rnd.randint(lo_h // 2, lo_h)
            lean = rnd.choice([-1, 1]) * rnd.randint(1, max(2, length // 7))
            if kind == 'meadow':
                b, t = rnd.randint(100, 130), rnd.randint(190, 235)
                colour = lambda u, b=b, t=t: grey(b + (t - b) * u)
            elif kind == 'lush':
                b, t = rnd.randint(80, 110), rnd.randint(180, 225)
                colour = lambda u, b=b, t=t: grey(b + (t - b) * u)
            else:
                b, t = rnd.uniform(0.58, 0.7), rnd.uniform(0.9, 1.0)
                colour = lambda u, b=b, t=t: [v * (b + (t - b) * u) for v in (226, 198, 132)]
            prev = None
            for r in range(length):
                u = r / max(1, length - 1)
                x = x0 + int(round(lean * u * u))
                y = 2 * N - 1 - r
                col = colour(u)
                if prev is not None and abs(x - prev) > 1:
                    step = 1 if x > prev else -1
                    for xx in range(prev + step, x, step):
                        put(xx, y + 1, col)
                put(x, y, col)
                if kind == 'lush' and u < 0.5:
                    put(x + (1 if lean >= 0 else -1), y, [v * 0.88 for v in col])
                prev = x
            if kind != 'lush' and length >= lo_h and rnd.random() < 0.35:
                heads.append((x0 + lean, 2 * N - length))
        for (hx, hy) in heads[:4]:
            col = grey(232) if kind == 'meadow' else (236, 214, 150)
            for k in range(5):
                put(hx + (k % 2), hy + k, [v * (1.0 - 0.05 * k) for v in col])
        for half, rows in (('top', px[:N]), ('bottom', px[N:])):
            Image.fromarray(np.ascontiguousarray(rows), 'RGBA').save(os.path.join(OUT, f'{prefix}_{size}_{half}{suffix}.png'))


# ---- More flowers and plants: wildflowers by biome, mushrooms, berry bushes, and two-block
# sunflowers, lilacs, foxgloves and ferns (32 x 64, cut in two like the tall grass).

class Tall:
    """A canvas `blocks` blocks high (32 texels each), saved as one sprite per block."""
    def __init__(self, blocks=2):
        self.h = N * blocks
        self.px = np.zeros((self.h, N, 4), np.uint8)

    def put(self, x, y, rgb, a=255):
        x, y = int(round(x)), int(round(y))
        if 0 <= x < N and 0 <= y < self.h:
            self.px[y, x, :3] = [max(0, min(255, int(c))) for c in rgb]
            self.px[y, x, 3] = a

    def save(self, name):
        if self.h == N:
            Image.fromarray(self.px, 'RGBA').save(os.path.join(OUT, name + '.png'))
            return
        for half, rows in (('top', self.px[:N]), ('bottom', self.px[N:])):
            Image.fromarray(np.ascontiguousarray(rows), 'RGBA').save(os.path.join(OUT, f'{name}_{half}.png'))


STEM = lambda u: (58 + 34 * u, 104 + 40 * u, 38 + 14 * u)


def stem(c, x0, length, lean, colour=STEM):
    """A whole-pixel stem from the bottom row; returns its tip."""
    prev = None
    for r in range(length):
        t = r / max(1, length - 1)
        x = x0 + int(round(lean * t * t))
        y = c.h - 1 - r
        if prev is not None and abs(x - prev) > 1:
            st = 1 if x > prev else -1
            for xx in range(prev + st, x, st):
                c.put(xx, y + 1, colour(t))
        c.put(x, y, colour(t))
        prev = x
    return prev, c.h - length


def leaves_at_base(c, rnd, n=6, lo=4, hi=9, colour=STEM):
    for _ in range(n):
        x0 = rnd.randint(6, 25)
        length = rnd.randint(lo, hi)
        lean = rnd.choice([-1, 1]) * rnd.randint(2, 5)
        stem(c, x0, length, lean, lambda u: [v * (0.85 + 0.2 * u) for v in colour(u)])


def head(c, tx, ty, petal, centre, shape, rnd):
    if shape == 'round':          # five-petal flower, 5 px across
        for ox in range(-2, 3):
            for oy in range(-2, 3):
                d = abs(ox) + abs(oy)
                if d <= 3 and not (abs(ox) == 2 and abs(oy) == 2):
                    c.put(tx + ox, ty + oy, [v * (1.0 if d <= 1 else 0.88 if d == 2 else 0.76) for v in petal])
        c.put(tx, ty, centre)
    elif shape == 'small':        # tiny 3 px flower
        for ox, oy in ((0, -1), (-1, 0), (1, 0), (0, 1)):
            c.put(tx + ox, ty + oy, petal)
        c.put(tx, ty, centre)
    elif shape == 'cup':          # tulip: a closed cup, 3 wide, 4 high, pointed top
        for oy in range(4):
            for ox in (-1, 0, 1):
                if oy == 0 and ox == 0:
                    continue
                k = 1.08 if ox == -1 else 0.95 if ox == 0 else 0.8
                c.put(tx + ox, ty + oy - 3, [v * k * (0.9 + 0.04 * oy) for v in petal])
        c.put(tx - 1, ty - 4, [v * 1.05 for v in petal])
        c.put(tx + 1, ty - 4, [v * 0.85 for v in petal])
    elif shape == 'bell':         # drooping bells down one side of an arched stem
        for k in range(4):
            bx, by = tx + 1 + (k % 2), ty + 2 + 2 * k
            c.put(bx, by, petal)
            c.put(bx + 1, by, [v * 0.8 for v in petal])
            c.put(bx, by + 1, [v * 0.9 for v in petal])
            c.put(bx + 1, by + 1, [v * 0.7 for v in petal])
    elif shape == 'thistle':      # spiky ball with a purple brush on top
        for ox in (-1, 0, 1):
            for oy in (0, 1):
                c.put(tx + ox, ty + oy, (92, 120, 62) if (ox + oy) % 2 else (70, 98, 50))
        for ox in (-2, -1, 0, 1, 2):
            c.put(tx + ox, ty - 1, [v * (1.0 - 0.06 * abs(ox)) for v in petal])
        for ox in (-1, 0, 1):
            c.put(tx + ox, ty - 2, petal)
        c.put(tx - 2, ty + 1, (96, 128, 66)); c.put(tx + 2, ty + 1, (96, 128, 66))
    elif shape == 'orchid':       # three broad petals and a lip
        for ox, oy, k in ((-2, -1, 0.9), (-1, -1, 1), (1, -1, 1), (2, -1, 0.9), (0, -2, 1.05), (0, -3, 0.95),
                          (-1, 0, 0.85), (1, 0, 0.85), (0, 1, 0.75)):
            c.put(tx + ox, ty + oy, [v * k for v in petal])
        c.put(tx, ty, centre); c.put(tx, ty - 1, centre)
    elif shape == 'fluff':        # cotton grass: a white tuft
        for ox, oy in ((0, 0), (-1, 0), (1, 0), (0, -1), (-1, -1), (0, 1), (1, -1)):
            c.put(tx + ox, ty + oy, [v - rnd.randint(0, 22) for v in petal])


def wildflower(name, seed, petal, centre, shape='round', heads=3, lo=12, hi=22, leaves=6, blocks=1, jitter=0.0):
    rnd = random.Random(seed)
    c = Tall(blocks)
    leaves_at_base(c, rnd, leaves)
    for h in range(heads):
        x0 = int(7 + 18 * (h + 0.5) / heads + rnd.uniform(-2.5, 2.5))
        length = rnd.randint(lo, hi)
        lean = rnd.choice([-1, 1]) * rnd.randint(0, 3)
        tx, ty = stem(c, x0, length, lean)
        col = [max(0, min(255, v + rnd.uniform(-jitter, jitter))) for v in petal]
        head(c, tx, ty, col, centre, shape, rnd)
    c.save(name)


def spike(name, seed, petal, lo, hi, heads=2, bells=False, blocks=1, leaves=7):
    """Tall flower spikes (goldenrod, foxglove, lupin): flowers along the top part of the stem."""
    rnd = random.Random(seed)
    c = Tall(blocks)
    leaves_at_base(c, rnd, leaves, 5, 11)
    for h in range(heads):
        x0 = int(9 + 14 * (h + 0.5) / heads + rnd.uniform(-2, 2))
        length = rnd.randint(lo, hi)
        tx, ty = stem(c, x0, length, rnd.choice([-1, 1]) * rnd.randint(0, 2))
        span = int(length * 0.45)
        for k in range(span):
            w = 1 if k < 2 else 2 if k < span * 0.6 else 1
            shade = 1.08 - 0.25 * k / span
            for ox in range(-w + (1 if bells and k % 2 else 0), w + 1):
                if bells and ox == 0 and k % 2 == 0:
                    continue
                c.put(tx + ox, ty + k, [v * shade * (0.86 if ox > 0 else 1.0) for v in petal])
            if bells and k % 3 == 1:
                c.put(tx + w, ty + k + 1, [v * 0.55 for v in petal])
    c.save(name)


def mushrooms(name, seed, cap, spots=None):
    rnd = random.Random(seed)
    c = Tall(1)
    for (x, h, r) in ((10, 7, 4), (20, 10, 5), (25, 4, 2)):
        x += rnd.randint(-1, 1)
        for y in range(N - h, N):
            c.put(x, y, (226, 218, 196)); c.put(x + 1, y, (196, 186, 164))
        top = N - h - 1
        for oy in range(-((r + 1) // 2), 1):
            w = r - (0 if oy == 0 else -oy)
            for ox in range(-w, w + 2):
                k = 1.1 if oy < 0 and ox < 0 else 0.9 if ox > w // 2 else 1.0
                c.put(x + ox, top + oy, [v * k for v in cap])
        for ox in range(-r + 1, r + 1):
            c.put(x + ox, top + 1, [v * 0.6 for v in cap])
        if spots:
            for _ in range(r):
                c.put(x + rnd.randint(-r + 1, r), top - rnd.randint(0, (r + 1) // 2), spots)
    c.save(name)


def clover(name, seed):
    """Low clover, greyscale (tinted like the grass), with white flower heads."""
    rnd = random.Random(seed)
    c = Tall(1)
    for _ in range(14):
        x0, length = rnd.randint(3, 28), rnd.randint(3, 8)
        tx, ty = stem(c, x0, length, rnd.choice([-1, 1]) * rnd.randint(0, 2), lambda u: grey(130 + 50 * u))
        for ox, oy in ((-1, 0), (1, 0), (0, -1), (-1, -1), (1, -1), (0, 0)):
            c.put(tx + ox, ty + oy - 1, grey(rnd.randint(170, 215)))
    for _ in range(4):
        x0 = rnd.randint(5, 26)
        tx, ty = stem(c, x0, rnd.randint(9, 12), 0, lambda u: grey(140 + 50 * u))
        for ox, oy in ((0, 0), (-1, 0), (1, 0), (0, -1), (0, 1)):
            c.put(tx + ox, ty + oy - 1, (236, 232, 222) if (ox + oy) % 2 == 0 else (222, 206, 212))
    c.save(name)


def sunflower(name, seed):
    rnd = random.Random(seed)
    c = Tall(2)
    tx, ty = stem(c, 15, 50, 2, lambda u: (70 + 30 * u, 116 + 26 * u, 44))
    for x in range(14, 18):  # a thicker stalk
        pass
    for y in range(c.h - 48, c.h):
        c.put(16 + (1 if y < c.h - 30 else 0), y, (60, 100, 40))
    for (ly, side) in ((c.h - 14, -1), (c.h - 24, 1), (c.h - 34, -1)):
        for k in range(7):
            for w in range(-1, 2):
                c.put(16 + side * (k + 1), ly - k // 2 + w + (k * k) // 12, [v * (0.85 + 0.03 * k) for v in (70, 124, 48)])
    cx, cy = tx + 1, ty + 1
    for oy in range(-7, 8):
        for ox in range(-7, 8):
            d = math.hypot(ox, oy)
            if d <= 3.4:
                c.put(cx + ox, cy + oy, (92, 58, 30) if (ox + oy) % 2 else (70, 42, 22))
            elif d <= 7.2 and (int(math.degrees(math.atan2(oy, ox)) + 360) // 24) % 2 == (0 if d < 6 else 1) or 3.4 < d <= 5.2:
                k = 1.08 if oy < 0 else 0.88
                c.put(cx + ox, cy + oy, [v * k for v in (250, 200, 40)])
    c.save(name)


def tall_fern(name, seed):
    """A two-block fern (greyscale, grass-tinted): long arching fronds."""
    rnd = random.Random(seed)
    c = Tall(2)
    for k in range(5):
        ang = math.radians(-40 + 80 * k / 4 + rnd.uniform(-4, 4))
        length = rnd.uniform(48, 60) * (1.0 - 0.2 * abs(math.sin(ang)))
        dx, dy = math.sin(ang), -math.cos(ang)
        steps = int(length * 2)
        for i in range(steps):
            t = i / steps
            dy2 = dy + 0.22 * t * t
            px, py = 16 + dx * length * t, c.h - 1 + dy2 * length * t * 0.95
            v = 115 + 100 * t
            c.put(px, py, grey(v))
            if i % 4 == 0 and 0.1 < t < 0.97:
                ll = (1 - t) * 4 + 1
                nx, ny = -dy2, dx
                nn = math.hypot(nx, ny) or 1
                for s in (-1, 1):
                    for j in range(1, int(ll) + 1):
                        c.put(px + s * nx / nn * j, py + s * ny / nn * j + j * 0.4, grey(v + 12 - j * 4))
    c.save(name)


def flowering_bush(name, seed, leaf, flower, blocks=1, cones=False):
    """A bush (rose, lilac) with flowers over it; `cones` gives lilac's upright clusters."""
    rnd = random.Random(seed)
    c = Tall(blocks)
    h = c.h
    cx, cy, rx, ry = 16, h - h * 0.45, 14.5, h * 0.42
    for y in range(h):
        for x in range(N):
            d = math.hypot((x - cx) / rx, (y - cy) / ry)
            if d > 1.0 or rnd.random() < 0.22 * (0.6 + 0.8 * d):
                continue
            k = 0.55 + 0.45 * (1 - (y - (cy - ry)) / (2 * ry)) + rnd.uniform(-0.12, 0.12)
            c.put(x, y, [v * k for v in leaf])
    for x in (13, 16, 19):
        for y in range(h - 4, h):
            c.put(x, y, (78, 58, 38))
    n = 10 if blocks == 1 else 16
    for _ in range(n):
        a, r = rnd.uniform(0, 2 * math.pi), math.sqrt(rnd.random()) * 0.85
        x, y = int(cx + math.cos(a) * rx * r), int(cy + math.sin(a) * ry * r * 0.9)
        if cones:
            for k in range(6):
                for ox in range(-(1 if k > 1 else 0), (2 if k > 2 else 1)):
                    c.put(x + ox, y + k - 5, [v * (1.1 - 0.06 * k) for v in flower])
        else:
            for ox, oy, kk in ((0, 0, 1.1), (1, 0, 0.95), (0, 1, 0.85), (1, 1, 0.75), (-1, 0, 0.9), (0, -1, 1.0)):
                c.put(x + ox, y + oy, [v * kk for v in flower])
            c.put(x, y, (246, 220, 120))
    c.save(name)


def big_leaf(name, seed):
    """Jungle undergrowth: a few broad, drooping leaves (greyscale, grass-tinted)."""
    rnd = random.Random(seed)
    c = Tall(1)
    for k in range(5):
        ang = math.radians(-60 + 120 * k / 4 + rnd.uniform(-8, 8))
        length = rnd.uniform(22, 28)
        for i in range(int(length * 2)):
            t = i / (length * 2)
            px = 16 + math.sin(ang) * length * t
            py = N - 1 - math.cos(ang) * length * t + 12 * t * t
            w = 4.5 * math.sin(math.pi * min(1, t * 1.05))
            for j in range(-int(w), int(w) + 1):
                v = 120 + 80 * t - (18 if j == 0 else 0) - 6 * abs(j)
                c.put(px + j * math.cos(ang), py + j * math.sin(ang), grey(v))
    c.save(name)


def more_plants():
    wildflower('tulip_red', 101, (214, 40, 44), (0, 0, 0), 'cup', heads=3, lo=10, hi=16, leaves=5)
    wildflower('tulip_orange', 102, (240, 128, 36), (0, 0, 0), 'cup', heads=3, lo=10, hi=16, leaves=5)
    wildflower('tulip_pink', 103, (238, 140, 186), (0, 0, 0), 'cup', heads=3, lo=10, hi=16, leaves=5)
    wildflower('tulip_white', 104, (240, 238, 226), (0, 0, 0), 'cup', heads=3, lo=10, hi=16, leaves=5)
    wildflower('buttercup', 105, (252, 220, 40), (220, 170, 20), 'small', heads=6, lo=8, hi=17, leaves=7)
    wildflower('forget_me_not', 106, (110, 160, 240), (246, 224, 90), 'small', heads=7, lo=5, hi=11, leaves=8)
    wildflower('bluebell', 107, (92, 96, 214), (0, 0, 0), 'bell', heads=3, lo=12, hi=18, leaves=6)
    wildflower('oxeye_daisy', 108, (246, 246, 238), (240, 196, 40), 'round', heads=4, lo=16, hi=26, leaves=5)
    wildflower('cosmos', 109, (232, 108, 170), (240, 200, 60), 'round', heads=3, lo=16, hi=27, leaves=4, jitter=20)
    wildflower('thistle', 110, (176, 82, 196), (0, 0, 0), 'thistle', heads=3, lo=14, hi=24, leaves=6)
    wildflower('orchid', 111, (226, 120, 220), (250, 240, 140), 'orchid', heads=2, lo=12, hi=18, leaves=5)
    wildflower('arctic_poppy', 112, (246, 222, 70), (200, 150, 30), 'round', heads=3, lo=6, hi=11, leaves=6)
    wildflower('cotton_grass', 113, (246, 246, 242), (0, 0, 0), 'fluff', heads=7, lo=12, hi=24, leaves=9)
    wildflower('marigold', 114, (236, 140, 30), (180, 70, 20), 'round', heads=3, lo=8, hi=14, leaves=7)
    spike('goldenrod', 115, (238, 196, 46), 16, 27, heads=3)
    spike('lupin', 116, (128, 106, 220), 14, 24, heads=3)
    mushrooms('red_mushroom', 117, (196, 42, 36), spots=(244, 236, 220))
    mushrooms('brown_mushroom', 118, (150, 108, 72))
    clover('clover', 119)
    big_leaf('big_leaf', 120)
    flowering_bush('blueberry_bush', 121, (58, 104, 64), (70, 86, 170))
    flowering_bush('wild_rose', 122, (64, 112, 50), (236, 120, 150))
    sunflower('sunflower', 123)
    tall_fern('tall_fern', 124)
    flowering_bush('lilac', 125, (68, 116, 54), (184, 140, 222), blocks=2, cones=True)
    spike('foxglove', 126, (206, 96, 176), 38, 54, heads=2, bells=True, blocks=2)


if __name__ == '__main__':
    grass_set({'short': 'short_grass', 'normal': 'grass_tuft', 'medium': 'medium_grass', 'tall': 'tall_grass'}, 11, 'meadow')
    grass_set({'short': 'short_grass', 'normal': 'grass_tuft', 'medium': 'medium_grass', 'tall': 'tall_grass'}, 61, 'meadow', '_2')
    grass_set({'short': 'dry_short', 'normal': 'dry_tuft', 'medium': 'dry_medium', 'tall': 'dry_tall'}, 12, 'dry')
    grass_set({'short': 'dry_short', 'normal': 'dry_tuft', 'medium': 'dry_medium', 'tall': 'dry_tall'}, 62, 'dry', '_2')
    grass_set({'short': 'lush_short', 'normal': 'lush_normal', 'medium': 'lush_medium', 'tall': 'lush_tall'}, 13, 'lush')
    grass_set({'short': 'lush_short', 'normal': 'lush_normal', 'medium': 'lush_medium', 'tall': 'lush_tall'}, 63, 'lush', '_2')
    grass_set({'short': 'frost_short', 'normal': 'frost_normal', 'medium': 'frost_medium', 'tall': 'frost_tall'}, 14, 'frost')
    grass_set({'short': 'frost_short', 'normal': 'frost_normal', 'medium': 'frost_medium', 'tall': 'frost_tall'}, 64, 'frost', '_2')
    tall_grass_set('grass', 21, 'meadow')
    tall_grass_set('grass', 71, 'meadow', '_2')
    tall_grass_set('dry', 22, 'dry')
    tall_grass_set('dry', 72, 'dry', '_2')
    tall_grass_set('lush', 23, 'lush')
    tall_grass_set('lush', 73, 'lush', '_2')
    fern('fern', 15)
    bush('leafy_bush', 16, grey(215))
    bush('berry_bush', 17, (70, 118, 52), berries=(196, 36, 46))
    flower('dandelion', 18, (246, 204, 52), (210, 150, 30))
    flower('poppy', 19, (212, 44, 36), (30, 20, 20))
    flower('cornflower', 20, (78, 118, 226), (50, 60, 140))
    flower('daisy', 21, (240, 240, 232), (230, 190, 50))
    flower('lavender', 22, (152, 112, 206), (152, 112, 206), heads=4, spike=True)
    dead_bush('dead_bush', 23)
    reeds('reeds', 24)
    heather('heather', 25)
    more_plants()
    print('painted plant sprites')
