//! Terrain height, in the spirit of Vintage Story's world generation: a continent map
//! decides where the land and the sea are, and a map of *landforms* (flat plains, rolling
//! hills, plateaus, mountain ranges, wetlands...) decides the shape of the land. Landforms
//! are chosen per region and blended smoothly across region borders.
//!
//! Scale: about one block per metre. Plains sit a few blocks above the sea, high hills
//! reach 50-70 blocks, the tallest mountain ranges about 170.
//!
//! Every function here is a pure function of the seed and the position, and can be
//! band-limited for level-of-detail terrain (see `noise`).

use crate::noise::{smoothstep, Fbm};
use crate::rng::hash3;

/// The shape of one kind of land. Heights in blocks above the sea.
#[derive(Clone, Copy, Debug)]
pub struct Landform {
    pub name: &'static str,
    /// How common it is, relative to the others.
    pub weight: f32,
    /// Average height of the land.
    pub base: f32,
    /// Broad rolling hills (wavelength ~250 blocks): amplitude.
    pub hills: f32,
    /// Small bumps (wavelength ~50 blocks): amplitude.
    pub detail: f32,
    /// Ridged mountain ranges (wavelength ~900 blocks): height of the crests.
    pub ridges: f32,
    /// Height of terrace steps (plateaus and mesas), 0 for none.
    pub terrace: f32,
}

/// The landforms the world is made of. Add an entry to add a kind of land.
pub const LANDFORMS: &[Landform] = &[
    Landform { name: "plains", weight: 3.0, base: 3.5, hills: 3.0, detail: 1.0, ridges: 0.0, terrace: 0.0 },
    Landform { name: "rolling hills", weight: 3.0, base: 9.0, hills: 14.0, detail: 2.5, ridges: 0.0, terrace: 0.0 },
    Landform { name: "high hills", weight: 2.0, base: 18.0, hills: 26.0, detail: 3.5, ridges: 30.0, terrace: 0.0 },
    Landform { name: "plateau", weight: 1.0, base: 32.0, hills: 22.0, detail: 2.0, ridges: 0.0, terrace: 9.0 },
    Landform { name: "mountains", weight: 1.6, base: 26.0, hills: 22.0, detail: 4.0, ridges: 170.0, terrace: 0.0 },
    Landform { name: "wetlands", weight: 1.0, base: 0.9, hills: 1.6, detail: 1.3, ridges: 0.0, terrace: 0.0 },
];

/// Landform regions are about this wide (blocks).
const REGION: f32 = 900.0;
/// Continent values below this are sea.
pub const COAST: f32 = -0.12;

/// The blended landform parameters at a point.
#[derive(Clone, Copy, Debug, Default)]
struct Shape {
    base: f32,
    hills: f32,
    detail: f32,
    ridges: f32,
    /// Weighted terrace step height, and how much of the point is terraced.
    terrace: f32,
    terraced: f32,
}

/// Terrain at one point.
#[derive(Clone, Copy, Debug)]
pub struct TerrainSample {
    /// Surface height in blocks; the sea surface is at 0.
    pub height: f32,
    /// `height` without its small bumps and terrace steps: for things that should follow the
    /// lie of the land rather than every step (how cold it is up there).
    pub smooth: f32,
    /// 0 at the coast (and at sea), 1 well inland.
    pub inland: f32,
}

pub struct Terrain {
    seed: u64,
    continent: Fbm,
    warp_x: Fbm,
    warp_z: Fbm,
    region_warp_x: Fbm,
    region_warp_z: Fbm,
    hills: Fbm,
    detail: Fbm,
    ridges: Fbm,
    sea_floor: Fbm,
    /// Long mountain chains follow the zero lines of this noise, across landform regions.
    chains: Fbm,
    /// Running total of the landform weights, for weighted picks.
    cumulative: Vec<f32>,
}

impl Terrain {
    pub fn new(seed: u64) -> Self {
        let s = |k: u64| seed.wrapping_mul(0x2545_F491_4F6C_DD1D).wrapping_add(k);
        let mut total = 0.0;
        let cumulative = LANDFORMS
            .iter()
            .map(|l| {
                total += l.weight;
                total
            })
            .collect();
        Terrain {
            seed,
            continent: Fbm::new(s(1), 7000.0, 5, 0.5),
            warp_x: Fbm::new(s(2), 2600.0, 2, 0.5),
            warp_z: Fbm::new(s(3), 2600.0, 2, 0.5),
            region_warp_x: Fbm::new(s(4), 1100.0, 2, 0.5),
            region_warp_z: Fbm::new(s(5), 1100.0, 2, 0.5),
            hills: Fbm::new(s(6), 260.0, 3, 0.45),
            detail: Fbm::new(s(7), 52.0, 3, 0.5),
            ridges: Fbm::new(s(8), 900.0, 6, 0.5),
            sea_floor: Fbm::new(s(9), 400.0, 3, 0.5),
            chains: Fbm::new(s(10), 5200.0, 2, 0.45),
            cumulative,
        }
    }

    /// Continentalness: below `COAST` is sea, higher is further inland. The spawn (0, 0)
    /// is always on land.
    pub fn continent(&self, x: f32, z: f32, cell: f32) -> f32 {
        let wx = x + 1400.0 * self.warp_x.at(x, z, cell);
        let wz = z + 1400.0 * self.warp_z.at(x, z, cell);
        let spawn = 0.45 * (-(x * x + z * z) / (2600.0 * 2600.0)).exp();
        self.continent.at(wx, wz, cell) * 1.7 + spawn
    }

    /// Which landform region `(rx, rz)` is, and where its centre is.
    fn region(&self, rx: i32, rz: i32) -> (usize, f32, f32) {
        let h = hash3(self.seed ^ 0x1A4D_F0E5, rx, 0, rz);
        let total = *self.cumulative.last().unwrap();
        let pick = (h & 0xFFFF) as f32 / 65536.0 * total;
        let kind = self.cumulative.iter().position(|&c| pick < c).unwrap_or(LANDFORMS.len() - 1);
        let jx = 0.15 + 0.7 * ((h >> 16) & 0xFFFF) as f32 / 65536.0;
        let jz = 0.15 + 0.7 * ((h >> 32) & 0xFFFF) as f32 / 65536.0;
        (kind, (rx as f32 + jx) * REGION, (rz as f32 + jz) * REGION)
    }

    /// The landform parameters at a point: the nearby regions' landforms, weighted by
    /// closeness, so borders between them are gradual.
    fn shape(&self, x: f32, z: f32, cell: f32) -> Shape {
        // Warp the lookup so region borders wander instead of following a cell pattern.
        let wx = x + 320.0 * self.region_warp_x.at(x, z, cell);
        let wz = z + 320.0 * self.region_warp_z.at(x, z, cell);
        let (rx, rz) = ((wx / REGION).floor() as i32, (wz / REGION).floor() as i32);
        let mut s = Shape::default();
        let mut total = 0.0;
        let sigma2 = (0.42 * REGION) * (0.42 * REGION);
        for dz in -1..=1 {
            for dx in -1..=1 {
                let (kind, cx, cz) = self.region(rx + dx, rz + dz);
                let d2 = (wx - cx) * (wx - cx) + (wz - cz) * (wz - cz);
                if d2 > 9.0 * sigma2 {
                    continue; // weight below 1e-4
                }
                let w = (-d2 / sigma2).exp();
                let l = &LANDFORMS[kind];
                s.base += w * l.base;
                s.hills += w * l.hills;
                s.detail += w * l.detail;
                s.ridges += w * l.ridges;
                if l.terrace > 0.0 {
                    s.terrace += w * l.terrace;
                    s.terraced += w;
                }
                total += w;
            }
        }
        let inv = 1.0 / total.max(1e-6);
        s.base *= inv;
        s.hills *= inv;
        s.detail *= inv;
        s.ridges *= inv;
        if s.terraced > 0.0 {
            s.terrace /= s.terraced; // the step height of the terraced landforms
        }
        s.terraced *= inv;
        s
    }

    /// The landform at a point (the one with the most weight), for debugging and tools.
    pub fn landform_at(&self, x: f32, z: f32) -> &'static Landform {
        let wx = x + 320.0 * self.region_warp_x.at(x, z, 1.0);
        let wz = z + 320.0 * self.region_warp_z.at(x, z, 1.0);
        let (rx, rz) = ((wx / REGION).floor() as i32, (wz / REGION).floor() as i32);
        let mut best = (f32::MAX, 0);
        for dz in -1..=1 {
            for dx in -1..=1 {
                let (kind, cx, cz) = self.region(rx + dx, rz + dz);
                let d2 = (wx - cx) * (wx - cx) + (wz - cz) * (wz - cz);
                if d2 < best.0 {
                    best = (d2, kind);
                }
            }
        }
        &LANDFORMS[best.1]
    }

    /// Surface height and how far inland a point is, band-limited for `cell`.
    pub fn sample(&self, x: f32, z: f32, cell: f32) -> TerrainSample {
        let c = self.continent(x, z, cell);
        // How much of the point is land (a narrow band at the coast), and how far inland.
        let land = smoothstep(COAST - 0.03, COAST + 0.03, c);
        let inland = smoothstep(COAST, COAST + 0.45, c);

        // The sea floor: a shelf near the coast, deeper further out.
        let deep = smoothstep(COAST, COAST - 0.4, c);
        let sea = -3.0 - 48.0 * deep + 4.0 * self.sea_floor.at(x, z, cell);
        if land <= 0.0 {
            return TerrainSample { height: sea, smooth: sea, inland: 0.0 };
        }

        let mut s = self.shape(x, z, cell);
        // Mountain chains: narrow bands along the zero lines of a large noise, so ranges run
        // for many kilometres through whatever landforms they cross.
        let chain = smoothstep(0.86, 0.975, 1.0 - self.chains.at(x, z, cell).abs());
        if chain > 0.0 {
            s.base += (26.0 - s.base).max(0.0) * chain;
            s.hills += (22.0 - s.hills).max(0.0) * chain;
            s.ridges = s.ridges.max(170.0 * chain);
            s.terraced *= 1.0 - chain;
        }
        // Coasts are low: the land rises to its landform's full height over the first stretch
        // inland, so mountains don't usually drop straight into the sea.
        let rise = inland.sqrt();
        let mut h = s.base * rise;
        h += s.hills * rise * self.hills.at(x, z, cell);
        if s.ridges > 0.5 {
            // Ridged noise is ~0.1-0.3 in valleys and approaches 1 on the crests.
            let r = self.ridges.ridged(x, z, cell);
            h += s.ridges * rise * (r - 0.12).max(0.0);
        }
        let smooth_h = h;
        h += s.detail * self.detail.at(x, z, cell);
        if s.terraced > 0.0 && s.terrace > 0.0 {
            // Flat steps with steep risers between them.
            let f = h / s.terrace;
            let stepped = (f.floor() + smoothstep(0.55, 0.85, f.fract())) * s.terrace;
            h += (stepped - h) * s.terraced;
        }
        // Land stays at least a little above the sea, except in wetlands' ponds and at the coast.
        let height = sea + (h.max(-4.0) - sea) * land;
        let smooth = sea + (smooth_h.max(-4.0) - sea) * land;
        TerrainSample { height, smooth, inland }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_world_is_mostly_land_with_seas_and_some_tall_mountains() {
        let t = Terrain::new(1234);
        let (mut land, mut n, mut tall, mut max, mut min) = (0, 0, 0, f32::MIN, f32::MAX);
        let mut kinds = vec![0; LANDFORMS.len()];
        for i in 0..160 {
            for j in 0..160 {
                let (x, z) = (i as f32 * 250.0 - 20_000.0, j as f32 * 250.0 - 20_000.0);
                let s = t.sample(x, z, 1.0);
                n += 1;
                if s.height > 0.0 {
                    land += 1;
                    let k = LANDFORMS.iter().position(|l| l.name == t.landform_at(x, z).name).unwrap();
                    kinds[k] += 1;
                }
                if s.height > 90.0 {
                    tall += 1;
                }
                max = max.max(s.height);
                min = min.min(s.height);
            }
        }
        let frac = land as f32 / n as f32;
        eprintln!("land {frac:.2}, tall {tall}, heights {min:.0}..{max:.0}, landforms {kinds:?}");
        assert!((0.55..0.85).contains(&frac), "land fraction {frac}");
        assert!(tall > 20, "some mountains");
        assert!(max < 230.0 && min > -70.0, "{min}..{max}");
        assert!(kinds.iter().all(|&k| k > 0), "every landform appears: {kinds:?}");
        assert!(t.sample(0.0, 0.0, 1.0).height > 0.0, "the spawn is on land");
    }

    #[test]
    fn coarse_samples_agree_with_full_detail_on_average() {
        let t = Terrain::new(77);
        for cell in [4.0, 32.0, 256.0] {
            let mut d = 0.0;
            for i in 0..400 {
                let (x, z) = (i as f32 * 61.0 - 9000.0, i as f32 * 17.0 + 300.0);
                d += t.sample(x, z, cell).height - t.sample(x, z, 1.0).height;
            }
            let mean = d / 400.0;
            assert!(mean.abs() < 8.0, "cell {cell}: mean difference {mean}");
        }
    }
}
