//! The land: its shape, climate, biomes and rivers. A pure function of (seed, x, z), so any
//! column can be worked out on its own, chunk by chunk, for an endless world, and band-limited
//! for distant terrain (see `noise`).
//!
//! Ideas from (no code from): Minecraft 1.18's terrain (continentalness for coast to inland,
//! "erosion" for flat to mountainous, blended by curves), AlcatrazEscapee's survey of river
//! generation for chunked worlds (rivers as warped noise channels: "context free", so they work
//! anywhere without knowing the rest of the world), Runevision's erosion filter (gullies and
//! ridges on mountains, see `erosion`), and Inigo Quilez's domain warping (natural outlines).
//!
//! Scale: a block is a metre. The sea is at y = 0; plains lie a few metres above it, hills
//! reach 40-60, mountains up to about 300.
//!
//! How a column is made, in order:
//! 1. **Where the land is** (`continent`, domain-warped, ~7 km features): ocean below 0, coast,
//!    inland. Inland ground rises gently.
//! 2. **How rough it is** (`roughness`, ~2.6 km): flat regions are vast grassy plains with
//!    barely a ripple; rough ones carry hills, and the roughest mountain ranges (ridged noise,
//!    sharpened by the erosion filter into gullies and crests).
//! 3. **Cliffs**: in some regions the land steps up in sheer escarpments (warped terrace steps
//!    a few blocks wide and 20-30 tall), as mesas in hot dry land; where highland meets the
//!    sea the coast rises as sea cliffs.
//! 4. **Climate**: temperature and humidity (~4-5 km), colder with altitude, wetter near the
//!    coast and drier in the lee of mountains. They pick the biome.
//! 5. **Dunes** in the dry heart of deserts: long ridges across a steady wind.
//! 6. **Rivers**: two networks of winding channels (big rivers and tributaries) in warped
//!    noise. Their water lies on a smooth level that falls toward the sea; the land slopes
//!    down into a valley on each side, the bed is U-shaped, and the banks never drop below the
//!    water (so no water hangs in the air). They widen toward the sea and fade out high in the
//!    mountains, where only their valleys remain.

use crate::erosion;
use crate::noise::{smoothstep, Fbm};

/// The sea surface: water fills up to and including this y.
pub const SEA: f32 = 0.0;
/// No water over a column.
pub const NO_WATER: f32 = -1.0e9;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Biome {
    Ocean,
    Beach,
    /// Vast flat grassland with scattered groves.
    Plains,
    /// Dry grassland (prairie) in cooler, drier country.
    Steppe,
    /// Temperate woods: oak, birch, maple.
    Forest,
    /// Old-growth forest: huge redwoods and spruces, a dark floor.
    OldForest,
    Swamp,
    /// Cold coniferous forest.
    Taiga,
    Tundra,
    /// Bare rock and snow high in the mountains.
    Peaks,
    Desert,
    /// Red rock steps and canyons in hot dry land.
    Mesa,
    Savanna,
    Jungle,
}

impl Biome {
    pub fn name(self) -> &'static str {
        match self {
            Biome::Ocean => "ocean",
            Biome::Beach => "beach",
            Biome::Plains => "plains",
            Biome::Steppe => "steppe",
            Biome::Forest => "forest",
            Biome::OldForest => "old-growth forest",
            Biome::Swamp => "swamp",
            Biome::Taiga => "taiga",
            Biome::Tundra => "tundra",
            Biome::Peaks => "peaks",
            Biome::Desert => "desert",
            Biome::Mesa => "mesa",
            Biome::Savanna => "savanna",
            Biome::Jungle => "jungle",
        }
    }

    /// How many trees grow per unit of ground (0..1, before the patchiness of forests).
    pub fn tree_density(self) -> f32 {
        match self {
            Biome::Forest => 0.85,
            Biome::OldForest => 0.95,
            Biome::Taiga => 0.8,
            Biome::Jungle => 0.95,
            Biome::Swamp => 0.45,
            Biome::Plains => 0.05,
            Biome::Steppe => 0.02,
            Biome::Savanna => 0.12,
            Biome::Tundra => 0.04,
            Biome::Desert => 0.03, // cacti
            Biome::Mesa => 0.02,
            Biome::Beach => 0.03, // palms
            Biome::Peaks | Biome::Ocean => 0.0,
        }
    }
}

/// One column of land.
#[derive(Clone, Copy, Debug)]
pub struct LandSample {
    /// The ground's surface, in blocks (fractional).
    pub height: f32,
    /// The water surface over it (`NO_WATER` for none): the sea, or a river.
    pub water: f32,
    pub biome: Biome,
    /// Temperature in degrees C and humidity 0..1 (for display and for ground cover).
    pub temperature: f32,
    pub humidity: f32,
    /// Raw climate, -1..1 (cold..hot, dry..wet), altitude included.
    pub t: f32,
    pub h: f32,
    /// Trees per unit of ground here, 0..1.
    pub forest: f32,
    /// How close a river is: 1 in its channel, falling to 0 at the edge of its valley.
    pub river: f32,
    /// How much of the column is cliff country (escarpments, mesas), 0..1.
    pub cliffs: f32,
    /// Mountain-ness 0..1 and flatness 0..1 of the region.
    pub mountains: f32,
    pub flat: f32,
    /// What kind of land this is, for display.
    pub form: &'static str,
}

/// The shape of the land before climate, dunes and rivers.
#[derive(Clone, Copy, Debug)]
struct Relief {
    height: f32,
    /// The broad level of the land (no hills, a fraction of the mountains): rivers lie on it.
    smooth: f32,
    continent: f32,
    inland: f32,
    mountains: f32,
    flat: f32,
    cliffs: f32,
    /// The ground without mountains and cliffs (the valley floor erosion fades toward).
    floor: f32,
    /// The climate at sea level: temperature and humidity -1..1 (before altitude and
    /// mountains), and how much of a desert it makes (0..1).
    t_sea: f32,
    h_raw: f32,
    #[allow(dead_code)]
    dry: f32,
    /// How much cliff terraces raise the land here (faded out in river valleys).
    terrace: f32,
}

pub struct Land {
    warp_x: Fbm,
    warp_z: Fbm,
    continent: Fbm,
    roughness: Fbm,
    ridges: Fbm,
    hills: Fbm,
    detail: Fbm,
    cliff_regions: Fbm,
    plateaus: Fbm,
    sea_cliffs: Fbm,
    temperature: Fbm,
    humidity: Fbm,
    jitter: Fbm,
    dunes: Fbm,
    river_warp_x: Fbm,
    river_warp_z: Fbm,
    rivers: Fbm,
    streams: Fbm,
    groves: Fbm,
    erosion: erosion::Params,
    /// The wind over deserts (dunes run across it), as a unit vector.
    wind: (f32, f32),
}

/// Positions and heights are divided by this for the erosion filter (so its slopes are real).
const EROSION_UNIT: f32 = 600.0;

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

impl Land {
    pub fn new(seed: u64) -> Land {
        let s = |k: u64| seed ^ k.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let angle = (seed % 6283) as f32 / 1000.0;
        Land {
            warp_x: Fbm::new(s(1), 2200.0, 3, 0.5),
            warp_z: Fbm::new(s(2), 2200.0, 3, 0.5),
            continent: Fbm::new(s(3), 7000.0, 5, 0.5),
            roughness: Fbm::new(s(4), 3600.0, 4, 0.5),
            ridges: Fbm::new(s(5), 1300.0, 6, 0.5),
            hills: Fbm::new(s(6), 420.0, 4, 0.5),
            detail: Fbm::new(s(7), 64.0, 3, 0.5),
            cliff_regions: Fbm::new(s(8), 3200.0, 3, 0.5),
            plateaus: Fbm::new(s(9), 900.0, 2, 0.5),
            sea_cliffs: Fbm::new(s(10), 1500.0, 3, 0.5),
            temperature: Fbm::new(s(11), 9000.0, 3, 0.5),
            humidity: Fbm::new(s(12), 7000.0, 3, 0.5),
            jitter: Fbm::new(s(13), 90.0, 2, 0.5),
            dunes: Fbm::new(s(14), 1.0, 3, 0.45),
            river_warp_x: Fbm::new(s(15), 900.0, 2, 0.5),
            river_warp_z: Fbm::new(s(16), 900.0, 2, 0.5),
            rivers: Fbm::new(s(17), 3400.0, 3, 0.45),
            streams: Fbm::new(s(18), 1200.0, 3, 0.45),
            groves: Fbm::new(s(19), 260.0, 3, 0.5),
            erosion: erosion::Params { scale: 0.35, strength: 0.2, octaves: 4, smooth_sign: 2.0, ..erosion::Params::default() },
            wind: (angle.cos(), angle.sin()),
        }
    }

    fn relief(&self, x: f32, z: f32, cell: f32) -> Relief {
        // Domain warping: coastlines and regions bend instead of following noise blobs.
        let wx = x + 420.0 * self.warp_x.at(x, z, cell);
        let wz = z + 420.0 * self.warp_z.at(x, z, cell);
        let c = self.continent.at(wx, wz, cell) * 1.7 + 0.2;
        let e = self.roughness.at(wx * 0.8 + 5000.0, wz * 0.8, cell) * 1.8;
        let inland = smoothstep(0.0, 0.6, c);
        let mountains = smoothstep(-0.1, -0.55, e) * smoothstep(0.05, 0.3, c);
        let flat = smoothstep(0.05, 0.45, e) * (1.0 - mountains);

        // Ocean floor: shelves near the coast, deep further out.
        let hills = self.hills.at(x, z, cell);
        let ocean = -5.0 - 55.0 * smoothstep(0.0, -0.45, c) + 3.0 * hills;
        // Land: rises gently inland; plains stay low and flat.
        let base = 3.0 + 26.0 * inland * (1.0 - 0.65 * flat);
        let swell = hills * lerp(17.0, 2.0, flat) * (1.0 - 0.4 * mountains);
        let rolling = swell + self.detail.at(x, z, cell) * lerp(2.8, 0.5, flat);
        let r = self.ridges.ridged_soft(wx, wz, cell, 0.08);
        let peaks = (r.powf(1.8) * 330.0 + 30.0 * hills.abs()) * mountains;

        // The climate at sea level (it decides where mesas stand).
        let jt = 0.05 * self.jitter.at(x, z, cell);
        let t_sea = self.temperature.at(x, z, cell) * 1.6 + jt;
        let h_raw = self.humidity.at(x + 3000.0, z, cell) * 1.6 + 0.25 * (1.0 - inland) - jt;
        let dry = smoothstep(0.15, 0.4, t_sea) * smoothstep(-0.05, -0.4, h_raw);

        // Cliff country: escarpments, and mesas in hot dry land (see `sample`): the land
        // steps up in warped terraces with near-vertical risers. Vast plains stay open; deserts
        // don't (their flat ground is where mesas stand).
        let regions = self.cliff_regions.at(x + 9000.0, z, cell);
        let cliffs = smoothstep(lerp(0.25, 0.05, dry), lerp(0.5, 0.3, dry), regions)
            * smoothstep(0.15, 0.35, c)
            * (1.0 - smoothstep(0.3, 0.6, mountains))
            * (1.0 - 0.85 * smoothstep(0.3, 0.8, flat) * (1.0 - dry));
        let mut steps = 0.0;
        if cliffs > 0.0 {
            let p = self.plateaus.at(wx * 1.3 + 300.0, wz * 1.3, cell);
            for k in 0..3 {
                let t = -0.12 + k as f32 * 0.17;
                // ~0.02 of noise is a few blocks: sheer.
                let w = (0.012 + cell / 2500.0).min(0.08);
                steps += smoothstep(t - w, t + w, p) * [26.0, 22.0, 18.0][k];
            }
        }

        let land = base + rolling + peaks + steps * cliffs;
        // Sea cliffs: where the coast is cliffy, the land steps up within a few blocks of it.
        // (The land of a cliffy coast stands higher all the way inland: highland meeting the sea.)
        let sea_cliff = if c > 0.004 { smoothstep(0.15, 0.45, self.sea_cliffs.at(x, z, cell)) * 22.0 * smoothstep(0.004, 0.009, c) } else { 0.0 };
        let k = smoothstep(-0.03, 0.03, c);
        let height = lerp(ocean, land + sea_cliff, k);
        let smooth = lerp(ocean, base + 0.25 * peaks, k);
        Relief { height, smooth, continent: c, inland, mountains, flat, cliffs, floor: lerp(ocean, base, k), t_sea, h_raw, dry, terrace: steps * cliffs * k }
    }

    /// The land at `(x, z)`, band-limited for terrain drawn in cells `cell` blocks wide
    /// (`cell <= 1`: full detail).
    pub fn sample(&self, x: f32, z: f32, cell: f32) -> LandSample {
        let r = self.relief(x, z, cell);
        let slope = if self.needs_slope(&r, cell) { self.broad_slope(x, z, cell, &mut |ix, iz, s| self.lattice_height(ix, iz, s, cell)) } else { (0.0, 0.0) };
        self.finish(x, z, cell, r, slope)
    }

    /// Spacing of the lattice the broad slope is taken on, for cells `cell` blocks wide.
    fn lattice_step(cell: f32) -> f32 {
        (cell * 2.0).max(16.0)
    }

    /// How many erosion octaves terrain drawn in cells `cell` blocks wide shows: octaves
    /// finer than about two cells would only alias.
    fn erosion_octaves(&self, cell: f32) -> u32 {
        let p = &self.erosion;
        let mut octaves = 0;
        let mut wl = p.scale * p.cell_scale * EROSION_UNIT;
        while octaves < p.octaves && wl >= 2.0 * cell {
            octaves += 1;
            wl /= p.lacunarity;
        }
        octaves
    }

    /// Whether a point needs the broad slope: erosion acts there and some of its octaves
    /// show at this distance (without them only its average height applies, no slope).
    fn needs_slope(&self, r: &Relief, cell: f32) -> bool {
        self.rough(r) > 0.01 && self.erosion_octaves(cell) > 0
    }

    /// The land's height at lattice point `(ix, iz)` (spacing `step`), with detail finer
    /// than two lattice steps left out.
    fn lattice_height(&self, ix: i32, iz: i32, step: f32, cell: f32) -> f32 {
        self.relief(ix as f32 * step, iz as f32 * step, cell.max(step * 2.0)).height
    }

    /// The slope of the land at large scale (over tens of blocks), for erosion: gullies run
    /// down the broad fall of the land, and small sharp bumps don't flip their direction
    /// (which made the filter jump by tens of blocks across every little crest). Central
    /// differences on a coarse lattice, interpolated: smooth everywhere. `h` gives lattice
    /// heights (and may cache them).
    fn broad_slope(&self, x: f32, z: f32, cell: f32, h: &mut dyn FnMut(i32, i32, f32) -> f32) -> (f32, f32) {
        let step = Self::lattice_step(cell);
        let (fx, fz) = (x / step, z / step);
        let (ix, iz) = (fx.floor() as i32, fz.floor() as i32);
        let (tx, tz) = (fx - ix as f32, fz - iz as f32);
        let mut corner = |a: i32, b: i32| {
            let (px, pz) = (ix + a, iz + b);
            ((h(px + 1, pz, step) - h(px - 1, pz, step)) / (2.0 * step), (h(px, pz + 1, step) - h(px, pz - 1, step)) / (2.0 * step))
        };
        let (c00, c10, c01, c11) = (corner(0, 0), corner(1, 0), corner(0, 1), corner(1, 1));
        let mix = |a: (f32, f32), b: (f32, f32), t: f32| (lerp(a.0, b.0, t), lerp(a.1, b.1, t));
        let s = mix(mix(c00, c10, tx), mix(c01, c11, tx), tz);
        s
    }

    /// `sample` over an `n` x `n` grid from `(x0, z0)` in steps of `step` blocks (row by
    /// row): the same values, for less work (the coarse lattice the slopes come from is
    /// shared).
    pub fn sample_grid(&self, x0: f32, z0: f32, step: f32, n: usize, cell: f32) -> Vec<LandSample> {
        let mut lattice: std::collections::HashMap<(i32, i32), f32> = std::collections::HashMap::new();
        (0..n * n)
            .map(|i| {
                let (x, z) = (x0 + (i % n) as f32 * step, z0 + (i / n) as f32 * step);
                let r = self.relief(x, z, cell);
                let slope = if self.needs_slope(&r, cell) {
                    self.broad_slope(x, z, cell, &mut |ix, iz, s| *lattice.entry((ix, iz)).or_insert_with(|| self.lattice_height(ix, iz, s, cell)))
                } else {
                    (0.0, 0.0)
                };
                self.finish(x, z, cell, r, slope)
            })
            .collect()
    }

    /// `sample` at many points, sharing the work between nearby ones (see `sample_grid`).
    pub fn sample_points(&self, points: &[(f32, f32)], cell: f32) -> Vec<LandSample> {
        let mut lattice: std::collections::HashMap<(i32, i32), f32> = std::collections::HashMap::new();
        points
            .iter()
            .map(|&(x, z)| {
                let r = self.relief(x, z, cell);
                let slope = if self.needs_slope(&r, cell) {
                    self.broad_slope(x, z, cell, &mut |ix, iz, s| *lattice.entry((ix, iz)).or_insert_with(|| self.lattice_height(ix, iz, s, cell)))
                } else {
                    (0.0, 0.0)
                };
                self.finish(x, z, cell, r, slope)
            })
            .collect()
    }

    fn rough(&self, r: &Relief) -> f32 {
        if r.continent <= 0.0 {
            return 0.0;
        }
        r.mountains.max(0.3 * (1.0 - r.flat) * smoothstep(0.1, 0.4, r.inland))
    }

    fn finish(&self, x: f32, z: f32, cell: f32, r: Relief, (sx, sz): (f32, f32)) -> LandSample {
        let mut height = r.height;

        // Erosion on mountains and rough hills: gullies and ridges.
        let rough = self.rough(&r);
        if rough > 0.01 {
            let p = &self.erosion;
            let octaves = self.erosion_octaves(cell);
            let fade = ((height - r.floor) / 160.0 * 2.0 - 1.0).clamp(-1.0, 1.0);
            let u = EROSION_UNIT;
            let k = p.strength * p.scale;
            let sum = |from: u32, to: u32| (from..to).map(|o| k * p.gain.powi(o as i32)).sum::<f32>();
            // The uplift uses every octave's strength, even those left out far away, so
            // distant terrain keeps the same average height; and each left-out octave pulls
            // toward the fade target, as it would where the ground is flat.
            let (dh, last_fade) = if octaves > 0 {
                let e = erosion::erode(x / u, z / u, height / u, sx, sz, fade, p, octaves);
                (e.dh, e.fade)
            } else {
                (0.0, fade)
            };
            let dh = dh + last_fade * sum(octaves, p.octaves) + 0.3 * sum(0, p.octaves);
            // Full strength on mountains, a touch on rough hills.
            height += dh * u * smoothstep(0.05, 0.8, rough);
        }

        // Climate: colder with altitude, wetter near the coast, drier among mountains.
        let t = r.t_sea - (height - 30.0).max(0.0) / 230.0;
        let h = r.h_raw - 0.25 * r.mountains;

        // Dunes in the dry heart of deserts.
        let dry = smoothstep(0.15, 0.4, t) * smoothstep(-0.05, -0.4, h);
        let sand_sea = dry * r.flat.max(0.4) * (1.0 - r.cliffs) * smoothstep(0.05, 0.25, r.continent);
        if sand_sea > 0.05 {
            let (wx, wz) = self.wind;
            // Along the wind and across it: long crests across the wind.
            let (a, b) = (x * wx + z * wz, -x * wz + z * wx);
            let warp = 40.0 * self.jitter.at(a * 0.3, b * 0.3, cell);
            let ridge = Fbm { wavelength: 1.0, ..self.dunes }.ridged((a + warp) / 110.0, b / 420.0, cell / 110.0);
            height += ridge * 14.0 * smoothstep(0.05, 0.6, sand_sea);
        }

        // Rivers.
        let (mut water, mut river) = (if height <= SEA { SEA } else { NO_WATER }, 0.0);
        let on_land = smoothstep(0.0, 0.05, r.continent) * (1.0 - smoothstep(0.55, 0.95, r.mountains));
        if on_land > 0.0 {
            let rx = x + 280.0 * self.river_warp_x.at(x, z, cell);
            let rz = z + 280.0 * self.river_warp_z.at(x, z, cell);
            // Wider toward the sea, narrower upland.
            let lowland = 1.0 - smoothstep(8.0, 140.0, r.smooth);
            let w_big = lerp(0.0035, 0.0065, lowland) * on_land;
            let w_small = w_big * 0.55 * smoothstep(0.08, 0.2, r.continent);
            let n_big = self.rivers.at(rx, rz, cell).abs();
            let n_small = self.streams.at(rx * 1.1, rz * 1.1, cell).abs();
            let ratio = (n_big / w_big.max(1e-6)).min(if w_small > 1e-5 { n_small / w_small } else { f32::MAX });
            // Upland valleys are wider (relative to their stream) than lowland ones, and the
            // higher the land stands over the water the wider its valley, so its sides slope
            // instead of standing as walls.
            let level = (r.smooth * 0.8 - 2.0).max(SEA + 1.0).round();
            let rise = (height - r.terrace - level).abs();
            let reach = (7.0 + 8.0 * (1.0 - lowland)) * (1.0 + rise / 18.0).min(3.5);
            if ratio < reach {
                // The water's level (above): smooth, falling toward the sea. Land around it
                // eases down to it, or (in a hollow) up to it, never as a wall.
                // Rivers carve the lowlands and hills; toward the mountains they fade to dry
                // gullies instead of cutting slots. (Decided by broad, smooth things only, so
                // every column across a river agrees on whether it holds water.)
                let carve = (1.0 - smoothstep(0.3, 0.6, r.mountains)) * smoothstep(0.0, 0.3, on_land);
                if carve > 0.0 {
                    river = (1.0 - smoothstep(1.0, reach, ratio)) * carve;
                    if ratio < 1.0 {
                        let depth = 1.5 + 3.5 * (1.0 - ratio * ratio) * (0.5 + 0.5 * lowland);
                        if carve > 0.6 {
                            height = height.min(level - depth);
                            water = level;
                        } else {
                            height = lerp(height, height.min(level + 1.0), carve);
                        }
                    } else {
                        // The valley: the land eases down to just above the water.
                        let bank = level + 1.0 + (ratio - 1.0) * 0.6;
                        let k = smoothstep(1.0, reach, ratio);
                        // Cliff terraces give way to the valley's slope near the river.
                        let ground = height - r.terrace * (1.0 - k);
                        let valley = lerp(bank, ground, k * k);
                        // Beside water the banks come down all the way (or the water would
                        // stand against a wall); farther out, as much as the river carves.
                        let strength = if carve > 0.6 { lerp(1.0, carve, smoothstep(1.0, 3.0, ratio)) } else { carve };
                        height = lerp(height, valley, strength);
                        if ratio < 1.8 && carve > 0.6 {
                            height = height.max(level + 1.0); // a levee: never below the water
                        }
                    }
                }
            }
        }

        if water == NO_WATER && height <= SEA {
            water = SEA; // carved below the sea: the sea fills it
        }
        let biome = choose(height, t, h, r, river);
        let mut forest = biome.tree_density();
        if forest > 0.0 {
            let g = self.groves.at(x, z, cell);
            forest *= match biome {
                // Plains: a few dense groves in open grassland.
                Biome::Plains | Biome::Steppe | Biome::Savanna => smoothstep(0.25, 0.45, g) * 6.0 + 0.4,
                // Forests: thick, with clearings.
                _ => smoothstep(-0.45, -0.15, g),
            };
            forest = forest.min(1.0) * (1.0 - 0.8 * river.powi(3));
        }
        let form = if biome == Biome::Ocean {
            "sea"
        } else if river > 0.6 {
            "river valley"
        } else if r.mountains > 0.5 {
            "mountains"
        } else if r.cliffs > 0.5 {
            if biome == Biome::Mesa { "mesa" } else { "escarpment" }
        } else if sand_sea > 0.5 {
            "dunes"
        } else if r.flat > 0.6 {
            "plains"
        } else {
            "hills"
        };
        LandSample {
            height,
            water,
            biome,
            temperature: 14.0 + 22.0 * t,
            humidity: (0.5 + 0.5 * h).clamp(0.0, 1.0),
            t,
            h,
            forest,
            river,
            cliffs: r.cliffs,
            mountains: r.mountains,
            flat: r.flat,
            form,
        }
    }

    /// Just the ground's height (cheaper: the same as `sample(..).height`).
    pub fn height(&self, x: f32, z: f32, cell: f32) -> f32 {
        self.sample(x, z, cell).height
    }

    /// The grass colour (sRGB) for a climate: dry and warm is straw-yellow, temperate is
    /// green, wet is deep green, cold is blue-green. Continuous, so biomes blend.
    pub fn grass_rgb(t: f32, h: f32) -> [u8; 3] {
        let dry = smoothstep(0.1, -0.5, h) * smoothstep(-0.2, 0.4, t);
        let wet = smoothstep(0.1, 0.7, h);
        let cold = smoothstep(-0.1, -0.6, t);
        let mut c = [0.42f32, 0.70, 0.27]; // temperate
        let mix = |c: &mut [f32; 3], to: [f32; 3], k: f32| {
            for i in 0..3 {
                c[i] += (to[i] - c[i]) * k;
            }
        };
        mix(&mut c, [0.28, 0.58, 0.22], wet);
        mix(&mut c, [0.74, 0.70, 0.36], dry);
        mix(&mut c, [0.42, 0.60, 0.48], cold);
        [(c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8]
    }
}

/// The biome of a column from its height, climate and land.
fn choose(height: f32, t: f32, h: f32, r: Relief, river: f32) -> Biome {
    if height <= SEA - 1.0 && r.continent < 0.02 {
        return Biome::Ocean;
    }
    if r.continent < 0.035 && height < 5.0 && river < 0.5 && r.cliffs < 0.3 {
        return Biome::Beach;
    }
    // High and cold: bare rock and snow.
    if height > 175.0 + 60.0 * t.clamp(-1.0, 1.0) || (t < -0.6 && r.mountains > 0.5) {
        return Biome::Peaks;
    }
    if t < -0.5 {
        return Biome::Tundra;
    }
    if t < -0.15 {
        return Biome::Taiga;
    }
    if t < 0.32 {
        // Temperate.
        if h > 0.45 && r.flat > 0.4 && height < 14.0 {
            return Biome::Swamp;
        }
        if h > 0.35 {
            return Biome::OldForest;
        }
        if h > 0.02 {
            return Biome::Forest;
        }
        if h < -0.35 {
            return Biome::Steppe;
        }
        return Biome::Plains;
    }
    // Hot.
    if h < -0.12 {
        return if r.cliffs > 0.35 { Biome::Mesa } else { Biome::Desert };
    }
    if h < 0.28 {
        return Biome::Savanna;
    }
    Biome::Jungle
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn deterministic() {
        let l = Land::new(42);
        let a = l.sample(1234.5, -876.0, 1.0);
        let b = Land::new(42).sample(1234.5, -876.0, 1.0);
        assert_eq!(a.height, b.height);
        assert_eq!(a.biome, b.biome);
    }

    #[test]
    fn every_biome_shows_up_and_the_world_is_mostly_land() {
        let l = Land::new(1337);
        let mut counts: HashMap<Biome, usize> = HashMap::new();
        let n = 160;
        for i in 0..n {
            for j in 0..n {
                let s = l.sample(i as f32 * 211.0 - 17000.0, j as f32 * 211.0 - 17000.0, 32.0);
                *counts.entry(s.biome).or_default() += 1;
            }
        }
        let total = (n * n) as f32;
        let ocean = *counts.get(&Biome::Ocean).unwrap_or(&0) as f32 / total;
        assert!((0.15..0.55).contains(&ocean), "ocean {ocean}: {counts:?}");
        for b in [Biome::Plains, Biome::Forest, Biome::Taiga, Biome::Desert, Biome::Savanna, Biome::Peaks, Biome::OldForest] {
            assert!(counts.get(&b).copied().unwrap_or(0) as f32 / total > 0.01, "{b:?} too rare: {counts:?}");
        }
    }

    #[test]
    fn rivers_have_water_banks_above_it_and_a_bed_below() {
        let l = Land::new(1337);
        let mut found = 0;
        'search: for i in 0..400 {
            for j in 0..400 {
                let (x, z) = (i as f32 * 37.0 - 7000.0, j as f32 * 37.0 - 7000.0);
                let s = l.sample(x, z, 1.0);
                if s.water > SEA && s.water != NO_WATER {
                    // In a river channel: the bed is below the water.
                    assert!(s.height < s.water, "bed {} water {}", s.height, s.water);
                    found += 1;
                    if found > 30 {
                        break 'search;
                    }
                }
            }
        }
        assert!(found > 10, "only {found} river columns");
    }

    #[test]
    fn plains_are_flat_and_mountains_are_tall() {
        let l = Land::new(1337);
        let (mut flat_range, mut peak) = (0.0f32, 0.0f32);
        let mut plains_seen = 0;
        for i in 0..300 {
            for j in 0..300 {
                let (x, z) = (i as f32 * 97.0 - 14000.0, j as f32 * 97.0 - 14000.0);
                let s = l.sample(x, z, 1.0);
                peak = peak.max(s.height);
                let open = |s: &LandSample| s.biome == Biome::Plains && s.flat > 0.9 && s.river < 0.01 && s.cliffs < 0.05;
                if open(&s) {
                    let n = l.sample(x + 40.0, z, 1.0);
                    if open(&n) {
                        plains_seen += 1;
                        // Over 40 blocks the plains barely rise or fall.
                        flat_range = flat_range.max((n.height - s.height).abs());
                    }
                }
            }
        }
        assert!(plains_seen > 50);
        assert!(flat_range < 9.0, "plains vary {flat_range} over 40 blocks");
        assert!(peak > 180.0, "highest peak {peak}");
    }
}
