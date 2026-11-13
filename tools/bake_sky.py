#!/usr/bin/env python3
"""Bakes equirectangular HDRIs into the game's sky (assets/sky).

    python3 tools/bake_sky.py path/to/day_sky.hdr        # the day sky
    python3 tools/bake_sky.py --night path/to/night.exr  # the night sky (stars and moon)

The game moves the sun, so the sun in the photo has to go. A clear sky is close to
"brightness by elevation" times "glow around the sun" (the Perez sky model has that shape),
so the photo is split the same way:

    sky(direction) = base(direction) * glow(angle to the sun)

The glow profile is measured from the photo (median brightness ratio per angle from its
sun), fitted with a small formula per colour channel, and divided out; the sun disc itself
and its glare are painted over with the sky around them. What is left, `base`, keeps the
clouds and the haze band but no sun. The game multiplies the glow back in around wherever
its own sun is, then adds the sun disc, sunset colours, night, stars and a moon.

Writes:
    assets/sky/sky.png   upper hemisphere (row 0 = straight up, last row = horizon),
                         RGBE: rgb mantissas + a shared exponent in alpha, lossless
    assets/sky/glow.txt  the glow formula's coefficients (see app/src/sky.rs)

The night sky is used differently: the stars (and the moon in the photo) turn with the sky
through the night, so the whole sphere is kept, in the photo's own orientation. Its smooth
background (a function of elevation, measured per row away from the moon) is taken out and
written as two colours, since the game draws the background by elevation in the world; what
is left is stars, the Milky Way and the moon with its halo, with the faint noise between
them cleared (which also makes the file small). The game turns the photo so that its moon
is where the game's moon is (opposite the sun).

    assets/sky/night.png  whole sphere (row 0 = up, last row = down), RGBE as above
    assets/sky/night.txt  where the moon is in the photo, and the background colours

Needs numpy, scipy, opencv-python, and OpenEXR for .exr files
(pip install numpy scipy opencv-python OpenEXR).
"""
import os
import sys

import cv2
import numpy as np

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, 'assets', 'sky')
LUM = np.array([0.2126, 0.7152, 0.0722])
FIT_FROM = np.radians(4.0)   # the glow is fitted outside this; the game draws the sun inside
PAINT = (np.radians(4.5), np.radians(8.0))  # sun + glare painted over inside these angles


def glow(gam, c):
    """The glow formula, one colour channel: c = (a, c1, w1, c2, w2, e)."""
    a, c1, w1, c2, w2, e = c
    return a + c1 * np.exp(-gam / w1) + c2 * np.exp(-gam / w2) + e * np.cos(gam) ** 2


def fit_glow(gam_bins, ratio, weight):
    """Shared widths for the three channels (grid search), linear least squares for the rest,
    minimising relative error."""
    best = None
    for w1 in np.geomspace(0.03, 0.6, 24):
        for w2 in np.geomspace(0.15, 2.0, 24):
            if w2 <= w1 * 1.5:
                continue
            A = np.stack([np.ones_like(gam_bins), np.exp(-gam_bins / w1), np.exp(-gam_bins / w2),
                          np.cos(gam_bins) ** 2], 1)
            err, coefs = 0.0, []
            for ch in range(3):
                wt = weight / ratio[:, ch]
                x, *_ = np.linalg.lstsq(A * wt[:, None], ratio[:, ch] * wt, rcond=None)
                err += np.sum(((A @ x - ratio[:, ch]) * wt) ** 2)
                coefs.append(x)
            if best is None or err < best[0]:
                best = (err, w1, w2, coefs)
    _, w1, w2, coefs = best
    return [(x[0], x[1], w1, x[2], w2, x[3]) for x in coefs]


def to_rgbe(img):
    """float RGB -> RGBE bytes (Radiance's encoding: shared exponent, 8-bit mantissas)."""
    m = img.max(-1)
    mant, exp = np.frexp(m)
    scale = np.where(m > 1e-32, mant * 256.0 / np.maximum(m, 1e-32), 0.0)
    out = np.zeros(img.shape[:2] + (4,), np.uint8)
    out[..., :3] = np.clip(np.floor(img * scale[..., None]), 0, 255)
    out[..., 3] = np.where(m > 1e-32, exp + 128, 0)
    return out


def from_rgbe(b):
    f = np.ldexp(1.0, b[..., 3].astype(np.int32) - 136)
    return (b[..., :3].astype(np.float64) + 0.5) * f[..., None] * (b[..., 3:] > 0)


def read_hdri(path):
    """Linear RGB, float64, from a Radiance .hdr or an OpenEXR .exr."""
    if path.lower().endswith('.exr'):
        import OpenEXR
        ch = OpenEXR.File(path).parts[0].channels
        if 'RGB' in ch or 'RGBA' in ch:
            im = (ch.get('RGB') or ch['RGBA']).pixels[..., :3]
        else:
            im = np.stack([ch[c].pixels for c in 'RGB'], -1)
        return im.astype(np.float64)
    im = cv2.imread(path, cv2.IMREAD_UNCHANGED)
    if im is None:
        sys.exit(f'could not read {path}')
    return im[:, :, ::-1].astype(np.float64)         # BGR -> RGB


def directions(h, w, H):
    """Unit directions of the pixel centres of the first h rows of an H-row equirect image."""
    el = np.pi / 2 - (np.arange(h) + 0.5) / H * np.pi
    az = (np.arange(w) + 0.5) / w * 2 * np.pi
    EL, AZ = np.meshgrid(el, az, indexing='ij')
    return np.stack([np.cos(EL) * np.sin(AZ), np.sin(EL), np.cos(EL) * np.cos(AZ)], -1)


# The night photo is scaled by this (it is brighter than the game's nights are meant to be).
NIGHT_EXPOSURE = 0.5
# Below this brightness (luminance, after exposure) what is left is noise: faded to black.
NIGHT_FLOOR = (0.004, 0.009)


def bake_night(path):
    im = read_hdri(path)
    H, W, _ = im.shape
    dirs = directions(H, W, H)
    lum = im @ LUM
    bright = lum > np.percentile(lum, 99.99)
    moon = (dirs[bright] * lum[bright, None]).sum(0)
    moon /= np.linalg.norm(moon)
    print(f'moon in the photo: elevation {np.degrees(np.arcsin(moon[1])):.1f} deg')
    from_moon = np.degrees(np.arccos(np.clip(dirs @ moon, -1, 1)))
    far = from_moon > 40
    bg = np.array([np.median(im[r][far[r]] if far[r].sum() > 100 else im[r], axis=0) for r in range(H)])
    k = cv2.getGaussianKernel(31, 8).ravel()
    bg = np.stack([np.convolve(np.pad(bg[:, c], 15, mode='edge'), k, 'valid') for c in range(3)], -1)
    layer = np.maximum(im - bg[:, None, :], 0) * NIGHT_EXPOSURE
    t = np.clip(((layer @ LUM) - NIGHT_FLOOR[0]) / (NIGHT_FLOOR[1] - NIGHT_FLOOR[0]), 0, 1)
    # Not around the moon: its halo fades out smoothly by itself.
    near = np.clip((40 - from_moon) / 15, 0, 1)
    layer *= np.maximum(t * t * (3 - 2 * t), near * near * (3 - 2 * near))[..., None]
    os.makedirs(OUT, exist_ok=True)
    cv2.imwrite(os.path.join(OUT, 'night.png'), cv2.cvtColor(to_rgbe(layer), cv2.COLOR_RGBA2BGRA),
                [cv2.IMWRITE_PNG_COMPRESSION, 9])
    row = lambda deg: bg[int((90 - deg) / 180 * H)] * NIGHT_EXPOSURE
    with open(os.path.join(OUT, 'night.txt'), 'w') as f:
        f.write('# Night sky photo (night.png). Written by tools/bake_sky.py.\n')
        f.write('# moon: towards the moon in the photo (x y z; y up, as the game)\n')
        f.write('# zenith, horizon: the background straight up and at the horizon (linear rgb)\n')
        f.write('moon ' + ' '.join(f'{v:.6f}' for v in moon) + '\n')
        f.write('zenith ' + ' '.join(f'{v:.6f}' for v in row(85)) + '\n')
        f.write('horizon ' + ' '.join(f'{v:.6f}' for v in row(2)) + '\n')
    kept = (layer.max(-1) > 0).mean()
    print(f'wrote {W}x{H} night sky ({os.path.getsize(os.path.join(OUT, "night.png")) / 1e6:.2f} MB, '
          f'{kept * 100:.1f}% of pixels lit) and night.txt')


def main():
    if len(sys.argv) == 3 and sys.argv[1] == '--night':
        return bake_night(sys.argv[2])
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    im = read_hdri(sys.argv[1])
    H, W, _ = im.shape
    up = im[:H // 2]                                 # sky above the horizon
    h = up.shape[0]
    # Same convention as the game: x = sin(az) cos(el), y = sin(el), z = cos(az) cos(el).
    dirs = directions(h, W, H)

    lum = up @ LUM
    bright = lum > np.percentile(lum, 99.995)
    sun = (dirs[bright] * lum[bright, None]).sum(0)
    sun /= np.linalg.norm(sun)
    gam = np.arccos(np.clip(dirs @ sun, -1, 1))
    print(f'sun in the photo: elevation {np.degrees(np.arcsin(sun[1])):.1f} deg')

    # Measure the glow: alternate "brightness per row" and "ratio per angle from the sun".
    valid = gam > FIT_FROM
    bins = np.linspace(FIT_FROM, gam.max(), 140)
    idx = np.digitize(gam, bins)
    P = np.ones_like(up)
    for _ in range(6):
        base_row = np.array([np.median((up[r] / P[r])[valid[r]], axis=0) for r in range(h)])
        ratio = up / base_row[:, None, :]
        prof, centres, counts = [], [], []
        for k in range(1, len(bins)):
            sel = (idx == k) & valid
            if sel.sum() < 50:
                continue
            prof.append(np.median(ratio[sel], axis=0))
            centres.append(0.5 * (bins[k - 1] + bins[k]))
            counts.append(sel.sum())
        prof, centres, counts = np.array(prof), np.array(centres), np.array(counts, float)
        P = np.stack([np.interp(gam, centres, prof[:, ch]) for ch in range(3)], -1)

    coefs = fit_glow(centres, prof, np.sqrt(counts / counts.max()))
    fitted = np.stack([glow(centres, c) for c in coefs], -1)
    rel = np.abs(fitted / prof - 1)
    print(f'glow fit: mean error {rel.mean() * 100:.1f}%, worst {rel.max() * 100:.1f}%')

    Pfit = np.stack([glow(np.maximum(gam, FIT_FROM), c) for c in coefs], -1)
    base = up / Pfit
    # Paint the sun and its glare over with the row's typical brightness (no clouds there).
    row_typ = np.array([np.median(base[r][gam[r] > PAINT[1] * 2], axis=0) for r in range(h)])
    t = np.clip((gam - PAINT[0]) / (PAINT[1] - PAINT[0]), 0, 1)
    t = (t * t * (3 - 2 * t))[..., None]
    base = row_typ[:, None, :] * (1 - t) + base * t

    os.makedirs(OUT, exist_ok=True)
    rgbe = to_rgbe(base)
    cv2.imwrite(os.path.join(OUT, 'sky.png'), cv2.cvtColor(rgbe, cv2.COLOR_RGBA2BGRA),
                [cv2.IMWRITE_PNG_COMPRESSION, 9])
    err = np.abs(from_rgbe(rgbe) / np.maximum(base, 1e-6) - 1)[..., 2]
    with open(os.path.join(OUT, 'glow.txt'), 'w') as f:
        f.write('# Sky glow around the sun, per colour channel, angle g in radians from the sun:\n')
        f.write('#   a + c1 * exp(-g / w1) + c2 * exp(-g / w2) + e * cos(g)^2\n')
        f.write('# Written by tools/bake_sky.py. Columns: a c1 w1 c2 w2 e\n')
        for name, c in zip('rgb', coefs):
            f.write(name + ' ' + ' '.join(f'{v:.6f}' for v in c) + '\n')
    print(f'wrote {W}x{h} sky ({os.path.getsize(os.path.join(OUT, "sky.png")) / 1e6:.2f} MB, '
          f'RGBE error in blue {err.mean() * 100:.2f}%) and glow.txt')


if __name__ == '__main__':
    main()
