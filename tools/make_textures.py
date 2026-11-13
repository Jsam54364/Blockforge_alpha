#!/usr/bin/env python3
"""Builds the 32x32 block textures in assets/textures/ from public-domain (CC0) photo textures.

Source: the ambientCG materials (https://ambientcg.com, CC0, by Lennart Demes), as the preview
sheets of https://github.com/fabien-michel/sweethome3d-textures-ambientcg (also CC0): 128 x 128
thumbnails of whole, seamless textures, plenty for 32 x 32.

    git clone --depth 1 https://github.com/fabien-michel/sweethome3d-textures-ambientcg st
    python3 tools/make_textures.py st/previews

Each texture: average 4 x 4 thumbnail pixels per texel in linear light, sharpen a little (the
crisp, pixelated look), make it seamless (blend the borders with a half-offset copy), then pull
its colour and contrast toward a target. Grass is made greyscale, Minecraft style: the game tints
it by biome. A grass block's side is dirt plus `<name>_overlay.png`, the greyscale fringe that is
tinted (transparent elsewhere). Textures not listed here are painted in code (app/src/texgen.rs).
Needs Pillow and numpy.
"""
import os
import sys

import numpy as np
from PIL import Image

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'assets', 'textures')

# name: (sheet, label, swatch index on the sheet in reading order, target mean sRGB or grey
# level, contrast)
PHOTOS = {
    'stone': ('Rock', 'Rock044', 43, (124, 123, 121), 1.2),
    'dirt': ('Ground', 'Ground067', 70, (126, 90, 62), 1.25),
    'sand': ('Ground', 'Ground079S', 83, (218, 204, 160), 1.6),
    'gravel': ('Gravel', 'Gravel022', 21, (128, 124, 121), 1.1),
    'snow': ('Snow', 'Snow008B', 8, (238, 243, 250), 1.8),
    'log_side': ('Bark', 'Bark014', 13, (104, 80, 52), 1.25),
    'bedrock': ('Rock', 'Rock031', 30, (78, 78, 80), 1.3),
}
# Greyscale, tinted by biome in the game.
GREY = {
    'grass_top': ('Grass', 'Grass004', 3, 150, 2.6),
    'dry_grass_top': ('Ground', 'Ground073', 76, 160, 1.7),
    'lush_grass_top': ('Moss', 'Moss003', 2, 140, 1.7),
    'cold_grass_top': ('Moss', 'Moss001', 0, 150, 1.5),
}


def swatch(previews, sheet, index, inset=2):
    im = Image.open(os.path.join(previews, sheet + '.webp')).convert('RGB')
    r, c = divmod(index, 7)
    x, y = 16 + 144 * c, 16 + 176 * r
    return im.crop((x + inset, y + inset, x + 128 - inset, y + 128 - inset))


def to_lin(a):
    return (np.clip(a, 0, 255) / 255.0) ** 2.2


def to_srgb(l):
    return np.clip(l, 0, 1) ** (1 / 2.2) * 255.0


def down32(im, sharpen=0.6):
    a = to_lin(np.asarray(im).astype(float))
    chans = [Image.fromarray(a[:, :, k].astype(np.float32), mode='F').resize((32, 32), Image.BOX) for k in range(3)]
    s = to_srgb(np.stack([np.asarray(ch) for ch in chans], axis=2))
    blur = sum(np.roll(np.roll(s, dy, 0), dx, 1) for dy in (-1, 0, 1) for dx in (-1, 0, 1)) / 9.0
    return np.clip(s + sharpen * (s - blur), 0, 255)


def seamless(s, band=7.0):
    shifted = np.roll(np.roll(s, 16, 0), 16, 1)
    d = np.minimum(np.arange(32) + 0.5, 32 - (np.arange(32) + 0.5))
    w = np.clip(1.0 - d / band, 0, 1)
    w = w * w * (3 - 2 * w)
    m = np.maximum(w[:, None], w[None, :])
    if s.ndim == 3:
        m = m[:, :, None]
    return s * (1 - m) + shifted * m


def toward(s, mean, contrast):
    """Scale contrast around the mean, then move the mean to `mean` (per channel)."""
    cur = s.reshape(-1, s.shape[-1]).mean(axis=0) if s.ndim == 3 else s.mean()
    out = (s - cur) * contrast + np.asarray(mean, dtype=float)
    return np.clip(out, 0, 255)


def grey(s):
    return s[:, :, 0] * 0.299 + s[:, :, 1] * 0.587 + s[:, :, 2] * 0.114


def fringe(seed):
    """Per-column fringe length (pixels hanging down from the top of a grass block's side)."""
    rng = np.random.default_rng(seed)
    base = 5 + 3 * np.interp(np.arange(32), np.linspace(0, 32, 9), rng.random(9))
    lens = base + rng.random(32) * 2.5
    strands = rng.random(32) > 0.8
    lens[strands] += 2 + 4 * rng.random(strands.sum())
    return np.round(lens).astype(int)


def save(arr, name, mode='RGB'):
    Image.fromarray(np.round(arr).astype(np.uint8), mode).save(os.path.join(OUT, name + '.png'))


def main(previews):
    os.makedirs(OUT, exist_ok=True)
    made = {}
    for name, (sheet, label, idx, mean, contrast) in PHOTOS.items():
        s = toward(seamless(down32(swatch(previews, sheet, idx))), mean, contrast)
        made[name] = s
        save(s, name)
        print(f'{name:16} <- {label}')
    for i, (name, (sheet, label, idx, level, contrast)) in enumerate(GREY.items()):
        g = toward(seamless(grey(down32(swatch(previews, sheet, idx)))), level, contrast)
        save(g, name, 'L')
        print(f'{name:16} <- {label} (greyscale)')
        # The side: dirt with a shadow under the fringe, plus the fringe as a tinted overlay.
        side = made['dirt'].copy()
        over = np.zeros((32, 32, 4))
        for x, n in enumerate(fringe(100 + i)):
            for y in range(min(n, 32)):
                v = g[y, x] * (1.0 - 0.3 * y / max(n, 1))
                over[y, x] = (v, v, v, 255)
            if n < 32:
                side[n, x] *= 0.62
        base = name.replace('_top', '_side')
        save(side, base)
        save(over, base + '_overlay', 'RGBA')
        print(f'{base:16} <- dirt + fringe overlay')


if __name__ == '__main__':
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(sys.argv[1])
