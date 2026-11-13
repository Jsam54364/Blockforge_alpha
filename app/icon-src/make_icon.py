"""Source of the Blockforge launcher icon: a 2x2x2 voxel block, front-top block missing,
glowing like hot metal, with a few sparks.

Writes fg.svg (adaptive foreground), bg.svg, mono.svg (themed-icon layer) and legacy.svg
into the current folder. To regenerate the PNGs in app/res (needs rsvg-convert):

    python3 make_icon.py
    for d in mdpi:1 hdpi:1.5 xhdpi:2 xxhdpi:3 xxxhdpi:4; do
      n=${d%%:*}; k=${d##*:}; L=$(python3 -c "print(round(48*$k))"); A=$(python3 -c "print(round(108*$k))")
      rsvg-convert -w $L -h $L legacy.svg > ../res/mipmap-$n/ic_launcher.png
      rsvg-convert -w $A -h $A fg.svg     > ../res/mipmap-$n/ic_launcher_foreground.png
      rsvg-convert -w $A -h $A mono.svg   > ../res/mipmap-$n/ic_launcher_monochrome.png
    done

The adaptive background colour lives in app/res/values/colors.xml.
"""
import math
C = (54.0, 57.0)   # cube centre in the 108x108 adaptive-icon canvas
R = 25.0           # hexagon radius
s = math.sqrt(3) / 2
top = (C[0], C[1] - R); ur = (C[0] + R * s, C[1] - R / 2); lr = (C[0] + R * s, C[1] + R / 2)
bot = (C[0], C[1] + R); ll = (C[0] - R * s, C[1] + R / 2); ul = (C[0] - R * s, C[1] - R / 2)
def pts(*p): return " ".join(f"{x:.2f},{y:.2f}" for x, y in p)
def lerp(a, b, t): return (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t)
def grid(a, b, c, d):
    # 3x3 voxel seams on the quad a-b-c-d (a->b and d->c are parallel edges)
    out = []
    for t in (1/3, 2/3):
        out.append((lerp(a, b, t), lerp(d, c, t)))
        out.append((lerp(a, d, t), lerp(b, c, t)))
    return "".join(f'<line x1="{p[0]:.2f}" y1="{p[1]:.2f}" x2="{q[0]:.2f}" y2="{q[1]:.2f}"/>' for p, q in out)
faces = {
    "top": (top, ur, C, ul),
    "left": (ul, C, bot, ll),
    "right": (C, ur, lr, bot),
}
sparks = [(73, 27, 3.2, "#FFE08A"), (80.5, 34, 2.4, "#FFB347"), (66.5, 22.5, 2.0, "#FF9A3C"), (78.5, 23.5, 1.6, "#FFD27A")]
def spark_svg(mono=False):
    out = []
    for x, y, w, col in sparks:
        fill = "#fff" if mono else col
        out.append(f'<rect x="{x - w/2:.2f}" y="{y - w/2:.2f}" width="{w}" height="{w}" fill="{fill}" transform="rotate(45 {x} {y})"/>')
    return "".join(out)

defs = '''<defs>
  <radialGradient id="glow" cx="54" cy="52" r="40" gradientUnits="userSpaceOnUse">
    <stop offset="0" stop-color="#FF8A3D" stop-opacity="0.55"/>
    <stop offset="0.55" stop-color="#FF6A2B" stop-opacity="0.16"/>
    <stop offset="1" stop-color="#FF6A2B" stop-opacity="0"/>
  </radialGradient>
  <linearGradient id="topg" x1="54" y1="32" x2="54" y2="57" gradientUnits="userSpaceOnUse">
    <stop offset="0" stop-color="#FFE9A8"/><stop offset="1" stop-color="#FFA640"/>
  </linearGradient>
  <linearGradient id="leftg" x1="32" y1="45" x2="54" y2="82" gradientUnits="userSpaceOnUse">
    <stop offset="0" stop-color="#E0662A"/><stop offset="1" stop-color="#A9441B"/>
  </linearGradient>
  <linearGradient id="rightg" x1="76" y1="45" x2="54" y2="82" gradientUnits="userSpaceOnUse">
    <stop offset="0" stop-color="#7A3A24"/><stop offset="1" stop-color="#4A2418"/>
  </linearGradient>
  <linearGradient id="bg" x1="0" y1="0" x2="0" y2="108" gradientUnits="userSpaceOnUse">
    <stop offset="0" stop-color="#2B2433"/><stop offset="1" stop-color="#16131B"/>
  </linearGradient>
</defs>'''

def P(x, y, z):
    # 3D voxel coords in [0,2]^3 -> screen; (2,2,2) is the corner nearest the viewer
    u = (R * s / 2, R / 4); v = (-R * s / 2, R / 4); w = (0, -R / 2)
    return (C[0] + (x - 2) * u[0] + (y - 2) * v[0] + (z - 2) * w[0],
            C[1] + (x - 2) * u[1] + (y - 2) * v[1] + (z - 2) * w[1])

def squares():
    out = []  # (kind, inner, 3D corners)
    for i in range(2):
        for j in range(2):
            if (i, j) != (1, 1):
                out.append(("top", False, [(i, j, 2), (i + 1, j, 2), (i + 1, j + 1, 2), (i, j + 1, 2)]))
                out.append(("right", False, [(2, i, j), (2, i + 1, j), (2, i + 1, j + 1), (2, i, j + 1)]))
                out.append(("left", False, [(i, 2, j), (i + 1, 2, j), (i + 1, 2, j + 1), (i, 2, j + 1)]))
    # the missing front-top voxel exposes a floor and two walls
    out.append(("top", True, [(1, 1, 1), (2, 1, 1), (2, 2, 1), (1, 2, 1)]))
    out.append(("right", True, [(1, 1, 1), (1, 2, 1), (1, 2, 2), (1, 1, 2)]))
    out.append(("left", True, [(1, 1, 1), (2, 1, 1), (2, 1, 2), (1, 1, 2)]))
    return out

def cube(mono=False):
    fills = {"top": "url(#topg)", "left": "url(#leftg)", "right": "url(#rightg)"}
    alphas = {"top": 1.0, "left": 0.72, "right": 0.45}
    out = []
    for kind, inner, q in squares():
        p = pts(*[P(*c) for c in q])
        if mono:
            a = alphas[kind] * (0.8 if inner else 1.0)
            out.append(f'<polygon points="{p}" fill="#fff" fill-opacity="{a:.2f}" stroke="#000" stroke-width="1.3" stroke-linejoin="round"/>')
        else:
            seam = "#8A3510" if kind == "top" else "#2A1008"
            out.append(f'<polygon points="{p}" fill="{fills[kind]}" stroke="{seam}" stroke-opacity="0.55" stroke-width="0.8" stroke-linejoin="round"/>')
            if inner:
                shade = 0.12 if kind == "top" else 0.32
                out.append(f'<polygon points="{p}" fill="#120604" fill-opacity="{shade}"/>')
    if not mono:
        out.append(f'<polyline points="{pts(ul, top, ur)}" fill="none" stroke="#FFF4D6" stroke-width="1.1" stroke-linejoin="round" stroke-opacity="0.9"/>')
        out.append(f'<polygon points="{pts(top, ur, lr, bot, ll, ul)}" fill="none" stroke="#1A0B06" stroke-width="1.2" stroke-linejoin="round" stroke-opacity="0.7"/>')
    return "".join(out)

head = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 108 108" width="108" height="108">'
fg = head + defs + '<circle cx="54" cy="52" r="40" fill="url(#glow)"/>' + cube() + spark_svg() + '</svg>'
bg = head + defs + '<rect width="108" height="108" fill="url(#bg)"/></svg>'
# monochrome: black seams must become transparent, so draw on a mask
mono = head + '<defs><mask id="m" maskUnits="userSpaceOnUse" x="0" y="0" width="108" height="108"><rect width="108" height="108" fill="#000"/>' + cube(True) + spark_svg(True) + '</mask></defs><rect width="108" height="108" fill="#fff" mask="url(#m)"/></svg>'
# legacy icon: the 72-unit visible area, rounded square, background + foreground
legacy = ('<svg xmlns="http://www.w3.org/2000/svg" viewBox="18 18 72 72" width="72" height="72">' + defs +
          '<clipPath id="rr"><rect x="19" y="19" width="70" height="70" rx="16"/></clipPath>'
          '<g clip-path="url(#rr)"><rect width="108" height="108" fill="url(#bg)"/><circle cx="54" cy="52" r="40" fill="url(#glow)"/>' + cube() + spark_svg() + '</g></svg>')
for name, svg in [("fg", fg), ("bg", bg), ("mono", mono), ("legacy", legacy)]:
    open(f"{name}.svg", "w").write(svg)
