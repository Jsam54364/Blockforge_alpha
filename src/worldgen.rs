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

use crate::biome::{self, BIOMES};
use crate::block::{BlockState, Shape, AIR};
use crate::chunk::Chunk;
use crate::coords::{ChunkPos, LocalPos, CHUNK_SIZE};
use crate::registry::BlockRegistry;
use crate::climate::{Climate, ClimateSample};
use crate::noise::Fbm;
use crate::rng::hash3;
use crate::terrain::{Terrain, TerrainSample};

pub const SEA_LEVEL: i32 = 0;
/// Bumped whenever the same seed would generate different terrain: worlds saved by an
/// older generator are kept apart (their edits would land on different ground).
pub const WORLDGEN_VERSION: u32 = 2;
pub const BEDROCK_Y: i32 = -128;
/// No caves below this; deep chunks collapse to a single uniform entry.
const CAVE_FLOOR: i32 = -48;

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
    /// Lowest / highest `top` block y (see `generate_chunk`) in this column.
    pub min_top: i32,
    pub max_top: i32,
}

impl Column {
    pub fn height_q_at(&self, lx: usize, lz: usize) -> i32 {
        self.height_q[lz * CHUNK_SIZE as usize + lx]
    }
}

pub struct WorldGen {
    seed: u64,
    terrain: Terrain,
    climate: Climate,
    stone: u16,
    sand: u16,
    gravel: u16,
    water: u16,
    bedrock: u16,
    /// (surface, subsurface) block per entry of `biome::BIOMES`.
    biome_blocks: Vec<(u16, u16)>,
    /// Gentle large-scale brightness variation of the ground (see `ground_tint`).
    variation: Fbm,
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

impl WorldGen {
    pub fn new(seed: u64, reg: &BlockRegistry) -> Self {
        let id = |n: &str| reg.id(n).unwrap_or_else(|| panic!("worldgen needs block '{n}'"));
        WorldGen {
            seed,
            terrain: Terrain::new(seed),
            climate: Climate::new(seed),
            stone: id("stone"),
            sand: id("sand"),
            gravel: id("gravel"),
            water: id("water"),
            bedrock: id("bedrock"),
            biome_blocks: BIOMES.iter().map(|b| (id(b.surface), id(b.subsurface))).collect(),
            variation: Fbm::new(seed ^ 0x7A11_7E5D, 60.0, 2, 0.5),
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
        (self.terrain.sample(x as f32, z as f32, cell as f32).height * 4.0).floor() as i32
    }

    /// The (surface, subsurface) blocks of a column whose partial (top) block is at `top`.
    fn ground_blocks(&self, x: i32, z: i32, top: i32, t: &TerrainSample, cell: f32) -> (u16, u16) {
        // Under water (top <= sea level), and beaches: dry ground at sea level near the coast.
        // Inland lakes and wetland ponds keep the biome's ground right to the water's edge.
        if top <= SEA_LEVEL || (top - 1 <= SEA_LEVEL && t.inland < 0.35) {
            // Sand, or gravel where it is cold or deep.
            let c = self.climate.at(x as f32, z as f32, 0.0, t.inland, cell);
            let b = if top < SEA_LEVEL - 10 || c.temperature < 0.0 { self.gravel } else { self.sand };
            return (b, b);
        }
        let c = self.climate.at(x as f32, z as f32, t.smooth, t.inland, cell);
        self.biome_blocks[biome::choose(c.temperature, c.rainfall, t.smooth)]
    }

    /// How the ground at `(x, z)` is coloured, for terrain drawn in cells `cell` blocks wide:
    /// the biome's grass colour (sRGB) and, in the last byte, a brightness variation (128 is
    /// neutral) that varies over tens of blocks and hides how textures repeat.
    pub fn ground_tint(&self, x: i32, z: i32, cell: i32) -> [u8; 4] {
        let t = self.terrain.sample(x as f32, z as f32, cell as f32);
        let c = self.climate.at(x as f32, z as f32, t.smooth.max(0.0), t.inland, cell as f32);
        let [r, g, b] = biome::grass_rgb(&BIOMES[biome::choose(c.temperature, c.rainfall, t.smooth.max(0.0))]);
        let v = self.variation.at(x as f32, z as f32, cell as f32);
        [r, g, b, (128.0 + 127.0 * v.clamp(-1.0, 1.0)) as u8]
    }

    /// Biome, landform, height and climate of the column at `(x, z)`.
    pub fn describe(&self, x: i32, z: i32) -> GroundInfo {
        let t = self.terrain.sample(x as f32, z as f32, 1.0);
        let c = self.climate.at(x as f32, z as f32, t.smooth, t.inland, 1.0);
        let biome = if t.height < (SEA_LEVEL + 1) as f32 { "sea" } else { BIOMES[biome::choose(c.temperature, c.rainfall, t.smooth)].name };
        GroundInfo { biome, landform: self.terrain.landform_at(x as f32, z as f32).name, height: t.height, climate: c }
    }

    /// Where a new player starts: the nearest temperate lowland to the origin (searched in
    /// 64-block steps, nearest first, up to 6 km out), as a point on the ground.
    pub fn find_spawn(&self) -> [f32; 3] {
        let ok = |x: i32, z: i32| {
            let g = self.describe(x, z);
            ((3.0..40.0).contains(&g.height) && matches!(g.biome, "grassland" | "temperate forest")).then_some(g.height)
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
        match best {
            Some((_, x, z, h)) => [x as f32 + 0.5, h, z as f32 + 0.5],
            None => [0.5, self.describe(0, 0).height.max(0.0), 0.5],
        }
    }

    /// The visible surface of the column at block `(x, z)` for level-of-detail terrain:
    /// its top in quarter blocks and the block it is made of. Matches `generate_chunk`
    /// (slab tops, the biome's ground, water up to the sea surface; caves are never visible
    /// from above) for `cell == 1`, and is band-limited for coarser cells.
    pub fn lod_surface(&self, x: i32, z: i32, cell: i32) -> (i32, u16) {
        let t = self.terrain.sample(x as f32, z as f32, cell as f32);
        let hq = (t.height * 4.0).floor() as i32;
        let top = hq.div_euclid(4);
        let slab = hq.rem_euclid(4) >= 2;
        // Water fills every block above the terrain up to SEA_LEVEL, and there are no
        // half-slabs under water (see `block_at`).
        if top <= SEA_LEVEL {
            return (4 * (SEA_LEVEL + 1), self.water);
        }
        let surf = self.ground_blocks(x, z, top, &t, cell as f32).0;
        (4 * top + if slab { 2 } else { 0 }, surf)
    }

    /// Just the height part of `lod_surface` (much cheaper: no climate).
    pub fn lod_height(&self, x: i32, z: i32, cell: i32) -> i32 {
        let hq = self.height_q_filtered(x, z, cell);
        let top = hq.div_euclid(4);
        if top <= SEA_LEVEL {
            return 4 * (SEA_LEVEL + 1);
        }
        4 * top + if hq.rem_euclid(4) >= 2 { 2 } else { 0 }
    }

    /// The top of the highest full block of the column at `(x, z)` in quarter blocks, at
    /// full detail: the ground under any half-slab or water.
    pub fn ground_q(&self, x: i32, z: i32) -> i32 {
        4 * self.height_q_filtered(x, z, 1).div_euclid(4)
    }

    pub fn column(&self, cx: i32, cz: i32) -> Column {
        let n = (CHUNK_SIZE * CHUNK_SIZE) as usize;
        let (mut heights, mut surface, mut subsurface) = (Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n));
        let (mut min_top, mut max_top) = (i32::MAX, i32::MIN);
        for lz in 0..CHUNK_SIZE {
            for lx in 0..CHUNK_SIZE {
                let (x, z) = (cx * CHUNK_SIZE + lx, cz * CHUNK_SIZE + lz);
                let t = self.terrain.sample(x as f32, z as f32, 1.0);
                let hq = (t.height * 4.0).floor() as i32;
                let top = hq.div_euclid(4);
                let (surf, sub) = self.ground_blocks(x, z, top, &t, 1.0);
                min_top = min_top.min(top);
                max_top = max_top.max(top);
                heights.push(hq);
                surface.push(surf);
                subsurface.push(sub);
            }
        }
        Column { cx, cz, height_q: heights, surface, subsurface, min_top, max_top }
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

    fn block_at(&self, wx: i32, wy: i32, wz: i32, top: i32, rem: i32, surf: u16, sub: u16) -> BlockState {
        if wy <= BEDROCK_Y {
            return BlockState::full(self.bedrock);
        }

        if wy > top {
            return if wy <= SEA_LEVEL { BlockState::full(self.water) } else { AIR };
        }
        if wy == top {
            // The partial block: a half-slab when the fractional height is >= 0.5, but never
            // under water: a slab there would leave a pocket of air in the sea (slabs are not
            // waterlogged) and its sides would z-fight with the water beside it.
            if wy <= SEA_LEVEL {
                return BlockState::full(self.water);
            }
            if rem >= 2 {
                return BlockState::new(surf, Shape::SlabBottom);
            }
            return AIR;
        }
        if wy < top - 6 && wy > CAVE_FLOOR && self.is_cave(wx, wy, wz) {
            return AIR;
        }
        if wy == top - 1 {
            return BlockState::full(if rem >= 2 { sub } else { surf });
        }
        if wy >= top - 4 {
            return BlockState::full(sub);
        }
        BlockState::full(self.stone)
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
        if y0 > BEDROCK_Y && y1 < col.min_top - 4 && y1 < CAVE_FLOOR {
            return Chunk::uniform(BlockState::full(self.stone));
        }

        let mut chunk = Chunk::uniform(AIR);
        for lz in 0..CHUNK_SIZE {
            for lx in 0..CHUNK_SIZE {
                let hq = col.height_q_at(lx as usize, lz as usize);
                let top = hq.div_euclid(4);
                let rem = hq.rem_euclid(4);
                let wx = col.cx * CHUNK_SIZE + lx;
                let wz = col.cz * CHUNK_SIZE + lz;
                let i = lz as usize * CHUNK_SIZE as usize + lx as usize;
                let (surf, sub) = (col.surface[i], col.subsurface[i]);
                for ly in 0..CHUNK_SIZE {
                    let state = self.block_at(wx, y0 + ly, wz, top, rem, surf, sub);
                    if !state.is_air() {
                        let l = LocalPos { x: lx as u8, y: ly as u8, z: lz as u8 };
                        chunk.set(l.index(), state);
                    }
                }
            }
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
                        .or_insert_with(|| g.generate_chunk(ChunkPos::new(cx, cy, cz), &col));
                    let l = LocalPos { x: lx as u8, y: top.rem_euclid(CHUNK_SIZE) as u8, z: lz as u8 };
                    let s = chunk.get(l.index());
                    if top <= SEA_LEVEL {
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
                                .or_insert_with(|| g.generate_chunk(ChunkPos::new(cx, cy, cz), &col));
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
        assert!(matches!(info.biome, "grassland" | "temperate forest"), "{info:?}");
        assert!((3.0..40.0).contains(&y) && x.abs() < 6000.0 && z.abs() < 6000.0, "{x},{y},{z}");
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

    #[test]
    fn sky_is_uniform_air_and_deep_is_uniform_stone_or_bedrock() {
        let (_r, g) = make();
        let col = g.column(0, 0);
        assert!(g.generate_chunk(ChunkPos::new(0, 20, 0), &col).is_uniform());
        assert!(g.generate_chunk(ChunkPos::new(0, -3, 0), &col).is_uniform());
        let bedrock = g.generate_chunk(ChunkPos::new(0, -10, 0), &col);
        assert_eq!(bedrock.uniform_state(), Some(BlockState::full(g.bedrock)));
    }
}
