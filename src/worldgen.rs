//! Deterministic terrain: a pure function of (seed, chunk position).
//!
//! That purity is what makes the "procedural base + edit journal" design work:
//! unloaded chunks can always be regenerated, then edits are replayed on top.
//!
//! Heights are in quarter-block units, so the surface can end in a half-slab
//! (the "terrain slab" idea) with no change to the chunk format.
//!
//! The land's shape comes from `terrain` (continents and landforms), its ground cover from
//! `climate` and `biome`.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::biome::{self, Place, BIOMES};
use crate::block::{BlockState, Shape, AIR};
use crate::chunk::Chunk;
use crate::coords::{ChunkPos, LocalPos, CHUNK_SIZE};
use crate::features::{Canvas, Feature, Kind, Wood};
use crate::registry::BlockRegistry;
use crate::climate::{Climate, ClimateSample};
use crate::noise::{smoothstep, Fbm};
use crate::rng::{hash3, Rng};
use crate::terrain::{Terrain, TerrainSample, NO_RIVER};

pub const SEA_LEVEL: i32 = 0;
/// Bumped whenever the same seed would generate different terrain: worlds saved by an
/// older generator are kept apart (their edits would land on different ground).
pub const WORLDGEN_VERSION: u32 = 6;
pub const BEDROCK_Y: i32 = -128;
/// No caves below this; deep chunks collapse to a single uniform entry (except under the
/// Hollow Earth, whose tunnels go down to its caverns).
const CAVE_FLOOR: i32 = -48;
/// The Hollow Earth's caverns lie between these heights.
const HOLLOW_TOP: i32 = -50;
const HOLLOW_BOTTOM: i32 = -120;
/// Below this its caverns are flooded: underground lakes.
const HOLLOW_LAKE: i32 = -104;
/// The tallest a feature reaches above the ground it stands on.
pub const FEATURE_UP: i32 = 78;

/// Per-(x,z) data shared by the whole vertical stack of chunks.
#[derive(Clone, Debug)]
pub struct Column {
    pub cx: i32,
    pub cz: i32,
    /// Surface height in quarter blocks, indexed `lz * CHUNK_SIZE + lx`.
    pub height_q: Vec<i32>,
    /// Top block and the blocks under it, from the column's biome, same indexing.
    pub surface: Vec<u16>,
    pub subsurface: Vec<u16>,
    /// The biome's own stone, under the subsurface, same indexing.
    pub stone: Vec<u16>,
    /// The rock under the ground (see `WorldGen::rock_at`), same indexing.
    pub rock: Vec<u16>,
    /// Lowest / highest `top` block y (see `generate_chunk`) in this column.
    pub min_top: i32,
    pub max_top: i32,
    /// How much of the Hollow Earth is under this column (0: none).
    pub hollow: f32,
    /// The highest block water fills to (the sea's, or a river's), same indexing.
    pub water: Vec<i32>,
    pub max_water: i32,
}

impl Column {
    pub fn height_q_at(&self, lx: usize, lz: usize) -> i32 {
        self.height_q[lz * CHUNK_SIZE as usize + lx]
    }
}

/// A biome's blocks (see `biome::Biome`).
#[derive(Clone, Copy, Debug)]
struct BiomeBlocks {
    surface: u16,
    patches: u16,
    subsurface: u16,
    stone: u16,
    stone_b: u16,
    mossy: u16,
}

/// The blocks features are made of.
struct FeatureBlocks {
    woods: HashMap<&'static str, Wood>,
    moss: u16,
    mossy: Vec<u16>,
    stem: u16,
    brown_cap: u16,
    red_cap: u16,
    glowcap: u16,
    packed_ice: u16,
    glacial_ice: u16,
    frost_crystal: u16,
    glow_crystal: u16,
    cactus: u16,
    bone: u16,
    cave_moss: u16,
    fossil: u16,
}

/// Features bucketed by where they stand, for looking up the ones near a point.
#[derive(Default)]
pub struct FeatureGrid {
    buckets: HashMap<(i32, i32), Vec<Feature>>,
}

impl FeatureGrid {
    /// Bucket width: as wide as the furthest any feature reaches, so the 3 x 3 buckets
    /// around a point hold every feature that can reach it.
    const SIZE: i32 = 20;

    pub fn new(feats: Vec<Feature>) -> FeatureGrid {
        let mut buckets: HashMap<(i32, i32), Vec<Feature>> = HashMap::new();
        for f in feats {
            buckets.entry((f.x.div_euclid(Self::SIZE), f.z.div_euclid(Self::SIZE))).or_default().push(f);
        }
        FeatureGrid { buckets }
    }

    /// The features that may stand over `(x, z)`.
    pub fn near(&self, x: i32, z: i32) -> impl Iterator<Item = &Feature> {
        let (bx, bz) = (x.div_euclid(Self::SIZE), z.div_euclid(Self::SIZE));
        (-1..=1).flat_map(move |dz| (-1..=1).map(move |dx| (bx + dx, bz + dz))).filter_map(|k| self.buckets.get(&k)).flatten()
    }

    pub fn is_empty(&self) -> bool {
        self.buckets.is_empty()
    }
}


/// Feature tiers: big things on a coarse grid, ordinary ones on a fine grid, and the
/// Hollow Earth's on their own.
const TIERS: [(u8, i32); 3] = [(0, 24), (1, 5), (2, 10)];

pub struct WorldGen {
    seed: u64,
    terrain: Terrain,
    climate: Climate,
    stone: u16,
    /// The rock types, in the order regions wind around each other (see `rock_at`).
    rocks: [u16; 5],
    rock_regions: Fbm,
    sand: u16,
    gravel: u16,
    water: u16,
    bedrock: u16,
    shingle: u16,
    mud: u16,
    snow: u16,
    /// Per entry of `biome::BIOMES`.
    biome_blocks: Vec<BiomeBlocks>,
    /// Gentle large-scale brightness variation of the ground (see `ground_tint`).
    variation: Fbm,
    /// Region noises: the Blightlands, the Petrified Kingdom, oases, the Hollow Earth.
    blight: Fbm,
    ancient: Fbm,
    oasis: Fbm,
    hollow: Fbm,
    /// Where a biome's second ground block shows through in patches.
    patches: Fbm,
    /// Groves and glades: where trees grow thick (see `grove`).
    groves: Fbm,
    fb: FeatureBlocks,
    ores: HashMap<&'static str, u16>,
    rock_ids: Vec<u16>,
    canopy_blocks: HashMap<&'static str, u16>,
    /// Feature anchors already worked out: (tier, cell x, cell z) -> the feature there.
    anchors: Mutex<HashMap<(u8, i32, i32), Option<Feature>>>,
}

/// What the ground at a column is like (see `WorldGen::describe`).
#[derive(Clone, Copy, Debug)]
pub struct GroundInfo {
    pub biome: &'static str,
    pub landform: &'static str,
    pub height: f32,
    pub climate: ClimateSample,
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

/// The highest block water fills to over terrain `t`: the sea's, or a river's (its
/// surface is the top of that block).
pub fn water_level(t: &TerrainSample) -> i32 {
    if t.water > NO_RIVER {
        (t.water as i32 - 1).max(SEA_LEVEL)
    } else {
        SEA_LEVEL
    }
}

/// A uniform number 0..1 from a hash.
fn unit(h: u64) -> f32 {
    (h >> 40) as f32 * (1.0 / 16_777_216.0)
}

/// Picks from (weight, choice) pairs with `u` in 0..1.
fn pick<T: Copy>(u: f32, table: &[(f32, T)]) -> Option<T> {
    let total: f32 = table.iter().map(|e| e.0).sum();
    let mut x = u * total;
    for &(w, t) in table {
        if x < w {
            return Some(t);
        }
        x -= w;
    }
    table.last().map(|e| e.1)
}

impl WorldGen {
    pub fn new(seed: u64, reg: &BlockRegistry) -> Self {
        let id = |n: &str| reg.id(n).unwrap_or_else(|| panic!("worldgen needs block '{n}'"));
        let biome_blocks: Vec<BiomeBlocks> = BIOMES
            .iter()
            .map(|b| BiomeBlocks {
                surface: id(b.surface),
                patches: id(b.patches),
                subsurface: id(b.subsurface),
                stone: id(b.stone),
                stone_b: id(b.stone_b),
                mossy: id(&format!("{}_mossy", b.stone)),
            })
            .collect();
        // Species sizes in percent of an oak's.
        let size = |sp: &str| match sp {
            "maple" | "fir" | "petrified" => 90,
            "birch" | "frostpine" | "stonebark" | "goldbark" | "golden_oak" => 85,
            "fig" | "larch" | "seapine" | "glowwood" => 80,
            "ironwood" | "cragpine" => 70,
            "elm" => 105,
            _ => 100,
        };
        let wood = |sp: &str, log: &str, leaves: &str| Wood { log: id(log), leaves: if leaves.is_empty() { 0 } else { id(leaves) }, size: size(sp) };
        let mut woods = HashMap::new();
        for sp in [
            "redwood", "elder", "maple", "goldbark", "birch", "fir", "frostpine", "pine", "spruce", "larch", "ironwood", "cragpine", "palm", "acacia", "bamboo", "fig", "cypress",
            "willow", "frostwood", "silverwood", "blightwood", "ashwood", "deadwood", "stonebark", "glowwood", "fungalwood", "seapine", "elm",
        ] {
            woods.insert(sp, wood(sp, &format!("{sp}_log"), &format!("{sp}_leaves")));
        }
        woods.insert("oak", wood("oak", "log", "leaves"));
        woods.insert("golden_oak", wood("golden_oak", "log", "golden_oak_leaves"));
        woods.insert("driftwood", wood("driftwood", "driftwood_log", ""));
        woods.insert("petrified", wood("petrified", "petrified_log", ""));
        let fb = FeatureBlocks {
            woods,
            moss: id("moss_block"),
            mossy: biome_blocks.iter().map(|b| b.mossy).collect(),
            stem: id("mushroom_stem"),
            brown_cap: id("brown_mushroom_cap"),
            red_cap: id("red_mushroom_cap"),
            glowcap: id("glowcap"),
            packed_ice: id("packed_ice"),
            glacial_ice: id("glacial_ice"),
            frost_crystal: id("frost_crystal"),
            glow_crystal: id("glow_crystal"),
            cactus: id("cactus"),
            bone: id("bone_block"),
            cave_moss: id("cave_moss"),
            fossil: id("fossil_stone"),
        };
        let mut ores = HashMap::new();
        for n in [
            "coal_ore", "iron_ore", "copper_ore", "desert_copper_ore", "silver_ore", "gold_ore", "ruby_ore", "sapphire_ore", "blight_ore", "rock_salt", "fossil_stone", "amber_block",
            "frost_crystal", "glow_crystal",
        ] {
            ores.insert(n, id(n));
        }
        let rocks = [id("basalt"), id("granite"), id("stone"), id("limestone"), id("sandstone")];
        let mut rock_ids: Vec<u16> = rocks.to_vec();
        for b in &biome_blocks {
            rock_ids.extend([b.stone, b.stone_b]);
        }
        rock_ids.sort_unstable();
        rock_ids.dedup();
        let s = |k: u64| seed.wrapping_mul(0x2545_F491_4F6C_DD1D).wrapping_add(k);
        WorldGen {
            seed,
            terrain: Terrain::new(seed),
            climate: Climate::new(seed),
            stone: id("stone"),
            rocks,
            rock_regions: Fbm::new(seed ^ 0x0C0F_FEE5, 2600.0, 2, 0.4),
            sand: id("sand"),
            gravel: id("gravel"),
            water: id("water"),
            bedrock: id("bedrock"),
            shingle: id("shingle"),
            mud: id("mud"),
            snow: id("snow"),
            biome_blocks,
            variation: Fbm::new(seed ^ 0x7A11_7E5D, 60.0, 2, 0.5),
            blight: Fbm::new(s(11), 2600.0, 3, 0.5),
            ancient: Fbm::new(s(12), 2200.0, 3, 0.5),
            oasis: Fbm::new(s(13), 260.0, 2, 0.5),
            hollow: Fbm::new(s(14), 1600.0, 2, 0.5),
            patches: Fbm::new(s(15), 22.0, 2, 0.55),
            groves: Fbm::new(s(16), 150.0, 3, 0.5),
            fb,
            ores,
            rock_ids,
            canopy_blocks: [
                "elder_leaves", "maple_leaves", "spruce_leaves", "fir_leaves", "cragpine_leaves", "palm_leaves", "cypress_leaves", "larch_leaves", "acacia_leaves", "blightwood_log",
                "petrified_log", "seapine_leaves", "leaves",
            ]
            .into_iter()
            .map(|n| (n, id(n)))
            .collect(),
            anchors: Mutex::new(HashMap::new()),
        }
    }

    pub fn water_id(&self) -> u16 {
        self.water
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Surface height in quarter-block units.
    pub fn height_q(&self, x: i32, z: i32) -> i32 {
        self.height_q_filtered(x, z, 1)
    }

    /// `height_q` as seen from far away, for terrain drawn as cells `cell` blocks wide:
    /// detail finer than a cell fades to its average instead of aliasing (a random-looking
    /// bump per cell that changes when the cell size changes), so neighbouring detail levels
    /// agree closely. `cell <= 1` is exactly `height_q`.
    pub fn height_q_filtered(&self, x: i32, z: i32, cell: i32) -> i32 {
        if cell <= 1 {
            return self.surface_q(x, z);
        }
        4 * self.terrain.sample(x as f32, z as f32, cell as f32).height.round() as i32
    }

    /// The surface at full detail, in quarter blocks: whole blocks (the height rounded), plus
    /// a half slab only at the foot of a step, where the ground rises toward a higher
    /// neighbour. Slopes get smoothed; flat ground stays flat.
    fn surface_q(&self, x: i32, z: i32) -> i32 {
        let h = |x: i32, z: i32| self.terrain.sample(x as f32, z as f32, 1.0).height;
        surface_from(h(x, z), [h(x + 1, z), h(x - 1, z), h(x, z + 1), h(x, z - 1)])
    }

    /// The rock of the region around `(x, z)`: the world's bedrock changes every kilometre or
    /// so (basalt, granite, andesite, limestone, sandstone). Regions wind around each other
    /// in that order. Near the surface each biome has its own stone (`Column::stone`).
    pub fn rock_at(&self, x: i32, z: i32, cell: f32) -> u16 {
        let v = self.rock_regions.at(x as f32, z as f32, cell);
        let i = ((v * 9.5 + 2.5).floor() as i32).clamp(0, 4);
        self.rocks[i as usize]
    }

    /// The biome at `(x, z)` (index into `BIOMES`), for terrain `t` there.
    pub fn biome_at(&self, x: i32, z: i32, t: &TerrainSample, cell: f32) -> usize {
        let alt = t.smooth.max(0.0);
        let c = self.climate.at(x as f32, z as f32, alt, t.inland, cell);
        self.biome_for(x, z, c, alt, t.inland, cell)
    }

    fn biome_for(&self, x: i32, z: i32, c: ClimateSample, alt: f32, inland: f32, cell: f32) -> usize {
        let (fx, fz) = (x as f32, z as f32);
        let place = Place {
            altitude: alt,
            inland,
            blight: self.blight.at(fx, fz, cell),
            ancient: self.ancient.at(fx, fz, cell),
            oasis: self.oasis.at(fx, fz, cell),
        };
        biome::choose(c.temperature, c.rainfall, place)
    }

    /// How much of the Hollow Earth lies under `(x, z)`: 0 none .. 1 its heart.
    pub fn hollow_at(&self, x: i32, z: i32) -> f32 {
        smoothstep(0.3, 0.5, self.hollow.at(x as f32, z as f32, 1.0))
    }

    /// Whether `(x, y, z)` is open cavern in the Hollow Earth (`zone`: `hollow_at` there).
    fn is_cavern(&self, x: i32, y: i32, z: i32, zone: f32) -> bool {
        if zone <= 0.0 || !(HOLLOW_BOTTOM..=HOLLOW_TOP).contains(&y) {
            return false;
        }
        let e = (y as f32 + 84.0) / 32.0;
        let (fx, fy, fz) = (x as f32, y as f32, z as f32);
        let big = value_noise_3d(self.seed ^ 0x4011_0E, fx / 42.0, fy / 22.0, fz / 42.0);
        let d = zone * big - 0.55 * e * e;
        if d < 0.2 {
            return false;
        }
        if d > 0.36 {
            return true;
        }
        let fine = value_noise_3d(self.seed ^ 0x4011_0F, fx / 12.0, fy / 9.0, fz / 12.0);
        d + 0.16 * (fine - 0.5) > 0.28
    }

    /// The (surface, subsurface, stone) blocks of a column whose partial (top) block is at
    /// `top`; `slope` is how many blocks its steepest neighbour differs by.
    fn ground_blocks(&self, x: i32, z: i32, top: i32, t: &TerrainSample, cell: f32, slope: i32) -> (u16, u16, u16) {
        let alt = t.smooth.max(0.0);
        let c = self.climate.at(x as f32, z as f32, alt, t.inland, cell);
        let b = self.biome_for(x, z, c, alt, t.inland, cell);
        let bb = self.biome_blocks[b];
        let stone = if self.patches.at(x as f32 + 900.0, z as f32, cell) > 0.25 { bb.stone_b } else { bb.stone };
        // Under water (top <= its level), and beaches: dry ground at sea level near the coast.
        // Inland lakes and wetland ponds keep the biome's ground right to the water's edge;
        // river beds are gravel and sand (clay and mud in the swamps).
        let wl = water_level(t);
        if wl > SEA_LEVEL && top <= wl {
            let s = if b == biome::MURKWATER { self.mud } else if self.patches.at(x as f32 + 400.0, z as f32, cell) > 0.0 { self.gravel } else { self.sand };
            return (s, s, stone);
        }
        if top <= SEA_LEVEL || (top - 1 <= SEA_LEVEL && t.inland < 0.35) {
            let s = if b == biome::MURKWATER {
                self.mud
            } else if b == biome::STORMBREAK || top < SEA_LEVEL - 10 || c.temperature < 0.0 {
                if b == biome::STORMBREAK { self.shingle } else { self.gravel }
            } else {
                self.sand
            };
            return (s, s, stone);
        }
        // Cliffs show the biome's stone.
        if slope >= 3 && b != biome::GLACIER {
            return (stone, stone, stone);
        }
        let mut surf = if self.patches.at(x as f32, z as f32, cell) > 0.3 { bb.patches } else { bb.surface };
        // The highlands are green below and snowbound up high.
        if b == biome::FROSTPEAK && c.temperature < -3.0 {
            surf = self.snow;
        }
        if b == biome::STONEFANG && slope >= 2 {
            surf = stone;
        }
        let surf = if surf == self.stone { stone } else { surf };
        (surf, bb.subsurface, stone)
    }

    /// How the ground at `(x, z)` is coloured, for terrain drawn in cells `cell` blocks wide:
    /// the biome's grass colour (sRGB) and, in the last byte, a brightness variation (128 is
    /// neutral) that varies over tens of blocks and hides how textures repeat.
    pub fn ground_tint(&self, x: i32, z: i32, cell: i32) -> [u8; 4] {
        let t = self.terrain.sample(x as f32, z as f32, cell as f32);
        let [r, g, b] = biome::grass_rgb(&BIOMES[self.biome_at(x, z, &t, cell as f32)]);
        let v = self.variation.at(x as f32, z as f32, cell as f32);
        [r, g, b, (128.0 + 127.0 * v.clamp(-1.0, 1.0)) as u8]
    }

    /// Biome, landform, height and climate of the column at `(x, z)`.
    pub fn describe(&self, x: i32, z: i32) -> GroundInfo {
        let t = self.terrain.sample(x as f32, z as f32, 1.0);
        let c = self.climate.at(x as f32, z as f32, t.smooth.max(0.0), t.inland, 1.0);
        let biome = if t.height < (SEA_LEVEL + 1) as f32 {
            "sea"
        } else if t.height.round() as i32 <= water_level(&t) {
            "river"
        } else {
            BIOMES[self.biome_at(x, z, &t, 1.0)].name
        };
        GroundInfo { biome, landform: self.terrain.landform_at(x as f32, z as f32).name, height: t.height, climate: c }
    }

    /// Where a new player starts: the nearest open lowland in the Giants' Grasslands to the
    /// origin (else in the woods), searched in 64-block steps, nearest first, up to 6 km out,
    /// as a point on the ground with no tree or boulder on it.
    pub fn find_spawn(&self) -> [f32; 3] {
        for biomes in [&["Giants' Grasslands"][..], &["Giants' Grasslands", "Amberwood", "Ancient Wildwood"][..]] {
            if let Some(p) = self.spawn_in(biomes) {
                return p;
            }
        }
        [0.5, self.describe(0, 0).height.max(0.0), 0.5]
    }

    fn spawn_in(&self, biomes: &[&str]) -> Option<[f32; 3]> {
        let ok = |x: i32, z: i32| {
            let g = self.describe(x, z);
            // Gentle ground all round (no cliff in the first steps), with no tree on it.
            let flat = || {
                [(12, 0), (-12, 0), (0, 12), (0, -12), (8, 8), (-8, -8), (8, -8), (-8, 8)]
                    .iter()
                    .all(|&(dx, dz)| (self.describe(x + dx, z + dz).height - g.height).abs() < 2.5)
            };
            ((3.0..40.0).contains(&g.height) && biomes.contains(&g.biome) && flat() && self.clear_of_features(x, z)).then_some(g.height)
        };
        let mut best: Option<(i64, i32, i32, f32)> = None;
        for ring in 0..96 {
            // Every point of this square ring; keep the closest match (rings overlap in distance).
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
        best.map(|(_, x, z, h)| [x as f32 + 0.5, h, z as f32 + 0.5])
    }

    /// Whether no feature stands within a few blocks of `(x, z)`.
    fn clear_of_features(&self, x: i32, z: i32) -> bool {
        for &(tier, cell) in &TIERS[..2] {
            let (gx, gz) = (x.div_euclid(cell), z.div_euclid(cell));
            let n = if tier == 0 { 1 } else { 2 };
            for dz in -n..=n {
                for dx in -n..=n {
                    if let Some(f) = self.anchor(tier, gx + dx, gz + dz) {
                        let r = f.kind.reach().0 + 2;
                        if (f.x - x).pow(2) + (f.z - z).pow(2) < r * r {
                            return false;
                        }
                    }
                }
            }
        }
        true
    }

    /// The visible surface of the column at block `(x, z)` for level-of-detail terrain:
    /// its top in quarter blocks and the block it is made of. Matches `generate_chunk`
    /// (slab tops, the biome's ground, water up to the sea surface; caves are never visible
    /// from above; trees and other features are not drawn far away) for `cell == 1`, and is
    /// band-limited for coarser cells.
    pub fn lod_surface(&self, x: i32, z: i32, cell: i32) -> (i32, u16) {
        let t = self.terrain.sample(x as f32, z as f32, cell as f32);
        let hq = if cell <= 1 { self.surface_q(x, z) } else { 4 * t.height.round() as i32 };
        let top = hq.div_euclid(4);
        let slab = hq.rem_euclid(4) >= 2;
        // Water fills every block above the terrain up to the sea's or a river's level, and
        // there are no half-slabs under water (see `block_at`).
        let wl = water_level(&t);
        if top <= wl {
            return (4 * (wl + 1), self.water);
        }
        let slope = if cell <= 1 {
            let h = |x: i32, z: i32| self.terrain.sample(x as f32, z as f32, 1.0).height.round() as i32;
            [h(x + 1, z), h(x - 1, z), h(x, z + 1), h(x, z - 1)].iter().map(|n| (n - top).abs()).max().unwrap_or(0)
        } else {
            0
        };
        let surf = self.ground_blocks(x, z, top, &t, cell as f32, slope).0;
        (4 * top + if slab { 2 } else { 0 }, surf)
    }

    /// The features distant terrain draws in the area `x0..x1, z0..z1` for cells `cell`
    /// blocks wide: each one, from cells up to 8 blocks; only the big ones (giant trees,
    /// spires, arches...) up to 32; none past that (`lod_cell` draws forests there as a
    /// canopy).
    pub fn lod_features(&self, x0: i32, z0: i32, x1: i32, z1: i32, cell: i32) -> FeatureGrid {
        let mut out = Vec::new();
        for &(tier, size) in &TIERS[..2] {
            if (tier == 1 && cell > 8) || cell > 32 {
                continue;
            }
            let reach = if tier == 0 { 19 } else { 9 };
            for gz in (z0 - reach).div_euclid(size)..=(z1 + reach).div_euclid(size) {
                for gx in (x0 - reach).div_euclid(size)..=(x1 + reach).div_euclid(size) {
                    if let Some(f) = self.anchor(tier, gx, gz) {
                        out.push(f);
                    }
                }
            }
        }
        FeatureGrid::new(out)
    }

    /// A biome's forest as distant terrain draws it where single trees are smaller than its
    /// cells: how much of the ground the crowns cover, how high they stand, their block.
    fn canopy(&self, b: usize) -> (f32, f32, &'static str) {
        use biome::*;
        match b {
            WILDWOOD => (0.85, 18.0, "elder_leaves"),
            AMBERWOOD => (0.6, 13.0, "maple_leaves"),
            TAIGA => (0.75, 16.0, "spruce_leaves"),
            FROSTPEAK => (0.3, 12.0, "fir_leaves"),
            STONEFANG => (0.15, 9.0, "cragpine_leaves"),
            OASIS => (0.45, 11.0, "palm_leaves"),
            MURKWATER => (0.6, 13.0, "cypress_leaves"),
            TUNDRA => (0.05, 5.0, "larch_leaves"),
            SUNSCORCHED => (0.02, 6.0, "acacia_leaves"),
            BLIGHT => (0.2, 9.0, "blightwood_log"),
            PETRIFIED => (0.15, 10.0, "petrified_log"),
            STORMBREAK => (0.15, 11.0, "seapine_leaves"),
            GRASSLANDS => (0.06, 12.0, "leaves"),
            _ => (0.0, 0.0, ""),
        }
    }

    /// The surface distant terrain draws for the cell sampled at `(x, z)`, `cell` blocks
    /// wide: the ground (`lod_surface`), raised by any of `feats` (from `lod_features`)
    /// standing over it, and, in cells too big for single trees, by the biome's canopy over
    /// the share of cells its crowns cover.
    pub fn lod_cell(&self, x: i32, z: i32, cell: i32, feats: &FeatureGrid) -> (i32, u16) {
        let (mut q, mut b) = self.lod_surface(x, z, cell);
        let ground = q;
        let half = cell / 2;
        for f in feats.near(x, z) {
            let (r, _, _) = f.kind.reach();
            if (f.x - x).abs() > r + half || (f.z - z).abs() > r + half {
                continue;
            }
            if let Some((h, blk)) = f.lod_top(x - f.x, z - f.z) {
                let top = 4 * (f.y + 1) + (h * 4.0) as i32;
                if top > q {
                    (q, b) = (top, blk);
                }
            }
        }
        if cell > 8 && b != self.water && ground == q {
            let t = self.terrain.sample(x as f32, z as f32, cell as f32);
            let biome = self.biome_at(x, z, &t, cell as f32);
            let (cover, height, name) = self.canopy(biome);
            let h = hash3(self.seed ^ 0xCA90, x, cell, z);
            // Groves show as solid canopy, glades as open ground, and the edge between them
            // a little ragged.
            let g = self.grove(x, z, cell as f32);
            let density = cover * match biome {
                biome::WILDWOOD | biome::TAIGA | biome::AMBERWOOD | biome::MURKWATER | biome::OASIS => 0.45 + 0.6 * g,
                biome::FROSTPEAK | biome::BLIGHT | biome::PETRIFIED | biome::STORMBREAK => 0.2 + g,
                _ => 0.1 + 2.0 * g * g,
            };
            if cover > 0.0 && density > 0.42 + 0.2 * unit(h) {
                if let Some(&blk) = self.canopy_blocks.get(name) {
                    let top = ground + (4.0 * height * (0.7 + 0.6 * unit(h.rotate_left(21)))) as i32;
                    (q, b) = (top, blk);
                }
            }
        }
        (q, b)
    }

    /// Just the height part of `lod_surface` (much cheaper: no climate).
    pub fn lod_height(&self, x: i32, z: i32, cell: i32) -> i32 {
        let t = self.terrain.sample(x as f32, z as f32, cell.max(1) as f32);
        let hq = if cell <= 1 { self.surface_q(x, z) } else { 4 * t.height.round() as i32 };
        let top = hq.div_euclid(4);
        let wl = water_level(&t);
        if top <= wl {
            return 4 * (wl + 1);
        }
        4 * top + if hq.rem_euclid(4) >= 2 { 2 } else { 0 }
    }

    /// The top of the highest full block of the column at `(x, z)` in quarter blocks, at
    /// full detail: the ground under any half-slab or water.
    pub fn ground_q(&self, x: i32, z: i32) -> i32 {
        4 * self.terrain.sample(x as f32, z as f32, 1.0).height.round() as i32
    }

    pub fn column(&self, cx: i32, cz: i32) -> Column {
        let n = (CHUNK_SIZE * CHUNK_SIZE) as usize;
        let (mut heights, mut surface, mut subsurface) = (Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n));
        let (mut rock, mut stone, mut water) = (Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n));
        let (mut min_top, mut max_top) = (i32::MAX, i32::MIN);
        // The terrain with a one-block border: slabs depend on the neighbouring heights.
        const W: i32 = CHUNK_SIZE + 2;
        let samples: Vec<TerrainSample> = (0..W * W)
            .map(|i| {
                let (x, z) = (cx * CHUNK_SIZE + i % W - 1, cz * CHUNK_SIZE + i / W - 1);
                self.terrain.sample(x as f32, z as f32, 1.0)
            })
            .collect();
        let h = |lx: i32, lz: i32| samples[((lz + 1) * W + lx + 1) as usize].height;
        for lz in 0..CHUNK_SIZE {
            for lx in 0..CHUNK_SIZE {
                let (x, z) = (cx * CHUNK_SIZE + lx, cz * CHUNK_SIZE + lz);
                let t = samples[((lz + 1) * W + lx + 1) as usize];
                let nb = [h(lx + 1, lz), h(lx - 1, lz), h(lx, lz + 1), h(lx, lz - 1)];
                let hq = surface_from(t.height, nb);
                let top = hq.div_euclid(4);
                let slope = nb.iter().map(|v| (v.round() as i32 - t.height.round() as i32).abs()).max().unwrap_or(0);
                let (surf, sub, st) = self.ground_blocks(x, z, top, &t, 1.0, slope);
                min_top = min_top.min(top);
                max_top = max_top.max(top);
                heights.push(hq);
                surface.push(surf);
                subsurface.push(sub);
                stone.push(st);
                water.push(water_level(&t));
                rock.push(self.rock_at(x, z, 1.0));
            }
        }
        let c = CHUNK_SIZE / 2;
        let hollow = [(0, 0), (c * 2, 0), (0, c * 2), (c * 2, c * 2), (c, c)]
            .iter()
            .map(|&(dx, dz)| self.hollow_at(cx * CHUNK_SIZE + dx, cz * CHUNK_SIZE + dz))
            .fold(0.0f32, f32::max);
        let max_water = water.iter().copied().max().unwrap_or(SEA_LEVEL);
        Column { cx, cz, height_q: heights, surface, subsurface, stone, rock, min_top, max_top, hollow, water, max_water }
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

    /// The terrain block at `(wx, wy, wz)` (without features), for a column whose partial
    /// block is at `top` with `rem` quarters in it. `zone`: the Hollow Earth under it.
    #[allow(clippy::too_many_arguments)]
    fn block_at(&self, wx: i32, wy: i32, wz: i32, top: i32, rem: i32, surf: u16, sub: u16, stone: u16, rock: u16, zone: f32, wl: i32) -> BlockState {
        if wy <= BEDROCK_Y {
            return BlockState::full(self.bedrock);
        }

        if wy > top {
            return if wy <= wl { BlockState::full(self.water) } else { AIR };
        }
        if wy == top {
            // The partial block: a half-slab when the fractional height is >= 0.5, but never
            // under water: a slab there would leave a pocket of air in the sea (slabs are not
            // waterlogged) and its sides would z-fight with the water beside it.
            if wy <= wl {
                return BlockState::full(self.water);
            }
            if rem >= 2 {
                return BlockState::new(surf, Shape::SlabBottom);
            }
            return AIR;
        }
        let floor = if zone > 0.0 { HOLLOW_BOTTOM + 10 } else { CAVE_FLOOR };
        if wy < top - 6 && wy > floor && self.is_cave(wx, wy, wz) {
            return AIR;
        }
        if self.is_cavern(wx, wy, wz, zone) {
            return if wy < HOLLOW_LAKE { BlockState::full(self.water) } else { AIR };
        }
        if wy == top - 1 {
            return BlockState::full(if rem >= 2 { sub } else { surf });
        }
        if wy >= top - 4 {
            return BlockState::full(sub);
        }
        if wy >= top - 14 {
            return BlockState::full(stone);
        }
        BlockState::full(rock)
    }

    /// The generated terrain block at `(wx, wy, wz)`, which must lie in `col` (what
    /// `generate_chunk` puts there before trees and other features).
    pub fn seed_block(&self, col: &Column, wx: i32, wy: i32, wz: i32) -> BlockState {
        let (lx, lz) = (wx - col.cx * CHUNK_SIZE, wz - col.cz * CHUNK_SIZE);
        debug_assert!((0..CHUNK_SIZE).contains(&lx) && (0..CHUNK_SIZE).contains(&lz));
        let hq = col.height_q_at(lx as usize, lz as usize);
        let i = lz as usize * CHUNK_SIZE as usize + lx as usize;
        let zone = if col.hollow > 0.0 { self.hollow_at(wx, wz) } else { 0.0 };
        self.block_at(wx, wy, wz, hq.div_euclid(4), hq.rem_euclid(4), col.surface[i], col.subsurface[i], col.stone[i], col.rock[i], zone, col.water[i])
    }

    // ------------------------------------------------------------------ features

    /// The top full block of the ground at `(x, z)`.
    fn ground_y(&self, x: i32, z: i32) -> i32 {
        self.terrain.sample(x as f32, z as f32, 1.0).height.round() as i32 - 1
    }

    /// The feature of tier `tier` in grid cell `(gx, gz)`, if any (cached).
    fn anchor(&self, tier: u8, gx: i32, gz: i32) -> Option<Feature> {
        if let Some(a) = self.anchors.lock().unwrap_or_else(|e| e.into_inner()).get(&(tier, gx, gz)) {
            return *a;
        }
        let a = self.make_anchor(tier, gx, gz);
        let mut m = self.anchors.lock().unwrap_or_else(|e| e.into_inner());
        if m.len() > 300_000 {
            m.clear();
        }
        m.insert((tier, gx, gz), a);
        a
    }

    fn make_anchor(&self, tier: u8, gx: i32, gz: i32) -> Option<Feature> {
        let cell = TIERS[tier as usize].1;
        let h = hash3(self.seed ^ 0xFEA7_0000 ^ tier as u64, gx, 0, gz);
        let margin = if tier == 1 { 1 } else { 3 };
        let span = (cell - 2 * margin).max(1) as u64;
        let x = gx * cell + margin + (h % span) as i32;
        let z = gz * cell + margin + ((h >> 16) % span) as i32;
        let seed = hash3(self.seed ^ 0x5EED, x, tier as i32, z);
        let roll = unit(h.rotate_left(13));
        let u = unit(h.rotate_left(29));
        if tier == 2 {
            return self.hollow_anchor(x, z, seed, roll, u);
        }
        let t = self.terrain.sample(x as f32, z as f32, 1.0);
        let y = t.height.round() as i32 - 1;
        let b = self.biome_at(x, z, &t, 1.0);
        if water_level(&t) > SEA_LEVEL && y < water_level(&t) + 1 {
            return None; // nothing grows in rivers
        }
        let underwater = y < SEA_LEVEL + 1;
        if underwater && !(b == biome::STORMBREAK && tier == 0) && !(b == biome::MURKWATER && y >= SEA_LEVEL - 2) {
            return None;
        }
        let kind = if tier == 0 { self.big_feature(b, roll, u, x, z, underwater)? } else { self.small_feature(b, roll, u, x, z)? };
        if underwater && !matches!(kind, Kind::Spire { .. } | Kind::Cypress(_) | Kind::Willow(_)) {
            return None;
        }
        let f = Feature { x, y, z, kind, seed };
        if tier == 1 && self.near_big(&f) {
            return None;
        }
        Some(f)
    }

    /// Whether a big feature (a giant tree, a spire...) stands where a small one would go.
    fn near_big(&self, f: &Feature) -> bool {
        let cell = TIERS[0].1;
        let (gx, gz) = (f.x.div_euclid(cell), f.z.div_euclid(cell));
        for dz in -1..=1 {
            for dx in -1..=1 {
                if let Some(b) = self.anchor(0, gx + dx, gz + dz) {
                    let r = match b.kind {
                        Kind::Boulder { .. } | Kind::FallenLog { .. } | Kind::Skeleton(_) | Kind::Crystals(_) | Kind::Sinkhole => 6,
                        k => k.reach().0 * 2 / 3,
                    };
                    if (b.x - f.x).pow(2) + (b.z - f.z).pow(2) < r * r {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// How thick the trees grow at `(x, z)`: 0 in a glade .. 1 in the heart of a grove.
    pub fn grove(&self, x: i32, z: i32, cell: f32) -> f32 {
        smoothstep(-0.3, 0.2, self.groves.at(x as f32, z as f32, cell))
    }

    /// What the grove noise does to a biome's tree density: forests keep most of it and
    /// fill up in their groves; open lands gather their few trees into copses.
    fn grove_factor(&self, b: usize, x: i32, z: i32) -> f32 {
        let g = self.grove(x, z, 1.0);
        use biome::*;
        match b {
            WILDWOOD | TAIGA | AMBERWOOD | MURKWATER | OASIS => 0.45 + 0.6 * g,
            FROSTPEAK | BLIGHT | PETRIFIED | STORMBREAK => 0.2 + 1.0 * g,
            _ => 0.1 + 2.0 * g * g,
        }
    }

    fn w(&self, sp: &str) -> Wood {
        self.fb.woods[sp]
    }

    fn big_feature(&self, b: usize, roll: f32, u: f32, x: i32, z: i32, underwater: bool) -> Option<Kind> {
        let bb = self.biome_blocks[b];
        let mossy = self.fb.mossy[b];
        let boulder = |m: bool| Kind::Boulder { stone: bb.stone, moss: if m { mossy } else { 0 }, big: true };
        let spire = Kind::Spire { stone: bb.stone, band: bb.stone_b };
        if self.hollow_at(x, z) > 0.85 && roll < 0.035 {
            return Some(Kind::Sinkhole);
        }
        use biome::*;
        let (chance, table): (f32, Vec<(f32, Kind)>) = match b {
            WILDWOOD => (0.6, vec![
                (0.32, Kind::Broadleaf(self.w("elder"), 2)),
                (0.12, Kind::Broadleaf(self.w("oak"), 2)),
                (0.34, Kind::Redwood(self.w("redwood"))),
                (0.12, boulder(true)),
                (0.1, Kind::FallenLog { log: self.w("redwood").log, moss: self.fb.moss, big: true }),
            ]),
            AMBERWOOD => (0.3, vec![(0.35, Kind::Broadleaf(self.w("goldbark"), 2)), (0.25, Kind::Broadleaf(self.w("maple"), 2)), (0.25, boulder(true)), (0.15, Kind::FallenLog { log: self.w("maple").log, moss: self.fb.moss, big: true })]),
            FROSTPEAK => (0.25, vec![(0.5, boulder(false)), (0.25, spire), (0.25, Kind::Conifer { wood: self.w("fir"), open: false, giant: true })]),
            STONEFANG => (0.5, vec![(0.5, spire), (0.15, Kind::Arch { stone: bb.stone, band: bb.stone_b }), (0.35, boulder(false))]),
            SUNSCORCHED => (0.18, vec![(0.35, spire), (0.35, boulder(false)), (0.3, Kind::Skeleton(self.fb.bone))]),
            OASIS => (0.2, vec![(0.6, boulder(true)), (0.4, Kind::Broadleaf(self.w("fig"), 2))]),
            MURKWATER => (0.35, vec![(0.55, Kind::Cypress(self.w("cypress"))), (0.2, boulder(true)), (0.25, Kind::Mushroom { stem: self.fb.stem, cap: self.fb.red_cap, giant: true })]),
            TUNDRA => (0.15, vec![(0.6, boulder(false)), (0.15, Kind::Skeleton(self.fb.bone)), (0.25, Kind::IceSpike { ice: self.fb.packed_ice, core: self.fb.glacial_ice, big: true })]),
            GLACIER => (0.35, vec![(0.6, Kind::IceSpike { ice: self.fb.packed_ice, core: self.fb.glacial_ice, big: true }), (0.2, Kind::Crystals(self.fb.frost_crystal)), (0.2, boulder(false))]),
            TAIGA => (0.35, vec![(0.6, Kind::Conifer { wood: self.w("spruce"), open: false, giant: true }), (0.25, boulder(true)), (0.15, Kind::FallenLog { log: self.w("spruce").log, moss: self.fb.moss, big: true })]),
            BLIGHT => (0.3, vec![(0.5, Kind::Dead { wood: self.w("blightwood"), giant: true }), (0.3, boulder(false)), (0.2, Kind::Skeleton(self.fb.bone))]),
            PETRIFIED => (0.55, vec![(0.6, Kind::Broadleaf(self.w("petrified"), 2)), (0.25, Kind::FallenLog { log: self.w("petrified").log, moss: 0, big: true }), (0.15, Kind::Skeleton(self.fb.bone))]),
            STORMBREAK => (if underwater { 0.12 } else { 0.22 }, vec![(0.5, boulder(false)), (0.5, spire)]),
            GRASSLANDS => (0.12, vec![(0.5, Kind::Broadleaf(self.w("oak"), 2)), (0.3, boulder(false)), (0.2, Kind::Skeleton(self.fb.bone))]),
            _ => (0.0, vec![]),
        };
        if roll >= chance {
            return None;
        }
        pick(u, &table)
    }

    fn small_feature(&self, b: usize, roll: f32, u: f32, x: i32, z: i32) -> Option<Kind> {
        let bb = self.biome_blocks[b];
        let mossy = self.fb.mossy[b];
        let boulder = |m: bool| Kind::Boulder { stone: bb.stone, moss: if m { mossy } else { 0 }, big: false };
        let log = |sp: &str| Kind::FallenLog { log: self.w(sp).log, moss: self.fb.moss, big: false };
        let bush = |sp: &str| Kind::Bush(self.w(sp));
        let small_cap = |cap: u16| Kind::Mushroom { stem: self.fb.stem, cap, giant: false };
        let con = |sp: &str, open: bool| Kind::Conifer { wood: self.w(sp), open, giant: false };
        use biome::*;
        let (chance, table): (f32, Vec<(f32, Kind)>) = match b {
            WILDWOOD => (0.95, vec![
                (0.3, Kind::Broadleaf(self.w("elder"), 1)),
                (0.15, Kind::Broadleaf(self.w("oak"), 1)),
                (0.1, Kind::Broadleaf(self.w("oak"), 0)),
                (0.15, bush("elder")),
                (0.07, boulder(true)),
                (0.08, log("elder")),
                (0.05, Kind::Stump(self.w("oak").log)),
                (0.05, small_cap(self.fb.brown_cap)),
                (0.05, con("redwood", false)),
            ]),
            AMBERWOOD => (0.85, vec![
                (0.28, Kind::Broadleaf(self.w("maple"), 1)),
                (0.2, Kind::Broadleaf(self.w("birch"), 1)),
                (0.14, Kind::Broadleaf(self.w("golden_oak"), 1)),
                (0.1, Kind::Broadleaf(self.w("goldbark"), 1)),
                (0.08, bush("maple")),
                (0.07, log("birch")),
                (0.05, boulder(true)),
                (0.04, Kind::Stump(self.w("maple").log)),
                (0.04, small_cap(self.fb.red_cap)),
            ]),
            FROSTPEAK => (0.4, vec![(0.35, con("fir", false)), (0.3, con("frostpine", false)), (0.2, boulder(false)), (0.1, bush("frostpine")), (0.05, Kind::Stump(self.w("fir").log))]),
            STONEFANG => (0.3, vec![(0.3, con("cragpine", true)), (0.15, Kind::Broadleaf(self.w("ironwood"), 0)), (0.35, boulder(false)), (0.2, bush("cragpine"))]),
            SUNSCORCHED => (0.12, vec![(0.6, Kind::Cactus(self.fb.cactus)), (0.12, Kind::Acacia(self.w("acacia"))), (0.18, boulder(false)), (0.1, Kind::Dead { wood: self.w("deadwood"), giant: false })]),
            OASIS => (0.6, vec![(0.4, Kind::Palm(self.w("palm"))), (0.2, Kind::Broadleaf(self.w("fig"), 1)), (0.2, Kind::Bamboo(self.w("bamboo"))), (0.15, bush("fig")), (0.05, boulder(true))]),
            MURKWATER => (0.8, vec![(0.35, Kind::Cypress(self.w("cypress"))), (0.3, Kind::Willow(self.w("willow"))), (0.1, log("willow")), (0.1, small_cap(self.fb.brown_cap)), (0.1, bush("willow")), (0.05, Kind::Stump(self.w("cypress").log))]),
            TUNDRA => (0.12, vec![(0.45, boulder(false)), (0.2, con("larch", false)), (0.2, Kind::Broadleaf(self.w("birch"), 0)), (0.15, bush("larch"))]),
            GLACIER => (0.08, vec![(0.5, Kind::Crystals(self.fb.frost_crystal)), (0.3, boulder(false)), (0.2, Kind::IceSpike { ice: self.fb.packed_ice, core: self.fb.glacial_ice, big: false })]),
            TAIGA => (0.95, vec![(0.35, con("spruce", false)), (0.3, con("pine", true)), (0.1, bush("spruce")), (0.1, boulder(true)), (0.08, log("pine")), (0.07, Kind::Stump(self.w("spruce").log))]),
            BLIGHT => (0.4, vec![(0.35, Kind::Dead { wood: self.w("blightwood"), giant: false }), (0.3, Kind::Dead { wood: self.w("ashwood"), giant: false }), (0.15, boulder(false)), (0.1, small_cap(self.fb.red_cap)), (0.1, Kind::Stump(self.w("ashwood").log))]),
            PETRIFIED => (0.3, vec![
                (0.3, Kind::Broadleaf(self.w("petrified"), 1)),
                (0.15, Kind::Broadleaf(self.w("stonebark"), 1)),
                (0.2, Kind::Dead { wood: self.w("deadwood"), giant: false }),
                (0.25, boulder(false)),
                (0.1, Kind::Boulder { stone: self.fb.fossil, moss: 0, big: false }),
            ]),
            STORMBREAK => (0.25, vec![(0.35, con("seapine", false)), (0.25, Kind::FallenLog { log: self.w("driftwood").log, moss: 0, big: false }), (0.25, boulder(false)), (0.15, bush("seapine"))]),
            GRASSLANDS => (0.06, vec![(0.35, Kind::Broadleaf(self.w("oak"), 1)), (0.25, Kind::Broadleaf(self.w("elm"), 1)), (0.25, bush("oak")), (0.15, boulder(false))]),
            _ => (0.0, vec![]),
        };
        // Forests grow in groves: dense where the grove noise is high, thinning to glades
        // (and, in open country, gathering scattered trees into copses).
        let chance = chance * self.grove_factor(b, x, z);
        if roll >= chance {
            return None;
        }
        pick(u, &table)
    }

    /// A feature on the floor of the Hollow Earth's caverns.
    fn hollow_anchor(&self, x: i32, z: i32, seed: u64, roll: f32, u: f32) -> Option<Feature> {
        let zone = self.hollow_at(x, z);
        if zone <= 0.0 || roll > 0.55 {
            return None;
        }
        // The floor: the first open block above solid ground, from the top down.
        let mut y = HOLLOW_TOP;
        while y > HOLLOW_LAKE {
            if self.is_cavern(x, y, z, zone) && !self.is_cavern(x, y - 1, z, zone) {
                let kind = pick(
                    u,
                    &[
                        (0.4, Kind::Mushroom { stem: self.fb.stem, cap: self.fb.glowcap, giant: true }),
                        (0.2, Kind::Mushroom { stem: self.w("fungalwood").log, cap: self.w("fungalwood").leaves, giant: true }),
                        (0.15, Kind::Broadleaf(self.w("glowwood"), 1)),
                        (0.15, Kind::Crystals(self.fb.glow_crystal)),
                        (0.1, Kind::Mushroom { stem: self.fb.stem, cap: self.fb.glowcap, giant: false }),
                    ],
                )?;
                return Some(Feature { x, y: y - 1, z, kind, seed });
            }
            y -= 1;
        }
        None
    }

    /// Every feature that may reach into the chunk at `pos`, in a fixed order.
    fn features_for(&self, pos: ChunkPos, col: &Column) -> Vec<Feature> {
        let (x0, y0, z0) = (pos.x * CHUNK_SIZE, pos.y * CHUNK_SIZE, pos.z * CHUNK_SIZE);
        let mut out = Vec::new();
        for &(tier, cell) in &TIERS {
            if tier == 2 && (col.hollow <= 0.0 || y0 > HOLLOW_TOP + 30 || y0 + CHUNK_SIZE < HOLLOW_BOTTOM) {
                continue;
            }
            if tier != 2 && y0 + CHUNK_SIZE < col.min_top - 16 && col.hollow <= 0.0 {
                continue;
            }
            let reach = if tier == 0 { 19 } else { 9 };
            let (gx0, gx1) = ((x0 - reach).div_euclid(cell), (x0 + CHUNK_SIZE + reach).div_euclid(cell));
            let (gz0, gz1) = ((z0 - reach).div_euclid(cell), (z0 + CHUNK_SIZE + reach).div_euclid(cell));
            for gz in gz0..=gz1 {
                for gx in gx0..=gx1 {
                    if let Some(f) = self.anchor(tier, gx, gz) {
                        let (r, down, up) = f.kind.reach();
                        if f.x + r < x0 || f.x - r >= x0 + CHUNK_SIZE || f.z + r < z0 || f.z - r >= z0 + CHUNK_SIZE {
                            continue;
                        }
                        if f.y + up < y0 || f.y - down >= y0 + CHUNK_SIZE {
                            continue;
                        }
                        out.push(f);
                    }
                }
            }
        }
        out
    }

    /// Ore veins in the rock of the chunk at `pos`: which ores depends on depth and biome.
    fn ores(&self, pos: ChunkPos, col: &Column, buf: &mut [BlockState]) {
        let (x0, y0, z0) = (pos.x * CHUNK_SIZE, pos.y * CHUNK_SIZE, pos.z * CHUNK_SIZE);
        if y0 > col.max_top || y0 <= BEDROCK_Y {
            return;
        }
        let (cx, cz) = (x0 + CHUNK_SIZE / 2, z0 + CHUNK_SIZE / 2);
        let t = self.terrain.sample(cx as f32, cz as f32, 1.0);
        let b = self.biome_at(cx, cz, &t, 1.0);
        let mut rng = Rng::new(hash3(self.seed ^ 0x0E5, pos.x, pos.y, pos.z));
        let n = 5 + rng.below(5);
        for _ in 0..n {
            let (lx, ly, lz) = (rng.below(32) as i32, rng.below(32) as i32, rng.below(32) as i32);
            let y = y0 + ly;
            use biome::*;
            let mut table: Vec<(f32, &str)> = vec![(1.0, "iron_ore")];
            if y > -40 {
                table.push((1.2, "coal_ore"));
            }
            if y > -60 {
                table.push((0.8, if b == SUNSCORCHED { "desert_copper_ore" } else { "copper_ore" }));
            }
            if y < -50 {
                table.push((0.3, "gold_ore"));
            }
            match b {
                FROSTPEAK | STONEFANG => table.extend([(0.5, "silver_ore"), (0.25, if b == STONEFANG { "ruby_ore" } else { "sapphire_ore" })]),
                GLACIER => table.extend([(0.4, "sapphire_ore"), (0.6, "frost_crystal")]),
                SUNSCORCHED | OASIS => table.push((0.8, "rock_salt")),
                AMBERWOOD => table.push((0.5, "amber_block")),
                PETRIFIED | GRASSLANDS => table.push((0.6, "fossil_stone")),
                BLIGHT => table.push((0.8, "blight_ore")),
                _ => {}
            }
            if col.hollow > 0.0 && y < HOLLOW_TOP + 10 {
                table.push((0.8, "glow_crystal"));
            }
            let ore = self.ores[pick(rng.next_f32(), &table).unwrap_or("iron_ore")];
            let size = 3 + rng.below(8) as i32;
            let (mut x, mut yy, mut z) = (lx, ly, lz);
            for _ in 0..size {
                if (0..32).contains(&x) && (0..32).contains(&yy) && (0..32).contains(&z) {
                    let i = LocalPos { x: x as u8, y: yy as u8, z: z as u8 }.index();
                    if self.rock_ids.binary_search(&buf[i].id()).is_ok() && buf[i].shape() == Shape::Full {
                        buf[i] = BlockState::full(ore);
                    }
                }
                match rng.below(3) {
                    0 => x += rng.below(3) as i32 - 1,
                    1 => yy += rng.below(3) as i32 - 1,
                    _ => z += rng.below(3) as i32 - 1,
                }
            }
        }
    }

    pub fn generate_chunk(&self, pos: ChunkPos, col: &Column) -> Chunk {
        self.generate(pos, col, true)
    }

    /// The chunk at `pos`: the terrain, and with `extras` its ore veins and features.
    pub fn generate(&self, pos: ChunkPos, col: &Column, extras: bool) -> Chunk {
        let y0 = pos.y * CHUNK_SIZE;
        let y1 = y0 + CHUNK_SIZE - 1;

        // Fast paths: most chunks in a cubic world are all-air or all-solid.
        if y1 <= BEDROCK_Y {
            return Chunk::uniform(BlockState::full(self.bedrock));
        }
        if y0 > col.max_top + FEATURE_UP && y0 > col.max_water {
            return Chunk::uniform(AIR);
        }
        let deep = col.hollow <= 0.0 && y0 > BEDROCK_Y && y1 < col.min_top - 16 && y1 < CAVE_FLOOR && col.rock.iter().all(|r| *r == col.rock[0]);
        let features = if deep || !extras { Vec::new() } else { self.features_for(pos, col) };
        if y0 > col.max_top && y0 > col.max_water && features.is_empty() {
            return Chunk::uniform(AIR);
        }
        let mut buf = vec![if deep { BlockState::full(col.rock[0]) } else { AIR }; (CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE) as usize];
        if !deep && !(y0 > col.max_top && y0 > col.max_water) {
            self.terrain_into(pos, col, &mut buf);
        }
        if extras {
            self.ores(pos, col, &mut buf);
        }
        if !features.is_empty() {
            let ground = |x: i32, z: i32| self.ground_y(x, z);
            let mut canvas = Canvas { buf: &mut buf, origin: [pos.x * CHUNK_SIZE, y0, pos.z * CHUNK_SIZE], water: self.water, ground: &ground };
            for f in &features {
                f.draw(&mut canvas);
            }
        }
        let mut chunk = Chunk::uniform(buf[0]);
        if buf.iter().any(|b| *b != buf[0]) {
            for (i, b) in buf.iter().enumerate() {
                if *b != buf[0] {
                    chunk.set(i, *b);
                }
            }
        }
        chunk
    }

    /// The terrain (no features) of the chunk at `pos` into `buf`.
    fn terrain_into(&self, pos: ChunkPos, col: &Column, buf: &mut [BlockState]) {
        let y0 = pos.y * CHUNK_SIZE;
        let in_hollow = col.hollow > 0.0 && y0 <= HOLLOW_TOP + 1 && y0 + CHUNK_SIZE >= HOLLOW_BOTTOM;
        for lz in 0..CHUNK_SIZE {
            for lx in 0..CHUNK_SIZE {
                let hq = col.height_q_at(lx as usize, lz as usize);
                let top = hq.div_euclid(4);
                let rem = hq.rem_euclid(4);
                let wx = col.cx * CHUNK_SIZE + lx;
                let wz = col.cz * CHUNK_SIZE + lz;
                let i = lz as usize * CHUNK_SIZE as usize + lx as usize;
                let (surf, sub, stone, rock) = (col.surface[i], col.subsurface[i], col.stone[i], col.rock[i]);
                let zone = if in_hollow { self.hollow_at(wx, wz) } else { 0.0 };
                for ly in 0..CHUNK_SIZE {
                    let wy = y0 + ly;
                    let mut state = self.block_at(wx, wy, wz, top, rem, surf, sub, stone, rock, zone, col.water[i]);
                    if zone > 0.0 && !state.is_air() && state.id() != self.water && wy < top - 1 && wy > HOLLOW_BOTTOM - 2 {
                        // The Hollow Earth's own rock, glowing moss on its floors and
                        // crystals on its ceilings.
                        let above = self.is_cavern(wx, wy + 1, wz, zone);
                        let below = self.is_cavern(wx, wy - 1, wz, zone);
                        let h = hash3(self.seed ^ 0xCAFE, wx, wy, wz);
                        if above && wy + 1 >= HOLLOW_LAKE {
                            state = BlockState::full(if unit(h) < 0.7 { self.fb.cave_moss } else { self.biome_blocks[biome::HOLLOW].patches });
                        } else if below && unit(h) < 0.04 {
                            state = BlockState::full(self.fb.glow_crystal);
                        } else if wy < HOLLOW_TOP + 8 && (wy - HOLLOW_BOTTOM) % 9 < 4 {
                            state = BlockState::full(self.biome_blocks[biome::HOLLOW].stone_b);
                        } else if wy < HOLLOW_TOP + 8 {
                            state = BlockState::full(self.biome_blocks[biome::HOLLOW].stone);
                        }
                    }
                    if !state.is_air() {
                        buf[LocalPos { x: lx as u8, y: ly as u8, z: lz as u8 }.index()] = state;
                    }
                }
            }
        }
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
                col.min_top <= SEA_LEVEL && col.max_top > SEA_LEVEL + 1
            })
            .expect("a coast within 128 km")
    }

    #[test]
    fn generation_is_deterministic() {
        let (_r, g) = make();
        let col = g.column(0, 0);
        let p = ChunkPos::new(0, 0, 0);
        assert_eq!(g.generate_chunk(p, &col), g.generate_chunk(p, &g.column(0, 0)));
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
                    let hq = col.height_q_at(lx, lz);
                    let (top, rem) = (hq.div_euclid(4), hq.rem_euclid(4));
                    let cy = top.div_euclid(CHUNK_SIZE);
                    let chunk = cache
                        .entry(cy)
                        .or_insert_with(|| g.generate(ChunkPos::new(cx, cy, cz), &col, false));
                    let l = LocalPos { x: lx as u8, y: top.rem_euclid(CHUNK_SIZE) as u8, z: lz as u8 };
                    let s = chunk.get(l.index());
                    if top <= col.water[lz * CHUNK_SIZE as usize + lx] {
                        // Under water: water, never a slab with a pocket of air above it.
                        assert_eq!(s, BlockState::full(g.water_id()), "at {cx},{cz} {lx},{lz}");
                        wet += 1;
                    } else if rem >= 2 {
                        assert_eq!(s.shape(), Shape::SlabBottom, "at {cx},{cz} {lx},{lz}");
                        slabs += 1;
                    } else {
                        assert!(s.is_air(), "at {cx},{cz} {lx},{lz}");
                    }
                }
            }
        }
        assert!(slabs > 200 && wet > 50, "slabs {slabs}, under water {wet}");
    }

    #[test]
    fn lod_surface_at_full_detail_matches_the_generated_blocks() {
        let (reg, g) = make();
        let water = reg.id("water").unwrap();
        let mut seen_water = false;
        let (kx, kz) = coast(&g);
        for cx in kx - 1..kx + 2 {
            for cz in kz - 1..kz + 2 {
                let col = g.column(cx, cz);
                let mut chunks = std::collections::HashMap::new();
                for lz in 0..CHUNK_SIZE {
                    for lx in 0..CHUNK_SIZE {
                        let (x, z) = (cx * CHUNK_SIZE + lx, cz * CHUNK_SIZE + lz);
                        let (surf_q, block) = g.lod_surface(x, z, 1);
                        // Find the real top: the highest non-air block, scanning down.
                        let mut found = None;
                        for y in (-80i32..240).rev() {
                            let cy = y.div_euclid(CHUNK_SIZE);
                            let chunk = chunks
                                .entry(cy)
                                .or_insert_with(|| g.generate(ChunkPos::new(cx, cy, cz), &col, false));
                            let l = LocalPos { x: lx as u8, y: y.rem_euclid(CHUNK_SIZE) as u8, z: lz as u8 };
                            let s = chunk.get(l.index());
                            if !s.is_air() {
                                let top_q = 4 * y + s.shape().y_range_q().1;
                                found = Some((top_q, s.id()));
                                break;
                            }
                        }
                        let (top_q, id) = found.expect("solid ground");
                        assert_eq!((surf_q, block), (top_q, id), "column {x},{z}");
                        seen_water |= block == water;
                    }
                }
            }
        }
        assert!(seen_water, "test area should include some sea");
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
            assert!((mean(cell) - full).abs() < 4.0, "cell {cell}: mean {} vs {full}", mean(cell));
        }
        // At the same spacing, coarse cells vary less between neighbours than full detail.
        let rough = |cell: i32| -> i64 {
            (0..2000).map(|i| (g.height_q_filtered(i * 64 + 64, 300, cell) - g.height_q_filtered(i * 64, 300, cell)).abs() as i64).sum()
        };
        assert!(rough(64) < rough(1), "{} vs {}", rough(64), rough(1));
        assert_eq!(g.height_q_filtered(123, -45, 1), g.height_q(123, -45));
    }

    #[test]
    fn players_start_in_temperate_lowland_near_the_origin() {
        let (_r, g) = make();
        let [x, y, z] = g.find_spawn();
        let info = g.describe(x.floor() as i32, z.floor() as i32);
        assert!(matches!(info.biome, "Giants' Grasslands" | "Amberwood" | "Ancient Wildwood"), "{info:?}");
        assert!((3.0..40.0).contains(&y) && x.abs() < 6000.0 && z.abs() < 6000.0, "{x},{y},{z}");
    }

    #[test]
    fn every_rock_appears_in_large_regions_and_bare_mountains_show_it() {
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
        // Deep underground is the region's rock, in real chunks.
        let col = g.column(3, -2);
        let chunk = g.generate_chunk(ChunkPos::new(3, -1, -2), &col);
        let deep = chunk.get(LocalPos { x: 4, y: 0, z: 9 }.index());
        assert_eq!(deep.id(), col.rock[9 * CHUNK_SIZE as usize + 4]);
        assert!(["stone", "granite", "basalt", "limestone", "sandstone"].contains(&reg.get(deep).name.as_str()));
    }

    #[test]
    fn half_slabs_only_smooth_steps_and_flat_ground_stays_flat() {
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
        assert!(slabs > 50 && slabs < checked / 3, "{slabs} slabs in {checked} columns");
    }

    #[test]
    fn ground_tints_follow_the_biomes_and_vary_gently() {
        let (_r, g) = make();
        let [x, _, z] = g.find_spawn();
        let t = g.ground_tint(x as i32, z as i32, 1);
        let b = crate::biome::BIOMES.iter().find(|b| b.name == g.describe(x as i32, z as i32).biome).unwrap();
        assert_eq!(&t[..3], &crate::biome::grass_rgb(b)[..]);
        let v: Vec<u8> = (0..200).map(|i| g.ground_tint(i * 7, 0, 1)[3]).collect();
        assert!(v.iter().any(|&a| a < 110) && v.iter().any(|&a| a > 146), "some variation: {v:?}");
        assert_eq!(g.ground_tint(5, 5, 4096)[3], 128, "far away the variation averages out");
    }

    /// The first column (walking out along +x from `start`) whose middle is in `biome`.
    fn find_biome(g: &WorldGen, biome: &str, start: (i32, i32)) -> (i32, i32) {
        (0..20_000)
            .map(|i| (start.0 + (i % 200) * 8, start.1 + (i / 200) * 8))
            .find(|&(cx, cz)| g.describe(cx * CHUNK_SIZE + 16, cz * CHUNK_SIZE + 16).biome == biome)
            .unwrap_or_else(|| panic!("no {biome}"))
    }

    #[test]
    fn forests_grow_trees_that_cross_chunk_borders_and_stand_on_the_ground() {
        let (reg, g) = make();
        let (cx, cz) = find_biome(&g, "Ancient Wildwood", (-300, -300));
        let leaves = |id: u16| reg.def(id).name.ends_with("leaves");
        let logs = |id: u16| reg.def(id).name.ends_with("_log") || reg.def(id).name == "log";
        let col = g.column(cx, cz);
        let (mut n_log, mut n_leaf) = (0, 0);
        let cy0 = col.min_top.div_euclid(CHUNK_SIZE);
        for cy in cy0..cy0 + 4 {
            let c = g.generate_chunk(ChunkPos::new(cx, cy, cz), &col);
            for i in 0..(CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE) as usize {
                let id = c.get(i).id();
                n_log += logs(id) as u32;
                n_leaf += leaves(id) as u32;
            }
        }
        assert!(n_log > 100 && n_leaf > 1000, "a forest: {n_log} logs, {n_leaf} leaves");
        // A feature drawn into two neighbouring chunks: the same blocks along their border
        // as drawing it into one canvas spanning both.
        let f = g.features_for(ChunkPos::new(cx, cy0 + 1, cz), &col);
        assert!(!f.is_empty());
        assert_eq!(g.generate_chunk(ChunkPos::new(cx, cy0 + 1, cz), &col), g.generate_chunk(ChunkPos::new(cx, cy0 + 1, cz), &g.column(cx, cz)));
    }

    #[test]
    fn rivers_flow_above_the_sea_in_beds_cut_into_the_land() {
        let (_, g) = make();
        // A river column well above the sea: water from its bed up to its level, then air.
        let (x, z): (i32, i32) = (0..400_000)
            .map(|i: i32| ((i % 800) * 6 - 2400, (i / 800) * 6 - 1500))
            .find(|&(x, z)| {
                let t = g.terrain.sample(x as f32, z as f32, 1.0);
                water_level(&t) >= SEA_LEVEL + 2 && (t.height.round() as i32) < water_level(&t)
            })
            .expect("a river");
        let (cx, cz) = (x.div_euclid(CHUNK_SIZE), z.div_euclid(CHUNK_SIZE));
        let col = g.column(cx, cz);
        let i = (z.rem_euclid(CHUNK_SIZE) * CHUNK_SIZE + x.rem_euclid(CHUNK_SIZE)) as usize;
        let (wl, top) = (col.water[i], col.height_q[i].div_euclid(4));
        assert!(wl > top);
        for y in top..=wl {
            assert_eq!(g.seed_block(&col, x, y, z), BlockState::full(g.water), "water at {y}");
        }
        assert!(g.seed_block(&col, x, wl + 1, z).is_air());
        let bed = g.seed_block(&col, x, top - 1, z).id();
        assert!([g.sand, g.gravel, g.mud].contains(&bed));
        assert_eq!(g.describe(x, z).biome, "river");
    }

    #[test]
    fn distant_terrain_shows_trees_and_forests() {
        let (_, g) = make();
        let (cx, cz) = find_biome(&g, "Ancient Wildwood", (-300, -300));
        let (x0, z0) = (cx * CHUNK_SIZE, cz * CHUNK_SIZE);
        // Near: the features themselves. A big tree's anchor shows its crown high above the
        // ground.
        let feats = g.lod_features(x0, z0, x0 + 64, z0 + 64, 2);
        let tall = feats.buckets.values().flatten().filter(|f| matches!(f.kind, Kind::Broadleaf(_, 1 | 2) | Kind::Redwood(_))).find(|f| (x0..x0 + 64).contains(&f.x) && (z0..z0 + 64).contains(&f.z)).expect("a tree");
        let ground = g.lod_surface(tall.x, tall.z, 1).0;
        assert!(g.lod_cell(tall.x, tall.z, 1, &feats).0 >= ground + 4 * 8);
        // Far: the canopy over most of the forest's cells.
        let cells: Vec<(i32, i32)> = (0..2000)
            .map(|k| (x0 + (k % 40) * 16 - 320 + 8, z0 + (k / 40) * 16 - 400 + 8))
            .filter(|&(x, z)| g.describe(x, z).biome == "Ancient Wildwood" && g.lod_surface(x, z, 16).1 != g.water)
            .take(200)
            .collect();
        assert!(cells.len() > 50);
        let n = cells.iter().filter(|&&(x, z)| g.lod_cell(x, z, 16, &FeatureGrid::default()).0 > g.lod_surface(x, z, 16).0).count();
        assert!(n * 10 > cells.len() * 7, "{n} of {} cells under a canopy", cells.len());
    }

    #[test]
    fn the_hollow_earth_has_caverns_with_glowing_floors() {
        let (reg, g) = make();
        let (x, z) = (0..4000)
            .map(|i| ((i % 64) * 256 - 8192, (i / 64) * 256 - 8192))
            .find(|&(x, z)| g.hollow_at(x, z) > 0.95)
            .expect("a hollow region");
        let (cx, cz) = (x.div_euclid(CHUNK_SIZE), z.div_euclid(CHUNK_SIZE));
        let col = g.column(cx, cz);
        let moss = reg.id("cave_moss").unwrap();
        let (mut open, mut glow) = (0, 0);
        for cy in -3..=-2 {
            let c = g.generate_chunk(ChunkPos::new(cx, cy, cz), &col);
            for i in 0..(CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE) as usize {
                open += c.get(i).is_air() as u32;
                glow += (c.get(i).id() == moss) as u32;
            }
        }
        assert!(open > 4000 && glow > 30, "{open} open, {glow} moss");
    }

    #[test]
    fn sky_is_uniform_air_and_deep_is_uniform_stone_or_bedrock() {
        let (_r, g) = make();
        let col = g.column(0, 0);
        assert!(g.generate_chunk(ChunkPos::new(0, 20, 0), &col).is_uniform());
        assert!(g.generate(ChunkPos::new(0, -3, 0), &col, false).is_uniform());
        let bedrock = g.generate_chunk(ChunkPos::new(0, -10, 0), &col);
        assert_eq!(bedrock.uniform_state(), Some(BlockState::full(g.bedrock)));
    }
}
