//! Block textures, painted in code: 32 x 32, seamless, in a muted, realistic style.
//!
//! Each texture is built from tileable noise, Voronoi cells (pebbles, gravel, cracks,
//! leaves) and strokes (grass blades), then shaded by a height field so it reads with some
//! relief, like a photo texture. Everything is deterministic: the same name always gives
//! the same pixels, and nothing is loaded from files.

pub const SIZE: usize = 32;
const S: f32 = SIZE as f32;

/// RGBA8, sRGB, row 0 at the top (for side faces, the top of the block).
pub type Pixels = Vec<u8>;

fn hash(seed: u64, a: i32, b: i32) -> u64 {
    let mut h = seed ^ 0x9E37_79B9_7F4A_7C15;
    h = (h ^ a as u32 as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = (h ^ (h >> 29) ^ b as u32 as u64).wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^ (h >> 32)
}

/// Uniform 0..1 per (seed, a, b).
fn hf(seed: u64, a: i32, b: i32) -> f32 {
    (hash(seed, a, b) >> 40) as f32 / 16_777_216.0
}

fn name_seed(name: &str) -> u64 {
    name.bytes().fold(0xCBF2_9CE4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01B3))
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

type Rgb = [f32; 3];

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)]
}

fn scale(c: Rgb, k: f32) -> Rgb {
    [c[0] * k, c[1] * k, c[2] * k]
}

/// Value noise on an `n` x `n` lattice stretched over the tile, wrapping (so it tiles).
fn vnoise(seed: u64, x: f32, y: f32, nx: i32, ny: i32) -> f32 {
    let (fx, fy) = (x / S * nx as f32, y / S * ny as f32);
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let (tx, ty) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
    let (ix, iy) = (x0 as i32, y0 as i32);
    let at = |i: i32, j: i32| hf(seed, i.rem_euclid(nx), j.rem_euclid(ny));
    lerp(lerp(at(ix, iy), at(ix + 1, iy), tx), lerp(at(ix, iy + 1), at(ix + 1, iy + 1), tx), ty)
}

/// Fractal value noise, 0..1: `octaves` layers from an `n`-cell lattice up, doubling.
fn fbm(seed: u64, x: f32, y: f32, n: i32, octaves: u32, gain: f32) -> f32 {
    fbm2(seed, x, y, n, n, octaves, gain)
}

/// `fbm` with different lattice sizes across and down (streaks, bark).
fn fbm2(seed: u64, x: f32, y: f32, nx: i32, ny: i32, octaves: u32, gain: f32) -> f32 {
    let (mut sum, mut norm, mut amp) = (0.0, 0.0, 1.0);
    let (mut cx, mut cy) = (nx, ny);
    for o in 0..octaves {
        sum += amp * vnoise(seed.wrapping_add(o as u64 * 7919), x, y, cx, cy);
        norm += amp;
        amp *= gain;
        cx = (cx * 2).min(SIZE as i32);
        cy = (cy * 2).min(SIZE as i32);
    }
    sum / norm
}

/// Wrapping Voronoi over an `n` x `n` grid of jittered points: (distance to the nearest
/// point, to the second nearest, a hash of the nearest cell).
fn voronoi(seed: u64, x: f32, y: f32, n: i32) -> (f32, f32, u64) {
    let cell = S / n as f32;
    let (cx, cy) = ((x / cell).floor() as i32, (y / cell).floor() as i32);
    let (mut f1, mut f2, mut id) = (f32::MAX, f32::MAX, 0);
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (gx, gy) = (cx + dx, cy + dy);
            let (wx, wy) = (gx.rem_euclid(n), gy.rem_euclid(n));
            let h = hash(seed, wx, wy);
            let px = (gx as f32 + 0.15 + 0.7 * ((h & 0xFFFF) as f32 / 65536.0)) * cell;
            let py = (gy as f32 + 0.15 + 0.7 * (((h >> 16) & 0xFFFF) as f32 / 65536.0)) * cell;
            let d = ((x - px) * (x - px) + (y - py) * (y - py)).sqrt();
            if d < f1 {
                f2 = f1;
                f1 = d;
                id = h;
            } else if d < f2 {
                f2 = d;
            }
        }
    }
    (f1, f2, id)
}

/// A 32 x 32 canvas of colours and a height field for relief shading.
struct Canvas {
    col: Vec<Rgb>,
    height: Vec<f32>,
}

impl Canvas {
    fn new() -> Self {
        Canvas { col: vec![[0.0; 3]; SIZE * SIZE], height: vec![0.0; SIZE * SIZE] }
    }

    fn fill(&mut self, f: impl Fn(f32, f32) -> (Rgb, f32)) {
        for y in 0..SIZE {
            for x in 0..SIZE {
                let (c, h) = f(x as f32 + 0.5, y as f32 + 0.5);
                self.col[y * SIZE + x] = c;
                self.height[y * SIZE + x] = h;
            }
        }
    }

    fn h(&self, x: i32, y: i32) -> f32 {
        self.height[y.rem_euclid(SIZE as i32) as usize * SIZE + x.rem_euclid(SIZE as i32) as usize]
    }

    /// Light from the upper left: slopes facing it brighten, the others darken.
    fn shade(&mut self, strength: f32) {
        let mut out = self.col.clone();
        for y in 0..SIZE as i32 {
            for x in 0..SIZE as i32 {
                let dx = self.h(x + 1, y) - self.h(x - 1, y);
                let dy = self.h(x, y + 1) - self.h(x, y - 1);
                let k = (1.0 + strength * (dx * 0.6 + dy * 0.8) * -0.5).clamp(0.7, 1.3);
                out[y as usize * SIZE + x as usize] = scale(self.col[y as usize * SIZE + x as usize], k);
            }
        }
        self.col = out;
    }

    /// Per-pixel grain: each pixel brighter or darker by up to `amount`.
    fn grain(&mut self, seed: u64, amount: f32) {
        for (i, c) in self.col.iter_mut().enumerate() {
            let k = 1.0 + amount * (hf(seed, i as i32, 77) * 2.0 - 1.0);
            *c = scale(*c, k);
        }
    }

    fn pixels(&self) -> Pixels {
        let mut p = Vec::with_capacity(SIZE * SIZE * 4);
        for c in &self.col {
            for v in c {
                p.push((v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
            }
            p.push(255);
        }
        p
    }
}

fn stone(seed: u64) -> Canvas {
    let mut c = Canvas::new();
    c.fill(|x, y| {
        let h = fbm(seed, x, y, 8, 3, 0.55) * 0.55 + fbm(seed + 7, x, y, 2, 2, 0.5) * 0.45;
        // Cracks: thin lines along some Voronoi edges.
        let wx = x + 3.0 * (fbm(seed + 1, x, y, 4, 2, 0.5) - 0.5);
        let wy = y + 3.0 * (fbm(seed + 2, x, y, 4, 2, 0.5) - 0.5);
        let (f1, f2, id) = voronoi(seed + 3, wx, wy, 4);
        let crack = (1.0 - smoothstep(0.0, 0.8, f2 - f1)) * if id % 3 == 0 { 1.0 } else { 0.0 };
        let tint = fbm(seed + 4, x, y, 2, 2, 0.5) - 0.5;
        let mut col = mix([0.41, 0.41, 0.41], [0.56, 0.55, 0.54], smoothstep(0.2, 0.8, h));
        col = [col[0] + tint * 0.05, col[1] + tint * 0.03, col[2] - tint * 0.02];
        let r = hf(seed + 5, x as i32, y as i32);
        if r < 0.05 {
            col = scale(col, 0.78);
        } else if r > 0.97 {
            col = scale(col, 1.14);
        }
        col = scale(col, 1.0 - 0.3 * crack);
        (col, h * 1.4 - crack * 0.4)
    });
    c.shade(0.9);
    c.grain(seed + 6, 0.05);
    c
}

fn dirt_canvas(seed: u64) -> Canvas {
    let mut c = Canvas::new();
    c.fill(|x, y| {
        let base = fbm(seed, x, y, 4, 4, 0.6);
        let (cf1, cf2, cid) = voronoi(seed + 5, x, y, 10);
        let clod = 0.5 * fbm(seed + 1, x, y, 8, 3, 0.5) + 0.3 * ((cid >> 24) & 0xFF) as f32 / 255.0 + 0.2 * smoothstep(0.0, 1.5, cf2 - cf1);
        let _ = cf1;
        let mut col = mix([0.27, 0.19, 0.12], [0.47, 0.34, 0.22], smoothstep(0.2, 0.8, base * 0.6 + clod * 0.4));
        let mut h = clod * 0.7 + base * 0.4;
        // Pebbles: some Voronoi cells hold a small round stone.
        let (f1, _, id) = voronoi(seed + 2, x, y, 7);
        let r = 0.7 + 1.2 * ((id >> 20) & 0xFF) as f32 / 255.0;
        if id % 6 < 2 && f1 < r {
            let bump = (1.0 - (f1 / r) * (f1 / r)).sqrt();
            let g = 0.38 + 0.16 * ((id >> 32) & 0xFF) as f32 / 255.0;
            col = mix(col, [g * 1.08, g, g * 0.88], 0.85);
            h += bump * 1.0;
        }
        // Dark specks (roots, holes).
        if hf(seed + 3, x as i32, y as i32) < 0.06 {
            col = scale(col, 0.6);
            h -= 0.4;
        }
        (col, h)
    });
    c.shade(1.0);
    c.grain(seed + 4, 0.07);
    c
}

/// Colours for one kind of grass: (dark, mid, tip).
#[derive(Clone, Copy)]
struct GrassColours {
    dark: Rgb,
    mid: Rgb,
    tip: Rgb,
}

const GRASS: GrassColours = GrassColours { dark: [0.15, 0.28, 0.09], mid: [0.27, 0.45, 0.15], tip: [0.43, 0.59, 0.25] };
const DRY_GRASS: GrassColours = GrassColours { dark: [0.4, 0.34, 0.16], mid: [0.62, 0.55, 0.3], tip: [0.8, 0.74, 0.46] };
const LUSH_GRASS: GrassColours = GrassColours { dark: [0.06, 0.22, 0.06], mid: [0.15, 0.4, 0.1], tip: [0.31, 0.55, 0.17] };
const COLD_GRASS: GrassColours = GrassColours { dark: [0.15, 0.23, 0.14], mid: [0.28, 0.38, 0.25], tip: [0.42, 0.5, 0.35] };

/// Grass seen from above: a dark base covered in short blades of varied colour.
fn grass_top(seed: u64, g: GrassColours) -> Canvas {
    let mut c = Canvas::new();
    c.fill(|x, y| {
        let n = fbm(seed, x, y, 4, 3, 0.5);
        (mix(g.dark, g.mid, n * 0.6), n * 0.3)
    });
    // Blades: short strokes in random directions, each from dark at its root to light at
    // its tip, drawn with wrap-around so the tile stays seamless.
    for b in 0..420 {
        let h = hash(seed + 1, b, 0);
        let (x0, y0) = ((h & 31) as f32, ((h >> 5) & 31) as f32);
        let angle = ((h >> 10) & 0xFFF) as f32 / 4096.0 * std::f32::consts::TAU;
        let len = 2.0 + ((h >> 22) & 7) as f32 * 0.6;
        let shade = ((h >> 25) & 0xFF) as f32 / 255.0;
        let steps = (len * 2.0) as i32;
        for s in 0..=steps {
            let t = s as f32 / steps as f32;
            let (x, y) = (x0 + angle.cos() * len * t, y0 + angle.sin() * len * t);
            let i = (y.floor() as i32).rem_euclid(32) as usize * SIZE + (x.floor() as i32).rem_euclid(32) as usize;
            let along = mix(g.dark, if shade > 0.5 { g.tip } else { g.mid }, t * (0.5 + 0.5 * shade));
            c.col[i] = along;
            c.height[i] = 0.4 + t * 0.8 * shade;
        }
    }
    let tone_seed = seed + 2;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let k = 0.88 + 0.24 * fbm(tone_seed, x as f32, y as f32, 2, 2, 0.5);
            c.col[y * SIZE + x] = scale(c.col[y * SIZE + x], k);
        }
    }
    c.shade(0.7);
    c.grain(seed + 3, 0.05);
    c
}

/// The side of a grass block: dirt with a ragged fringe of grass hanging over the top.
fn grass_side(seed: u64, g: GrassColours) -> Canvas {
    let mut c = dirt_canvas(name_seed("dirt"));
    let top = grass_top(seed, g);
    for x in 0..SIZE {
        let r = hf(seed + 4, x as i32, 0);
        let mut len = 5.0 + 4.0 * fbm(seed + 5, x as f32, 0.0, 8, 2, 0.5) + r * 2.5;
        if r > 0.8 {
            len += 5.0 * hf(seed + 6, x as i32, 1); // a longer strand
        }
        let len = len.round() as usize;
        for y in 0..len.min(SIZE) {
            let i = y * SIZE + x;
            // Grass colours from the top texture, darkening toward the fringe's lower edge.
            let k = 1.0 - 0.35 * (y as f32 / len as f32);
            c.col[i] = scale(top.col[i], k);
        }
        // A thin shadow under the fringe.
        if len < SIZE {
            let i = len * SIZE + x;
            c.col[i] = scale(c.col[i], 0.62);
        }
    }
    c
}

fn sand(seed: u64) -> Canvas {
    let mut c = Canvas::new();
    c.fill(|x, y| {
        let n = fbm(seed, x, y, 4, 3, 0.5);
        // Low ripples, three across the tile, wandering a little.
        let w = y + 4.0 * (fbm(seed + 1, x, y, 2, 2, 0.5) - 0.5) + x * 0.25;
        let ripple = (w / S * 3.0 * std::f32::consts::TAU).sin() * 0.5 + 0.5;
        let mut col = mix([0.78, 0.71, 0.5], [0.89, 0.83, 0.63], n * 0.85 + ripple * 0.15);
        let r = hf(seed + 2, x as i32, y as i32);
        if r < 0.07 {
            col = scale(col, 0.84);
        } else if r > 0.95 {
            col = scale(col, 1.07);
        }
        (col, ripple * 0.2 + n * 0.4)
    });
    c.shade(0.5);
    c.grain(seed + 3, 0.035);
    c
}

fn gravel(seed: u64) -> Canvas {
    let mut c = Canvas::new();
    c.fill(|x, y| {
        let (f1, f2, id) = voronoi(seed, x, y, 6);
        let edge = (f2 - f1) * 0.5; // distance to the stone's rim
        let g = 0.3 + 0.24 * ((id >> 24) & 0xFF) as f32 / 255.0;
        let warm = 0.04 * (((id >> 40) & 0xFF) as f32 / 255.0 - 0.3);
        let mut col = [g + warm, g + warm * 0.4, g - warm * 0.5];
        let mut h = (edge / 2.2).min(1.0).sqrt();
        if edge < 0.55 {
            col = [0.16, 0.15, 0.14]; // the gaps between stones
            h = 0.0;
        }
        col = scale(col, 0.92 + 0.16 * fbm(seed + 1, x, y, 16, 1, 0.5));
        (col, h * 1.3)
    });
    c.shade(1.1);
    c.grain(seed + 2, 0.05);
    c
}

fn snow(seed: u64) -> Canvas {
    let mut c = Canvas::new();
    c.fill(|x, y| {
        let n = fbm(seed, x, y, 4, 3, 0.5);
        let col = mix([0.8, 0.86, 0.94], [0.95, 0.96, 0.98], smoothstep(0.2, 0.7, n));
        (col, n * 1.2)
    });
    c.shade(0.8);
    for i in 0..SIZE * SIZE {
        if hf(seed + 1, i as i32, 3) > 0.985 {
            c.col[i] = [1.0, 1.0, 1.0];
        }
    }
    c.grain(seed + 2, 0.015);
    c
}

fn water(seed: u64) -> Canvas {
    let mut c = Canvas::new();
    c.fill(|x, y| {
        let n = fbm2(seed, x, y, 4, 8, 3, 0.5);
        let r = fbm(seed + 1, x, y, 8, 2, 0.5);
        let col = mix([0.13, 0.3, 0.55], [0.27, 0.47, 0.75], smoothstep(0.3, 0.8, n * 0.7 + r * 0.3));
        (col, n)
    });
    c.shade(0.7);
    c
}

fn log_side(seed: u64) -> Canvas {
    let mut c = Canvas::new();
    c.fill(|x, y| {
        // Bark: furrows running up the trunk.
        let n = fbm2(seed, x, y, 8, 2, 3, 0.55);
        let furrow = smoothstep(0.38, 0.55, n);
        let col = mix([0.17, 0.12, 0.07], [0.43, 0.32, 0.21], furrow * 0.8 + 0.2 * fbm(seed + 1, x, y, 8, 1, 0.5));
        (col, furrow * 1.6)
    });
    c.shade(1.4);
    c.grain(seed + 2, 0.05);
    c
}

fn log_top(seed: u64) -> Canvas {
    let mut c = Canvas::new();
    c.fill(|x, y| {
        let (dx, dy) = (x - 16.0, y - 16.0);
        let edge = dx.abs().max(dy.abs());
        if edge > 14.0 {
            let n = fbm(seed + 3, x, y, 8, 2, 0.5);
            return (mix([0.18, 0.13, 0.08], [0.36, 0.27, 0.17], n), n);
        }
        let d = (dx * dx + dy * dy).sqrt() + 1.6 * fbm(seed, x, y, 4, 2, 0.5);
        let ring = (d * 1.25).sin() * 0.5 + 0.5;
        let col = mix([0.52, 0.38, 0.22], [0.72, 0.57, 0.36], ring * 0.7 + 0.3 * fbm(seed + 1, x, y, 16, 1, 0.5));
        (col, ring * 0.4)
    });
    c.shade(0.6);
    c.grain(seed + 2, 0.04);
    c
}

fn leaves(seed: u64) -> Canvas {
    let mut c = Canvas::new();
    c.fill(|x, y| {
        let (f1, _, id) = voronoi(seed, x, y, 9);
        let r = 1.6 + 0.9 * ((id >> 20) & 0xFF) as f32 / 255.0;
        if f1 > r {
            return ([0.05, 0.11, 0.04], 0.0); // gaps between the leaves
        }
        let t = ((id >> 32) & 0xFF) as f32 / 255.0;
        let col = mix([0.12, 0.3, 0.09], [0.28, 0.5, 0.18], t);
        let bump = 1.0 - f1 / r;
        (scale(col, 0.8 + 0.3 * bump), bump * 1.5)
    });
    c.shade(1.3);
    c.grain(seed + 1, 0.05);
    c
}

fn glowstone(seed: u64) -> Canvas {
    let mut c = Canvas::new();
    c.fill(|x, y| {
        let (f1, f2, id) = voronoi(seed, x, y, 5);
        let edge = f2 - f1;
        let t = ((id >> 24) & 0xFF) as f32 / 255.0;
        let mut col = mix([0.93, 0.66, 0.3], [1.0, 0.92, 0.6], t * 0.6 + 0.4 * (1.0 - f1 / 6.0).max(0.0));
        if edge < 0.9 {
            col = [0.5, 0.32, 0.12];
        }
        (col, edge.min(3.0) * 0.4)
    });
    c.shade(0.6);
    c
}

fn bedrock(seed: u64) -> Canvas {
    let mut c = Canvas::new();
    c.fill(|x, y| {
        let n = fbm(seed, x, y, 4, 4, 0.65);
        let (f1, f2, _) = voronoi(seed + 1, x, y, 4);
        let chunk = smoothstep(0.0, 2.0, f2 - f1);
        let g = 0.1 + 0.32 * smoothstep(0.3, 0.75, n) * chunk;
        ([g, g, g * 1.04], n * 2.0 + chunk)
    });
    c.shade(1.8);
    c.grain(seed + 2, 0.08);
    c
}

/// The texture called `name` (the names in `assets/blocks.toml`); unknown names get a
/// magenta and black checkerboard.
pub fn generate(name: &str) -> Pixels {
    let seed = name_seed(name);
    let canvas = match name {
        "stone" => stone(seed),
        "dirt" => dirt_canvas(seed),
        "grass_top" => grass_top(seed, GRASS),
        "grass_side" => grass_side(seed, GRASS),
        "dry_grass_top" => grass_top(seed, DRY_GRASS),
        "dry_grass_side" => grass_side(seed, DRY_GRASS),
        "lush_grass_top" => grass_top(seed, LUSH_GRASS),
        "lush_grass_side" => grass_side(seed, LUSH_GRASS),
        "cold_grass_top" => grass_top(seed, COLD_GRASS),
        "cold_grass_side" => grass_side(seed, COLD_GRASS),
        "sand" => sand(seed),
        "gravel" => gravel(seed),
        "snow" => snow(seed),
        "water" => water(seed),
        "log_side" => log_side(seed),
        "log_top" => log_top(seed),
        "leaves" => leaves(seed),
        "glowstone" => glowstone(seed),
        "bedrock" => bedrock(seed),
        _ => {
            let mut c = Canvas::new();
            c.fill(|x, y| {
                let on = ((x as i32 / 8) + (y as i32 / 8)) % 2 == 0;
                (if on { [1.0, 0.0, 1.0] } else { [0.0, 0.0, 0.0] }, 0.0)
            });
            c
        }
    };
    canvas.pixels()
}

/// Whether `generate` paints `name` (rather than the missing-texture checkerboard).
pub fn known(name: &str) -> bool {
    generate(name) != generate("\u{0}missing")
}

/// The next mip level: each pixel the average of a 2 x 2 block, averaged in linear light
/// (averaging sRGB values directly would darken distant terrain).
pub fn downsample(src: &[u8], size: usize) -> Pixels {
    let to_lin = |v: u8| (v as f32 / 255.0).powf(2.2);
    let half = size / 2;
    let mut out = Vec::with_capacity(half * half * 4);
    for y in 0..half {
        for x in 0..half {
            for ch in 0..4 {
                let px = |dx: usize, dy: usize| src[((2 * y + dy) * size + 2 * x + dx) * 4 + ch];
                let v = if ch == 3 {
                    (px(0, 0) as f32 + px(1, 0) as f32 + px(0, 1) as f32 + px(1, 1) as f32) / 4.0 / 255.0
                } else {
                    let l = (to_lin(px(0, 0)) + to_lin(px(1, 0)) + to_lin(px(0, 1)) + to_lin(px(1, 1))) / 4.0;
                    l.powf(1.0 / 2.2)
                };
                out.push((v * 255.0 + 0.5) as u8);
            }
        }
    }
    out
}

/// The average colour of a texture, linear RGB (for things drawn too small to texture).
pub fn average_linear(px: &[u8]) -> [f32; 3] {
    let mut sum = [0.0f32; 3];
    for p in px.chunks(4) {
        for ch in 0..3 {
            sum[ch] += (p[ch] as f32 / 255.0).powf(2.2);
        }
    }
    let n = (px.len() / 4) as f32;
    [sum[0] / n, sum[1] / n, sum[2] / n]
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [&str; 19] = [
        "stone", "dirt", "grass_top", "grass_side", "dry_grass_top", "dry_grass_side", "lush_grass_top", "lush_grass_side",
        "cold_grass_top", "cold_grass_side", "sand", "gravel", "snow", "water", "log_side", "log_top", "leaves", "glowstone",
        "bedrock",
    ];

    #[test]
    fn textures_are_deterministic_and_complete() {
        for n in ALL {
            let a = generate(n);
            assert_eq!(a.len(), SIZE * SIZE * 4);
            assert_eq!(a, generate(n), "{n}");
            assert!(known(n), "{n} has no painter");
        }
        assert!(!known("no_such_texture"));
    }

    #[test]
    fn tiling_textures_have_no_seam() {
        // Across the wrap-around edge, neighbouring pixels differ about as much as inside.
        for n in ALL {
            if n == "log_top" || n.ends_with("_side") && n != "log_side" {
                continue; // drawn once per face, not tiled seamlessly by design
            }
            let p = generate(n);
            let lum = |x: usize, y: usize| {
                let i = (y * SIZE + x) * 4;
                p[i] as f32 + p[i + 1] as f32 + p[i + 2] as f32
            };
            // Average difference between neighbours across the wrap edge vs everywhere inside.
            let (mut inside, mut seam) = (0.0, 0.0);
            for y in 0..SIZE {
                seam += (lum(SIZE - 1, y) - lum(0, y)).abs() + (lum(y, SIZE - 1) - lum(y, 0)).abs();
                for x in 0..SIZE - 1 {
                    inside += (lum(x, y) - lum(x + 1, y)).abs() + (lum(y, x) - lum(y, x + 1)).abs();
                }
            }
            let (seam, inside) = (seam / SIZE as f32, inside / (SIZE * (SIZE - 1)) as f32);
            assert!(seam < inside * 1.6 + 6.0, "{n}: seam {seam} vs inside {inside}");
        }
    }

    #[test]
    fn mips_average_in_linear_light() {
        let mut px = vec![0u8; 4 * 4 * 4];
        for (i, p) in px.chunks_mut(4).enumerate() {
            let v = if i % 2 == 0 { 255 } else { 0 };
            p.copy_from_slice(&[v, v, v, 255]);
        }
        let m = downsample(&px, 4);
        assert_eq!(m.len(), 2 * 2 * 4);
        // Half white, half black averages to linear 0.5: about 186 in sRGB, not 128.
        assert!((180..192).contains(&m[0]), "{}", m[0]);
        assert_eq!(m[3], 255);
    }
}
