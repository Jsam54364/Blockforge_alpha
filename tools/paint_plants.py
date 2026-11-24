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


if __name__ == '__main__':
    grass('short_grass', 11, 34, 6, 13)
    grass('grass_tuft', 12, 32, 11, 21)
    grass('tall_grass', 13, 28, 18, 30, heads=3)
    grass('dry_tuft', 14, 18, 9, 19, tinted=False, base=(150, 175), top=(205, 230))
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
    print('painted 15 plant sprites')
