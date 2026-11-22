// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, you can obtain one at
// https://mozilla.org/MPL/2.0/.
//
// Rune Skovbo Johansen's erosion filter ("Fast and Gorgeous Erosion Filter",
// https://blog.runevision.com/2026/03/fast-and-gorgeous-erosion-filter.html, Shadertoy
// https://www.shadertoy.com/view/wXcfWn, MPL-2.0), building on work by Fewes, Clay John and
// Inigo Quilez. Translated to plain Rust (no dependencies) from the CPU port in the
// `bevy_erosion_filter` crate (MPL-2.0). Only this file is under the MPL.

//! Erosion as a filter: branching gullies and sharp ridges added to any height function,
//! every point evaluated on its own (no simulation, no neighbours), so it works chunk by
//! chunk in an infinite world. Give it the height and its slope at a point; it returns how
//! much to change the height there.
//!
//! Units: the position, the height and the slope must share one scale (pass positions and
//! heights divided by the same length, so slopes are real slopes).

use std::f32::consts::TAU;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    /// Size of the largest gullies, in the caller's units.
    pub scale: f32,
    /// How deep they cut (times `scale`), before each octave's `gain`.
    pub strength: f32,
    pub gully_weight: f32,
    /// How far finer octaves spread beyond the steep slopes.
    pub detail: f32,
    /// Ridge rounding, crease rounding, initial multiplier, per-octave multiplier.
    pub rounding: [f32; 4],
    /// Slope thresholds: erosion onset (input, octaves), ridge map (input, octaves).
    pub onset: [f32; 4],
    /// Idealised slope for gully directions, and how much of it to use.
    pub assumed_slope: [f32; 2],
    pub cell_scale: f32,
    pub normalization: f32,
    pub octaves: u32,
    pub lacunarity: f32,
    pub gain: f32,
    /// 0: gullies bend each finer octave by the sign of the wave (the reference: straight
    /// gullies, but the height jumps where the sign flips, which a block world shows as
    /// walls). Above 0: the sign eases over a band this many times narrower than the wave,
    /// so the height stays continuous.
    pub smooth_sign: f32,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            scale: 0.15,
            strength: 0.22,
            gully_weight: 0.5,
            detail: 1.5,
            rounding: [0.1, 0.0, 0.1, 2.0],
            onset: [1.25, 1.25, 2.8, 1.5],
            assumed_slope: [0.7, 1.0],
            cell_scale: 0.7,
            normalization: 0.5,
            octaves: 5,
            lacunarity: 2.0,
            gain: 0.5,
            smooth_sign: 0.0,
        }
    }
}

/// What the filter does at a point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Eroded {
    /// Change of height, and of its slope (x, z).
    pub dh: f32,
    pub dslope: [f32; 2],
    /// The sum of every octave's strength (the most the height could change).
    pub magnitude: f32,
    /// Ridges positive, creases negative: where water would gather (drainage streaks).
    pub ridge_map: f32,
    /// The fade target after the last octave run (what left-out octaves would pull toward).
    pub fade: f32,
}

fn fract(v: f32) -> f32 {
    v - v.floor()
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn clamp01(t: f32) -> f32 {
    t.clamp(0.0, 1.0)
}

fn pow_inv(t: f32, power: f32) -> f32 {
    1.0 - (1.0 - clamp01(t)).powf(power)
}

fn ease_out(t: f32) -> f32 {
    let v = 1.0 - clamp01(t);
    1.0 - v * v
}

fn smooth_start(t: f32, smoothing: f32) -> f32 {
    if t >= smoothing {
        return t - 0.5 * smoothing;
    }
    0.5 * t * t / smoothing
}

fn sign(v: f32) -> f32 {
    if v > 0.0 {
        1.0
    } else if v < 0.0 {
        -1.0
    } else {
        0.0
    }
}

/// Inigo Quilez's 2D hash, roughly -1..1 per component.
#[allow(clippy::approx_constant)]
fn hash2(x: f32, y: f32) -> (f32, f32) {
    let (kx, ky) = (0.318_309_9f32, 0.367_879_4f32);
    let (ax, ay) = (x * kx + ky, y * ky + kx);
    let s = fract(ax * ay * (ax + ay));
    (-1.0 + 2.0 * fract(16.0 * kx * s), -1.0 + 2.0 * fract(16.0 * ky * s))
}

/// Phacelle noise: a stripe pattern running along `dir` (unit), blended across a jittered
/// grid so it bends with the direction from cell to cell. Returns (cos wave, sin wave,
/// side direction x, z) with the side direction premultiplied by `freq * TAU`.
fn phacelle(px: f32, pz: f32, dir: (f32, f32), freq: f32, offset_cycles: f32, normalization: f32) -> [f32; 4] {
    let side = (-dir.1 * freq * TAU, dir.0 * freq * TAU);
    let offset = offset_cycles * TAU;
    let (ix, iz) = (px.floor(), pz.floor());
    let (fx, fz) = (px - ix, pz - iz);
    let (mut sum_c, mut sum_s, mut weight_sum) = (0.0f32, 0.0f32, 0.0f32);
    for i in -1..=2 {
        for j in -1..=2 {
            let (gi, gj) = (i as f32, j as f32);
            let (rx, rz) = hash2(ix + gi, iz + gj);
            let (vx, vz) = (fx - gi - rx * 0.5, fz - gj - rz * 0.5);
            let w = ((-(vx * vx + vz * vz) * 2.0).exp() - 0.011_11).max(0.0);
            weight_sum += w;
            let phase = vx * side.0 + vz * side.1 + offset;
            let (s, c) = phase.sin_cos();
            sum_c += c * w;
            sum_s += s * w;
        }
    }
    let (c, s) = (sum_c / weight_sum, sum_s / weight_sum);
    let m = (1.0 - normalization).max((c * c + s * s).sqrt());
    [c / m, s / m, side.0, side.1]
}

/// The filter at `(x, z)` for ground of height `h` with slope `(sx, sz)` there. `fade` is
/// -1 in valleys .. 1 on peaks (it decides whether flat ground is a crease or a crest);
/// `octaves` may be fewer than `p.octaves` to leave out detail (distant terrain).
pub fn erode(x: f32, z: f32, h: f32, sx: f32, sz: f32, fade: f32, p: &Params, octaves: u32) -> Eroded {
    let _ = h;
    let mut strength = p.strength * p.scale;
    let mut fade_target = fade.clamp(-1.0, 1.0);
    let (mut dh, mut dsx, mut dsz) = (0.0f32, 0.0f32, 0.0f32);
    let mut freq = 1.0 / (p.scale * p.cell_scale);
    let slope_len = (sx * sx + sz * sz).sqrt().max(1e-10);
    let mut magnitude = 0.0;
    let mut rounding_mult = 1.0;

    let rounding_in = mix(p.rounding[1], p.rounding[0], clamp01(fade_target + 0.5)) * p.rounding[2];
    let mut mask = ease_out(smooth_start(slope_len * p.onset[0], rounding_in * p.onset[0]));
    let mut ridge_mask = ease_out(slope_len * p.onset[2]);
    let mut ridge_fade = fade_target;

    let a = p.assumed_slope;
    let (mut gx, mut gz) = (mix(sx, sx / slope_len * a[0], a[1]), mix(sz, sz / slope_len * a[0], a[1]));

    for _ in 0..octaves.min(p.octaves) {
        let gl = (gx * gx + gz * gz).sqrt();
        let dir = if gl > 1e-10 { (gx / gl, gz / gl) } else { (gx, gz) };
        let ph = phacelle(x * freq, z * freq, dir, p.cell_scale, 0.25, p.normalization);
        let (zwx, zwz) = (ph[2] * -freq, ph[3] * -freq);
        let sloping = ph[1].abs();
        let sgn = if p.smooth_sign > 0.0 { (ph[1] * p.smooth_sign).clamp(-1.0, 1.0) } else { sign(ph[1]) };
        let k = sgn * strength * p.gully_weight;
        gx += zwx * k;
        gz += zwz * k;

        let (oh, osx, osz) = (ph[0], ph[1] * zwx, ph[1] * zwz);
        let fh = mix(fade_target, oh * p.gully_weight, mask);
        let fsx = osx * p.gully_weight * mask;
        let fsz = osz * p.gully_weight * mask;
        dh += fh * strength;
        dsx += fsx * strength;
        dsz += fsz * strength;
        magnitude += strength;
        fade_target = fh;

        let rounding_oct = mix(p.rounding[1], p.rounding[0], clamp01(ph[0] + 0.5)) * rounding_mult;
        let new_mask = ease_out(smooth_start(sloping * p.onset[1], rounding_oct * p.onset[1]));
        mask = pow_inv(mask, p.detail) * new_mask;

        ridge_fade = mix(ridge_fade, oh, ridge_mask);
        ridge_mask *= ease_out(sloping * p.onset[3]);

        strength *= p.gain;
        freq *= p.lacunarity;
        rounding_mult *= p.rounding[3];
    }
    Eroded { dh, dslope: [dsx, dsz], magnitude, ridge_map: ridge_fade * (1.0 - ridge_mask), fade: fade_target }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_ground_only_moves_toward_its_fade_target() {
        // No slope: no gullies, only the fade (crease or crest) applies.
        let p = Params::default();
        let e = erode(3.1, -7.4, 0.0, 0.0, 0.0, 0.0, &p, 5);
        assert!(e.dh.abs() < 1e-6 && e.ridge_map.abs() < 1e-3);
        let e = erode(3.1, -7.4, 0.0, 0.0, 0.0, -1.0, &p, 5);
        assert!((e.dh + e.magnitude).abs() < 1e-5, "{e:?}");
    }

    #[test]
    fn slopes_get_gullies_that_vary_across_the_slope() {
        let p = Params::default();
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for i in 0..200 {
            let x = i as f32 * 0.003;
            let e = erode(x, 0.0, 0.0, 0.0, 0.8, 0.0, &p, 5);
            lo = lo.min(e.dh);
            hi = hi.max(e.dh);
            assert!(e.dh.abs() <= e.magnitude + 1e-6);
        }
        assert!(hi - lo > 0.3 * p.strength * p.scale, "gullies {lo}..{hi}");
    }

    #[test]
    fn matches_the_reference_port() {
        // The value `bevy_erosion_filter`'s CPU port gives for this input (default params).
        let e = erode(0.3, 0.7, 0.5, 0.2, -0.1, 0.0, &Params::default(), 5);
        assert!((e.dh - 0.000_164_210_8).abs() < 2e-5, "{e:?}");
        assert!((e.magnitude - 0.063_937_5).abs() < 1e-5);
    }
}
