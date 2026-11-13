//! Deterministic terrain: a pure function of (seed, chunk position).
//!
//! That purity is what makes the "procedural base + edit journal" design work:
//! unloaded chunks can always be regenerated, then edits are replayed on top.
//!
//! Heights are in quarter-block units, so the surface can end in a half-slab
//! (the "terrain slab" idea) with no change to the chunk format.

use crate::block::{BlockState, Shape, AIR};
use crate::chunk::Chunk;
use crate::coords::{ChunkPos, LocalPos, CHUNK_SIZE};
use crate::registry::BlockRegistry;
use crate::rng::hash3;

pub const SEA_LEVEL: i32 = 0;
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
    stone: u16,
    dirt: u16,
    grass: u16,
    sand: u16,
    water: u16,
    bedrock: u16,
}

fn lattice2(seed: u64, ix: i32, iz: i32) -> f32 {
    (hash3(seed, ix, 0, iz) >> 40) as f32 * (1.0 / 16_777_216.0)
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

fn value_noise_2d(seed: u64, x: f32, z: f32) -> f32 {
    let x0 = x.floor();
    let z0 = z.floor();
    let tx = smooth(x - x0);
    let tz = smooth(z - z0);
    let (ix, iz) = (x0 as i32, z0 as i32);
    let a = lattice2(seed, ix, iz);
    let b = lattice2(seed, ix + 1, iz);
    let c = lattice2(seed, ix, iz + 1);
    let d = lattice2(seed, ix + 1, iz + 1);
    lerp(lerp(a, b, tx), lerp(c, d, tx), tz)
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
            stone: id("stone"),
            dirt: id("dirt"),
            grass: id("grass"),
            sand: id("sand"),
            water: id("water"),
            bedrock: id("bedrock"),
        }
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Surface height in quarter-block units.
    pub fn height_q(&self, x: i32, z: i32) -> i32 {
        let (fx, fz) = (x as f32, z as f32);
        let (mut amp, mut freq) = (1.0f32, 1.0f32 / 96.0);
        let (mut sum, mut norm) = (0.0f32, 0.0f32);
        for o in 0..4u64 {
            sum += amp * value_noise_2d(self.seed.wrapping_add(o), fx * freq, fz * freq);
            norm += amp;
            amp *= 0.5;
            freq *= 2.0;
        }
        let n = sum / norm; // roughly 0..1
        let h = 6.0 + (n - 0.5) * 2.0 * 36.0; // blocks
        (h * 4.0).floor() as i32
    }

    pub fn column(&self, cx: i32, cz: i32) -> Column {
        let mut heights = Vec::with_capacity((CHUNK_SIZE * CHUNK_SIZE) as usize);
        let (mut min_top, mut max_top) = (i32::MAX, i32::MIN);
        for lz in 0..CHUNK_SIZE {
            for lx in 0..CHUNK_SIZE {
                let hq = self.height_q(cx * CHUNK_SIZE + lx, cz * CHUNK_SIZE + lz);
                let top = hq.div_euclid(4);
                min_top = min_top.min(top);
                max_top = max_top.max(top);
                heights.push(hq);
            }
        }
        Column { cx, cz, height_q: heights, min_top, max_top }
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

    fn block_at(&self, wx: i32, wy: i32, wz: i32, top: i32, rem: i32, sandy: bool) -> BlockState {
        if wy <= BEDROCK_Y {
            return BlockState::full(self.bedrock);
        }
        let surf = if sandy { self.sand } else { self.grass };
        let sub = if sandy { self.sand } else { self.dirt };

        if wy > top {
            return if wy <= SEA_LEVEL { BlockState::full(self.water) } else { AIR };
        }
        if wy == top {
            // The partial block: a half-slab when the fractional height is >= 0.5.
            if rem >= 2 {
                return BlockState::new(surf, Shape::SlabBottom);
            }
            return if wy <= SEA_LEVEL { BlockState::full(self.water) } else { AIR };
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
                let sandy = top - 1 <= SEA_LEVEL;
                for ly in 0..CHUNK_SIZE {
                    let state = self.block_at(wx, y0 + ly, wz, top, rem, sandy);
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
        let col = g.column(0, 0);
        let mut slabs = 0;
        let mut cache: std::collections::HashMap<i32, Chunk> = std::collections::HashMap::new();
        for lz in 0..CHUNK_SIZE as usize {
            for lx in 0..CHUNK_SIZE as usize {
                let hq = col.height_q_at(lx, lz);
                let (top, rem) = (hq.div_euclid(4), hq.rem_euclid(4));
                let cy = top.div_euclid(CHUNK_SIZE);
                let chunk = cache
                    .entry(cy)
                    .or_insert_with(|| g.generate_chunk(ChunkPos::new(0, cy, 0), &col));
                let l = LocalPos { x: lx as u8, y: top.rem_euclid(CHUNK_SIZE) as u8, z: lz as u8 };
                let s = chunk.get(l.index());
                if rem >= 2 {
                    assert_eq!(s.shape(), Shape::SlabBottom, "at {lx},{lz}");
                    slabs += 1;
                } else {
                    assert!(s.is_air() || s.shape() == Shape::Full);
                }
            }
        }
        assert!(slabs > 0, "expected some half-slab surface cells");
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
