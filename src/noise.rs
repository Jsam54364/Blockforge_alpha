//! 2D gradient noise and fractal sums of it, for terrain and climate.
//!
//! Every fractal sum can be evaluated *band-limited*: for terrain drawn in cells `cell`
//! blocks wide (level-of-detail terrain), octaves whose wavelength nears the cell size fade
//! to their average instead of aliasing into a random-looking bump per cell. `cell <= 1`
//! is full detail.

/// 16 unit gradient directions, 22.5 degrees apart.
const GRADS: [(f32, f32); 16] = {
    let mut g = [(0.0f32, 0.0f32); 16];
    // cos/sin of k * 22.5 degrees (const fn trig isn't available, so spelled out).
    let c = [
        1.0, 0.923_879_5, 0.707_106_8, 0.382_683_4, 0.0, -0.382_683_4, -0.707_106_8, -0.923_879_5,
        -1.0, -0.923_879_5, -0.707_106_8, -0.382_683_4, 0.0, 0.382_683_4, 0.707_106_8, 0.923_879_5,
    ];
    let mut k = 0;
    while k < 16 {
        g[k] = (c[k], c[(k + 12) % 16]);
        k += 1;
    }
    g
};

/// Octave k is rotated by k * 0.5 radians so the lattices of different octaves don't line
/// up (that would show as grid-aligned ridges).
const ROT: [(f32, f32); 8] = [
    (1.0, 0.0),
    (0.877_582_6, 0.479_425_5),
    (0.540_302_3, 0.841_470_96),
    (0.070_737_2, 0.997_495),
    (-0.416_146_8, 0.909_297_4),
    (-0.801_143_6, 0.598_472_1),
    (-0.989_992_5, 0.141_120_01),
    (-0.936_456_9, -0.350_783_2),
];

fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// `x.floor() as i32` without a library call (`floor` is one on x86 without SSE4.1).
#[inline]
fn floor_i(x: f32) -> i32 {
    let i = x as i32;
    i - (x < i as f32) as i32
}

/// Which of the 16 gradients a lattice point gets: a cheap hash (two multiplies), good
/// enough in its top bits for this.
#[inline]
fn grad_index(seed: u64, i: i32, j: i32) -> usize {
    let mut h = seed ^ (i as u32 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (j as u32 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 31;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    (h >> 60) as usize
}

/// Gradient (Perlin) noise, roughly in -1..1 with mean 0.
pub fn perlin(seed: u64, x: f32, z: f32) -> f32 {
    let (ix, iz) = (floor_i(x), floor_i(z));
    let (fx, fz) = (x - ix as f32, z - iz as f32);
    let g = |i: i32, j: i32, dx: f32, dz: f32| {
        let (gx, gz) = GRADS[grad_index(seed, i, j)];
        gx * dx + gz * dz
    };
    let n00 = g(ix, iz, fx, fz);
    let n10 = g(ix + 1, iz, fx - 1.0, fz);
    let n01 = g(ix, iz + 1, fx, fz - 1.0);
    let n11 = g(ix + 1, iz + 1, fx - 1.0, fz - 1.0);
    let (u, v) = (fade(fx), fade(fz));
    lerp(lerp(n00, n10, u), lerp(n01, n11, u), v) * 1.41
}

/// How much of an octave of this wavelength to keep in terrain drawn as cells `cell`
/// blocks wide: all of it at two cells per wavelength or more, none at half a cell or less.
pub fn keep(wavelength: f32, cell: f32) -> f32 {
    if cell <= 1.0 {
        1.0
    } else {
        ((wavelength / cell - 0.5) / 1.5).clamp(0.0, 1.0)
    }
}

/// A fractal sum of `octaves` noise layers, each half the wavelength and `gain` times the
/// amplitude of the one before.
#[derive(Clone, Copy, Debug)]
pub struct Fbm {
    pub seed: u64,
    /// Wavelength of the first (largest) octave, in blocks.
    pub wavelength: f32,
    pub octaves: u32,
    pub gain: f32,
}

impl Fbm {
    pub const fn new(seed: u64, wavelength: f32, octaves: u32, gain: f32) -> Self {
        Fbm { seed, wavelength, octaves, gain }
    }

    fn octave(&self, o: u32, x: f32, z: f32, wl: f32) -> f32 {
        let (c, s) = ROT[o as usize % ROT.len()];
        let (rx, rz) = (x * c - z * s, x * s + z * c);
        perlin(self.seed.wrapping_add(o as u64 * 0x9E37), rx / wl, rz / wl)
    }

    /// The sum, normalised to roughly -1..1 (mean 0), band-limited for `cell`.
    pub fn at(&self, x: f32, z: f32, cell: f32) -> f32 {
        let (mut amp, mut wl) = (1.0f32, self.wavelength);
        let (mut sum, mut norm) = (0.0f32, 0.0f32);
        for o in 0..self.octaves {
            let k = keep(wl, cell);
            if k > 0.0 {
                sum += amp * k * self.octave(o, x, z, wl);
            }
            norm += amp;
            amp *= self.gain;
            wl *= 0.5;
        }
        sum / norm
    }

    /// Ridged multifractal, 0..1: sharp crests where the noise crosses zero, each octave
    /// weighted by the one before so detail gathers on the ridges (mountain ranges).
    pub fn ridged(&self, x: f32, z: f32, cell: f32) -> f32 {
        let (mut amp, mut wl) = (1.0f32, self.wavelength);
        let (mut sum, mut norm, mut weight) = (0.0f32, 0.0f32, 1.0f32);
        for o in 0..self.octaves {
            let k = keep(wl, cell);
            // A faded octave contributes its average instead (about a third of its weight).
            let mut n = 0.33 * weight;
            if k > 0.0 {
                let r = (1.0 - self.octave(o, x, z, wl).abs()).max(0.0);
                n = lerp(n, r * r * weight, k);
            }
            sum += amp * n;
            weight = (n * 2.0).clamp(0.0, 1.0);
            norm += amp;
            amp *= self.gain;
            wl *= 0.5;
        }
        sum / norm
    }
}

pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradients_are_unit_vectors() {
        for (x, z) in GRADS {
            assert!(((x * x + z * z) - 1.0).abs() < 1e-5);
        }
        for (c, s) in ROT {
            assert!(((c * c + s * s) - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn noise_is_zero_on_the_lattice_and_bounded() {
        let (mut lo, mut hi, mut sum) = (0.0f32, 0.0f32, 0.0f64);
        for i in 0..20_000 {
            let (x, z) = (i as f32 * 0.137 - 900.0, (i * 7 % 1013) as f32 * 0.291);
            let n = perlin(5, x, z);
            lo = lo.min(n);
            hi = hi.max(n);
            sum += n as f64;
        }
        assert!(lo > -1.1 && hi < 1.1 && lo < -0.6 && hi > 0.6, "{lo}..{hi}");
        assert!((sum / 20_000.0).abs() < 0.05);
        assert_eq!(perlin(5, 3.0, -7.0), 0.0);
    }

    #[test]
    fn band_limiting_fades_fine_octaves_only() {
        let f = Fbm::new(9, 512.0, 6, 0.5);
        // Full detail is unchanged by a cell of 1.
        assert_eq!(f.at(123.0, 45.0, 1.0), f.at(123.0, 45.0, 0.0));
        // Huge cells keep nothing: the average.
        assert_eq!(f.at(123.0, 45.0, 4096.0), 0.0);
        // At the same spacing, band-limited samples vary less than full detail.
        let rough = |cell: f32| {
            (0..500).map(|i| (f.at((i + 1) as f32 * 64.0, 9.0, cell) - f.at(i as f32 * 64.0, 9.0, cell)).abs()).sum::<f32>()
        };
        assert!(rough(64.0) < rough(1.0), "{} vs {}", rough(64.0), rough(1.0));
        let r = f.ridged(10.0, 20.0, 1.0);
        assert!((0.0..=1.0).contains(&r));
    }
}
