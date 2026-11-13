//! Clouds, the way Vintage Story draws them: the sky above is divided into square tiles, and
//! every tile that holds cloud is a flat-bottomed box whose height follows the cloud's
//! density. Seen from the ground that reads as blocky cumulus; the boxes are slightly
//! see-through, so layers show through each other.
//!
//! The density is noise (a fine one for the clouds' shapes times a broad one that leaves some
//! regions clear and others overcast), fixed in "cloud space", which drifts with the wind. A
//! window of `GRID` x `GRID` tiles around the camera is turned into one mesh on a background
//! thread; the renderer only moves it with the wind, and a new mesh is made when the camera
//! has moved `RECENTRE` tiles from the middle of the window. Tiles of one height share faces
//! (see `build`).
//!
//! Vertices use the terrain's 8-byte layout (two u32): see `pack`.

use std::sync::mpsc::{channel, Receiver, Sender};

use blockforge::noise::Fbm;

/// A cloud tile is this many blocks wide.
pub const TILE: i32 = 32;
/// Tiles per side of the window around the camera (4 km across).
pub const GRID: i32 = 128;
/// A new window is built when the camera is this many tiles from the middle of the current one.
const RECENTRE: i32 = 10;
/// Clouds have faded out completely this far from the camera, in blocks (inside the window
/// even when the camera is `RECENTRE` tiles off its middle).
pub const FADE_END: f32 = ((GRID / 2 - RECENTRE - 2) * TILE) as f32;
/// The clouds' flat bottoms are at this height, in blocks.
pub const BASE_Y: f32 = 290.0;
/// The tallest a cloud gets, and the steps its top comes in, in blocks.
const MAX_THICKNESS: f32 = 112.0;
const STEP: f32 = 16.0;
/// Wind: blocks per second along x and z.
pub const WIND: [f64; 2] = [3.2, 1.1];

pub type Vertex = [u32; 2];

/// a: x(8) | z(8) << 8 | y(10) << 16 | face(3) << 26   (x, z in tiles from the window's corner,
///    y in blocks above `BASE_Y`, face as the terrain's: +x -x +y -y +z -z)
/// b: opacity(8)
fn pack(x: i32, z: i32, y: i32, face: u32, opacity: u32) -> Vertex {
    [(x as u32 & 255) | ((z as u32 & 255) << 8) | ((y as u32 & 1023) << 16) | (face << 26), opacity]
}

/// Where clouds are: thickness in blocks (0 = clear sky) for the tile at (tx, tz) in cloud space.
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

    pub fn thickness(&self, tx: i32, tz: i32) -> f32 {
        let (x, z) = ((tx * TILE) as f32, (tz * TILE) as f32);
        // Broad weather: from nearly clear to mostly covered.
        let cover = self.cover.at(x, z, 1.0) * 0.9;
        let d = self.shape.at(x, z, 1.0) + 0.35 * self.detail.at(x, z, 1.0) + cover - 0.07;
        if d <= 0.0 {
            return 0.0;
        }
        ((d * 420.0).min(MAX_THICKNESS) / STEP).ceil() * STEP
    }
}

/// Rectangles covering every cell of a `GRID` x `GRID` grid whose key is not 0, each rectangle
/// over cells with one key (greedy: as wide as possible, then as deep): (x0, z0, x1, z1, key).
fn rectangles(key: &[i32]) -> Vec<(i32, i32, i32, i32, i32)> {
    let n = GRID as usize;
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

/// The mesh of the window whose corner tile is (ox, oz): quads of four vertices.
///
/// Neighbouring tiles of one height share faces: bottoms and tops are merged into rectangles,
/// sides into runs, which about halves the quads. Order: bottoms, tops,
/// then sides far to near. With depth writes on, whichever of bottom and top faces the camera
/// is drawn before anything that could be behind it, so each pixel of cloud is shaded once or
/// twice instead of once per layer; sides blend over what is behind them.
pub fn build(density: &Density, ox: i32, oz: i32) -> Vec<Vertex> {
    let n = GRID as usize;
    let mut thick = vec![0i32; n * n];
    for z in 0..GRID {
        for x in 0..GRID {
            thick[z as usize * n + x as usize] = density.thickness(ox + x, oz + z) as i32;
        }
    }
    let at = |x: i32, z: i32| if x < 0 || z < 0 || x >= GRID || z >= GRID { 0 } else { thick[z as usize * n + x as usize] };
    let mut out = Vec::new();
    let mut quad = |c: [(i32, i32, i32); 4], face: u32, opacity: u32| {
        for (vx, vy, vz) in c {
            out.push(pack(vx, vz, vy, face, opacity));
        }
    };
    // Bottoms are all at one height and get one opacity, so they merge whatever is above them.
    let covered: Vec<i32> = thick.iter().map(|t| (*t > 0) as i32).collect();
    for (x, z, x1, z1, _) in rectangles(&covered) {
        quad([(x, 0, z), (x1, 0, z), (x1, 0, z1), (x, 0, z1)], 3, BOTTOM_OPACITY);
    }
    for (x, z, x1, z1, t) in rectangles(&thick) {
        quad([(x, t, z), (x, t, z1), (x1, t, z1), (x1, t, z)], 2, opacity(t));
    }
    // Sides: down to the top of the neighbouring tile (nothing where it is as tall), merged
    // along the edge while both heights stay the same. (distance, face, line, from, to, low, top)
    let mut sides: Vec<(i32, u32, i32, i32, i32, i32, i32)> = Vec::new();
    for (face, dx, dz) in [(0u32, 1, 0), (1, -1, 0), (4, 0, 1), (5, 0, -1)] {
        for line in 0..GRID {
            let mut i = 0;
            while i < GRID {
                // `line` is the tile's x for the x faces, its z for the z faces.
                let cell = |j: i32| if dx != 0 { (line, j) } else { (j, line) };
                let heights = |j: i32| {
                    let (x, z) = cell(j);
                    (at(x + dx, z + dz), at(x, z))
                };
                let (low, top) = heights(i);
                if low >= top {
                    i += 1;
                    continue;
                }
                let mut j = i + 1;
                while j < GRID && heights(j) == (low, top) {
                    j += 1;
                }
                let (cx, cz) = cell((i + j) / 2);
                let (ddx, ddz) = (2 * cx + 1 - GRID, 2 * cz + 1 - GRID);
                sides.push((ddx * ddx + ddz * ddz, face, line, i, j, low, top));
                i = j;
            }
        }
    }
    sides.sort_unstable_by_key(|s| std::cmp::Reverse(s.0));
    for (_, face, line, a, b, low, top) in sides {
        let (p, q) = match face {
            0 => ((line + 1, a), (line + 1, b)),
            1 => ((line, b), (line, a)),
            4 => ((b, line + 1), (a, line + 1)),
            _ => ((a, line), (b, line)),
        };
        quad([(p.0, low, p.1), (q.0, low, q.1), (q.0, top, q.1), (p.0, top, p.1)], face, opacity(top));
    }
    out
}

/// A finished window: its corner tile (cloud space) and its mesh.
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

    /// Call once a frame: asks for a new window when the camera has moved far enough (in
    /// cloud space: the wind counts), and returns a finished one to upload.
    pub fn update(&mut self, cam: [f32; 3], seconds: f64) -> Option<Window> {
        let (cx, cz) = camera_tile(cam, seconds);
        let want = (cx - GRID / 2, cz - GRID / 2);
        let far = |o: (i32, i32)| (o.0 - want.0).abs().max((o.1 - want.1).abs()) >= RECENTRE;
        if self.asked.map_or(true, far) {
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

    #[test]
    fn a_window_is_merged_quads_that_cover_every_cloud_tile_exactly_once() {
        let d = Density::new(1337);
        let v = build(&d, -64, -64);
        assert!(v.len() % 4 == 0 && v.len() > 2000, "{} vertices", v.len());
        let x = |p: &Vertex| (p[0] & 255) as i32;
        let z = |p: &Vertex| ((p[0] >> 8) & 255) as i32;
        let y = |p: &Vertex| ((p[0] >> 16) & 1023) as i32;
        let (mut top, mut bottom) = (vec![0i32; (GRID * GRID) as usize], vec![0i32; (GRID * GRID) as usize]);
        let (mut tiles, mut last_side) = (0, i32::MAX);
        for q in v.chunks(4) {
            let face = q[0][0] >> 26;
            for p in q {
                assert!(x(p) <= GRID && z(p) <= GRID && y(p) as f32 <= MAX_THICKNESS && (p[0] >> 26) == face);
                assert!((100..=240).contains(&p[1]));
            }
            let (x0, x1) = (q.iter().map(x).min().unwrap(), q.iter().map(x).max().unwrap());
            let (z0, z1) = (q.iter().map(z).min().unwrap(), q.iter().map(z).max().unwrap());
            if face == 2 || face == 3 {
                for tz in z0..z1 {
                    for tx in x0..x1 {
                        let cell = &mut (if face == 2 { &mut top } else { &mut bottom })[(tz * GRID + tx) as usize];
                        *cell += 1;
                        // A top is at the height the density gives its tile.
                        assert!(face == 3 || y(&q[0]) as f32 == d.thickness(tx - 64, tz - 64));
                    }
                }
            } else {
                // Sides come far to near.
                let (cx, cz) = (x0 + x1 - GRID, z0 + z1 - GRID);
                let d2 = cx * cx + cz * cz;
                assert!(d2 <= last_side.saturating_add(8 * GRID), "sides are ordered far to near: {d2} after {last_side}");
                last_side = d2;
            }
        }
        for tz in 0..GRID {
            for tx in 0..GRID {
                let want = (d.thickness(tx - 64, tz - 64) > 0.0) as i32;
                tiles += want;
                assert_eq!((top[(tz * GRID + tx) as usize], bottom[(tz * GRID + tx) as usize]), (want, want), "tile {tx},{tz}");
            }
        }
        // Merging pays: under two quads per tile (unmerged, a tile has a top, a bottom and
        // about one and a half sides).
        assert!(v.len() / 4 < tiles as usize * 2, "{} quads for {tiles} tiles", v.len() / 4);
        assert!(v.len() / 4 <= 100_000, "fits the shared index buffer");
    }

    #[test]
    fn the_window_follows_the_camera_and_the_wind() {
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
        let w = got.expect("a cloud window arrives");
        assert_eq!(w.origin, (3 - GRID / 2, -2 - GRID / 2));
        // It drifts with the wind without being rebuilt...
        let o0 = c.world_origin(0.0).unwrap();
        let o1 = c.world_origin(10.0).unwrap();
        assert!((o1[0] - o0[0] - 32.0).abs() < 0.01 && (o1[1] - o0[1] - 11.0).abs() < 0.01);
        assert!(c.update(cam, 10.0).is_none() && c.asked == Some(w.origin));
        // ...until the wind has carried it far enough from the camera.
        let later = (RECENTRE * TILE) as f64 / WIND[0] + 5.0;
        c.update(cam, later);
        assert_ne!(c.asked, Some(w.origin));
        // The camera always stays inside the fade distance of whichever window is current.
        assert!(FADE_END <= ((GRID / 2 - RECENTRE) * TILE) as f32);
    }
}
