//! Drawing with the UI atlas (`atlas.rs`): plain rectangles, painted frames stretched
//! without stretching their edges (nine-slice), and distance-field text and icons.

use super::{HudVertex, MODE_IMAGE, MODE_SDF, MODE_THUMB};
use crate::atlas::{atlas, FontId};

/// A textured rectangle: `uv` is (u0, v0, u1, v1).
#[allow(clippy::too_many_arguments)]
pub fn quad(out: &mut Vec<HudVertex>, x: f32, y: f32, w: f32, h: f32, uv: [f32; 4], color: [u8; 4], mode: u32) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let [u0, v0, u1, v1] = uv;
    let v = |px: f32, py: f32, u: f32, vv: f32| HudVertex { pos: [px, py], uv: [u, vv], color, mode };
    out.extend_from_slice(&[v(x, y, u0, v0), v(x + w, y, u1, v0), v(x + w, y + h, u1, v1), v(x, y, u0, v0), v(x + w, y + h, u1, v1), v(x, y + h, u0, v1)]);
}

/// Atlas pixels to texture coordinates.
fn uv_of(r: [f32; 4]) -> [f32; 4] {
    let s = atlas().size;
    [r[0] / s, r[1] / s, (r[0] + r[2]) / s, (r[1] + r[3]) / s]
}

fn white_uv() -> [f32; 4] {
    let r = atlas().piece("white").rect;
    uv_of([r[0] + r[2] * 0.5, r[1] + r[3] * 0.5, 0.0, 0.0])
}

/// A plain coloured rectangle.
pub fn rect(out: &mut Vec<HudVertex>, x: f32, y: f32, w: f32, h: f32, c: [u8; 4]) {
    quad(out, x, y, w, h, white_uv(), c, MODE_IMAGE);
}

/// A painted piece, stretched to the rectangle as it is.
pub fn image(out: &mut Vec<HudVertex>, name: &str, x: f32, y: f32, w: f32, h: f32, c: [u8; 4]) {
    quad(out, x, y, w, h, uv_of(atlas().piece(name).rect), c, MODE_IMAGE);
}

/// A thumbnail (`gfx::thumb_uv` gives its place).
pub fn thumb(out: &mut Vec<HudVertex>, uv: [f32; 4], x: f32, y: f32, w: f32, h: f32) {
    quad(out, x, y, w, h, uv, [255, 255, 255, 255], MODE_THUMB);
}

/// A picture from the thumbnails texture, multiplied by `c` (a sepia wash, a fade).
pub fn thumb_tinted(out: &mut Vec<HudVertex>, uv: [f32; 4], x: f32, y: f32, w: f32, h: f32, c: [u8; 4]) {
    quad(out, x, y, w, h, uv, c, MODE_THUMB);
}

/// A painted frame stretched over the rectangle without stretching its edges: its margins
/// are drawn `k` screen pixels per atlas pixel. Pieces with no top and bottom margins (bars,
/// plaques) keep their proportions in height and only stretch in the middle.
#[allow(clippy::too_many_arguments)]
pub fn nine(out: &mut Vec<HudVertex>, name: &str, x: f32, y: f32, w: f32, h: f32, k: f32, c: [u8; 4]) {
    let p = atlas().piece(name);
    let [rx, ry, rw, rh] = p.rect;
    let [ml, mt, mr, mb] = p.margins;
    let (sl, st, sr, sb, kk) = if mt == 0.0 && mb == 0.0 {
        let s = h / rh;
        (ml * s, 0.0, mr * s, 0.0, s)
    } else {
        (ml * k, mt * k, mr * k, mb * k, k)
    };
    // Never more margin than half the rectangle.
    let fx = ((w * 0.5) / sl.max(1e-3)).min(1.0).min((w * 0.5) / sr.max(1e-3));
    let fy = if st == 0.0 { 1.0 } else { ((h * 0.5) / st.max(1e-3)).min(1.0).min((h * 0.5) / sb.max(1e-3)) };
    let (sl, sr, st, sb) = (sl * fx, sr * fx, st * fy, sb * fy);
    let _ = kk;
    let xs = [x, x + sl, x + w - sr, x + w];
    let ys = [y, y + st, y + h - sb, y + h];
    let us = [rx, rx + ml, rx + rw - mr, rx + rw];
    let vs = [ry, ry + mt, ry + rh - mb, ry + rh];
    for j in 0..3 {
        for i in 0..3 {
            let (w2, h2) = (xs[i + 1] - xs[i], ys[j + 1] - ys[j]);
            if w2 > 0.01 && h2 > 0.01 {
                quad(out, xs[i], ys[j], w2, h2, uv_of([us[i], vs[j], us[i + 1] - us[i], vs[j + 1] - vs[j]]), c, MODE_IMAGE);
            }
        }
    }
}

/// Draws `s` with its baseline at `baseline`, starting at `x`; returns its width.
#[allow(clippy::too_many_arguments)]
pub fn text(out: &mut Vec<HudVertex>, font: FontId, s: &str, x: f32, baseline: f32, size: f32, c: [u8; 4]) -> f32 {
    let f = atlas().font(font);
    let k = size / f.em;
    let mut pen = x;
    for ch in s.chars() {
        let g = f.glyph(ch);
        if let Some(r) = g.rect {
            quad(out, pen + g.off[0] * k, baseline + g.off[1] * k, r[2] * k, r[3] * k, uv_of(r), c, MODE_SDF);
        }
        pen += g.adv * k;
    }
    pen - x
}

pub fn width(font: FontId, s: &str, size: f32) -> f32 {
    atlas().font(font).width(s, size)
}

/// The biggest size up to `size` at which `s` fits in `w`.
pub fn fit(font: FontId, s: &str, size: f32, w: f32) -> f32 {
    let full = width(font, s, size);
    if full <= w || full <= 0.0 {
        size
    } else {
        size * w / full
    }
}

/// Where the baseline goes for text of `size` centred on `cy`.
pub fn baseline_for(font: FontId, cy: f32, size: f32) -> f32 {
    cy + atlas().font(font).cap_height(size) * 0.5
}

/// Text centred on (cx, cy), with a shadow under it when `shadow` is set.
#[allow(clippy::too_many_arguments)]
pub fn centered(out: &mut Vec<HudVertex>, font: FontId, s: &str, cx: f32, cy: f32, size: f32, c: [u8; 4], shadow: bool) {
    let x = cx - width(font, s, size) * 0.5;
    let b = baseline_for(font, cy, size);
    if shadow {
        let o = (size * 0.06).max(1.0);
        text(out, font, s, x + o, b + o, size, [0, 0, 0, (c[3] as f32 * 0.75) as u8]);
    }
    text(out, font, s, x, b, size, c);
}

/// Text starting at `x`, centred on `cy`.
#[allow(clippy::too_many_arguments)]
pub fn left(out: &mut Vec<HudVertex>, font: FontId, s: &str, x: f32, cy: f32, size: f32, c: [u8; 4], shadow: bool) -> f32 {
    let b = baseline_for(font, cy, size);
    if shadow {
        let o = (size * 0.06).max(1.0);
        text(out, font, s, x + o, b + o, size, [0, 0, 0, (c[3] as f32 * 0.75) as u8]);
    }
    text(out, font, s, x, b, size, c)
}

/// Text ending at `right`, centred on `cy`.
#[allow(clippy::too_many_arguments)]
pub fn right(out: &mut Vec<HudVertex>, font: FontId, s: &str, right: f32, cy: f32, size: f32, c: [u8; 4], shadow: bool) {
    let x = right - width(font, s, size);
    left(out, font, s, x, cy, size, c, shadow);
}

/// An icon (by its Material Icons name), `size` pixels to the em, its drawing centred on
/// (cx, cy).
pub fn icon(out: &mut Vec<HudVertex>, name: &str, cx: f32, cy: f32, size: f32, c: [u8; 4]) {
    let a = atlas();
    let g = a.icon(name);
    let Some(r) = g.rect else { return };
    let k = size / a.icon_em;
    // Material icons are drawn centred in their em square, which sits on the baseline:
    // centre that square (not the ink, which would shift arrows and chevrons).
    let (x0, baseline) = (cx - g.adv * k * 0.5, cy + a.icon_em * k * 0.5);
    quad(out, x0 + g.off[0] * k, baseline + g.off[1] * k, r[2] * k, r[3] * k, uv_of(r), c, MODE_SDF);
}

/// A framed panel: weathered stone in an iron frame with bronze corner brackets. The frame
/// pieces are cut from the atlas at `k` screen pixels per atlas pixel.
pub fn panel(out: &mut Vec<HudVertex>, x: f32, y: f32, w: f32, h: f32, k: f32) {
    // The picture has 10 atlas pixels round the frame for the corner brackets.
    let p = 10.0 * k;
    nine(out, "panel", x - p, y - p, w + 2.0 * p, h + 2.0 * p, k, [255, 255, 255, 255]);
}

/// A sunken box with a thin iron and bronze rim (list rows, pictures, value boxes, fields).
pub fn field(out: &mut Vec<HudVertex>, x: f32, y: f32, w: f32, h: f32, k: f32) {
    nine(out, "field", x, y, w, h, k, [255, 255, 255, 255]);
}

/// Splits `s` into lines no wider than `w` at `size` (at spaces).
pub fn wrap(font: FontId, s: &str, size: f32, w: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in s.split_whitespace() {
        let next = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if !line.is_empty() && width(font, &next, size) > w {
            lines.push(std::mem::take(&mut line));
            line = word.to_string();
        } else {
            line = next;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}
