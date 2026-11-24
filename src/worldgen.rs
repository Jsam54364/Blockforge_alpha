//! Deterministic terrain: a pure function of (seed, chunk position).
//!
//! That purity is what makes the "procedural base + edit journal" design work:
//! unloaded chunks can always be regenerated, then edits are replayed on top.
//!
//! Heights are in quarter-block units, so the surface can end in a half-slab
//! (the "terrain slab" idea) with no change to the chunk format.
//!
//! The land's shape, climate, biomes and rivers come from `land`; trees, bushes, boulders and
//! fallen logs from `flora`. This file turns them into blocks: the ground cover of each biome
//! (grass with meadow patches on the plains, leaf litter under thick woods, needles in the
//! taiga, sand and sandstone in deserts, banded red rock in mesas, snow above the snow line),
//! bare rock on cliffs and steep slopes, river beds and banks of sand, gravel and clay, water
//! in seas and rivers (frozen where it is cold), caves, and the features stamped across
//! chunk borders.

use crate::block::{BlockState, Shape, AIR};
use crate::chunk::Chunk;
use crate::coords::{ChunkPos, LocalPos, CHUNK_SIZE};
use crate::flora::{Feature, Kind, Palette, MAX_REACH};
use crate::land::{Biome, Land, LandSample, NO_WATER};
use crate::noise::Fbm;
use crate::registry::BlockRegistry;
use crate::rng::hash3;

pub const SEA_LEVEL: i32 = 0;
/// Bumped whenever the same seed would generate different terrain: worlds saved by an
/// older generator are kept apart (their edits would land on different ground).
pub const WORLDGEN_VERSION: u32 = 13;
pub const BEDROCK_Y: i32 = -128;
/// No caves below this; deep chunks collapse to a single uniform entry.
const CAVE_FLOOR: i32 = -48;
/// No water in a column.
const DRY: i32 = i32::MIN;
/// Trees and other features grow on a jittered grid of cells this wide.
const TREE_GRID: i32 = 5;
/// Bushes, boulders and fallen logs: a sparser grid.
const SMALL_GRID: i32 = 11;
/// In `Column::rock`: rock in coloured bands by height (mesas), see `WorldGen::band`.
const BANDED: u16 = u16::MAX;

/// Per-(x,z) data shared by the whole vertical stack of chunks.
#[derive(Clone, Debug)]
pub struct Column {
    pub cx: i32,
    pub cz: i32,
    /// Surface height in quarter blocks, indexed `lz * CHUNK_SIZE + lx`.
    pub height_q: Vec<i32>,
    /// Top block and the blocks under it, same indexing.
    pub surface: Vec<u16>,
    pub subsurface: Vec<u16>,
    /// The rock under the ground (or `BANDED`), same indexing.
    pub rock: Vec<u16>,
    /// The top of the water over each column (blocks at or below it are water), or `DRY`.
    pub water: Vec<i32>,
    /// Whether the water's surface freezes.
    pub ice: Vec<bool>,
    /// Lowest / highest `top` block y (see `generate_chunk`) in this column; `max_top`
    /// includes water and features.
    pub min_top: i32,
    pub max_top: i32,
    /// Features that reach into this column of chunks.
    pub features: Vec<Feature>,
    /// The plant (grass, flower, bush...) growing on each column's ground, or 0.
    pub plant: Vec<u16>,
}

impl Column {
    pub fn height_q_at(&self, lx: usize, lz: usize) -> i32 {
        self.height_q[lz * CHUNK_SIZE as usize + lx]
    }
}

/// The named blocks the generator uses.
/// The plants the generator grows (0 for any the registry lacks).
#[derive(Clone, Debug)]
struct Plants {
    short: u16,
    tuft: u16,
    tall: u16,
    dry: u16,
    fern: u16,
    bush: u16,
    berries: u16,
    dandelion: u16,
    poppy: u16,
    cornflower: u16,
    daisy: u16,
    lavender: u16,
    dead: u16,
    reeds: u16,
    heather: u16,
}

impl Plants {
    fn all(&self) -> [u16; 15] {
        [
            self.short, self.tuft, self.tall, self.dry, self.fern, self.bush, self.berries, self.dandelion, self.poppy, self.cornflower, self.daisy,
            self.lavender, self.dead, self.reeds, self.heather,
        ]
    }
}

/// One of `table`'s plants, by weight, for `pick` in 0..1.
fn choose(pick: f32, table: &[(u16, f32)]) -> u16 {
    let total: f32 = table.iter().map(|t| t.1).sum();
    let mut at = pick * total;
    for &(id, w) in table {
        if at < w {
            return id;
        }
        at -= w;
    }
    table.last().map_or(0, |t| t.0)
}

#[derive(Clone, Debug)]
struct Blocks {
    #[allow(dead_code)]
    stone: u16,
    dirt: u16,
    grass: u16,
    lush_grass: u16,
    dry_grass: u16,
    prairie_grass: u16,
    cold_grass: u16,
    snowy_grass: u16,
    meadow: u16,
    leaf_litter: u16,
    needle_floor: u16,
    rich_soil: u16,
    moss: u16,
    mud: u16,
    clay: u16,
    peat: u16,
    sand: u16,
    red_sand: u16,
    sandstone: u16,
    red_sandstone: u16,
    gravel: u16,
    snow: u16,
    ice: u16,
    tundra_moss: u16,
    bands: [u16; 7],
}

pub struct WorldGen {
    seed: u64,
    land: Land,
    b: Blocks,
    plants: Plants,
    /// The rock types, in the order regions wind around each other (see `rock_at`).
    rocks: [u16; 5],
    rock_regions: Fbm,
    /// Patches of ground cover (meadow flowers, leaf litter, moss...).
    patches: Fbm,
    water: u16,
    bedrock: u16,
    palette: Palette,
    /// Gentle large-scale brightness variation of the ground (see `ground_tint`).
    variation: Fbm,
}

/// A column of level-of-detail terrain (`WorldGen::lod_column`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LodColumn {
    /// Top in quarter blocks: the ground, or the water surface over water.
    pub top_q: i32,
    /// The ground under any water, in quarter blocks.
    pub ground_q: i32,
    /// The top block (water over water), the few blocks under it, and the rock below.
    pub surface: u16,
    pub sub: u16,
    pub rock: u16,
    pub water: bool,
    /// A forest canopy over the ground: its bottom and top (blocks) and its leaves.
    pub canopy: Option<(i32, i32, u16)>,
}

/// What the ground at a column is like (see `WorldGen::describe`).
#[derive(Clone, Copy, Debug)]
pub struct GroundInfo {
    pub biome: &'static str,
    pub landform: &'static str,
    pub height: f32,
    /// Degrees C and 0..1.
    pub temperature: f32,
    pub rainfall: f32,
}

fn lattice3(seed: u64, ix: i32, iy: i32, iz: i32) -> f32 {
    (hash3(seed, ix, iy, iz) >> 40) as f32 * (1.0 / 16_777_216.0)
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn value_noise_3d(seed: u64, x: f32, y: f32, z: f32) -> f32 {
    let (x0, y0, z0) = (x.floor(), y.floor(), z.floor());
    let (tx, ty, tz) = (smooth(x - x0), smooth(y - y0), smooth(z - z0));
    let (ix, iy, iz) = (x0 as i32, y0 as i32, z0 as i32);
    let c000 = lattice3(seed, ix, iy, iz);
    let c100 = lattice3(seed, ix + 1, iy, iz);
    let c010 = lattice3(seed, ix, iy + 1, iz);
    let c110 = lattice3(seed, ix + 1, iy + 1, iz);
    let c001 = lattice3(seed, ix, iy, iz + 1);
    let c101 = lattice3(seed, ix + 1, iy, iz + 1);
    let c011 = lattice3(seed, ix, iy + 1, iz + 1);
    let c111 = lattice3(seed, ix + 1, iy + 1, iz + 1);
    let x00 = lerp(c000, c100, tx);
    let x10 = lerp(c010, c110, tx);
    let x01 = lerp(c001, c101, tx);
    let x11 = lerp(c011, c111, tx);
    lerp(lerp(x00, x10, ty), lerp(x01, x11, ty), tz)
}

/// The surface in quarter blocks for ground at height `h` with the given neighbouring
/// heights (see `WorldGen::surface_q`).
fn surface_from(h: f32, neighbours: [f32; 4]) -> i32 {
    let t = h.round() as i32;
    let rising = h >= t as f32 && neighbours.iter().any(|n| n.round() as i32 > t);
    4 * t + if rising { 2 } else { 0 }
}

fn unit(h: u64, shift: u32) -> f32 {
    ((h >> shift) & 0xFFFF) as f32 / 65536.0
}

/// The snow line for a climate (`t` -1..1): lower where it is colder.
fn snow_line(t: f32) -> f32 {
    150.0 + 90.0 * t.clamp(-1.0, 1.0)
}

impl WorldGen {
    pub fn new(seed: u64, reg: &BlockRegistry) -> Self {
        let id = |n: &str| reg.id(n).unwrap_or_else(|| panic!("worldgen needs block '{n}'"));
        let or = |n: &str, f: &str| reg.id(n).unwrap_or_else(|| id(f));
        let b = Blocks {
            stone: id("stone"),
            dirt: id("dirt"),
            grass: id("grass"),
            lush_grass: or("lush_grass", "grass"),
            dry_grass: or("dry_grass", "grass"),
            prairie_grass: or("prairie_grass", "grass"),
            cold_grass: or("cold_grass", "grass"),
            snowy_grass: or("snowy_grass", "snow"),
            meadow: or("meadow", "grass"),
            leaf_litter: or("leaf_litter", "dirt"),
            needle_floor: or("needle_floor", "dirt"),
            rich_soil: or("rich_soil", "dirt"),
            moss: or("moss_block", "grass"),
            mud: or("mud", "dirt"),
            clay: or("clay", "dirt"),
            peat: or("peat", "dirt"),
            sand: id("sand"),
            red_sand: or("red_sand", "sand"),
            sandstone: or("sandstone", "stone"),
            red_sandstone: or("red_sandstone", "sandstone"),
            gravel: id("gravel"),
            snow: id("snow"),
            ice: or("ice", "snow"),
            tundra_moss: or("tundra_moss", "grass"),
            bands: [
                or("red_sandstone", "sandstone"),
                or("ochrestone", "sandstone"),
                or("red_sandstone", "sandstone"),
                or("honeystone", "sandstone"),
                or("sandstone", "stone"),
                or("red_sandstone", "sandstone"),
                or("sunstone", "sandstone"),
            ],
        };
        let p = |n: &str| reg.id(n).unwrap_or(0);
        let plants = Plants {
            short: p("short_grass"),
            tuft: p("grass_tuft"),
            tall: p("tall_grass"),
            dry: p("dry_tuft"),
            fern: p("fern"),
            bush: p("leafy_bush"),
            berries: p("berry_bush"),
            dandelion: p("dandelion"),
            poppy: p("poppy"),
            cornflower: p("cornflower"),
            daisy: p("daisy"),
            lavender: p("lavender"),
            dead: p("dead_bush"),
            reeds: p("reeds"),
            heather: p("heather"),
        };
        let mut wood = [(0u16, 0u16); 16];
        let w = |log: &str, leaves: &str| (or(log, "log"), or(leaves, "leaves"));
        wood[Kind::Oak as usize] = w("log", "leaves");
        wood[Kind::BigOak as usize] = w("log", "leaves");
        wood[Kind::Birch as usize] = w("birch_log", "birch_leaves");
        wood[Kind::Maple as usize] = w("maple_log", "maple_leaves");
        wood[Kind::Spruce as usize] = w("spruce_log", "spruce_leaves");
        wood[Kind::Fir as usize] = w("fir_log", "fir_leaves");
        wood[Kind::Pine as usize] = w("pine_log", "pine_leaves");
        wood[Kind::Redwood as usize] = w("redwood_log", "redwood_leaves");
        wood[Kind::Acacia as usize] = w("acacia_log", "acacia_leaves");
        wood[Kind::Willow as usize] = w("willow_log", "willow_leaves");
        wood[Kind::Palm as usize] = w("palm_log", "palm_leaves");
        wood[Kind::Jungle as usize] = w("fig_log", "fig_leaves");
        wood[Kind::Bush as usize] = w("log", "leaves");
        wood[Kind::FallenLog as usize] = w("log", "leaves");
        let palette = Palette {
            wood,
            cactus: or("cactus", "leaves"),
            boulder: [id("stone"), or("mossy_cobbled_stone", "stone"), or("granite", "stone")],
        };
        WorldGen {
            seed,
            land: Land::new(seed),
            b,
            plants,
            rocks: [id("basalt"), id("granite"), id("stone"), id("limestone"), id("slate")],
            rock_regions: Fbm::new(seed ^ 0x0C0F_FEE5, 2600.0, 2, 0.4),
            patches: Fbm::new(seed ^ 0x9A7C_4E55, 24.0, 3, 0.55),
            water: id("water"),
            bedrock: id("bedrock"),
            palette,
            variation: Fbm::new(seed ^ 0x7A11_7E5D, 60.0, 2, 0.5),
        }
    }

    pub fn water_id(&self) -> u16 {
        self.water
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn land(&self) -> &Land {
        &self.land
    }

    /// Surface height in quarter-block units.
    pub fn height_q(&self, x: i32, z: i32) -> i32 {
        self.height_q_filtered(x, z, 1)
    }

    /// `height_q` as seen from far away, for terrain drawn as cells `cell` blocks wide:
    /// detail finer than a cell fades to its average instead of aliasing (see `noise`).
    /// `cell <= 1` is exactly `height_q`.
    pub fn height_q_filtered(&self, x: i32, z: i32, cell: i32) -> i32 {
        if cell <= 1 {
            return self.surface_q(x, z);
        }
        4 * self.land.height(x as f32, z as f32, cell as f32).round() as i32
    }

    /// The surface at full detail, in quarter blocks: whole blocks (the height rounded), plus
    /// a half slab only at the foot of a step, where the ground rises toward a higher
    /// neighbour. Slopes get smoothed; flat ground stays flat.
    fn surface_q(&self, x: i32, z: i32) -> i32 {
        let h = |x: i32, z: i32| self.land.height(x as f32, z as f32, 1.0);
        surface_from(h(x, z), [h(x + 1, z), h(x - 1, z), h(x, z + 1), h(x, z - 1)])
    }

    /// The rock of the region around `(x, z)`: the world's bedrock changes every kilometre or
    /// so (basalt, granite, stone, limestone, slate), and shows on cliffs and bare mountains.
    pub fn rock_at(&self, x: i32, z: i32, cell: f32) -> u16 {
        let v = self.rock_regions.at(x as f32, z as f32, cell);
        let i = ((v * 9.5 + 2.5).floor() as i32).clamp(0, 4);
        self.rocks[i as usize]
    }

    /// The rock of a mesa's banded cliffs at height `y`.
    fn band(&self, x: i32, y: i32, z: i32) -> u16 {
        // Bands a few blocks thick, gently tilted and wavy.
        let wave = (x as f32 * 0.013).sin() * 2.0 + (z as f32 * 0.009).cos() * 2.0;
        let k = ((y as f32 + wave) / 3.0).floor() as i32;
        self.b.bands[(k.rem_euclid(self.b.bands.len() as i32)) as usize]
    }

    /// The (surface, subsurface, rock) blocks of a column whose top block is at `top`, with
    /// the steepness of the ground there (height change per block).
    fn ground(&self, x: i32, z: i32, top: i32, s: &LandSample, slope: f32, cell: f32) -> (u16, u16, u16) {
        let b = &self.b;
        let mut rock = self.rock_at(x, z, cell);
        if s.biome == Biome::Mesa || (s.biome == Biome::Desert && s.cliffs > 0.3) {
            rock = BANDED;
        }
        let patch = self.patches.at(x as f32, z as f32, cell);
        let wet = s.water != NO_WATER && s.water >= top as f32;
        if wet {
            // Under water: river beds of gravel and sand with clay; sand on shallow sea floors,
            // gravel deep down.
            if s.water > crate::land::SEA + 0.5 || s.river > 0.6 {
                let bed = if patch > 0.35 { b.clay } else if patch > -0.1 { b.gravel } else { b.sand };
                return (bed, b.gravel, rock);
            }
            let floor = if top < SEA_LEVEL - 14 || patch > 0.45 { b.gravel } else if s.biome == Biome::Swamp { b.mud } else { b.sand };
            return (floor, floor, rock);
        }
        // Cliffs and steep slopes are bare rock.
        let steep = if s.mountains > 0.5 { 1.0 } else { 1.35 };
        if slope > steep {
            let r = if rock == BANDED { b.red_sandstone } else { rock };
            return (r, r, rock);
        }
        // Snow above the snow line (bare rock where it is steepish).
        if (top as f32) > snow_line(s.t) {
            let r = if rock == BANDED { b.red_sandstone } else { rock };
            return if slope > 0.75 { (r, r, rock) } else { (b.snow, b.snow, rock) };
        }
        // River banks: sand and gravel near the water.
        if s.river > 0.75 && s.biome != Biome::Swamp {
            return (if patch > 0.2 { b.gravel } else { b.sand }, b.sand, rock);
        }
        let (surf, sub) = match s.biome {
            Biome::Ocean | Biome::Beach => (b.sand, b.sand),
            Biome::Desert => (b.sand, b.sandstone),
            Biome::Mesa => (b.red_sand, b.red_sandstone),
            Biome::Peaks => (if slope > 0.6 { b.gravel } else { b.snow }, rock),
            Biome::Tundra => (if patch > 0.25 { b.snow } else if patch < -0.3 { b.tundra_moss } else { b.snowy_grass }, b.dirt),
            Biome::Taiga => (if patch > 0.2 && s.forest > 0.4 { b.needle_floor } else { b.cold_grass }, b.dirt),
            Biome::Forest => (if patch > 0.3 && s.forest > 0.5 { b.leaf_litter } else { b.grass }, b.dirt),
            Biome::OldForest => (
                if patch > 0.15 { b.leaf_litter } else if patch < -0.35 { b.moss } else if patch < -0.2 { b.needle_floor } else { b.grass },
                b.rich_soil,
            ),
            Biome::Swamp => (if patch > 0.3 { b.mud } else if patch < -0.3 { b.moss } else { b.grass }, if patch > 0.0 { b.mud } else { b.peat }),
            Biome::Plains => (if patch > 0.38 { b.meadow } else { b.grass }, b.dirt),
            Biome::Steppe => (if patch > 0.25 { b.dry_grass } else { b.prairie_grass }, b.dirt),
            Biome::Savanna => (if patch > 0.3 { b.dirt } else { b.dry_grass }, b.dirt),
            Biome::Jungle => (if patch > 0.3 { b.moss } else { b.lush_grass }, b.rich_soil),
        };
        // Steepish ground loses its grass.
        if slope > 0.9 && !matches!(s.biome, Biome::Desert | Biome::Beach | Biome::Mesa) {
            return (if patch > 0.0 { b.gravel } else { b.dirt }, b.dirt, rock);
        }
        (surf, sub, rock)
    }

    /// How the ground at `(x, z)` is coloured, for terrain drawn in cells `cell` blocks wide:
    /// the grass colour of its climate (sRGB) and, in the last byte, a brightness variation
    /// (128 is neutral) that varies over tens of blocks and hides how textures repeat.
    pub fn ground_tint(&self, x: i32, z: i32, cell: i32) -> [u8; 4] {
        let s = self.land.sample(x as f32, z as f32, cell as f32);
        let [r, g, b] = Land::grass_rgb(s.t, s.h);
        let v = self.variation.at(x as f32, z as f32, cell as f32);
        [r, g, b, (128.0 + 127.0 * v.clamp(-1.0, 1.0)) as u8]
    }

    /// Biome, landform, height and climate of the column at `(x, z)`.
    pub fn describe(&self, x: i32, z: i32) -> GroundInfo {
        let s = self.land.sample(x as f32, z as f32, 1.0);
        GroundInfo { biome: s.biome.name(), landform: s.form, height: s.height, temperature: s.temperature, rainfall: s.humidity }
    }

    /// Where a new player starts: the nearest open temperate lowland (plains,
    /// dry and gently sloping) to the origin, searched in 64-block steps, nearest first, up
    /// to 6 km out; a point on the ground.
    pub fn find_spawn(&self) -> [f32; 3] {
        let ok = |x: i32, z: i32| {
            let s = self.land.sample(x as f32, z as f32, 1.0);
            let dry = s.water == NO_WATER || s.water < s.height;
            let gentle = (self.land.height(x as f32 + 4.0, z as f32, 1.0) - s.height).abs() < 2.0;
            (dry && gentle && (3.0..60.0).contains(&s.height) && matches!(s.biome, Biome::Plains) && s.river < 0.2)
                .then_some(s.height)
        };
        let mut best: Option<(i64, i32, i32, f32)> = None;
        for ring in 0..96 {
            if let Some((d, ..)) = best {
                if d < ((ring * 64) as i64).pow(2) {
                    break;
                }
            }
            let r = ring * 64;
            for i in -ring..=ring {
                for (x, z) in [(i * 64, -r), (i * 64, r), (-r, i * 64), (r, i * 64)] {
                    let d = (x as i64).pow(2) + (z as i64).pow(2);
                    if best.map_or(true, |b| d < b.0) {
                        if let Some(h) = ok(x, z) {
                            best = Some((d, x, z, h));
                        }
                    }
                }
            }
        }
        match best {
            Some((_, x, z, h)) => [x as f32 + 0.5, h.round(), z as f32 + 0.5],
            None => [0.5, self.land.height(0.0, 0.0, 1.0).max(1.0).round(), 0.5],
        }
    }

    /// The visible surface of the column at block `(x, z)` for level-of-detail terrain:
    /// its top in quarter blocks and the block it is made of.
    pub fn lod_surface(&self, x: i32, z: i32, cell: i32) -> (i32, u16) {
        let c = self.lod_column(x, z, cell);
        (c.top_q, c.surface)
    }

    /// One column for level-of-detail voxels (see `lod::build_section`), band-limited for
    /// cells `cell` blocks wide.
    pub fn lod_column(&self, x: i32, z: i32, cell: i32) -> LodColumn {
        let fc = cell as f32;
        let s = self.land.sample(x as f32, z as f32, fc);
        // The slope over the cell, for bare rock on cliffs.
        let d = fc.max(1.0);
        let slope = ((self.land.height(x as f32 + d, z as f32, fc) - s.height).abs()).max((self.land.height(x as f32, z as f32 + d, fc) - s.height).abs()) / d;
        self.lod_column_from(x, z, cell, &s, slope)
    }

    /// `lod_column` over an `n` x `n` grid of cells `cell` blocks wide from `(x0, z0)` (row
    /// by row), for a third of the work (slopes come from the neighbours).
    pub fn lod_grid(&self, x0: i32, z0: i32, cell: i32, n: usize) -> Vec<LodColumn> {
        let step = cell.max(1);
        let m = n + 1;
        let samples = self.land.sample_grid(x0 as f32, z0 as f32, step as f32, m, cell as f32);
        (0..n * n)
            .map(|i| {
                let (a, b) = (i % n, i / n);
                let s = &samples[b * m + a];
                let slope = (samples[b * m + a + 1].height - s.height).abs().max((samples[(b + 1) * m + a].height - s.height).abs()) / step as f32;
                self.lod_column_from(x0 + a as i32 * step, z0 + b as i32 * step, cell, s, slope)
            })
            .collect()
    }

    fn lod_column_from(&self, x: i32, z: i32, cell: i32, s: &LandSample, slope: f32) -> LodColumn {
        let fc = cell as f32;
        let s = *s;
        let top = s.height.round() as i32;
        let (surface, sub, rock) = self.ground(x, z, top, &s, slope, fc);
        let rock = if rock == BANDED { self.b.red_sandstone } else { rock };
        let water = s.water != NO_WATER && s.water >= s.height;
        let ground_q = 4 * top;
        if water {
            return LodColumn { top_q: 4 * (s.water as i32 + 1), ground_q, surface: self.water, sub: surface, rock, water: true, canopy: None };
        }
        // Forests far off: a canopy at the height of the biome's trees, as often as trees grow.
        let mut canopy = None;
        // (Solid where the forest is thick: a speckle of single columns would cost five faces
        // each and shimmer.)
        if s.forest > 0.4 && cell >= 8 {
            let kind = Kind::tree_for(s.biome, 0.3);
            let f = Feature::new(kind, x, top, z, 0x5EED);
            if let Some((_, bottom, ctop)) = f.crown() {
                canopy = Some((bottom, ctop, self.palette.wood[kind as usize].1));
            }
        }
        LodColumn { top_q: ground_q, ground_q, surface, sub, rock, water: false, canopy }
    }

    /// Just the height part of `lod_surface`.
    pub fn lod_height(&self, x: i32, z: i32, cell: i32) -> i32 {
        let s = self.land.sample(x as f32, z as f32, cell as f32);
        if s.water != NO_WATER && s.water >= s.height {
            return 4 * (s.water as i32 + 1);
        }
        4 * s.height.round() as i32
    }

    /// The top of the highest full block of the column at `(x, z)` in quarter blocks, at
    /// full detail: the ground under any half-slab or water.
    pub fn ground_q(&self, x: i32, z: i32) -> i32 {
        4 * self.land.height(x as f32, z as f32, 1.0).round() as i32
    }

    /// `ground_q` at many points at once (much cheaper than one by one).
    pub fn ground_q_many(&self, points: &[(i32, i32)]) -> Vec<i32> {
        let p: Vec<(f32, f32)> = points.iter().map(|&(x, z)| (x as f32, z as f32)).collect();
        self.land.sample_points(&p, 1.0).iter().map(|s| 4 * s.height.round() as i32).collect()
    }

    /// The feature (if any) that grows from grid cell `(gx, gz)` of `grid`, given the land at
    /// its spot (`sample` gives the land at a point).
    /// Where in grid cell `(gx, gz)` of `grid` its feature would grow, and the cell's hash.
    fn anchor(&self, grid: i32, gx: i32, gz: i32) -> (i32, i32, u64) {
        let h = hash3(self.seed ^ 0xF10A ^ grid as u64, gx, 0, gz);
        (gx * grid + (h % grid as u64) as i32, gz * grid + ((h >> 8) % grid as u64) as i32, h)
    }

    fn feature_in(&self, grid: i32, gx: i32, gz: i32, sample: &mut dyn FnMut(i32, i32) -> (LandSample, Option<f32>)) -> Option<Feature> {
        let (x, z, h) = self.anchor(grid, gx, gz);
        let r = unit(h, 16);
        let (s, steep) = sample(x, z);
        if s.water != NO_WATER && s.water + 0.5 >= s.height {
            return None;
        }
        let kind = if grid == TREE_GRID {
            if r >= s.forest {
                return None;
            }
            Kind::tree_for(s.biome, unit(h, 32))
        } else {
            let k = unit(h, 32);
            let (bush, boulder, log) = match s.biome {
                Biome::Plains => (0.18, 0.05, 0.0),
                Biome::Steppe => (0.06, 0.08, 0.0),
                Biome::Forest | Biome::OldForest => (0.2, 0.04, 0.14),
                Biome::Taiga => (0.12, 0.12, 0.1),
                Biome::Tundra => (0.02, 0.1, 0.0),
                Biome::Peaks => (0.0, 0.08, 0.0),
                Biome::Savanna => (0.12, 0.05, 0.0),
                Biome::Jungle | Biome::Swamp => (0.3, 0.0, 0.08),
                Biome::Desert | Biome::Mesa => (0.0, 0.05, 0.0),
                Biome::Beach | Biome::Ocean => (0.0, 0.0, 0.0),
            };
            if r < bush {
                Kind::Bush
            } else if r < bush + boulder {
                Kind::Boulder
            } else if r < bush + boulder + log && k < 0.9 {
                Kind::FallenLog
            } else {
                return None;
            }
        };
        // Not on cliffs or in the snow.
        if s.height > snow_line(s.t) + 10.0 && kind != Kind::Boulder {
            return None;
        }
        // (`steep`: the height change over two blocks; unknown far away.)
        if steep.is_some_and(|d| d > if kind.is_tree() { 2.6 } else { 3.5 }) {
            return None;
        }
        let f = Feature::new(kind, x, s.height.round() as i32, z, h);
        Some(f)
    }

    /// Every feature that can reach into the area `[x0, x1) x [z0, z1)`.
    fn features_near(&self, x0: i32, z0: i32, x1: i32, z1: i32, sample: &mut dyn FnMut(i32, i32) -> (LandSample, Option<f32>)) -> Vec<Feature> {
        let mut out = Vec::new();
        for grid in [TREE_GRID, SMALL_GRID] {
            let (gx0, gx1) = ((x0 - MAX_REACH).div_euclid(grid), (x1 + MAX_REACH).div_euclid(grid));
            let (gz0, gz1) = ((z0 - MAX_REACH).div_euclid(grid), (z1 + MAX_REACH).div_euclid(grid));
            for gz in gz0..=gz1 {
                for gx in gx0..=gx1 {
                    if let Some(f) = self.feature_in(grid, gx, gz, sample) {
                        let (lo, hi) = f.bounds();
                        if hi[0] >= x0 && lo[0] < x1 && hi[2] >= z0 && lo[2] < z1 {
                            out.push(f);
                        }
                    }
                }
            }
        }
        out
    }

    /// The trees growing in `[x0, x1) x [z0, z1)` (for distant terrain), with the land
    /// sampled at cells `cell` blocks wide, and their log and leaves.
    pub fn lod_trees(&self, x0: i32, z0: i32, x1: i32, z1: i32, cell: i32) -> Vec<(Feature, u16, u16)> {
        let (gx0, gx1) = (x0.div_euclid(TREE_GRID), (x1 - 1).div_euclid(TREE_GRID));
        let (gz0, gz1) = (z0.div_euclid(TREE_GRID), (z1 - 1).div_euclid(TREE_GRID));
        // The land at every anchor, in one batch.
        let mut anchors = Vec::new();
        for gz in gz0..=gz1 {
            for gx in gx0..=gx1 {
                let (x, z, _) = self.anchor(TREE_GRID, gx, gz);
                anchors.push((x, z));
            }
        }
        let pts: Vec<(f32, f32)> = anchors.iter().map(|&(x, z)| (x as f32, z as f32)).collect();
        let land: std::collections::HashMap<(i32, i32), LandSample> = anchors.into_iter().zip(self.land.sample_points(&pts, cell as f32)).collect();
        let mut sample = |x: i32, z: i32| (land.get(&(x, z)).copied().unwrap_or_else(|| self.land.sample(x as f32, z as f32, cell as f32)), None);
        let mut out = Vec::new();
        for gz in gz0..=gz1 {
            for gx in gx0..=gx1 {
                if let Some(f) = self.feature_in(TREE_GRID, gx, gz, &mut sample) {
                    if f.crown().is_some() {
                        let (log, leaves) = self.palette.wood[f.kind as usize];
                        out.push((f, log, leaves));
                    }
                }
            }
        }
        out
    }

    pub fn column(&self, cx: i32, cz: i32) -> Column {
        let n = (CHUNK_SIZE * CHUNK_SIZE) as usize;
        let mut c = Column {
            cx,
            cz,
            height_q: Vec::with_capacity(n),
            surface: Vec::with_capacity(n),
            subsurface: Vec::with_capacity(n),
            rock: Vec::with_capacity(n),
            water: Vec::with_capacity(n),
            ice: Vec::with_capacity(n),
            min_top: i32::MAX,
            max_top: i32::MIN,
            features: Vec::new(),
            plant: Vec::with_capacity(n),
        };
        // The land with a one-block border: slabs and slopes depend on the neighbours.
        const W: i32 = CHUNK_SIZE + 2;
        let (bx, bz) = (cx * CHUNK_SIZE, cz * CHUNK_SIZE);
        let samples = self.land.sample_grid((bx - 1) as f32, (bz - 1) as f32, 1.0, W as usize, 1.0);
        let at = |lx: i32, lz: i32| &samples[((lz + 1) * W + lx + 1) as usize];
        let h = |lx: i32, lz: i32| at(lx, lz).height;
        for lz in 0..CHUNK_SIZE {
            for lx in 0..CHUNK_SIZE {
                let (x, z) = (bx + lx, bz + lz);
                let s = at(lx, lz);
                let mut hq = surface_from(s.height, [h(lx + 1, lz), h(lx - 1, lz), h(lx, lz + 1), h(lx, lz - 1)]);
                let water = if s.water != NO_WATER && s.water >= s.height { s.water as i32 } else { DRY };
                if water != DRY && hq.rem_euclid(4) != 0 {
                    hq -= hq.rem_euclid(4); // no slabs under water
                }
                let top = hq.div_euclid(4);
                let slope = ((h(lx + 1, lz) - h(lx - 1, lz)).abs()).max((h(lx, lz + 1) - h(lx, lz - 1)).abs()) * 0.5;
                let (surf, sub, rock) = self.ground(x, z, top - 1, s, slope, 1.0);
                c.min_top = c.min_top.min(top);
                c.max_top = c.max_top.max(top).max(if water != DRY { water + 1 } else { top });
                c.height_q.push(hq);
                c.surface.push(surf);
                c.subsurface.push(sub);
                c.rock.push(rock);
                c.water.push(water);
                c.ice.push(water != DRY && s.t < -0.5);
                // Plants grow on dry ground with a whole block on top (not on a slab).
                c.plant.push(if water == DRY && hq.rem_euclid(4) < 2 { self.plant_for(x, z, s, surf, slope) } else { 0 });
            }
        }
        // Features: inside the column the land is already known.
        // Anchors outside the column (and the points beside them for steepness): one batch.
        let inside = |x: i32, z: i32| (-1..=CHUNK_SIZE).contains(&(x - bx)) && (-1..=CHUNK_SIZE).contains(&(z - bz));
        let mut outside = Vec::new();
        for grid in [TREE_GRID, SMALL_GRID] {
            for gz in (bz - MAX_REACH).div_euclid(grid)..=(bz + CHUNK_SIZE + MAX_REACH).div_euclid(grid) {
                for gx in (bx - MAX_REACH).div_euclid(grid)..=(bx + CHUNK_SIZE + MAX_REACH).div_euclid(grid) {
                    let (x, z, _) = self.anchor(grid, gx, gz);
                    for p in [(x, z), (x + 2, z), (x, z + 2)] {
                        if !inside(p.0, p.1) {
                            outside.push(p);
                        }
                    }
                }
            }
        }
        outside.sort_unstable();
        outside.dedup();
        let pts: Vec<(f32, f32)> = outside.iter().map(|&(x, z)| (x as f32, z as f32)).collect();
        let far: std::collections::HashMap<(i32, i32), LandSample> = outside.into_iter().zip(self.land.sample_points(&pts, 1.0)).collect();
        let get = |x: i32, z: i32| -> LandSample {
            if inside(x, z) {
                *at(x - bx, z - bz)
            } else {
                far.get(&(x, z)).copied().unwrap_or_else(|| self.land.sample(x as f32, z as f32, 1.0))
            }
        };
        let height = |x: i32, z: i32| get(x, z).height;
        let mut sample = |x: i32, z: i32| {
            let s = get(x, z);
            let steep = (height(x + 2, z) - s.height).abs().max((height(x, z + 2) - s.height).abs());
            (s, Some(steep))
        };
        c.features = self.features_near(bx, bz, bx + CHUNK_SIZE, bz + CHUNK_SIZE, &mut sample);
        for f in &c.features {
            c.max_top = c.max_top.max(f.bounds().1[1]);
        }
        c
    }

    /// The highest y (exclusive) at which `(x, z)` (in `col`) holds anything generated:
    /// ground, water or a feature. Searching down from here finds the top block.
    pub fn top_hint(&self, col: &Column, x: i32, z: i32) -> i32 {
        let (lx, lz) = ((x - col.cx * CHUNK_SIZE) as usize, (z - col.cz * CHUNK_SIZE) as usize);
        let i = lz * CHUNK_SIZE as usize + lx;
        let mut y = (col.height_q[i].div_euclid(4) + 1).max(col.water[i] + 1).max(SEA_LEVEL + 1);
        for f in &col.features {
            let (lo, hi) = f.bounds();
            if (lo[0]..=hi[0]).contains(&x) && (lo[2]..=hi[2]).contains(&z) {
                y = y.max(hi[1] + 1);
            }
        }
        y
    }

    /// Two thin noise "sheets" intersecting give tunnel-like caves. Constants need tuning.
    fn is_cave(&self, x: i32, y: i32, z: i32) -> bool {
        let (fx, fy, fz) = (x as f32, y as f32, z as f32);
        // Most cells fail the first sheet, so only sample the second one when it matters.
        let a = value_noise_3d(self.seed ^ 0xA5A5, fx / 28.0, fy / 18.0, fz / 28.0);
        if (a - 0.5).abs() >= 0.055 {
            return false;
        }
        let b = value_noise_3d(self.seed ^ 0x5A5A, fx / 28.0, fy / 18.0, fz / 28.0);
        (b - 0.5).abs() < 0.055
    }

    #[allow(clippy::too_many_arguments)]
    fn block_at(&self, wx: i32, wy: i32, wz: i32, top: i32, rem: i32, surf: u16, sub: u16, rock: u16, water: i32, ice: bool) -> BlockState {
        if wy <= BEDROCK_Y {
            return BlockState::full(self.bedrock);
        }
        if wy > top || (wy == top && rem < 2) {
            if wy <= water {
                return BlockState::full(if ice && wy == water { self.b.ice } else { self.water });
            }
            return AIR;
        }
        if wy == top {
            // The partial block: a half slab (never under water: see `column`).
            return BlockState::new(surf, Shape::SlabBottom);
        }
        if wy < top - 6 && wy > CAVE_FLOOR && self.is_cave(wx, wy, wz) {
            return AIR;
        }
        let rock = if rock == BANDED { self.band(wx, wy, wz) } else { rock };
        if wy == top - 1 {
            return BlockState::full(if rem >= 2 { sub } else { surf });
        }
        if wy >= top - 4 {
            return BlockState::full(if sub == rock || surf == rock { rock } else { sub });
        }
        BlockState::full(rock)
    }

    /// The generated block at `(wx, wy, wz)`, which must lie in `col` (what `generate_chunk`
    /// puts there).
    pub fn seed_block(&self, col: &Column, wx: i32, wy: i32, wz: i32) -> BlockState {
        let (lx, lz) = (wx - col.cx * CHUNK_SIZE, wz - col.cz * CHUNK_SIZE);
        debug_assert!((0..CHUNK_SIZE).contains(&lx) && (0..CHUNK_SIZE).contains(&lz));
        let i = lz as usize * CHUNK_SIZE as usize + lx as usize;
        let hq = col.height_q[i];
        let mut state = self.block_at(wx, wy, wz, hq.div_euclid(4), hq.rem_euclid(4), col.surface[i], col.subsurface[i], col.rock[i], col.water[i], col.ice[i]);
        if col.plant[i] != 0 && wy == hq.div_euclid(4) {
            state = BlockState::full(col.plant[i]);
        }
        // Features, in the same order and with the same rules as `generate_chunk`.
        for f in &col.features {
            let (lo, hi) = f.bounds();
            if wx < lo[0] || wx > hi[0] || wz < lo[2] || wz > hi[2] || wy < lo[1] || wy > hi[1] {
                continue;
            }
            f.blocks(&self.palette, &mut |x, y, z, b, solid| {
                if (x, y, z) == (wx, wy, wz) && self.feature_may_replace(state, solid) {
                    state = BlockState::full(b);
                }
            });
        }
        state
    }

    /// Whether a feature's block may go where `state` is: everything fills air (and water,
    /// for solid parts), solid parts replace leaves and water plants' leaves too.
    fn feature_may_replace(&self, state: BlockState, solid: bool) -> bool {
        state.is_air()
            || (solid && (self.palette.wood.iter().any(|w| w.1 == state.id() && w.1 != 0)))
            || (state.id() != 0 && self.plants.all().contains(&state.id()))
    }

    /// The plant growing on the ground at `(x, z)`, whose top block is `surf`, or 0. As in
    /// Vintage Story the ground is mostly covered: grass of three heights everywhere it is
    /// green, ferns under the trees, flowers in meadow patches, reeds along rivers, dry tufts
    /// and dead bushes where it is dry. Patch noise makes clumps and clearings.
    fn plant_for(&self, x: i32, z: i32, s: &LandSample, surf: u16, slope: f32) -> u16 {
        if slope > 1.3 {
            return 0;
        }
        let (b, p) = (&self.b, &self.plants);
        let soil = [b.grass, b.lush_grass, b.dry_grass, b.prairie_grass, b.cold_grass, b.meadow, b.leaf_litter, b.needle_floor, b.rich_soil, b.moss, b.mud, b.peat, b.tundra_moss]
            .contains(&surf);
        let sandy = surf == b.sand || surf == b.red_sand;
        if !soil && !sandy {
            return 0;
        }
        let h = hash3(self.seed ^ 0x9_1A47, x, 7, z);
        let roll = (h & 0xFFFF) as f32 / 65536.0;
        let pick = ((h >> 16) & 0xFFFF) as f32 / 65536.0;
        let patch = self.patches.at(x as f32 * 0.8 + 311.0, z as f32 * 0.8 - 97.0, 1.0);
        let clump = 0.45 + 0.9 * crate::noise::smoothstep(-0.35, 0.55, patch);
        if sandy {
            let density = match s.biome {
                Biome::Desert | Biome::Mesa => 0.012,
                Biome::Beach | Biome::Savanna | Biome::Steppe => 0.05,
                _ => 0.0,
            };
            return if roll < density * clump { choose(pick, &[(p.dead, 3.0), (p.dry, 1.0)]) } else { 0 };
        }
        if s.river > 0.55 && roll < 0.5 {
            return choose(pick, &[(p.reeds, 4.0), (p.tall, 2.0), (p.tuft, 1.0)]);
        }
        let grasses = |short: f32, tuft: f32, tall: f32| [(p.short, short), (p.tuft, tuft), (p.tall, tall)];
        let flowers = patch > 0.3;
        let (density, table): (f32, Vec<(u16, f32)>) = match s.biome {
            Biome::Plains if flowers => (
                0.7,
                vec![(p.short, 3.0), (p.tuft, 3.0), (p.dandelion, 2.0), (p.poppy, 1.5), (p.cornflower, 1.5), (p.daisy, 2.0), (p.lavender, 0.6)],
            ),
            Biome::Plains => (0.62, [grasses(4.0, 3.5, 1.6).to_vec(), vec![(p.daisy, 0.15), (p.dandelion, 0.15), (p.bush, 0.08)]].concat()),
            Biome::Steppe => (0.45, vec![(p.dry, 4.0), (p.short, 2.0), (p.tuft, 2.0), (p.lavender, if flowers { 0.8 } else { 0.1 }), (p.dead, 0.15)]),
            Biome::Savanna => (0.5, vec![(p.dry, 3.5), (p.tall, 2.5), (p.tuft, 2.0), (p.dead, 0.15)]),
            Biome::Forest => (
                0.42,
                [grasses(2.5, 1.5, 0.3).to_vec(), vec![(p.fern, 2.5), (p.bush, 0.6), (p.berries, 0.3), (p.daisy, if flowers { 0.6 } else { 0.05 })]].concat(),
            ),
            Biome::OldForest => (0.48, vec![(p.fern, 5.0), (p.short, 1.5), (p.tuft, 0.8), (p.bush, 0.7), (p.berries, 0.5)]),
            Biome::Swamp => (0.6, vec![(p.tall, 3.0), (p.reeds, 2.0), (p.tuft, 2.0), (p.fern, 1.0)]),
            Biome::Taiga => (0.36, vec![(p.fern, 3.0), (p.short, 2.0), (p.berries, 0.8), (p.bush, 0.4), (p.heather, 0.5)]),
            Biome::Tundra => (0.22, vec![(p.short, 3.0), (p.heather, 2.0), (p.dry, 1.0)]),
            Biome::Jungle => (0.7, vec![(p.fern, 3.0), (p.tall, 3.0), (p.tuft, 2.0), (p.bush, 1.0)]),
            Biome::Peaks => (0.1, vec![(p.short, 3.0), (p.heather, 1.0)]),
            Biome::Beach => (0.06, vec![(p.dry, 1.0), (p.short, 1.0)]),
            _ => (0.0, Vec::new()),
        };
        if table.is_empty() || roll >= density * clump {
            return 0;
        }
        choose(pick, &table)
    }

    pub fn generate_chunk(&self, pos: ChunkPos, col: &Column) -> Chunk {
        let y0 = pos.y * CHUNK_SIZE;
        let y1 = y0 + CHUNK_SIZE - 1;

        // Fast paths: most chunks in a cubic world are all-air or all-solid.
        if y1 <= BEDROCK_Y {
            return Chunk::uniform(BlockState::full(self.bedrock));
        }
        if y0 > col.max_top && y0 > SEA_LEVEL {
            return Chunk::uniform(AIR);
        }
        if y0 > BEDROCK_Y && y1 < col.min_top - 5 && y1 < CAVE_FLOOR && col.rock.iter().all(|r| *r == col.rock[0] && *r != BANDED) {
            return Chunk::uniform(BlockState::full(col.rock[0]));
        }

        let mut chunk = Chunk::uniform(AIR);
        for lz in 0..CHUNK_SIZE {
            for lx in 0..CHUNK_SIZE {
                let i = lz as usize * CHUNK_SIZE as usize + lx as usize;
                let hq = col.height_q[i];
                let (top, rem) = (hq.div_euclid(4), hq.rem_euclid(4));
                let (wx, wz) = (col.cx * CHUNK_SIZE + lx, col.cz * CHUNK_SIZE + lz);
                if y0 > top.max(col.water[i]) {
                    continue; // only air here (features come below)
                }
                for ly in 0..CHUNK_SIZE {
                    let state = self.block_at(wx, y0 + ly, wz, top, rem, col.surface[i], col.subsurface[i], col.rock[i], col.water[i], col.ice[i]);
                    if !state.is_air() {
                        chunk.set(LocalPos { x: lx as u8, y: ly as u8, z: lz as u8 }.index(), state);
                    }
                }
                // The plant on the ground (trees, stamped next, take its place).
                if col.plant[i] != 0 && (y0..=y1).contains(&top) {
                    chunk.set(LocalPos { x: lx as u8, y: (top - y0) as u8, z: lz as u8 }.index(), BlockState::full(col.plant[i]));
                }
            }
        }
        // Features: the parts that fall inside this chunk.
        let (bx, bz) = (col.cx * CHUNK_SIZE, col.cz * CHUNK_SIZE);
        for f in &col.features {
            let (lo, hi) = f.bounds();
            if hi[1] < y0 || lo[1] > y1 {
                continue;
            }
            f.blocks(&self.palette, &mut |x, y, z, b, solid| {
                let (lx, ly, lz) = (x - bx, y - y0, z - bz);
                if !(0..CHUNK_SIZE).contains(&lx) || !(0..CHUNK_SIZE).contains(&ly) || !(0..CHUNK_SIZE).contains(&lz) {
                    return;
                }
                let idx = LocalPos { x: lx as u8, y: ly as u8, z: lz as u8 }.index();
                if self.feature_may_replace(chunk.get(idx), solid) {
                    chunk.set(idx, BlockState::full(b));
                }
            });
        }
        chunk
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make() -> (BlockRegistry, WorldGen) {
        let reg = BlockRegistry::builtin();
        let g = WorldGen::new(1234, &reg);
        (reg, g)
    }

    /// A chunk column with both sea and land in it: the first one found walking out from
    /// the spawn along +x (the spawn itself is always on land).
    fn coast(g: &WorldGen) -> (i32, i32) {
        (0..4000)
            .map(|cx| (cx, 0))
            .find(|&(cx, cz)| {
                let col = g.column(cx, cz);
                col.min_top <= SEA_LEVEL && col.height_q.iter().any(|q| q.div_euclid(4) > SEA_LEVEL + 2)
            })
            .expect("a coast within 128 km")
    }

    #[test]
    fn generation_is_deterministic() {
        let (_r, g) = make();
        let col = g.column(0, 0);
        for cy in -1..3 {
            let p = ChunkPos::new(0, cy, 0);
            assert_eq!(g.generate_chunk(p, &col), g.generate_chunk(p, &g.column(0, 0)));
        }
    }

    #[test]
    fn surface_slab_matches_fractional_height() {
        let (_r, g) = make();
        let (mut slabs, mut wet) = (0, 0);
        let (kx, kz) = coast(&g);
        for (cx, cz) in [(0, 0), (6, -3), (kx, kz), (kx + 1, kz), (kx, kz + 1)] {
            let col = g.column(cx, cz);
            let mut cache: std::collections::HashMap<i32, Chunk> = std::collections::HashMap::new();
            for lz in 0..CHUNK_SIZE as usize {
                for lx in 0..CHUNK_SIZE as usize {
                    let i = lz * CHUNK_SIZE as usize + lx;
                    let hq = col.height_q_at(lx, lz);
                    let (top, rem) = (hq.div_euclid(4), hq.rem_euclid(4));
                    let cy = top.div_euclid(CHUNK_SIZE);
                    let chunk = cache.entry(cy).or_insert_with(|| g.generate_chunk(ChunkPos::new(cx, cy, cz), &col));
                    let s = chunk.get(LocalPos { x: lx as u8, y: top.rem_euclid(CHUNK_SIZE) as u8, z: lz as u8 }.index());
                    if col.water[i] >= top {
                        // Under water: water (or ice), never a slab with a pocket of air above it.
                        assert!(s.id() == g.water_id() || s.id() == g.b.ice, "at {cx},{cz} {lx},{lz}: {s:?}");
                        wet += 1;
                    } else if rem >= 2 {
                        assert_eq!(s.shape(), Shape::SlabBottom, "at {cx},{cz} {lx},{lz}");
                        slabs += 1;
                    } else {
                        // Air, or the foot of a feature standing there.
                        assert_ne!(s.shape(), Shape::SlabBottom, "at {cx},{cz} {lx},{lz}");
                    }
                }
            }
        }
        assert!(slabs > 100 && wet > 50, "slabs {slabs}, under water {wet}");
    }

    #[test]
    fn lod_columns_at_full_detail_match_the_ground() {
        let (_r, g) = make();
        let (kx, kz) = coast(&g);
        let mut seen_water = false;
        for (cx, cz) in [(kx, kz), (0, 0)] {
            let col = g.column(cx, cz);
            for lz in 0..CHUNK_SIZE {
                for lx in 0..CHUNK_SIZE {
                    let (x, z) = (cx * CHUNK_SIZE + lx, cz * CHUNK_SIZE + lz);
                    let i = (lz * CHUNK_SIZE + lx) as usize;
                    let c = g.lod_column(x, z, 1);
                    if col.water[i] != DRY {
                        assert!(c.water && c.top_q == 4 * (col.water[i] + 1), "column {x},{z}");
                        seen_water = true;
                    } else {
                        // The LOD ground is the full blocks' top (half slabs left out).
                        assert_eq!(c.ground_q.div_euclid(4), col.height_q[i].div_euclid(4), "column {x},{z}");
                    }
                }
            }
        }
        assert!(seen_water);
    }

    #[test]
    fn filtered_heights_keep_the_average_and_smooth_out_detail() {
        let (_r, g) = make();
        let mean = |cell: i32| -> f32 {
            let sum: i64 = (0..400).map(|i| g.height_q_filtered(i * 37 - 7000, i * 53 + 900, cell) as i64).sum();
            sum as f32 / 400.0 / 4.0
        };
        let full = mean(1);
        for cell in [2, 4, 16, 64, 256] {
            // (Kilometres away a few blocks don't show.)
            let tolerance = if cell >= 256 { 9.0 } else { 6.0 };
            assert!((mean(cell) - full).abs() < tolerance, "cell {cell}: mean {} vs {full}", mean(cell));
        }
        let rough = |cell: i32| -> i64 {
            (0..2000).map(|i| (g.height_q_filtered(i * 64 + 64, 300, cell) - g.height_q_filtered(i * 64, 300, cell)).abs() as i64).sum()
        };
        assert!(rough(64) < rough(1), "{} vs {}", rough(64), rough(1));
        assert_eq!(g.height_q_filtered(123, -45, 1), g.height_q(123, -45));
    }

    #[test]
    fn players_start_on_open_dry_land_near_the_origin() {
        let (_r, g) = make();
        let [x, y, z] = g.find_spawn();
        let info = g.describe(x as i32, z as i32);
        assert!(matches!(info.biome, "plains" | "forest"), "{info:?}");
        assert!((3.0..60.0).contains(&y) && x.abs() < 6000.0 && z.abs() < 6000.0, "{x},{y},{z}");
    }

    #[test]
    fn every_rock_appears_in_large_regions() {
        let (reg, g) = make();
        let mut count = std::collections::HashMap::new();
        let (mut changes, mut last) = (0, 0u16);
        for i in 0..4000 {
            let r = g.rock_at(i * 16 - 32_000, 500, 1.0);
            *count.entry(r).or_insert(0) += 1;
            if i > 0 && r != last {
                changes += 1;
            }
            last = r;
        }
        assert_eq!(count.len(), 5, "{count:?}");
        assert!(count.values().all(|n| *n > 120), "no rock is rare: {count:?}");
        assert!(changes < 200, "regions are hundreds of blocks wide: {changes} changes in 64 km");
        let names: Vec<&str> = count.keys().map(|r| reg.def(*r).name.as_str()).collect();
        assert!(names.contains(&"granite") && names.contains(&"slate"), "{names:?}");
    }

    #[test]
    fn half_slabs_only_smooth_steps() {
        let (_r, g) = make();
        let (mut slabs, mut checked) = (0, 0);
        for (cx, cz) in [(0, 0), (6, -3), (-9, 4), (20, 2)] {
            let col = g.column(cx, cz);
            for lz in 1..CHUNK_SIZE as usize - 1 {
                for lx in 1..CHUNK_SIZE as usize - 1 {
                    let q = col.height_q_at(lx, lz);
                    let top = |a: usize, b: usize| col.height_q_at(a, b).div_euclid(4);
                    let higher = [top(lx + 1, lz), top(lx - 1, lz), top(lx, lz + 1), top(lx, lz - 1)].iter().any(|&t| t > q.div_euclid(4));
                    checked += 1;
                    if q.rem_euclid(4) != 0 {
                        assert_eq!(q.rem_euclid(4), 2);
                        assert!(higher, "a slab with no higher neighbour at {cx},{cz} {lx},{lz}");
                        slabs += 1;
                    }
                }
            }
        }
        assert!(slabs > 20 && slabs < checked / 2, "{slabs} slabs in {checked} columns");
    }

    #[test]
    fn ground_tints_follow_the_climate_and_vary_gently() {
        let (_r, g) = make();
        let [x, _, z] = g.find_spawn();
        let t = g.ground_tint(x as i32, z as i32, 1);
        let s = g.land.sample(x, z, 1.0);
        assert_eq!(&t[..3], &Land::grass_rgb(s.t, s.h)[..]);
        let v: Vec<u8> = (0..200).map(|i| g.ground_tint(i * 7, 0, 1)[3]).collect();
        assert!(v.iter().any(|&a| a < 110) && v.iter().any(|&a| a > 146), "some variation: {v:?}");
        assert_eq!(g.ground_tint(5, 5, 4096)[3], 128, "far away the variation averages out");
    }

    #[test]
    fn sky_is_uniform_air_and_deep_is_uniform_rock_or_bedrock() {
        let (_r, g) = make();
        let col = g.column(0, 0);
        assert!(g.generate_chunk(ChunkPos::new(0, 20, 0), &col).is_uniform());
        assert!(g.generate_chunk(ChunkPos::new(0, -3, 0), &col).is_uniform());
        let bedrock = g.generate_chunk(ChunkPos::new(0, -10, 0), &col);
        assert_eq!(bedrock.uniform_state(), Some(BlockState::full(g.bedrock)));
    }

    #[test]
    fn features_are_seamless_across_chunk_borders() {
        // Every block a feature puts in a column of chunks is what that column generates,
        // whichever column the feature grows from; and features show up at all.
        let (_r, g) = make();
        let [sx, _, sz] = g.find_spawn();
        let (cx, cz) = ((sx as i32).div_euclid(CHUNK_SIZE), (sz as i32).div_euclid(CHUNK_SIZE));
        let mut crossing = 0;
        let mut total = 0;
        for (dx, dz) in [(0, 0), (3, 1), (-2, 4), (7, -5)] {
            let a = g.column(cx + dx, cz + dz);
            total += a.features.len();
            for f in &a.features {
                let (lo, hi) = f.bounds();
                // Columns of chunks this feature reaches.
                for ncx in lo[0].div_euclid(CHUNK_SIZE)..=hi[0].div_euclid(CHUNK_SIZE) {
                    for ncz in lo[2].div_euclid(CHUNK_SIZE)..=hi[2].div_euclid(CHUNK_SIZE) {
                        if (ncx, ncz) == (a.cx, a.cz) {
                            continue;
                        }
                        let b = g.column(ncx, ncz);
                        let mut reaches = false;
                        f.blocks(&g.palette, &mut |x, _, z, _, _| reaches |= x.div_euclid(CHUNK_SIZE) == ncx && z.div_euclid(CHUNK_SIZE) == ncz);
                        if reaches {
                            crossing += 1;
                            assert!(b.features.contains(f), "{f:?} reaches {ncx},{ncz} but that column doesn't know it");
                        }
                    }
                }
            }
        }
        assert!(total > 10, "only {total} features near the spawn");
        assert!(crossing > 0, "no feature crosses a border: test proves nothing");
    }

    #[test]
    fn river_water_is_held_in_by_its_banks() {
        // Find river water above the sea: every neighbour of a water column is water (at most
        // a block lower), or ground at least as high as the water: no water stands over dry
        // lower ground.
        let (_r, g) = make();
        let mut checked = 0;
        'search: for i in 0..120 {
            for j in 0..120 {
                let (x, z): (i32, i32) = (i * 61 - 3600, j * 61 - 3600);
                let s = g.land.sample(x as f32, z as f32, 1.0);
                if !(s.water > 1.0 && s.water != NO_WATER && s.water >= s.height) {
                    continue;
                }
                let col = g.column(x.div_euclid(CHUNK_SIZE), z.div_euclid(CHUNK_SIZE));
                for lz in 1..CHUNK_SIZE as usize - 1 {
                    for lx in 1..CHUNK_SIZE as usize - 1 {
                        let k = lz * CHUNK_SIZE as usize + lx;
                        let w = col.water[k];
                        if w == DRY || w <= SEA_LEVEL {
                            continue;
                        }
                        for (ax, az) in [(lx + 1, lz), (lx - 1, lz), (lx, lz + 1), (lx, lz - 1)] {
                            let n = az * CHUNK_SIZE as usize + ax;
                            // (A river's level may step down a block between neighbours: a rapid.)
                            let held = (col.water[n] != DRY && col.water[n] >= w - 1) || col.height_q[n].div_euclid(4) > w;
                            assert!(held, "water at {w} leaks next to ground {} (water {})", col.height_q[n] / 4, col.water[n]);
                        }
                        checked += 1;
                    }
                }
                if checked > 400 {
                    break 'search;
                }
            }
        }
        assert!(checked > 50, "only {checked} river columns checked");
    }

    #[test]
    fn each_biome_has_its_own_ground_cover() {
        let (reg, g) = make();
        let mut seen: std::collections::HashMap<&'static str, std::collections::HashSet<String>> = Default::default();
        for i in 0..200 {
            for j in 0..200 {
                let (x, z) = (i * 157 - 15000, j * 157 - 15000);
                let c = g.lod_column(x, z, 1);
                let b = g.land.sample(x as f32, z as f32, 1.0).biome.name();
                seen.entry(b).or_default().insert(reg.def(c.surface).name.clone());
            }
        }
        let has = |b: &str, block: &str| seen.get(b).is_some_and(|s| s.contains(block));
        assert!(has("desert", "sand"), "{seen:?}");
        assert!(has("plains", "grass") && has("plains", "meadow"), "{seen:?}");
        assert!(has("taiga", "cold_grass"), "{seen:?}");
        assert!(has("savanna", "dry_grass"), "{seen:?}");
        assert!(has("peaks", "snow"), "{seen:?}");
    }
}
