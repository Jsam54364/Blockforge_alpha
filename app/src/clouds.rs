//! Clouds, the way Vintage Story draws them: the sky above is divided into square tiles, and
//! every tile that holds cloud is a flat-bottomed box whose height follows the cloud's
//! density. Seen from the ground that reads as blocky cumulus; the boxes are slightly
//! see-through, so layers show through each other.
//!
//! The density is noise (a fine one for the clouds' shapes times a broad one that leaves some
//! regions clear and others overcast), fixed in "cloud space", which drifts with the wind.
//!
//! **Levels of detail.** Around the camera the tiles are 32 blocks; further out they are 64,
//! 128 and 256 blocks (`LEVELS` square rings, each `SIDE` cells across with the next finer
//! level in its middle half), so every ring costs the same and the clouds reach 4.8 km for
//! about a third of the cells one 32-block grid would need to reach 1.7 km. Far away a big
//! tile is a few pixels, so nothing is lost; coarse tiles take the density at their centre
//! with the fine detail faded out (it would only flicker). The whole set is one mesh, built
//! on a background thread when the camera (in cloud space: the wind counts) crosses a
//! 256-block line, and moved with the wind in between. Tiles of one height share faces
//! (see `build`).
//!
//! Vertices use the terrain's 8-byte layout (two u32): see `pack`.

use std::sync::mpsc::{channel, Receiver, Sender};

use blockforge::mesh::FACE_CORNERS;
use blockforge::noise::Fbm;

/// The finest cloud tile is this many blocks wide.
pub const TILE: i32 = 32;
/// Levels of detail: tiles of 1, 2, 4 and 8 fine tiles.
pub const LEVELS: usize = 4;
/// Cells per side of each level's square (the next finer level fills its middle half).
pub const SIDE: i32 = 40;
/// The whole set's side, in fine tiles (the coarsest level's square).
pub const SPAN: i32 = SIDE << (LEVELS - 1);
/// The set is centred on a multiple of this many fine tiles (the coarsest cell): the camera
/// is never more than half of it from the middle, so the finest level always reaches 16
/// tiles (512 blocks) around it.
const SNAP: i32 = 1 << (LEVELS - 1);
/// Clouds have faded out completely this far from the camera, in blocks (inside the set even
/// with the camera half a `SNAP` off its middle).
pub const FADE_END: f32 = ((SPAN / 2 - SNAP) * TILE) as f32 - 192.0;
/// The clouds' flat bottoms are at this height, in blocks.
pub const BASE_Y: f32 = 290.0;
/// The tallest a cloud gets, and the steps its top comes in, in blocks.
const MAX_THICKNESS: f32 = 112.0;
const STEP: f32 = 16.0;
/// Wind: blocks per second along x and z.
pub const WIND: [f64; 2] = [3.2, 1.1];

pub type Vertex = [u32; 2];

/// a: x(9) | z(9) << 9 | y(10) << 18 | face(3) << 28   (x, z in fine tiles from the set's
///    corner, y in blocks above `BASE_Y`, face as the terrain's: +x -x +y -y +z -z)
/// b: opacity(8)
fn pack(x: i32, z: i32, y: i32, face: u32, opacity: u32) -> Vertex {
    [(x as u32 & 511) | ((z as u32 & 511) << 9) | ((y as u32 & 1023) << 18) | (face << 28), opacity]
}

/// Where clouds are: thickness in blocks (0 = clear sky).
pub struct Density {
    shape: Fbm,
    detail: Fbm,
    cover: Fbm,
}

impl Density {
    pub fn new(seed: u64) -> Density {
        Density {
            shape: Fbm::new(seed ^ 0xC10D_5EED, 620.0, 3, 0.5),
            detail: Fbm::new(seed ^ 0xC10D_0002, 150.0, 2, 0.5),
            cover: Fbm::new(seed ^ 0xC10D_0003, 6500.0, 2, 0.5),
        }
    }

    /// The fine tile at (tx, tz) in cloud space.
    pub fn thickness(&self, tx: i32, tz: i32) -> f32 {
        self.cell(tx, tz, 0)
    }

    /// A level-`level` cell whose first fine tile is (tx, tz): sampled at its middle, the
    /// fine detail fading out as cells grow (it would only flicker between neighbours).
    pub fn cell(&self, tx: i32, tz: i32, level: usize) -> f32 {
        let half = ((TILE << level) / 2) as f32;
        let (x, z) = ((tx * TILE) as f32 + half, (tz * TILE) as f32 + half);
        // Broad weather: from nearly clear to mostly covered.
        let cover = self.cover.at(x, z, 1.0) * 0.9;
        let detail = [0.35, 0.3, 0.2, 0.0][level.min(3)];
        let d = self.shape.at(x, z, 1.0) + detail * self.detail.at(x, z, 1.0) + cover - 0.07;
        if d <= 0.0 {
            return 0.0;
        }
        ((d * 420.0).min(MAX_THICKNESS) / STEP).ceil() * STEP
    }
}

/// Rectangles covering every cell of a `SIDE` x `SIDE` grid whose key is not 0, each
/// rectangle over cells with one key (greedy: as wide as possible, then as deep):
/// (x0, z0, x1, z1, key).
fn rectangles(key: &[i32]) -> Vec<(i32, i32, i32, i32, i32)> {
    let n = SIDE as usize;
    let mut used = vec![false; n * n];
    let mut out = Vec::new();
    for z in 0..n {
        for x in 0..n {
            let k = key[z * n + x];
            if k == 0 || used[z * n + x] {
                continue;
            }
            let mut x1 = x + 1;
            while x1 < n && key[z * n + x1] == k && !used[z * n + x1] {
                x1 += 1;
            }
            let mut z1 = z + 1;
            while z1 < n && (x..x1).all(|c| key[z1 * n + c] == k && !used[z1 * n + c]) {
                z1 += 1;
            }
            for r in z..z1 {
                used[r * n + x..r * n + x1].fill(true);
            }
            out.push((x as i32, z as i32, x1 as i32, z1 as i32, k));
        }
    }
    out
}

const BOTTOM_OPACITY: u32 = 215;

fn opacity(thickness: i32) -> u32 {
    // Thin edges of a cloud are wispier than its thick middle.
    (150.0 + 90.0 * (thickness as f32 / MAX_THICKNESS)).min(240.0) as u32
}

/// Where level `level`'s square starts in the set, in fine tiles from its corner.
fn level_offset(level: usize) -> i32 {
    (SPAN - (SIDE << level)) / 2
}

/// Whether cell (i, j) of level `level` is drawn by the finer level instead.
fn in_hole(level: usize, i: i32, j: i32) -> bool {
    level > 0 && (SIDE / 4..SIDE * 3 / 4).contains(&i) && (SIDE / 4..SIDE * 3 / 4).contains(&j)
}

/// The mesh of the set whose corner is fine tile (ox, oz): quads of four vertices, wound
/// counter-clockwise seen from outside (back faces are culled).
///
/// Neighbouring tiles of one height share faces: bottoms and tops are merged into rectangles,
/// sides into runs, which about halves the quads. Order: bottoms, tops, then sides far to
/// near. With depth writes on, whichever of bottom and top faces the camera is drawn before
/// anything that could be behind it, so each pixel of cloud is shaded once or twice instead
/// of once per layer; sides blend over what is behind them.
pub fn build(density: &Density, ox: i32, oz: i32) -> Vec<Vertex> {
    let mut out = Vec::new();
    // (x0, x1, y0, y1, z0, z1) in fine tiles and blocks, face, opacity
    let mut quad = |b: [i32; 6], face: usize, opacity: u32| {
        for c in FACE_CORNERS[face] {
            out.push(pack(b[c[0] as usize], b[4 + c[2] as usize], b[2 + c[1] as usize], face as u32, opacity));
        }
    };
    let n = SIDE as usize;
    let mut tops = Vec::new();
    let mut sides: Vec<(i32, usize, [i32; 6], u32)> = Vec::new();
    let mut covered_all = Vec::new();
    for level in 0..LEVELS {
        let unit = 1 << level;
        let off = level_offset(level);
        // The level's cells and a ring around them (neighbours for the sides: the next level
        // there is near enough the same).
        let g = SIDE + 2;
        let mut thick = vec![0i32; (g * g) as usize];
        for j in -1..=SIDE {
            for i in -1..=SIDE {
                thick[((j + 1) * g + i + 1) as usize] = density.cell(ox + off + i * unit, oz + off + j * unit, level) as i32;
            }
        }
        let at = |i: i32, j: i32| thick[((j + 1) * g + i + 1) as usize];
        let own: Vec<i32> = (0..n * n).map(|k| (k % n, k / n)).map(|(i, j)| if in_hole(level, i as i32, j as i32) { 0 } else { at(i as i32, j as i32) }).collect();
        let fine = |v: i32| off + v * unit;
        covered_all.push((level, own.iter().map(|t| (*t > 0) as i32).collect::<Vec<_>>()));
        for (x, z, x1, z1, t) in rectangles(&own) {
            tops.push(([fine(x), fine(x1), t, t, fine(z), fine(z1)], opacity(t)));
        }
        // Sides: down to the top of the neighbouring cell (nothing where it is as tall),
        // merged along the edge while both heights stay the same.
        for (face, dx, dz) in [(0usize, 1, 0), (1, -1, 0), (4, 0, 1), (5, 0, -1)] {
            for line in 0..SIDE {
                let cell = |k: i32| if dx != 0 { (line, k) } else { (k, line) };
                let heights = |k: i32| {
                    let (i, j) = cell(k);
                    if in_hole(level, i, j) {
                        (0, 0)
                    } else {
                        (at(i + dx, j + dz), at(i, j))
                    }
                };
                let mut k = 0;
                while k < SIDE {
                    let (low, top) = heights(k);
                    if low >= top {
                        k += 1;
                        continue;
                    }
                    let mut e = k + 1;
                    while e < SIDE && heights(e) == (low, top) {
                        e += 1;
                    }
                    let (a, b) = (fine(k), fine(e));
                    let l0 = fine(line);
                    let l1 = fine(line + 1);
                    let (bx, mid) = match face {
                        0 => ([l1, l1, low, top, a, b], (l1 * 2, a + b)),
                        1 => ([l0, l0, low, top, a, b], (l0 * 2, a + b)),
                        4 => ([a, b, low, top, l1, l1], (a + b, l1 * 2)),
                        _ => ([a, b, low, top, l0, l0], (a + b, l0 * 2)),
                    };
                    let (ddx, ddz) = (mid.0 - SPAN, mid.1 - SPAN);
                    sides.push((ddx * ddx + ddz * ddz, face, bx, opacity(top)));
                    k = e;
                }
            }
        }
    }
    // Bottoms are all at one height and get one opacity, so they merge whatever is above them.
    for (level, covered) in &covered_all {
        let (unit, off) = (1 << level, level_offset(*level));
        for (x, z, x1, z1, _) in rectangles(covered) {
            quad([off + x * unit, off + x1 * unit, 0, 0, off + z * unit, off + z1 * unit], 3, BOTTOM_OPACITY);
        }
    }
    for (b, o) in tops {
        quad(b, 2, o);
    }
    sides.sort_unstable_by_key(|s| std::cmp::Reverse(s.0));
    for (_, face, b, o) in sides {
        quad(b, face, o);
    }
    out
}

/// A finished set: its corner (fine tiles, cloud space) and its mesh.
pub struct Window {
    pub origin: (i32, i32),
    pub vertices: Vec<Vertex>,
}

/// Keeps a cloud window around the camera, built on a background thread.
pub struct Clouds {
    jobs: Sender<(i32, i32)>,
    done: Receiver<Window>,
    /// The corner of the window last asked for, and of the one the renderer has.
    asked: Option<(i32, i32)>,
    current: Option<(i32, i32)>,
}

/// The camera's tile in cloud space, `seconds` after the clouds started drifting.
fn camera_tile(cam: [f32; 3], seconds: f64) -> (i32, i32) {
    let x = cam[0] as f64 - WIND[0] * seconds;
    let z = cam[2] as f64 - WIND[1] * seconds;
    ((x / TILE as f64).floor() as i32, (z / TILE as f64).floor() as i32)
}

impl Clouds {
    pub fn new(seed: u64) -> Clouds {
        let (jobs, rx) = channel::<(i32, i32)>();
        let (tx, done) = channel();
        std::thread::Builder::new()
            .name("clouds".into())
            .spawn(move || {
                let density = Density::new(seed);
                while let Ok(mut job) = rx.recv() {
                    while let Ok(newer) = rx.try_recv() {
                        job = newer;
                    }
                    if tx.send(Window { origin: job, vertices: build(&density, job.0, job.1) }).is_err() {
                        return;
                    }
                }
            })
            .expect("clouds thread");
        Clouds { jobs, done, asked: None, current: None }
    }

    /// Call once a frame: asks for a new set when the camera has crossed a `SNAP` line (in
    /// cloud space: the wind counts), and returns a finished one to upload.
    pub fn update(&mut self, cam: [f32; 3], seconds: f64) -> Option<Window> {
        let (cx, cz) = camera_tile(cam, seconds);
        let snap = |v: i32| (v + SNAP / 2).div_euclid(SNAP) * SNAP;
        let want = (snap(cx) - SPAN / 2, snap(cz) - SPAN / 2);
        if self.asked != Some(want) {
            self.asked = Some(want);
            let _ = self.jobs.send(want);
        }
        let mut newest = None;
        while let Ok(w) = self.done.try_recv() {
            newest = Some(w);
        }
        if let Some(w) = &newest {
            self.current = Some(w.origin);
        }
        newest
    }

    /// Where the current window's corner is in the world right now (x, z in blocks), if
    /// there is a window.
    pub fn world_origin(&self, seconds: f64) -> Option<[f32; 2]> {
        self.current.map(|(ox, oz)| {
            [((ox * TILE) as f64 + WIND[0] * seconds) as f32, ((oz * TILE) as f64 + WIND[1] * seconds) as f32]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sky_is_partly_cloudy_with_flat_bottoms_and_stepped_tops() {
        let d = Density::new(1337);
        let (mut covered, mut total, mut tallest) = (0, 0, 0.0f32);
        for z in (-600..600).step_by(3) {
            for x in (-600..600).step_by(3) {
                let t = d.thickness(x, z);
                total += 1;
                if t > 0.0 {
                    covered += 1;
                    tallest = tallest.max(t);
                    assert!((STEP..=MAX_THICKNESS).contains(&t) && t % STEP == 0.0, "{t}");
                }
            }
        }
        let share = covered as f32 / total as f32;
        assert!((0.25..0.6).contains(&share), "cloud cover {share}");
        assert!(tallest >= MAX_THICKNESS - STEP, "some clouds tower: {tallest}");
    }

    /// What the set holds over each fine tile: (bottoms, top height) from its quads.
    fn coverage(v: &[Vertex]) -> (Vec<i32>, Vec<i32>) {
        let n = SPAN as usize;
        let (mut bottom, mut top) = (vec![0; n * n], vec![0; n * n]);
        for q in v.chunks(4) {
            let face = q[0][0] >> 28;
            if face != 2 && face != 3 {
                continue;
            }
            let x = |p: &Vertex| (p[0] & 511) as usize;
            let z = |p: &Vertex| ((p[0] >> 9) & 511) as usize;
            let (x0, x1) = (q.iter().map(x).min().unwrap(), q.iter().map(x).max().unwrap());
            let (z0, z1) = (q.iter().map(z).min().unwrap(), q.iter().map(z).max().unwrap());
            for tz in z0..z1 {
                for tx in x0..x1 {
                    if face == 3 {
                        bottom[tz * n + tx] += 1;
                    } else {
                        top[tz * n + tx] += 1;
                    }
                }
            }
        }
        (bottom, top)
    }

    #[test]
    fn the_levels_cover_the_sky_once_finest_near_the_middle() {
        let d = Density::new(1337);
        let (ox, oz) = (-SPAN / 2, -SPAN / 2);
        let v = build(&d, ox, oz);
        let quads = v.len() / 4;
        // Each fine tile is covered by exactly one level: once if that level's cell there
        // holds cloud, else not at all; tops and bottoms agree.
        let (bottom, top) = coverage(&v);
        let mut cells = 0;
        for level in 0..LEVELS {
            let (unit, off) = (1 << level, level_offset(level));
            for j in 0..SIDE {
                for i in 0..SIDE {
                    if in_hole(level, i, j) {
                        continue;
                    }
                    let want = (d.cell(ox + off + i * unit, oz + off + j * unit, level) > 0.0) as i32;
                    cells += want;
                    for tz in off + j * unit..off + (j + 1) * unit {
                        for tx in off + i * unit..off + (i + 1) * unit {
                            let k = (tz * SPAN + tx) as usize;
                            assert_eq!((bottom[k], top[k]), (want, want), "level {level} cell {i},{j}");
                        }
                    }
                }
            }
        }
        // Around the middle (the camera), the 32-block tiles.
        let mid = |level: usize| level_offset(level) + (SIDE << level) / 2;
        assert!((0..LEVELS).all(|l| mid(l) == SPAN / 2));
        // Cheap: a few thousand quads for 4.8 km (one 32-block grid took 10 000 for 1.7 km).
        assert!(quads < cells as usize * 3 && quads < 8000, "{quads} quads for {cells} cloudy cells");
        assert!(FADE_END >= 4500.0, "reaches {FADE_END}");
        // Every quad faces out: its corners wind counter-clockwise seen from outside.
        for q in v.chunks(4) {
            let p = |k: usize| {
                let a = q[k][0];
                [((a & 511) * 32) as f32, ((a >> 18) & 1023) as f32, (((a >> 9) & 511) * 32) as f32]
            };
            let (a, b, c) = (p(0), p(1), p(2));
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let n = [u[1] * w[2] - u[2] * w[1], u[2] * w[0] - u[0] * w[2], u[0] * w[1] - u[1] * w[0]];
            let want: [f32; 3] = match q[0][0] >> 28 {
                0 => [1.0, 0.0, 0.0],
                1 => [-1.0, 0.0, 0.0],
                2 => [0.0, 1.0, 0.0],
                3 => [0.0, -1.0, 0.0],
                4 => [0.0, 0.0, 1.0],
                _ => [0.0, 0.0, -1.0],
            };
            assert!(n[0] * want[0] + n[1] * want[1] + n[2] * want[2] > 0.0, "face {} winds inward", q[0][0] >> 28);
        }
    }

    #[test]
    fn the_set_follows_the_camera_and_the_wind() {
        let mut c = Clouds::new(7);
        let cam = [100.0, 80.0, -40.0];
        let mut got = None;
        for _ in 0..2000 {
            if let Some(w) = c.update(cam, 0.0) {
                got = Some(w);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let w = got.expect("a cloud set arrives");
        // Centred on the camera's tile, to the nearest SNAP.
        assert_eq!(w.origin, (-SPAN / 2, -SPAN / 2));
        // It drifts with the wind without being rebuilt...
        let o0 = c.world_origin(0.0).unwrap();
        let o1 = c.world_origin(10.0).unwrap();
        assert!((o1[0] - o0[0] - 32.0).abs() < 0.01 && (o1[1] - o0[1] - 11.0).abs() < 0.01);
        assert!(c.update(cam, 10.0).is_none() && c.asked == Some(w.origin));
        // ...until the wind has carried it far enough from the camera.
        let later = (SNAP * TILE) as f64 / WIND[0] + 5.0;
        c.update(cam, later);
        assert_ne!(c.asked, Some(w.origin));
    }
}
