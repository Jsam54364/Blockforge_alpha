"""Stitches twelve in-game views into the title screen's panorama (assets/title/panorama.jpg).

The views are 1280 x 576 renders from one spot, yaw k * 30 degrees (k = 0..11, the game's
camera yaw: 0 looks towards -z, pi/2 towards -x), pitch 0, vertical field of view 1.1 rad,
named v0.png .. v11.png. The result is cylindrical: u = yaw / 2pi, v linear in elevation from
+30 degrees (top) to -30 degrees (bottom), which is what app/src/title.wgsl samples.

    python3 tools/stitch_panorama.py <views dir> assets/title/panorama.jpg
"""
import math, sys
import numpy as np
from PIL import Image

W, H = 1280, 576
T = math.tan(1.1 / 2)
ASPECT = W / H
LAT = math.radians(30)
OUT_W = 4096
OUT_H = int(round(OUT_W * (2 * LAT) / (2 * math.pi)))
VIEWS = sys.argv[1]
views = [np.asarray(Image.open(f"{VIEWS}/v{k}.png").convert("RGB"), dtype=np.float32) for k in range(12)]

u = (np.arange(OUT_W) + 0.5) / OUT_W
v = (np.arange(OUT_H) + 0.5) / OUT_H
yaw = u * 2 * math.pi                     # [OUT_W]
el = LAT - v * 2 * LAT                     # [OUT_H]
Y, E = np.meshgrid(yaw, el)                # [OUT_H, OUT_W]
d = np.stack([-np.sin(Y) * np.cos(E), np.sin(E), -np.cos(Y) * np.cos(E)], -1)

acc = np.zeros((OUT_H, OUT_W, 3), np.float32)
wsum = np.zeros((OUT_H, OUT_W), np.float32)
for k, img in enumerate(views):
    a = k * math.pi / 6
    f = np.array([-math.sin(a), 0.0, -math.cos(a)])
    s = np.cross(f, [0.0, 1.0, 0.0]); s /= np.linalg.norm(s)
    up = np.cross(s, f)
    df = d @ f
    x = (d @ s) / (df * T * ASPECT)
    y = (d @ up) / (df * T)
    px = (x + 1) / 2 * W - 0.5
    py = (1 - y) / 2 * H - 0.5
    ok = (df > 0) & (px >= 0) & (px <= W - 1) & (py >= 0) & (py <= H - 1)
    # Angular distance from this view's centre, wrapped: linear crossfade over 30 degrees.
    dy = np.abs((Y - a + math.pi) % (2 * math.pi) - math.pi)
    w = np.clip(1 - dy / (math.pi / 6), 0, 1) * ok
    w = w + 1e-4 * ok  # anything this view sees beats nothing at all
    pxc, pyc = np.clip(px, 0, W - 1), np.clip(py, 0, H - 1)
    x0, y0 = np.floor(pxc).astype(int), np.floor(pyc).astype(int)
    x1, y1 = np.minimum(x0 + 1, W - 1), np.minimum(y0 + 1, H - 1)
    fx, fy = (pxc - x0)[..., None], (pyc - y0)[..., None]
    c = (img[y0, x0] * (1 - fx) + img[y0, x1] * fx) * (1 - fy) + (img[y1, x0] * (1 - fx) + img[y1, x1] * fx) * fy
    acc += c * w[..., None]
    wsum += w
missing = (wsum <= 0).sum()
print("size", OUT_W, OUT_H, "uncovered pixels", int(missing))
out = (acc / np.maximum(wsum, 1e-6)[..., None]).clip(0, 255).astype(np.uint8)
Image.fromarray(out).save(sys.argv[2], quality=90, optimize=True, progressive=False)
