#!/usr/bin/env python3
"""Paints the item sprites (assets/textures/<item>.png, 32 x 32, see-through around them).

Items are drawn in the hand as their sprite one texel thick, at the game's 32 texels to the
metre, so a sprite is drawn to scale: a weapon lies corner to corner, pommel at the bottom
left (the hand holds it there, see GRIP in app/src/combat.rs), and runs up and right for its
real length (a one-handed sword about a metre: 21 texels along the diagonal; a staff the
whole 31).

Weapons are drawn as profiles along that diagonal: for each stretch of its length, how far
it reaches either side and of what material, lit from the upper left.
"""
import math
import os
import random

import numpy as np
from PIL import Image

N = 32
OUT = os.path.join(os.path.dirname(__file__), '..', 'assets', 'textures')
S2 = math.sqrt(2.0)

# Materials: (base colour, how much each pixel varies)
MAT = {
    'steel': ((196, 204, 214), 8),
    'iron': ((158, 160, 166), 10),
    'dark_iron': ((104, 106, 114), 8),
    'bronze': ((186, 128, 66), 10),
    'gold': ((226, 184, 72), 10),
    'wood': ((128, 88, 52), 12),
    'dark_wood': ((92, 62, 38), 10),
    'leather': ((96, 60, 36), 8),
    'cloth_red': ((150, 40, 36), 8),
    'bone': ((222, 214, 190), 8),
}


def rgb(c):
    return tuple(int(max(0, min(255, v))) for v in c)


class Sprite:
    def __init__(self):
        self.px = np.zeros((N, N, 4), np.uint8)

    def put(self, x, y, c):
        if 0 <= x < N and 0 <= y < N:
            self.px[y, x, :3] = rgb(c)
            self.px[y, x, 3] = 255

    def get(self, x, y):
        return self.px[y, x] if 0 <= x < N and 0 <= y < N else None

    def outline(self, dark=0.45):
        """A darker rim on the pixels at the edge facing down and right (reads well small)."""
        a = self.px[:, :, 3] > 0
        out = self.px.copy()
        for y in range(N):
            for x in range(N):
                if a[y, x] and ((x + 1 < N and not a[y, x + 1]) or (y + 1 < N and not a[y + 1, x])):
                    out[y, x, :3] = (out[y, x, :3] * dark).astype(np.uint8)
        self.px = out

    def save(self, name):
        Image.fromarray(self.px, 'RGBA').save(os.path.join(OUT, name + '.png'))


def weapon(name, seed, parts, origin=(1, 30)):
    """Draws a weapon along the diagonal. `parts` are (u0, u1, half_width(u, t) or number,
    material, extra) with u in texels along the weapon from the pommel; extra may be
    ('side', lo, hi) for a head that sticks out on one side between v = lo and hi (an axe),
    or 'fuller' for a blade's darker middle line."""
    rnd = random.Random(seed)
    s = Sprite()
    ox, oy = origin
    for y in range(N):
        for x in range(N):
            fx, fy = x + 0.5 - ox, oy - (y + 0.5)
            u = (fx + fy) / S2
            v = (fy - fx) / S2  # + toward the upper left
            for (u0, u1, w, mat, extra) in parts:
                if not (u0 <= u <= u1):
                    continue
                t = (u - u0) / max(1e-6, u1 - u0)
                hw = w(u, t) if callable(w) else w
                lo, hi = -hw, hw
                if isinstance(extra, tuple) and extra[0] == 'side':
                    lo, hi = extra[1](t), extra[2](t)
                if not (lo <= v <= hi):
                    continue
                base, var = MAT[mat]
                # Lit from the upper left: the +v edge bright, the -v edge dark.
                k = 1.0 + 0.28 * (v / max(0.6, max(abs(lo), abs(hi))))
                if extra == 'fuller' and abs(v) < 0.45 and hw > 1.0:
                    k *= 0.82
                if isinstance(extra, tuple) and extra[0] == 'side' and v > hi - 1.0:
                    k *= 1.18  # the honed edge
                c = [b * k + rnd.uniform(-var, var) for b in base]
                if mat in ('wood', 'dark_wood') and (int(u * 1.7) % 3 == 0):
                    c = [v_ * 0.9 for v_ in c]
                s.put(x, y, c)
                break
    s.outline()
    s.save(name)


def taper(w0, w1):
    return lambda u, t: w0 + (w1 - w0) * t


def point(w):
    return lambda u, t: w * (1 - t) + 0.15


def sword(name, seed, L, blade, guard='iron', grip='leather', bw=1.3, gw=3.2):
    weapon(name, seed, [
        (0, 1.6, 1.25, guard, None),            # pommel
        (1.6, 5.4, 0.75, grip, None),           # grip
        (5.4, 6.9, gw, guard, None),            # crossguard
        (6.9, L - 2.6, bw, blade, 'fuller'),    # blade
        (L - 2.6, L, point(bw), blade, None),   # tip
    ])


def all_weapons():
    sword('bronze_sword', 1, 20.0, 'bronze', guard='bronze')
    sword('iron_sword', 2, 21.5, 'iron')
    sword('steel_sword', 3, 22.5, 'steel', guard='gold')
    weapon('iron_dagger', 4, [
        (0, 1.3, 1.0, 'iron', None), (1.3, 4.0, 0.7, 'leather', None), (4.0, 5.0, 2.0, 'iron', None),
        (5.0, 9.0, 0.95, 'iron', 'fuller'), (9.0, 11.0, point(0.95), 'iron', None)])
    sword('steel_greatsword', 5, 31.5, 'steel', guard='dark_iron', bw=1.75, gw=4.6)
    # Axes: a haft with a head out to one side near its end.
    weapon('iron_axe', 6, [
        (13.0, 19.0, 0, 'iron', ('side', lambda t: -1.2, lambda t: 2.0 + 3.0 * math.sin(math.pi * min(1, t * 1.05)))),
        (0, 18.5, 0.75, 'wood', None)])
    weapon('battle_axe', 7, [
        (21.0, 29.5, 0, 'steel', ('side', lambda t: -(1.8 + 3.6 * math.sin(math.pi * t)), lambda t: 1.8 + 3.6 * math.sin(math.pi * t))),
        (29.5, 31.0, point(0.9), 'dark_iron', None),
        (0, 29.5, 0.8, 'dark_wood', None)])
    weapon('iron_mace', 8, [
        (0, 1.4, 1.0, 'iron', None), (1.4, 5.0, 0.75, 'leather', None), (5.0, 13.5, 0.7, 'dark_iron', None),
        (13.5, 18.5, lambda u, t: 1.6 + 1.1 * (1 if int(u * 1.4) % 2 else 0.4), 'iron', None),
        (18.5, 19.6, point(1.2), 'iron', None)])
    weapon('wooden_club', 9, [(0, 17.5, taper(0.8, 2.3), 'wood', None), (17.5, 18.6, point(2.3), 'wood', None)])
    weapon('iron_spear', 10, [
        (0, 24.5, 0.7, 'wood', None), (24.5, 26.0, 1.0, 'dark_iron', None),
        (26.0, 31.5, lambda u, t: 1.7 * math.sin(math.pi * min(1.0, 0.35 + t * 0.8)) + 0.1, 'steel', 'fuller')])
    weapon('warhammer', 11, [
        (21.5, 27.0, 0, 'dark_iron', ('side', lambda t: -4.2, lambda t: 4.2)),
        (27.0, 30.0, point(1.0), 'dark_iron', None),
        (0, 1.5, 1.0, 'dark_iron', None), (1.5, 6.0, 0.8, 'leather', None), (6.0, 27.0, 0.8, 'dark_wood', None)])


def staff(name, seed, gem, glow):
    """A gnarled staff (the whole diagonal) with a gem held in claws at the top."""
    rnd = random.Random(seed)
    weapon(name, seed, [
        (25.5, 30.5, lambda u, t: 2.1 * math.sin(math.pi * min(1, 0.15 + t * 0.85)) + 0.2, 'gold', None),
        (0, 25.5, lambda u, t: 0.75 + 0.25 * math.sin(u * 1.3) + (0.5 if t > 0.9 else 0), 'dark_wood', None)])
    s = Sprite()
    s.px = np.array(Image.open(os.path.join(OUT, name + '.png')))
    # The gem: a small bright diamond inside the claws.
    cx, cy = 1 + 28.0 / S2 + 0.0, 30 - 28.0 / S2
    for y in range(N):
        for x in range(N):
            d = abs(x + 0.5 - cx) + abs(y + 0.5 - cy)
            if d <= 2.2:
                k = 1.25 if (x + 0.5 < cx and y + 0.5 < cy) else 1.0 if d < 1.2 else 0.75
                s.put(x, y, [c * k for c in gem])
    hx, hy = int(cx - 0.8), int(cy - 0.8)
    s.put(hx, hy, glow)
    s.save(name)


def all_staves():
    staff('ember_staff', 20, (230, 86, 30), (255, 220, 150))
    staff('frost_staff', 21, (110, 190, 240), (230, 250, 255))
    staff('storm_staff', 22, (150, 120, 240), (235, 225, 255))
    staff('staff_of_mending', 23, (90, 210, 110), (220, 255, 220))


def shape(name, seed, rows, palette, shade_x=True):
    """Draws a pixel shape from ASCII rows (centred), each letter a palette colour."""
    rnd = random.Random(seed)
    s = Sprite()
    h = len(rows)
    w = max(len(r) for r in rows)
    x0, y0 = (N - w) // 2, (N - h) // 2
    for j, r in enumerate(rows):
        for i, ch in enumerate(r):
            if ch == ' ' or ch == '.':
                continue
            base, var = palette[ch]
            k = 1.0
            if shade_x:
                k = 1.12 - 0.3 * (i / max(1, w - 1))
            s.put(x0 + i, y0 + j, [b * k + rnd.uniform(-var, var) for b in base])
    s.outline(0.55)
    s.save(name)


HELM = [
    "   aaaaaa   ",
    "  aaaaaaaa  ",
    " aaaaaaaaaa ",
    " aaaaaaaaaa ",
    " aabbbbbbaa ",
    " aab .. baa ",
    " aab    baa ",
    " aa      aa ",
    " aa      aa ",
]
BODY = [
    " aaa    aaa ",
    "aaaaaaaaaaaa",
    "aaaaaaaaaaaa",
    "aaaaaaaaaaaa",
    " aaaaaaaaaa ",
    "  aaaaaaaa  ",
    "  aaaaaaaa  ",
    "  aaaaaaaa  ",
    "  bbbbbbbb  ",
    "  aaaaaaaa  ",
    "  aaaaaaaa  ",
]
LEGS = [
    "bbbbbbbbbb",
    "aaaaaaaaaa",
    "aaaaaaaaaa",
    "aaaa  aaaa",
    "aaaa  aaaa",
    "aaaa  aaaa",
    "aaaa  aaaa",
    "aaaa  aaaa",
    "aaaa  aaaa",
    "aaaa  aaaa",
]
BOOTS = [
    "aaa    aaa ",
    "aaa    aaa ",
    "aaa    aaa ",
    "aaaa   aaaa",
    "bbbbb  bbbbb",
]
HOOD = [
    "    aaaa    ",
    "   aaaaaa   ",
    "  aaaaaaaa  ",
    " aaaa..aaaa ",
    " aaa....aaa ",
    " aaa....aaa ",
    " aaaa..aaaa ",
    "aaaaaaaaaaaa",
]
ROBE = [
    " aaa    aaa ",
    "aaaaabbaaaaa",
    "aaaaabbaaaaa",
    "aaaaabbaaaaa",
    " aaaabbaaaa ",
    " aaaabbaaaa ",
    " aaaabbaaaa ",
    " aaaaaaaaaa ",
    "aaaaaaaaaaaa",
    "aaaaaaaaaaaa",
    "aaaaaaaaaaaa",
]


def big(rows, k=2):
    """Each row and column doubled (the shapes are drawn at half size)."""
    out = []
    for r in rows:
        line = ''.join(ch * k for ch in r)
        out.extend([line] * k)
    return out


def all_armour():
    sets = {
        'leather': ({'a': ((132, 86, 50), 10), 'b': ((92, 58, 34), 8)}, ['leather_cap', 'leather_jerkin', 'leather_trousers', 'leather_boots']),
        'chain': ({'a': ((150, 154, 162), 22), 'b': ((96, 70, 44), 8)}, ['chain_coif', 'chain_hauberk', 'chain_leggings', 'chain_boots']),
        'plate': ({'a': ((190, 196, 206), 6), 'b': ((214, 172, 70), 8)}, ['plate_helm', 'plate_cuirass', 'plate_greaves', 'plate_sabatons']),
    }
    for (pal, names), seed in zip(sets.values(), (30, 31, 32)):
        for rows, n in zip((HELM, BODY, LEGS, BOOTS), names):
            shape(n, seed, big(rows), pal)
    mage = {'a': ((58, 64, 150), 8), 'b': ((214, 176, 70), 6)}
    shape('mage_hood', 40, big(HOOD), mage)
    shape('mage_robe', 41, big(ROBE), mage)


BOTTLE = [
    "    cc    ",
    "    gg    ",
    "    gg    ",
    "   gllg   ",
    "  glllll  ",
    " glllllll ",
    " glllllll ",
    " glllllll ",
    " glllllll ",
    "  glllll  ",
    "   ggg    ",
]


def all_potions():
    glass = ((206, 220, 226), 6)
    cork = ((150, 110, 70), 8)
    for name, liquid, seed in (('health_potion', (200, 34, 40), 50), ('mana_potion', (50, 90, 220), 51), ('stamina_potion', (70, 190, 70), 52)):
        shape(name, seed, big(BOTTLE), {'c': cork, 'g': glass, 'l': (liquid, 10)})
        # A highlight on the glass.
        s = Sprite()
        s.px = np.array(Image.open(os.path.join(OUT, name + '.png')))
        for (x, y) in ((11, 16), (11, 17), (11, 18), (12, 14)):
            s.put(x, y, (255, 255, 255))
        s.save(name)


if __name__ == '__main__':
    all_weapons()
    all_staves()
    all_armour()
    all_potions()
    print('painted item sprites')
