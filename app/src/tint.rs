//! The biome tint map: what colour grass (and other tinted textures) is everywhere, and a
//! gentle brightness variation that hides how textures repeat (see `WorldGen::ground_tint`).
//!
//! Two levels of 256 x 256 texels each, centred on the camera: one texel per 16 blocks out to
//! about 2 km, one per 128 blocks out to 16 km (the LOD terrain). The GPU samples them with
//! bilinear filtering, so colours blend smoothly across biome borders, like Minecraft's biome
//! blend. Texel (tx, tz) is stored at (tx mod 256, tz mod 256), so a repeating sampler reads
//! straight from world coordinates; the shader only needs each level's centre. Levels are
//! computed on a background thread and replaced whole when the camera has moved far enough.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use blockforge::worldgen::WorldGen;

/// Texels per side of each level.
pub const TEXELS: i32 = 256;
/// Blocks per texel, per level.
pub const SCALES: [i32; 2] = [16, 128];
/// Recompute a level when the camera is this many texels from its centre.
const RECENTRE: i32 = 32;

pub type LevelData = Vec<u8>;

/// The RGBA data of level `level` centred on texel (cx, cz), laid out for upload.
pub fn level_data(gen: &WorldGen, level: usize, cx: i32, cz: i32) -> LevelData {
    let s = SCALES[level];
    let mut data = vec![0u8; (TEXELS * TEXELS * 4) as usize];
    for tz in cz - TEXELS / 2..cz + TEXELS / 2 {
        for tx in cx - TEXELS / 2..cx + TEXELS / 2 {
            let t = gen.ground_tint(tx * s + s / 2, tz * s + s / 2, s);
            let i = ((tz.rem_euclid(TEXELS) * TEXELS + tx.rem_euclid(TEXELS)) * 4) as usize;
            data[i..i + 4].copy_from_slice(&t);
        }
    }
    data
}

pub struct TintMap {
    jobs: Sender<(usize, i32, i32)>,
    done: Receiver<(usize, i32, i32, LevelData)>,
    /// The centre (in texels) of the data on the GPU, per level.
    centre: [Option<(i32, i32)>; 2],
    /// The centre last asked of the worker, per level.
    asked: [Option<(i32, i32)>; 2],
}

impl TintMap {
    pub fn new(gen: Arc<WorldGen>) -> TintMap {
        let (jobs, rx) = channel::<(usize, i32, i32)>();
        let (tx, done) = channel();
        std::thread::Builder::new()
            .name("tint map".into())
            .spawn(move || {
                while let Ok(mut job) = rx.recv() {
                    // Only the newest request per level matters.
                    let mut latest = [None, None];
                    latest[job.0] = Some(job);
                    while let Ok(j) = rx.try_recv() {
                        latest[j.0] = Some(j);
                    }
                    for j in latest.into_iter().flatten() {
                        job = j;
                        let data = level_data(&gen, job.0, job.1, job.2);
                        if tx.send((job.0, job.1, job.2, data)).is_err() {
                            return;
                        }
                    }
                }
            })
            .expect("tint map thread");
        TintMap { jobs, done, centre: [None, None], asked: [None, None] }
    }

    /// Asks for levels around the camera as it moves; returns finished levels to upload
    /// (level, data). Call once a frame.
    pub fn update(&mut self, cam: [f32; 3]) -> Vec<(usize, LevelData)> {
        for level in 0..2 {
            let s = SCALES[level] as f32;
            let want = ((cam[0] / s).floor() as i32, (cam[2] / s).floor() as i32);
            let far = |c: (i32, i32)| (c.0 - want.0).abs().max((c.1 - want.1).abs()) > RECENTRE;
            if self.asked[level].map_or(true, far) {
                self.asked[level] = Some(want);
                let _ = self.jobs.send((level, want.0, want.1));
            }
        }
        let mut out = Vec::new();
        while let Ok((level, cx, cz, data)) = self.done.try_recv() {
            self.centre[level] = Some((cx, cz));
            out.push((level, data));
        }
        out
    }

    /// Each level's centre in blocks (x0, z0, x1, z1), for the shader.
    pub fn window(&self) -> [f32; 4] {
        let c = |l: usize| self.centre[l].map_or((0.0, 0.0), |(x, z)| ((x * SCALES[l]) as f32, (z * SCALES[l]) as f32));
        let (a, b) = (c(0), c(1));
        [a.0, a.1, b.0, b.1]
    }

    /// Bit 0: level 0 is on the GPU; bit 1: level 1 is.
    pub fn ready_bits(&self) -> u32 {
        self.centre[0].is_some() as u32 | (self.centre[1].is_some() as u32) << 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blockforge::registry::BlockRegistry;

    #[test]
    fn texels_are_stored_where_a_repeating_sampler_finds_them() {
        let gen = WorldGen::new(5, &BlockRegistry::builtin());
        let data = level_data(&gen, 0, 1000, -37);
        // World texel (1000 + 3, -37 - 100) lands at (tx mod 256, tz mod 256).
        let (tx, tz): (i32, i32) = (1003, -137);
        let i = ((tz.rem_euclid(TEXELS) * TEXELS + tx.rem_euclid(TEXELS)) * 4) as usize;
        assert_eq!(&data[i..i + 4], &gen.ground_tint(tx * 16 + 8, tz * 16 + 8, 16));
    }

    #[test]
    fn levels_arrive_and_follow_the_camera() {
        let gen = Arc::new(WorldGen::new(5, &BlockRegistry::builtin()));
        let mut t = TintMap::new(gen);
        let mut got = Vec::new();
        let start = std::time::Instant::now();
        while t.ready_bits() != 3 {
            got.extend(t.update([100.0, 0.0, 200.0]).into_iter().map(|(l, _)| l));
            assert!(start.elapsed().as_secs() < 60);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(got.contains(&0) && got.contains(&1));
        assert_eq!(t.window(), [96.0, 192.0, 0.0, 128.0]);
        // A short move asks for nothing new; a long one recentres.
        t.update([300.0, 0.0, 200.0]);
        assert_eq!(t.asked[0], Some((6, 12)));
        t.update([100.0 + 16.0 * 40.0, 0.0, 200.0]);
        assert_eq!(t.asked[0], Some((46, 12)));
    }
}
