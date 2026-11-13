//! Clouds, the way Vintage Story draws them: the sky above is divided into square tiles, and
//! every tile that holds cloud is a flat-bottomed box whose height follows the cloud's
//! density. Seen from the ground that reads as blocky cumulus; the boxes are slightly
//! see-through, so layers show through each other.
//!
//! The density is noise (a fine one for the clouds' shapes times a broad one that leaves some
//! regions clear and others overcast), fixed in "cloud space", which drifts with the wind. A
//! window of `GRID` x `GRID` tiles around the camera is turned into one mesh on a background
//! thread; the renderer only moves it with the wind, and a new mesh is made when the camera
//! has moved `RECENTRE` tiles from the middle of the window. Tiles are ordered far to near
//! from the window's middle, so blending them in that order is right, or close to it.
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
const STEP: f32 = 8.0;
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

/// The mesh of the window whose corner tile is (ox, oz): quads of four vertices.
pub fn build(density: &Density, ox: i32, oz: i32) -> Vec<Vertex> {
    let n = GRID as usize;
    let mut thick = vec![0.0f32; n * n];
    for z in 0..GRID {
        for x in 0..GRID {
            thick[z as usize * n + x as usize] = density.thickness(ox + x, oz + z);
        }
    }
    let at = |x: i32, z: i32| if x < 0 || z < 0 || x >= GRID || z >= GRID { 0.0 } else { thick[z as usize * n + x as usize] };
    // Far to near from the middle of the window.
    let mut order: Vec<(i32, i32, i32)> = Vec::new();
    for z in 0..GRID {
        for x in 0..GRID {
            if at(x, z) > 0.0 {
                let (dx, dz) = (2 * x + 1 - GRID, 2 * z + 1 - GRID);
                order.push((dx * dx + dz * dz, x, z));
            }
        }
    }
    order.sort_unstable_by_key(|t| std::cmp::Reverse(t.0));

    let mut out = Vec::with_capacity(order.len() * 12);
    for (_, x, z) in order {
        let t = at(x, z);
        let top = t as i32;
        // Thin edges of a cloud are wispier than its thick middle.
        let opacity = (150.0 + 90.0 * (t / MAX_THICKNESS)).min(240.0) as u32;
        let mut quad = |c: [(i32, i32, i32); 4], face: u32| {
            for (vx, vy, vz) in c {
                out.push(pack(vx, vz, vy, face, opacity));
            }
        };
        let (x1, z1) = (x + 1, z + 1);
        quad([(x, top, z), (x, top, z1), (x1, top, z1), (x1, top, z)], 2);
        quad([(x, 0, z), (x1, 0, z), (x1, 0, z1), (x, 0, z1)], 3);
        // Sides, down to the top of the neighbouring tile (nothing where it is as tall).
        for (face, nx, nz, a, b) in [
            (0u32, x + 1, z, (x1, z), (x1, z1)),
            (1, x - 1, z, (x, z1), (x, z)),
            (4, x, z + 1, (x1, z1), (x, z1)),
            (5, x, z - 1, (x, z), (x1, z)),
        ] {
            let low = at(nx, nz) as i32;
            if low < top {
                quad([(a.0, low, a.1), (b.0, low, b.1), (b.0, top, b.1), (a.0, top, a.1)], face);
            }
        }
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
    fn a_window_is_whole_quads_within_the_index_budget_ordered_far_to_near() {
        let d = Density::new(1337);
        let v = build(&d, -64, -64);
        assert!(v.len() % 4 == 0 && v.len() > 4000, "{} vertices", v.len());
        assert!(v.len() / 4 <= 100_000, "fits the shared index buffer");
        // Every vertex decodes inside the window and the height range.
        let dist = |q: &[Vertex]| {
            let cx = q.iter().map(|p| (p[0] & 255) as f32).sum::<f32>() / 4.0 - GRID as f32 / 2.0;
            let cz = q.iter().map(|p| ((p[0] >> 8) & 255) as f32).sum::<f32>() / 4.0 - GRID as f32 / 2.0;
            cx * cx + cz * cz
        };
        let mut last = f32::MAX;
        for q in v.chunks(4) {
            for p in q {
                assert!((p[0] & 255) <= GRID as u32 && ((p[0] >> 8) & 255) <= GRID as u32);
                assert!(((p[0] >> 16) & 1023) as f32 <= MAX_THICKNESS && (p[0] >> 26) < 6);
                assert!((100..=240).contains(&p[1]));
            }
            // Top faces come first for each tile and are in far-to-near order.
            if (q[0][0] >> 26) == 2 {
                let d2 = dist(q);
                assert!(d2 <= last + 1e-3, "tiles are ordered far to near");
                last = d2;
            }
        }
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
